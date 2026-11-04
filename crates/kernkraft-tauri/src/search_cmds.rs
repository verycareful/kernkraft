// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name search and the Documents view: a walk of folders that streams what
//! it finds.
//!
//! Both are background operations ([`OpId`], `op-progress`, `op-done`,
//! [`Kernkraft::cancel_op`]; see the `ops` module) over
//! `kernkraft_fs::scan`, which follows symlinks to folders and enters each
//! folder once by device and inode, so a symlink loop ends. Matches go out
//! as [`EVENT_SEARCH_HIT`] in batches of [`SEARCH_BATCH`], or sooner: what
//! has been found waits at most [`SEARCH_FLUSH`] before it is sent, so a
//! search with few hits shows them while the walk goes on. The last, shorter
//! batch goes out before `op-done`, whose `scan` holds the count. Entries are
//! sent as found and not kept, so a walk of a large tree holds one batch.
//!
//! A search or Documents scan only reads, so unlike the file operations it
//! does not make undo or redo wait, and may run while one does.

use std::mem;
use std::path::PathBuf;
use std::sync::{Condvar, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use kernkraft_fs::{scan, Entry, EntryKind, Progress, ScanOptions};
use kernkraft_xdg_apps::MimeDb;
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError};
use crate::fs_cmds::WireEntry;
use crate::ops::{Job, OpId};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

pub const EVENT_SEARCH_HIT: &str = "kernkraft://search-hit";

/// Entries per [`EVENT_SEARCH_HIT`]: the view renders its list once per
/// batch, so a search with thousands of hits re-renders tens of times, not
/// thousands, while the first hits still show at once.
pub const SEARCH_BATCH: usize = 200;

/// The longest a found entry waits for its batch to fill: 100 ms, the
/// response time within which an interface feels immediate (RAIL), so the
/// first hits of a slow walk show at once, while the view still renders at
/// most ten times a second however fast the hits come.
pub const SEARCH_FLUSH: Duration = Duration::from_millis(100);

/// Payload of [`EVENT_SEARCH_HIT`]: found entries of operation `op`,
/// started under `key`, in walk order.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchHitEvent {
    pub key: String,
    pub op: OpId,
    pub entries: Vec<WireEntry>,
}

/// What a finished walk reports in `op-done`'s `scan`: how many entries
/// matched, the folders (or roots) that could not be read, and whether it
/// was cancelled.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireScanDone {
    pub found: u64,
    pub unreadable: Vec<WirePath>,
    pub cancelled: bool,
}

/// Which entries a walk keeps.
pub(crate) enum ScanFilter {
    /// Names containing this text, compared in lower case.
    Name(String),
    /// Text files, by name.
    Documents,
}

/// Whether `e` is a document for the Documents view (design D5.1: in
/// 0.1.0.0 a supported document is a text file): a regular file, or a
/// symlink to one, whose name types it as text (`MimeDb::is_text`, which
/// takes subclasses of `text/plain` too). Typed by name only, never by
/// content: a scan of a home folder must not open every file in it.
fn is_document(e: &Entry, db: &MimeDb) -> bool {
    let file = match e.kind {
        EntryKind::File => true,
        EntryKind::Symlink => std::fs::metadata(&e.path).is_ok_and(|m| m.is_file()),
        EntryKind::Dir | EntryKind::Other => false,
    };
    file && db.by_name(&e.name).is_some_and(|m| db.is_text(m))
}

/// Runs `walk`, which hands each item it finds to the sink it is given, and
/// passes the items to `send` in batches in the order they were handed over:
/// as soon as `size` are waiting, or after `interval` for those waiting
/// when it passes, whichever is first. What is left when `walk` returns is
/// sent before this returns `walk`'s result.
fn stream_batches<T: Send, R>(
    size: usize,
    interval: Duration,
    send: &(dyn Fn(Vec<T>) + Sync),
    walk: impl FnOnce(&(dyn Fn(T) + Sync)) -> R,
) -> R {
    let pending: Mutex<Vec<T>> = Mutex::new(Vec::with_capacity(size));
    // Sending with the lock held keeps the batches in the order of the items.
    let flush = || {
        let mut p = lock(&pending);
        if !p.is_empty() {
            send(mem::replace(&mut *p, Vec::with_capacity(size)));
        }
    };
    let stop = (Mutex::new(false), Condvar::new());
    let result = thread::scope(|scope| {
        // A walk can be held up inside one folder (a stalled mount), so the
        // timer is its own thread: what was found before the stall is sent
        // while the walk waits.
        scope.spawn(|| loop {
            {
                let (stopped, wake) = &stop;
                let waited = wake
                    .wait_timeout_while(lock(stopped), interval, |stopped| !*stopped)
                    .unwrap_or_else(PoisonError::into_inner);
                if *waited.0 {
                    break;
                }
            }
            flush();
        });
        // Stops the timer when the walk ends, also by a panic, which would
        // otherwise leave the scope waiting for it.
        let _stop = StopTimer(&stop);
        walk(&|item| {
            let mut p = lock(&pending);
            p.push(item);
            if p.len() >= size {
                send(mem::replace(&mut *p, Vec::with_capacity(size)));
            }
        })
    });
    flush();
    result
}

/// Tells the timer of [`stream_batches`] to end when dropped.
struct StopTimer<'a>(&'a (Mutex<bool>, Condvar));

impl Drop for StopTimer<'_> {
    fn drop(&mut self) {
        *lock(&self.0 .0) = true;
        self.0 .1.notify_all();
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// Starts a search of `roots` and everything under them for names
    /// containing `query`, ignoring case and the query's leading and
    /// trailing whitespace; dotfiles and dot-folders only with `hidden`. A
    /// query that is empty or only whitespace is refused: it would list
    /// every file. `origin` is the label of the window asking; closing it
    /// cancels the search.
    pub fn search(
        &self,
        key: String,
        roots: Vec<WirePath>,
        query: String,
        hidden: bool,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        let needle = query.trim().to_lowercase();
        if needle.is_empty() {
            return Err(WireError::from(PluginError::InvalidArgument {
                what: "search query",
                value: query,
            }));
        }
        self.start_scan(key, roots, hidden, ScanFilter::Name(needle), origin)
    }

    /// Starts the Documents walk of `roots`: every text file under them, by
    /// name, dotfiles left out.
    pub fn documents(
        &self,
        key: String,
        roots: Vec<WirePath>,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        self.start_scan(key, roots, false, ScanFilter::Documents, origin)
    }

    fn start_scan(
        &self,
        key: String,
        roots: Vec<WirePath>,
        hidden: bool,
        filter: ScanFilter,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        let roots = roots
            .into_iter()
            .map(|p| PathBuf::try_from(p).map_err(WireError::from))
            .collect::<Result<Vec<_>, _>>()?;
        // Every entry sent carries its type.
        self.mime()?;
        self.spawn(
            Job::Scan {
                key,
                roots,
                hidden,
                filter,
            },
            origin,
        )
    }

    /// Runs a walk for operation `op`, sending its batches.
    pub(crate) fn run_scan(
        &self,
        op: OpId,
        progress: &dyn Progress,
        key: String,
        roots: &[PathBuf],
        hidden: bool,
        filter: &ScanFilter,
    ) -> Result<WireScanDone, WireError> {
        let db = self.mime()?;
        let opts = ScanOptions {
            include_hidden: hidden,
            ..ScanOptions::default()
        };
        let found = Mutex::new(0u64);
        let send = |entries: Vec<WireEntry>| {
            let event = SearchHitEvent {
                key: key.clone(),
                op,
                entries,
            };
            // As for every event: emit fails only on a payload serde cannot
            // serialize, and this is plain data.
            let _ = self.0.app.emit(EVENT_SEARCH_HIT, event);
        };
        let report = stream_batches(SEARCH_BATCH, SEARCH_FLUSH, &send, |sink| {
            let keep = |e: &Entry| {
                let hit = match filter {
                    ScanFilter::Name(q) => e.name.to_lowercase().contains(q.as_str()),
                    ScanFilter::Documents => is_document(e, db),
                };
                if hit {
                    *lock(&found) += 1;
                    sink(WireEntry::new(e.clone(), db));
                }
                // Never kept in the report: sent in batches instead.
                false
            };
            scan(roots, &opts, &keep, progress)
        });
        let found = *lock(&found);
        Ok(WireScanDone {
            found,
            unreadable: report.unreadable.iter().map(WirePath::from).collect(),
            cancelled: report.cancelled,
        })
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::mpsc;
    use std::time::Duration;

    use super::stream_batches;

    /// Longer than any test waits for a flush it expects, shorter than the
    /// time a hung test is worth waiting for.
    const PATIENCE: Duration = Duration::from_secs(20);

    #[test]
    fn the_first_hits_arrive_before_the_walk_ends() {
        let (sent, received) = mpsc::channel::<Vec<u32>>();
        let sent = std::sync::Mutex::new(sent);
        let send = move |batch: Vec<u32>| {
            let _ = sent.lock().unwrap().send(batch);
        };
        // The walk finds two hits, then waits for them to be sent: with
        // batches sent only when full, it waits for ever.
        let first = stream_batches(200, Duration::from_millis(20), &send, |sink| {
            sink(1);
            sink(2);
            received.recv_timeout(PATIENCE)
        });
        assert_eq!(first.unwrap(), [1, 2]);
        // Nothing was left over for the end.
        assert!(received.try_recv().is_err());
    }

    /// An interval no test reaches, so only the size sends.
    const NEVER: Duration = Duration::from_secs(3600);

    #[test]
    fn full_batches_go_out_at_the_size_and_the_rest_at_the_end() {
        let batches = std::sync::Mutex::new(Vec::<Vec<u32>>::new());
        let send = |batch: Vec<u32>| batches.lock().unwrap().push(batch);
        let walked = stream_batches(2, NEVER, &send, |sink| {
            for i in 1..=5 {
                sink(i);
            }
            "walked"
        });
        assert_eq!(walked, "walked");
        assert_eq!(*batches.lock().unwrap(), [vec![1, 2], vec![3, 4], vec![5]]);
    }

    #[test]
    fn a_walk_that_panics_does_not_leave_the_timer_running() {
        let send = |_: Vec<u32>| {};
        // The timer is joined when the walk unwinds; a timer left waiting
        // for its next tick would hang here.
        let outcome = std::panic::catch_unwind(|| {
            stream_batches(2, NEVER, &send, |sink| {
                sink(1);
                panic!("walk failed");
            })
        });
        assert!(outcome.is_err());
    }
}
