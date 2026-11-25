// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Jobs: the long operations an application can cancel and watch.
//!
//! The application names a job by a number of its own, which it never uses
//! twice, and gives it in the `job` field of a long request. While the
//! request runs, [`Engine::cancel_job`](crate::Engine::cancel_job) stops it
//! at the library's next safe point (the request then fails with
//! `Cancelled`, and nothing it was making is left behind), and
//! [`Engine::job_progress`](crate::Engine::job_progress) says how far it has
//! come. A request with no `job` runs as before and cannot be cancelled.
//!
//! A job that works on a document stops when the document is closed
//! ([`Jobs::cancel_for`]).
//!
//! A cancel that arrives before its request has begun (the application asked
//! for the cancel on another thread while the request was on its way) is
//! kept, and the request is cancelled as it begins. At most
//! [`EARLY_CANCELS`] such cancels are kept, the oldest dropped first: a job
//! number is never reused, so one that never begins is only a stale entry.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use kernkraft_fs::Progress;
use serde::Deserialize;
use serde_json::json;

use crate::error::WireError;
use crate::fields::Fields;

/// The number an application gives a job.
pub type JobId = u64;

/// Cancels kept for jobs that have not begun.
const EARLY_CANCELS: usize = 64;

/// A request that names a job.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JobRequest {
    pub job: JobId,
}

/// What the library reports of a job, and whether it is to stop.
#[derive(Default)]
struct Job {
    /// The document the job works on, if it works on one.
    handle: Option<i64>,
    cancelled: AtomicBool,
    done: AtomicU64,
    total: AtomicU64,
    stage: Mutex<String>,
}

#[derive(Default)]
struct Table {
    running: HashMap<JobId, Arc<Job>>,
    early: VecDeque<JobId>,
}

/// The jobs running now.
#[derive(Default)]
pub struct Jobs {
    table: Mutex<Table>,
}

/// A job that has begun: the [`Progress`] the library is given. The job is
/// over when this is dropped.
pub struct Running<'a> {
    jobs: &'a Jobs,
    id: Option<JobId>,
    job: Arc<Job>,
}

impl Jobs {
    /// Begins the job `id`, or a job nobody can cancel when there is none.
    /// `handle` is the document it works on. A job number in use is
    /// `BadRequest`.
    pub fn begin(&self, id: Option<JobId>, handle: Option<i64>) -> Result<Running<'_>, WireError> {
        let job = Arc::new(Job {
            handle,
            ..Job::default()
        });
        if let Some(id) = id {
            let mut table = self.table();
            if table.running.contains_key(&id) {
                return Err(WireError::bad_request(format!(
                    "job {id} is already running"
                )));
            }
            if let Some(at) = table.early.iter().position(|early| *early == id) {
                table.early.remove(at);
                job.cancelled.store(true, Ordering::SeqCst);
            }
            table.running.insert(id, job.clone());
        }
        Ok(Running {
            jobs: self,
            id,
            job,
        })
    }

    /// Asks the job `id` to stop. `running` is whether the job was running;
    /// when it was not, the cancel is kept for a job that is about to begin.
    pub fn cancel(&self, id: JobId) -> Fields {
        let mut table = self.table();
        let running = match table.running.get(&id) {
            Some(job) => {
                job.cancelled.store(true, Ordering::SeqCst);
                true
            }
            None => {
                if !table.early.contains(&id) {
                    if table.early.len() == EARLY_CANCELS {
                        table.early.pop_front();
                    }
                    table.early.push_back(id);
                }
                false
            }
        };
        let mut fields = Fields::new();
        fields.insert("running".to_owned(), json!(running));
        fields
    }

    /// Stops every job that works on the document `handle` names.
    pub fn cancel_for(&self, handle: i64) {
        for job in self.table().running.values() {
            if job.handle == Some(handle) {
                job.cancelled.store(true, Ordering::SeqCst);
            }
        }
    }

    /// How far the job `id` has come: `done` out of `total` units of what
    /// `stage` names (a unit is what the operation counts: objects, pages,
    /// files). `total` is 0 until the operation knows it. A job that is not
    /// running is `running: false` with nothing counted.
    pub fn progress(&self, id: JobId) -> Fields {
        let job = self.table().running.get(&id).cloned();
        let mut fields = Fields::new();
        fields.insert("running".to_owned(), json!(job.is_some()));
        let (done, total, stage) = job.map_or((0, 0, String::new()), |job| {
            (
                job.done.load(Ordering::Relaxed),
                job.total.load(Ordering::Relaxed),
                job.stage
                    .lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .clone(),
            )
        });
        fields.insert("done".to_owned(), json!(done));
        fields.insert("total".to_owned(), json!(total));
        fields.insert("stage".to_owned(), json!(stage));
        fields
    }

    /// The table. Each step that holds the lock is one map call, so a panic
    /// elsewhere leaves it whole.
    fn table(&self) -> MutexGuard<'_, Table> {
        self.table.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl Progress for Running<'_> {
    fn report(&self, done: u64, total: u64, stage: &str) {
        self.job.done.store(done, Ordering::Relaxed);
        self.job.total.store(total, Ordering::Relaxed);
        let mut current = self
            .job
            .stage
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if *current != stage {
            current.clear();
            current.push_str(stage);
        }
    }

    fn is_cancelled(&self) -> bool {
        self.job.cancelled.load(Ordering::SeqCst)
    }
}

impl std::fmt::Debug for Running<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Running").field("job", &self.id).finish()
    }
}

impl Drop for Running<'_> {
    fn drop(&mut self) {
        if let Some(id) = self.id {
            self.jobs.table().running.remove(&id);
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::path::Path;

    use kernkraft_pdf::doc::OpenOptions;
    use kernkraft_pdf::file::ReadLimits;

    use super::*;
    use crate::engine::{Engine, HandleRequest};
    use crate::open::Source;

    #[test]
    fn a_job_number_in_use_is_refused_and_free_again_when_the_job_is_over() {
        let jobs = Jobs::default();
        let first = jobs.begin(Some(1), None).unwrap();

        let again = jobs.begin(Some(1), None).unwrap_err();
        assert_eq!(again.kind, "BadRequest");
        // A job with no number is nobody's to cancel, so it cannot collide.
        assert!(jobs.begin(None, None).is_ok());

        drop(first);
        assert!(jobs.begin(Some(1), None).is_ok());
    }

    #[test]
    fn a_job_reports_what_the_library_tells_it_and_is_over_when_dropped() {
        let jobs = Jobs::default();
        let running = jobs.begin(Some(2), None).unwrap();
        assert_eq!(jobs.progress(2)["total"], 0);

        running.report(3, 10, "objects");
        running.report(4, 10, "objects");

        let seen = jobs.progress(2);
        assert_eq!(seen["running"], true);
        assert_eq!(seen["done"], 4);
        assert_eq!(seen["total"], 10);
        assert_eq!(seen["stage"], "objects");
        drop(running);
        let over = jobs.progress(2);
        assert_eq!(over["running"], false);
        assert_eq!(over["done"], 0);
        assert_eq!(over["stage"], "");
    }

    #[test]
    fn a_cancel_reaches_the_running_job_and_only_that_one() {
        let jobs = Jobs::default();
        let one = jobs.begin(Some(1), None).unwrap();
        let two = jobs.begin(Some(2), None).unwrap();

        assert_eq!(jobs.cancel(1)["running"], true);

        assert!(one.is_cancelled());
        assert!(!two.is_cancelled());
    }

    #[test]
    fn a_cancel_before_the_job_begins_is_kept_for_that_job_once() {
        let jobs = Jobs::default();
        assert_eq!(jobs.cancel(5)["running"], false);
        // Asking twice keeps one.
        jobs.cancel(5);

        let first = jobs.begin(Some(5), None).unwrap();
        assert!(first.is_cancelled());
        drop(first);
        let second = jobs.begin(Some(5), None).unwrap();
        assert!(!second.is_cancelled());
    }

    #[test]
    fn the_oldest_early_cancel_is_dropped_past_the_limit() {
        let jobs = Jobs::default();
        let limit = EARLY_CANCELS as u64;
        for job in 0..=limit {
            jobs.cancel(job);
        }

        // 0 was the oldest of limit + 1 and is gone; 1 is the oldest kept.
        assert!(!jobs.begin(Some(0), None).unwrap().is_cancelled());
        assert!(jobs.begin(Some(1), None).unwrap().is_cancelled());
        assert!(jobs.begin(Some(limit), None).unwrap().is_cancelled());
    }

    #[test]
    fn the_jobs_of_one_document_stop_with_it_and_the_others_go_on() {
        let jobs = Jobs::default();
        let on_one = jobs.begin(Some(1), Some(10)).unwrap();
        let on_another = jobs.begin(Some(2), Some(11)).unwrap();
        let on_none = jobs.begin(Some(3), None).unwrap();

        jobs.cancel_for(10);

        assert!(on_one.is_cancelled());
        assert!(!on_another.is_cancelled());
        assert!(!on_none.is_cancelled());
    }

    #[test]
    fn closing_a_document_stops_the_jobs_working_on_it() {
        let engine = Engine::new(OpenOptions {
            limits: ReadLimits {
                max_object_elements: 1 << 20,
                max_xref_entries: 1 << 24,
                max_object_stream_bytes: 1 << 26,
            },
            cached_objects: 64,
            cached_object_streams: 4,
            max_diagnostics: 100,
        });
        let handle = engine
            .open(Source {
                path: Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf"),
                password: String::new(),
            })
            .unwrap()["handle"]
            .as_i64()
            .unwrap();
        let working = engine.jobs.begin(Some(1), Some(handle)).unwrap();
        let elsewhere = engine.jobs.begin(Some(2), None).unwrap();

        engine.close(HandleRequest { handle }).unwrap();

        assert!(working.is_cancelled());
        assert!(!elsewhere.is_cancelled());
    }
}
