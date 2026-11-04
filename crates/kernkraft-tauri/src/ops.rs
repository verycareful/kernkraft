// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Background file operations: copy, move, trash, permanent delete, empty
//! trash and properties.
//!
//! [`Kernkraft::start_op`] checks its arguments, registers the operation
//! and returns its [`OpId`] at once; a worker on Tauri's blocking pool does
//! the work and reports through events:
//!
//! - [`EVENT_OP_PROGRESS`] ([`OpProgressEvent`]), at most one per
//!   [`PROGRESS_INTERVAL_MS`] per operation, plus a final one;
//! - [`EVENT_OP_CONFLICT`] ([`OpConflictEvent`]) when a name is taken at
//!   the destination. The worker blocks until
//!   [`Kernkraft::answer_conflict`] answers it, or until the operation is
//!   cancelled ([`Kernkraft::cancel_op`], or the window that started it
//!   closing, or the app exiting), which releases it;
//! - [`EVENT_OP_DONE`] ([`OpDoneEvent`]) once, last, with every item's
//!   outcome. The operation leaves the registry before this event, so an
//!   undo asked for on receiving it is not refused as busy.
//!
//! Copy, move and trash run item by item. A move takes each item's prior
//! stamps for the undo journal right before that item moves, and a trash
//! right before that item is trashed, so a chain of operations on one item
//! stays undoable (see `kernkraft_fs::Journal::prior_stamps`).
//!
//! Every registered operation, `properties` included, makes undo and redo
//! return `Busy`: a properties walk is short next to a user reaching for
//! undo. The exceptions are search and the Documents walk (see
//! `search_cmds`), registered as readers: they can run for minutes over a
//! home folder, only read, and neither wait for nor hold back an undo.
//! Operations may run side by side; each item's failure, a source removed
//! by another operation included, is in that operation's report.
//!
//! Rename, create_dir and create_file are refused with `Busy` while a
//! non-reader operation runs (copy, move, trash, delete, empty trash,
//! properties), as undo and redo are, and the other way round. An operation
//! is journalled when it ends, with each placed item stamped then, so a
//! rename of an item it had placed, or a new file at a name it had used,
//! would make its undo act on something it did not place or find the item
//! gone. A program outside Kern can still do either; the undo then stops
//! with `Changed` rather than act on a file that differs from the one
//! recorded. Search and the Documents walk only read and never block these.
//! A rename or create lasts moments, and an operation or an undo that starts
//! meanwhile is refused with `Busy`; no lock of the registry is held while it
//! works, so cancelling, answering a conflict and the exit never wait for it.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use kernkraft_fs::{
    copy, delete_permanently, measure, move_items, undoable_from_report, Change, Conflict, Entry,
    FsError, IrreversibleReason, OpKind, OpReport, PriorStamps, Progress, Trash, TrashListing,
    TrashedItem, Undoable,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError, WireItemError};
use crate::fs_cmds::{describe, WireEntry};
use crate::search_cmds::{ScanFilter, WireScanDone};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

/// Identifies a running operation for its events, [`Kernkraft::cancel_op`]
/// and [`Kernkraft::answer_conflict`]. Counted from 1 per app; well inside
/// the integers a JavaScript number holds exactly.
pub type OpId = u64;

pub const EVENT_OP_PROGRESS: &str = "kernkraft://op-progress";
pub const EVENT_OP_CONFLICT: &str = "kernkraft://op-conflict";
pub const EVENT_OP_DONE: &str = "kernkraft://op-done";

/// Least time between two progress events of one operation: ten updates a
/// second is smooth for a progress bar and cheap for IPC.
pub const PROGRESS_INTERVAL_MS: u64 = 100;

/// Name of the directory holding trashed items inside a trash directory
/// (FreeDesktop Trash 1.0, "Contents of a trash directory").
const TRASH_FILES_DIR: &str = "files";

/// Payload of [`EVENT_OP_PROGRESS`]. `unit` says what `done` and `total`
/// count: `"bytes"` (copy, move), `"items"` (trash, delete, empty trash) or
/// `"entries"` (properties, whose `total` is 0: unknown until the walk
/// ends). `stage` is the path being worked on, for display only: bytes that
/// are not UTF-8 are shown as U+FFFD.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpProgressEvent {
    pub op: OpId,
    pub done: u64,
    pub total: u64,
    pub unit: String,
    /// Display-only text: the path's bytes decoded lossily (U+FFFD for any
    /// that are not UTF-8). Never turn it back into a path; use the
    /// `WirePath`s of the report.
    pub stage: String,
}

/// Payload of [`EVENT_OP_CONFLICT`]: `source` would land on `existing`.
/// Each entry is `None` when it could not be read at the moment of asking
/// (removed meanwhile, or unreadable).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpConflictEvent {
    pub op: OpId,
    pub conflict: u64,
    pub source: WirePath,
    pub existing: WirePath,
    pub source_entry: Option<WireEntry>,
    pub existing_entry: Option<WireEntry>,
}

/// One completed item: its source, and where it ended up for an operation
/// that places it somewhere (`None` for a delete or purge).
///
/// By operation: copy and move, the source as given and the new path;
/// trash, the item's absolute original path and its path in the trash;
/// restore, its path in the trash and the path it is back at; delete, the
/// path; purge and empty trash, the item's original path.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireDone {
    pub source: WirePath,
    pub dest: Option<WirePath>,
}

/// Every item's outcome of an operation, for the UI to show in full.
///
/// `not_reached` lists the items a cancel stopped: never started, or
/// interrupted (a copy interrupted midway leaves nothing behind; a folder
/// merge interrupted midway keeps what it had merged). `cancelled` is set
/// when the operation was cancelled at all.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireReport {
    pub done: Vec<WireDone>,
    pub skipped: Vec<WirePath>,
    pub failed: Vec<WireItemError>,
    pub warnings: Vec<WireItemError>,
    pub cancelled: bool,
    pub not_reached: Vec<WirePath>,
}

impl WireReport {
    pub(crate) fn new(report: &OpReport, not_reached: &[PathBuf]) -> WireReport {
        WireReport {
            done: report
                .done
                .iter()
                .map(|(s, d)| WireDone {
                    source: WirePath::from(s),
                    dest: d.as_ref().map(WirePath::from),
                })
                .collect(),
            skipped: report.skipped.iter().map(WirePath::from).collect(),
            failed: report
                .failed
                .iter()
                .map(|(p, e)| WireItemError::new(p, e))
                .collect(),
            warnings: report
                .warnings
                .iter()
                .map(|(p, e)| WireItemError::new(p, e))
                .collect(),
            cancelled: report.cancelled,
            not_reached: not_reached.iter().map(WirePath::from).collect(),
        }
    }
}

/// What `properties` found (see `kernkraft_fs::measure`): files (every
/// entry that is not a directory, symlinks included and never followed),
/// directories, bytes of regular files (hard links once), and the paths
/// that could not be read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireProperties {
    pub files: u64,
    pub dirs: u64,
    pub bytes: u64,
    pub unreadable: Vec<WirePath>,
    pub cancelled: bool,
}

/// Payload of [`EVENT_OP_DONE`]. `report` is set for the file operations,
/// `properties` for properties, `scan` for a search or Documents walk.
/// `error` is set instead of any of them when the worker itself failed (a
/// panic, which is a bug, or a walk whose MIME database is unavailable);
/// nothing of the operation's outcome is known then beyond the files
/// themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpDoneEvent {
    pub op: OpId,
    pub report: Option<WireReport>,
    pub properties: Option<WireProperties>,
    #[serde(default)]
    pub scan: Option<WireScanDone>,
    pub error: Option<WireError>,
}

/// The running operations, and whether an undo or redo runs.
#[derive(Default)]
pub(crate) struct Registry {
    last: OpId,
    ops: HashMap<OpId, Arc<OpHandle>>,
    undo_running: bool,
    /// Renames and creates under way (see [`Kernkraft::admit_edit`]).
    edits: usize,
    /// Background workers not yet finished, op-done sent included.
    workers: usize,
}

/// Longest the app's exit waits for cancelled workers. A cancelled copy
/// stops within one 1 MiB chunk and then removes its temp file or the
/// folder it was building; two seconds covers that on a slow disk, while a
/// worker stuck in the kernel (a hung network mount) cannot hold the exit
/// open for good.
pub(crate) const EXIT_WAIT: Duration = Duration::from_secs(2);

/// Counts a background worker until dropped, at the very end of its thread.
struct Worker<R: Runtime> {
    kk: Kernkraft<R>,
}

impl<R: Runtime> Drop for Worker<R> {
    fn drop(&mut self) {
        lock(&self.kk.0.registry).workers -= 1;
        self.kk.0.idle.notify_all();
    }
}

/// A running operation's cancel flag and conflict prompt.
pub(crate) struct OpHandle {
    id: OpId,
    /// Label of the window that started it, if any.
    origin: Option<String>,
    /// Only reads: does not make undo wait, and may start while one runs.
    reader: bool,
    cancelled: AtomicBool,
    prompt: Mutex<Prompt>,
    wake: Condvar,
}

#[derive(Default)]
struct Prompt {
    last: u64,
    /// The conflict the worker waits on.
    open: Option<u64>,
    answer: Option<(Conflict, bool)>,
    /// A choice given with "apply to all": every later conflict of the
    /// operation takes it without asking.
    for_all: Option<Conflict>,
}

impl OpHandle {
    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Sets the cancel flag and wakes a worker waiting on the prompt. The
    /// prompt lock is taken before notifying, so a worker between checking
    /// the flag and waiting cannot miss the wake-up.
    fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
        let _prompt = lock(&self.prompt);
        self.wake.notify_all();
    }
}

/// Keeps an operation in the registry; dropping it (at the end of the work,
/// or while a panic unwinds) takes the operation out.
pub(crate) struct Running<R: Runtime> {
    kk: Kernkraft<R>,
    handle: Arc<OpHandle>,
}

impl<R: Runtime> Drop for Running<R> {
    fn drop(&mut self) {
        lock(&self.kk.0.registry).ops.remove(&self.handle.id);
    }
}

/// Marks an undo or redo as running until dropped.
pub(crate) struct UndoGuard<R: Runtime> {
    kk: Kernkraft<R>,
}

impl<R: Runtime> Drop for UndoGuard<R> {
    fn drop(&mut self) {
        lock(&self.kk.0.registry).undo_running = false;
    }
}

/// What `Busy` says to an operation or an undo that starts during a rename
/// or create, which lasts moments.
const EDIT_RUNNING: &str = "a rename or create is running; try again in a moment";

/// Counts a rename or create as under way until dropped, also when it
/// unwinds. It holds no lock: the registry is free while the edit works.
pub(crate) struct EditGuard<R: Runtime> {
    kk: Kernkraft<R>,
}

impl<R: Runtime> Drop for EditGuard<R> {
    fn drop(&mut self) {
        let mut registry = lock(&self.kk.0.registry);
        registry.edits = registry.edits.saturating_sub(1);
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Mode {
    Copy,
    Move,
}

pub(crate) enum Job {
    Transfer {
        mode: Mode,
        sources: Vec<PathBuf>,
        dest: PathBuf,
    },
    Trash {
        trash: Trash,
        sources: Vec<PathBuf>,
    },
    Delete {
        sources: Vec<PathBuf>,
    },
    EmptyTrash {
        trash: Trash,
        listing: TrashListing,
        seen: Vec<Seen>,
    },
    Properties {
        paths: Vec<PathBuf>,
    },
    Scan {
        key: String,
        roots: Vec<PathBuf>,
        hidden: bool,
        filter: ScanFilter,
    },
}

enum Outcome {
    Report(WireReport),
    Properties(WireProperties),
    Scan(WireScanDone),
    Failed(WireError),
}

fn invalid(what: &'static str, value: &str) -> WireError {
    WireError::from(PluginError::InvalidArgument {
        what,
        value: value.to_owned(),
    })
}

fn decode_all(paths: Vec<WirePath>) -> Result<Vec<PathBuf>, WireError> {
    paths
        .into_iter()
        .map(|p| PathBuf::try_from(p).map_err(WireError::from))
        .collect()
}

/// Fails unless `dest` is a directory.
pub(crate) fn check_dest(dest: &Path) -> Result<(), WireError> {
    let meta = std::fs::metadata(dest).map_err(|e| {
        WireError::from(if e.kind() == std::io::ErrorKind::NotFound {
            FsError::NotFound {
                path: dest.to_path_buf(),
            }
        } else {
            FsError::Io {
                path: dest.to_path_buf(),
                source: e,
            }
        })
    })?;
    if meta.is_dir() {
        Ok(())
    } else {
        Err(WireError::from(FsError::Io {
            path: dest.to_path_buf(),
            source: std::io::Error::from(std::io::ErrorKind::NotADirectory),
        }))
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// Starts a background operation. `kind` is `"copy"` or `"move"` (both
    /// need `dest`, an existing directory), `"trash"`, `"delete"` (for good)
    /// or `"empty_trash"` (no sources). `origin` is the label of the window
    /// asking; closing that window cancels the operation.
    pub fn start_op(
        &self,
        kind: &str,
        sources: Vec<WirePath>,
        dest: Option<WirePath>,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        let sources = decode_all(sources)?;
        let dest = dest
            .map(|d| PathBuf::try_from(d).map_err(WireError::from))
            .transpose()?;
        let mode = match kind {
            "copy" => Some(Mode::Copy),
            "move" => Some(Mode::Move),
            "trash" | "delete" | "empty_trash" => None,
            other => return Err(invalid("operation kind", other)),
        };
        let job = match (mode, dest) {
            (Some(mode), Some(dest)) => {
                check_dest(&dest)?;
                // A conflict prompt shows both entries with their types.
                self.mime()?;
                Job::Transfer {
                    mode,
                    sources,
                    dest,
                }
            }
            (Some(_), None) => return Err(invalid("destination", "none given for a copy or move")),
            (None, Some(dest)) => {
                return Err(invalid(
                    "destination",
                    &format!("{} given for {kind}", dest.display()),
                ))
            }
            (None, None) => match kind {
                "trash" => Job::Trash {
                    trash: self.trash()?.clone(),
                    sources,
                },
                "delete" => Job::Delete { sources },
                _ => {
                    if let Some(first) = sources.first() {
                        return Err(invalid("source", &first.to_string_lossy()));
                    }
                    let trash = self.trash()?.clone();
                    let listing = trash.list().map_err(WireError::from)?;
                    let seen = listing.items.iter().map(Seen::of).collect();
                    Job::EmptyTrash {
                        trash,
                        listing,
                        seen,
                    }
                }
            },
        };
        self.spawn(job, origin)
    }

    /// Starts counting the files, folders and bytes in `paths`, in the
    /// background like [`Kernkraft::start_op`]. Its `op-done` carries
    /// `properties`.
    pub fn properties(
        &self,
        paths: Vec<WirePath>,
        origin: Option<String>,
    ) -> Result<OpId, WireError> {
        let paths = decode_all(paths)?;
        self.spawn(Job::Properties { paths }, origin)
    }

    /// Answers conflict `conflict` of operation `op` with `choice`
    /// (`"skip"`, `"replace"` or `"keep_both"`); with `apply_to_all`, every
    /// later conflict of the operation takes the same choice unasked.
    pub fn answer_conflict(
        &self,
        op: OpId,
        conflict: u64,
        choice: &str,
        apply_to_all: bool,
    ) -> Result<(), WireError> {
        let choice = match choice {
            "skip" => Conflict::Skip,
            "replace" => Conflict::Replace,
            "keep_both" => Conflict::KeepBoth,
            other => return Err(invalid("conflict choice", other)),
        };
        let handle = self.handle(op)?;
        let mut prompt = lock(&handle.prompt);
        if prompt.open != Some(conflict) || prompt.answer.is_some() {
            return Err(WireError::from(PluginError::UnknownConflict {
                op,
                conflict,
            }));
        }
        prompt.answer = Some((choice, apply_to_all));
        handle.wake.notify_all();
        Ok(())
    }

    /// Cancels operation `op`. It stops at its next safe point (between
    /// chunks of a file, or between items) and reports what it did.
    pub fn cancel_op(&self, op: OpId) -> Result<(), WireError> {
        self.handle(op)?.cancel();
        Ok(())
    }

    /// Waits until every background worker has finished, at most
    /// `limit`. Returns whether they all did.
    pub(crate) fn wait_for_workers(&self, limit: Duration) -> bool {
        let deadline = Instant::now() + limit;
        let mut registry = lock(&self.0.registry);
        while registry.workers > 0 {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            registry = self
                .0
                .idle
                .wait_timeout(registry, deadline - now)
                .map(|(guard, _)| guard)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        true
    }

    pub(crate) fn cancel_from(&self, label: Option<&str>) {
        let handles: Vec<Arc<OpHandle>> = lock(&self.0.registry)
            .ops
            .values()
            .filter(|h| label.is_none() || h.origin.as_deref() == label)
            .cloned()
            .collect();
        for h in handles {
            h.cancel();
        }
    }

    fn handle(&self, op: OpId) -> Result<Arc<OpHandle>, WireError> {
        lock(&self.0.registry)
            .ops
            .get(&op)
            .cloned()
            .ok_or_else(|| WireError::from(PluginError::UnknownOp(op)))
    }

    /// Registers an operation. Refused while an undo, a redo or a rename or
    /// create runs.
    pub(crate) fn register(&self, origin: Option<String>) -> Result<Running<R>, WireError> {
        self.register_as(origin, false)
    }

    /// Registers an operation; a `reader` is not refused while an undo, a
    /// redo or a rename or create runs.
    fn register_as(&self, origin: Option<String>, reader: bool) -> Result<Running<R>, WireError> {
        let mut registry = lock(&self.0.registry);
        if registry.undo_running && !reader {
            return Err(WireError::from(PluginError::Busy(
                "an undo or redo is running",
            )));
        }
        if registry.edits > 0 && !reader {
            return Err(WireError::from(PluginError::Busy(EDIT_RUNNING)));
        }
        registry.last += 1;
        let handle = Arc::new(OpHandle {
            id: registry.last,
            origin,
            reader,
            cancelled: AtomicBool::new(false),
            prompt: Mutex::new(Prompt::default()),
            wake: Condvar::new(),
        });
        registry.ops.insert(handle.id, handle.clone());
        Ok(Running {
            kk: self.clone(),
            handle,
        })
    }

    /// Marks an undo or redo as running. Refused while any operation, or
    /// another undo or redo, runs.
    pub(crate) fn begin_undo(&self) -> Result<UndoGuard<R>, WireError> {
        let mut registry = lock(&self.0.registry);
        if registry.undo_running {
            return Err(WireError::from(PluginError::Busy(
                "an undo or redo is running",
            )));
        }
        if registry.ops.values().any(|h| !h.reader) {
            return Err(WireError::from(PluginError::Busy(
                "a file operation is running; undo and redo wait until it ends",
            )));
        }
        if registry.edits > 0 {
            return Err(WireError::from(PluginError::Busy(EDIT_RUNNING)));
        }
        registry.undo_running = true;
        Ok(UndoGuard { kk: self.clone() })
    }

    /// Admits a rename or a create, which is refused with `Busy` while an
    /// undo, a redo or any file operation runs (search and the Documents
    /// walk, which only read, never count). An undo holds the journal from
    /// its check to its last action, a move back across filesystems can take
    /// minutes, and an operation is journalled when it ends, so an item
    /// renamed while the operation runs is not where its entry says it is
    /// and a name reused meanwhile would be taken for the item the operation
    /// placed.
    ///
    /// The check and the count happen in one critical section of the
    /// registry, which is released before the edit touches the file system:
    /// the edit may sit on a hung mount, and no other use of the registry
    /// (cancelling, answering, the exit's wait) may wait for it. While the
    /// returned guard lives, an operation or an undo that tries to start is
    /// refused with `Busy` instead, so none starts between the check and the
    /// edit.
    pub(crate) fn admit_edit(&self) -> Result<EditGuard<R>, WireError> {
        let mut registry = lock(&self.0.registry);
        if registry.undo_running {
            return Err(WireError::from(PluginError::Busy(
                "an undo or redo is running; wait for it to finish",
            )));
        }
        if registry.ops.values().any(|h| !h.reader) {
            return Err(WireError::from(PluginError::Busy(
                "a file operation is running; wait for it to finish",
            )));
        }
        registry.edits += 1;
        Ok(EditGuard { kk: self.clone() })
    }

    pub(crate) fn spawn(&self, job: Job, origin: Option<String>) -> Result<OpId, WireError> {
        let reader = matches!(job, Job::Scan { .. });
        let running = self.register_as(origin, reader)?;
        let id = running.handle.id;
        let kk = self.clone();
        lock(&self.0.registry).workers += 1;
        let counted = Worker { kk: self.clone() };
        // Detached: the worker reports through events, never a return value.
        drop(tauri::async_runtime::spawn_blocking(move || {
            // Dropped last, after op-done is out.
            let _counted = counted;
            let worker = kk.clone();
            let result = catch_unwind(AssertUnwindSafe(move || {
                let handle = running.handle.clone();
                let outcome = worker.run(&handle, job);
                // Out of the registry before op-done goes out.
                drop(running);
                outcome
            }));
            let mut done = OpDoneEvent {
                op: id,
                report: None,
                properties: None,
                scan: None,
                error: None,
            };
            match result {
                Ok(Outcome::Report(r)) => done.report = Some(r),
                Ok(Outcome::Properties(p)) => done.properties = Some(p),
                Ok(Outcome::Scan(s)) => done.scan = Some(s),
                Ok(Outcome::Failed(e)) => done.error = Some(e),
                Err(panic) => {
                    let message = panic
                        .downcast_ref::<&str>()
                        .map(|s| (*s).to_owned())
                        .or_else(|| panic.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "worker panicked".to_owned());
                    done.error = Some(WireError::from(PluginError::Internal(message)));
                }
            }
            emit(&kk, EVENT_OP_DONE, done);
        }));
        Ok(id)
    }

    fn run(&self, op: &OpHandle, job: Job) -> Outcome {
        match job {
            Job::Transfer {
                mode,
                sources,
                dest,
            } => {
                let progress = Throttle::new(self, op, "bytes");
                Outcome::Report(self.transfer(op, &progress, mode, &sources, &dest))
            }
            Job::Trash { trash, sources } => {
                let progress = Throttle::new(self, op, "items");
                Outcome::Report(self.trash_items(op, &progress, &trash, &sources))
            }
            Job::Delete { sources } => {
                let progress = Throttle::new(self, op, "items");
                let report = delete_permanently(&sources, &progress);
                progress.finish();
                let not_reached = unaccounted(&report, &sources);
                self.record_irreversible(&report, || {
                    let paths: Vec<PathBuf> = report.done.iter().map(|(s, _)| s.clone()).collect();
                    format!("delete of {}", describe(&paths))
                });
                Outcome::Report(WireReport::new(&report, &not_reached))
            }
            Job::EmptyTrash {
                trash,
                listing,
                seen,
            } => {
                let progress = Throttle::new(self, op, "items");
                Outcome::Report(self.empty_trash(op, &progress, &trash, listing, &seen))
            }
            Job::Properties { paths } => {
                let progress = Throttle::new(self, op, "entries");
                let m = measure(&paths, &progress);
                progress.finish();
                Outcome::Properties(WireProperties {
                    files: m.files,
                    dirs: m.dirs,
                    bytes: m.bytes,
                    unreadable: m.unreadable.iter().map(WirePath::from).collect(),
                    cancelled: m.cancelled,
                })
            }
            Job::Scan {
                key,
                roots,
                hidden,
                filter,
            } => {
                let progress = Throttle::new(self, op, "entries");
                let done = self.run_scan(op.id, &progress, key, &roots, hidden, &filter);
                progress.finish();
                match done {
                    Ok(s) => Outcome::Scan(s),
                    Err(e) => Outcome::Failed(e),
                }
            }
        }
    }

    /// Copies or moves each source on its own, so a move can take each
    /// item's prior stamps right before it moves.
    fn transfer(
        &self,
        op: &OpHandle,
        progress: &Throttle<'_, R>,
        mode: Mode,
        sources: &[PathBuf],
        dest: &Path,
    ) -> WireReport {
        let total = measure(sources, &CancelOnly(op)).bytes;
        let asker = Asker {
            kk: self,
            op,
            cancel_skipped: Mutex::new(Vec::new()),
        };
        let resolve = |src: &Path, existing: &Path| asker.ask(src, existing);
        let mut report = OpReport::default();
        let mut not_reached: Vec<PathBuf> = Vec::new();
        let mut prior = PriorStamps::default();
        let mut base = 0u64;
        for (i, src) in sources.iter().enumerate() {
            if op.is_cancelled() {
                not_reached.extend_from_slice(&sources[i..]);
                break;
            }
            let one = std::slice::from_ref(src);
            if mode == Mode::Move {
                prior.merge(lock(&self.0.journal).prior_stamps(one));
            }
            let offset = Offset {
                inner: progress,
                base,
                total,
                item_total: AtomicU64::new(0),
            };
            let result = match mode {
                Mode::Copy => copy(one, dest, &resolve, &offset),
                Mode::Move => move_items(one, dest, &resolve, &offset),
            };
            base += offset.item_total.load(Ordering::SeqCst);
            match result {
                Ok(r) => {
                    let interrupted = r.cancelled;
                    absorb(&mut report, r);
                    if interrupted {
                        not_reached.extend_from_slice(&sources[i..]);
                        break;
                    }
                }
                Err(e) => report.failed.push((src.clone(), e)),
            }
        }
        // A prompt released by a cancel answered skip; that item was not
        // skipped by the user but stopped.
        let stopped = std::mem::take(&mut *lock(&asker.cancel_skipped));
        report.skipped.retain(|p| !stopped.contains(p));
        for p in stopped {
            if !not_reached.contains(&p) {
                not_reached.insert(0, p);
            }
        }
        report.cancelled = op.is_cancelled();
        progress.finish();

        let kind = match mode {
            Mode::Copy => OpKind::Copy,
            Mode::Move => OpKind::Move,
        };
        // Stamped before the journal is locked: stamping a large tree takes
        // a while, and a rename waits on the journal.
        // kernkraft-fs lists a replace when it happens, so one answered but
        // never carried out is not there, and one carried out before the
        // item failed or was cancelled is.
        let entry = undoable_from_report(kind, &report);
        lock(&self.0.journal).record_after(entry, prior);
        WireReport::new(&report, &not_reached)
    }

    fn trash_items(
        &self,
        op: &OpHandle,
        progress: &Throttle<'_, R>,
        trash: &Trash,
        sources: &[PathBuf],
    ) -> WireReport {
        let mut report = OpReport::default();
        let mut not_reached = Vec::new();
        let total = sources.len() as u64;
        let mut prior = PriorStamps::default();
        for (i, src) in sources.iter().enumerate() {
            if op.is_cancelled() {
                report.cancelled = true;
                not_reached.extend_from_slice(&sources[i..]);
                break;
            }
            progress.report(i as u64, total, &src.to_string_lossy());
            // The journal keys a trashed item by its absolute path with the
            // parent resolved, which is what the trash records; stamp it
            // under that path as well as the one given.
            let mut keys = vec![src.clone()];
            if let Some(real) = real_parent_path(src) {
                if &real != src {
                    keys.push(real);
                }
            }
            prior.merge(lock(&self.0.journal).prior_stamps(&keys));
            match trash.put(src) {
                Ok(item) => report.done.push((item.original.clone(), Some(held(&item)))),
                Err(e) => report.failed.push((src.clone(), e)),
            }
        }
        progress.report(total, total, "");
        progress.finish();
        let entry = undoable_from_report(OpKind::Trash, &report);
        lock(&self.0.journal).record_after(entry, prior);
        WireReport::new(&report, &not_reached)
    }

    fn empty_trash(
        &self,
        op: &OpHandle,
        progress: &Throttle<'_, R>,
        trash: &Trash,
        listing: TrashListing,
        seen: &[Seen],
    ) -> WireReport {
        let mut report = OpReport::default();
        let mut not_reached = Vec::new();
        // An info file that could not be read leaves its item in the trash;
        // the user is told the trash is not fully empty.
        report.failed.extend(listing.unreadable);
        let total = listing.items.len() as u64;
        for (i, (item, seen)) in listing.items.iter().zip(seen).enumerate() {
            if op.is_cancelled() {
                report.cancelled = true;
                not_reached.extend(listing.items[i..].iter().map(|it| it.original.clone()));
                break;
            }
            progress.report(i as u64, total, &item.original.to_string_lossy());
            match seen.check().and_then(|()| trash.purge(item)) {
                Ok(()) => report.done.push((item.original.clone(), None)),
                Err(e) => report.failed.push((item.original.clone(), e)),
            }
        }
        progress.report(total, total, "");
        progress.finish();
        self.record_irreversible(&report, || "empty trash".to_owned());
        WireReport::new(&report, &not_reached)
    }

    /// Records a permanent deletion, so the next undo says it cannot be
    /// undone instead of reaching past it. Nothing deleted, nothing
    /// recorded.
    pub(crate) fn record_irreversible(&self, report: &OpReport, what: impl FnOnce() -> String) {
        if report.done.is_empty() {
            return;
        }
        lock(&self.0.journal).record(Undoable::Irreversible {
            what: what(),
            reason: IrreversibleReason::PermanentDelete,
        });
    }

    /// Runs `f` registered as an operation without events, so undo and redo
    /// are refused while it runs and it is refused while they run.
    pub(crate) fn run_registered<T>(
        &self,
        f: impl FnOnce() -> Result<T, WireError>,
    ) -> Result<T, WireError> {
        let _running = self.register(None)?;
        f()
    }
}

/// Name of the directory of info files inside a trash directory, and their
/// suffix (FreeDesktop Trash 1.0, "Contents of a trash directory").
const TRASH_INFO_DIR: &str = "info";
const TRASH_INFO_SUFFIX: &str = ".trashinfo";

/// Device, inode and change time of an entry, not following a symlink. A
/// trash name is reused once its entry is purged, and so can an inode
/// number; a new entry under the same name has a new change time as well.
type Ident = (u64, u64, i64, i64);

fn ident(path: &Path) -> std::io::Result<Ident> {
    use std::os::unix::fs::MetadataExt;
    let m = std::fs::symlink_metadata(path)?;
    Ok((m.dev(), m.ino(), m.ctime(), m.ctime_nsec()))
}

/// A trash entry as it was listed: the identity of its file under `files/`
/// and of its info file. Emptying the trash lists it when asked and purges
/// later; an entry purged and replaced by a new item of the same name in
/// between must not be purged in its place.
pub(crate) struct Seen {
    held: PathBuf,
    held_id: Option<Ident>,
    info: PathBuf,
    info_id: Option<Ident>,
}

impl Seen {
    pub(crate) fn of(item: &TrashedItem) -> Seen {
        let held = held(item);
        let info = item
            .trash_dir
            .join(TRASH_INFO_DIR)
            .join(format!("{}{TRASH_INFO_SUFFIX}", item.name));
        Seen {
            held_id: ident(&held).ok(),
            info_id: ident(&info).ok(),
            held,
            info,
        }
    }

    /// Both files could be read.
    pub(crate) fn complete(&self) -> bool {
        self.held_id.is_some() && self.info_id.is_some()
    }

    /// The two identities as one opaque string, for the webview to hand
    /// back. A file that could not be read shows as `-`.
    pub(crate) fn id(&self) -> String {
        let part = |id: Option<Ident>| match id {
            Some((dev, ino, sec, nsec)) => format!("{dev}:{ino}:{sec}:{nsec}"),
            None => "-".to_owned(),
        };
        format!("{}/{}", part(self.held_id), part(self.info_id))
    }

    /// Fails unless both files are still the entries listed. An entry that
    /// could not be read when listed cannot be shown to be the same, and
    /// is refused.
    pub(crate) fn check(&self) -> Result<(), FsError> {
        for (path, then) in [(&self.held, self.held_id), (&self.info, self.info_id)] {
            match ident(path) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    return Err(FsError::NotFound { path: path.clone() })
                }
                Err(e) => {
                    return Err(FsError::Io {
                        path: path.clone(),
                        source: e,
                    })
                }
                Ok(now) if Some(now) != then => {
                    return Err(FsError::Changed {
                        path: path.clone(),
                        what: Change::Taken,
                    })
                }
                Ok(_) => {}
            }
        }
        Ok(())
    }
}

/// `<trash dir>/files/<name>`, where `item` is held.
pub(crate) fn held(item: &TrashedItem) -> PathBuf {
    item.trash_dir.join(TRASH_FILES_DIR).join(&item.name)
}

/// `path` with its parent directory resolved through symlinks, as the trash
/// records an original path; `None` when the parent cannot be resolved.
fn real_parent_path(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?;
    let parent = match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.canonicalize().ok()?,
        _ => std::env::current_dir().ok()?,
    };
    Some(parent.join(name))
}

/// The sources a cancelled single-call operation never reported on.
fn unaccounted(report: &OpReport, sources: &[PathBuf]) -> Vec<PathBuf> {
    if !report.cancelled {
        return Vec::new();
    }
    sources
        .iter()
        .filter(|s| {
            !report.done.iter().any(|(d, _)| d == *s)
                && !report.failed.iter().any(|(f, _)| f == *s)
                && !report.skipped.contains(s)
        })
        .cloned()
        .collect()
}

fn absorb(into: &mut OpReport, from: OpReport) {
    into.done.extend(from.done);
    into.skipped.extend(from.skipped);
    into.failed.extend(from.failed);
    into.warnings.extend(from.warnings);
    into.replaced.extend(from.replaced);
    into.cancelled |= from.cancelled;
}

fn emit<R: Runtime, S: Serialize + Clone>(kk: &Kernkraft<R>, event: &str, payload: S) {
    // emit fails only for an invalid event name or a payload serde cannot
    // serialize; the names are constants and the payloads plain data. A
    // window that has gone has no listener left to tell.
    let _ = kk.0.app.emit(event, payload);
}

/// Asks the UI about each conflict and waits for the answer.
struct Asker<'a, R: Runtime> {
    kk: &'a Kernkraft<R>,
    op: &'a OpHandle,
    /// Sources whose prompt a cancel released.
    cancel_skipped: Mutex<Vec<PathBuf>>,
}

impl<R: Runtime> Asker<'_, R> {
    fn ask(&self, src: &Path, existing: &Path) -> Conflict {
        let op = self.op;
        let mut prompt = lock(&op.prompt);
        if let Some(choice) = prompt.for_all {
            drop(prompt);
            return choice;
        }
        if op.is_cancelled() {
            drop(prompt);
            lock(&self.cancel_skipped).push(src.to_path_buf());
            return Conflict::Skip;
        }
        prompt.last += 1;
        let conflict = prompt.last;
        prompt.open = Some(conflict);
        prompt.answer = None;
        drop(prompt);

        let entry = |p: &Path| {
            let mime = self.kk.mime().ok()?;
            Entry::of(p).ok().map(|e| WireEntry::new(e, mime))
        };
        emit(
            self.kk,
            EVENT_OP_CONFLICT,
            OpConflictEvent {
                op: op.id,
                conflict,
                source: WirePath::from(src),
                existing: WirePath::from(existing),
                source_entry: entry(src),
                existing_entry: entry(existing),
            },
        );

        let mut prompt = lock(&op.prompt);
        let answer = loop {
            if let Some(answer) = prompt.answer.take() {
                break Some(answer);
            }
            if op.is_cancelled() {
                break None;
            }
            prompt = op.wake.wait(prompt).unwrap_or_else(PoisonError::into_inner);
        };
        prompt.open = None;
        match answer {
            Some((choice, all)) => {
                if all {
                    prompt.for_all = Some(choice);
                }
                drop(prompt);
                choice
            }
            None => {
                drop(prompt);
                lock(&self.cancel_skipped).push(src.to_path_buf());
                Conflict::Skip
            }
        }
    }
}

/// Progress that only passes on cancellation.
struct CancelOnly<'a>(&'a OpHandle);

impl Progress for CancelOnly<'_> {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {}

    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// Turns an operation's progress into events, at most one per
/// [`PROGRESS_INTERVAL_MS`], and passes on its cancel flag.
struct Throttle<'a, R: Runtime> {
    kk: &'a Kernkraft<R>,
    op: &'a OpHandle,
    unit: &'static str,
    state: Mutex<ThrottleState>,
}

#[derive(Default)]
struct ThrottleState {
    last: Option<Instant>,
    done: u64,
    total: u64,
    stage: String,
}

impl<'a, R: Runtime> Throttle<'a, R> {
    fn new(kk: &'a Kernkraft<R>, op: &'a OpHandle, unit: &'static str) -> Self {
        Throttle {
            kk,
            op,
            unit,
            state: Mutex::new(ThrottleState::default()),
        }
    }

    fn payload(&self, s: &ThrottleState) -> OpProgressEvent {
        OpProgressEvent {
            op: self.op.id,
            done: s.done,
            total: s.total,
            unit: self.unit.to_owned(),
            stage: s.stage.clone(),
        }
    }

    /// The final update, sent whatever the time since the last one.
    fn finish(&self) {
        let payload = self.payload(&lock(&self.state));
        emit(self.kk, EVENT_OP_PROGRESS, payload);
    }
}

impl<R: Runtime> Progress for Throttle<'_, R> {
    fn report(&self, done: u64, total: u64, stage: &str) {
        let interval = Duration::from_millis(PROGRESS_INTERVAL_MS);
        let payload = {
            let mut s = lock(&self.state);
            s.done = done;
            // A total read before the work can fall short of what the work
            // finds; the bar never runs past its end.
            s.total = if total == 0 { 0 } else { total.max(done) };
            if !stage.is_empty() {
                stage.clone_into(&mut s.stage);
            }
            let now = Instant::now();
            if s.last.is_some_and(|t| now.duration_since(t) < interval) {
                return;
            }
            s.last = Some(now);
            self.payload(&s)
        };
        emit(self.kk, EVENT_OP_PROGRESS, payload);
    }

    fn is_cancelled(&self) -> bool {
        self.op.is_cancelled()
    }
}

/// One item's progress inside a whole operation's: `base` bytes done before
/// it, out of `total` for the operation.
struct Offset<'a, R: Runtime> {
    inner: &'a Throttle<'a, R>,
    base: u64,
    total: u64,
    /// The item's own total, as its last report gave it.
    item_total: AtomicU64,
}

impl<R: Runtime> Progress for Offset<'_, R> {
    fn report(&self, done: u64, total: u64, stage: &str) {
        self.item_total.store(total, Ordering::SeqCst);
        let total = self.total.max(self.base + total);
        self.inner.report(self.base + done, total, stage);
    }

    fn is_cancelled(&self) -> bool {
        self.inner.is_cancelled()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::fs;

    use kernkraft_fs::{FsError, Trash};

    use super::Seen;
    use crate::state::lock;

    #[test]
    fn a_same_named_entry_trashed_after_listing_is_not_the_one_seen() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let trash = Trash::with_home_trash(base.join("trash"));
        let a = base.join("a");
        fs::write(&a, "first").unwrap();
        trash.put(&a).unwrap();
        let listed = trash.list().unwrap().items;
        let seen = Seen::of(&listed[0]);
        assert!(seen.check().is_ok());

        // Purged by someone else, and a new `a` trashed under the freed name.
        trash.purge(&listed[0]).unwrap();
        fs::write(&a, "second").unwrap();
        let again = trash.put(&a).unwrap();
        assert_eq!(again.name, listed[0].name);

        assert!(matches!(seen.check(), Err(FsError::Changed { .. })));
        assert_eq!(trash.list().unwrap().items.len(), 1);
    }

    #[test]
    fn a_purged_entry_is_reported_missing() {
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let trash = Trash::with_home_trash(base.join("trash"));
        let a = base.join("a");
        fs::write(&a, "").unwrap();
        trash.put(&a).unwrap();
        let listed = trash.list().unwrap().items;
        let seen = Seen::of(&listed[0]);
        trash.purge(&listed[0]).unwrap();
        assert!(matches!(seen.check(), Err(FsError::NotFound { .. })));
    }

    /// While an undo runs it holds the journal, which a rename or a create
    /// must also take: they say so at once instead of waiting out the undo
    /// (a move back across filesystems can take minutes).
    #[test]
    fn rename_and_create_do_not_wait_out_an_undo() {
        use std::sync::mpsc;
        use std::time::Duration;

        use tauri::test::mock_app;

        use crate::state::{lock, Config, Kernkraft};
        use crate::wire::WirePath;

        let app = mock_app();
        let kk = Kernkraft::new(app.handle().clone(), Config::default());
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        let file = base.join("a");
        fs::write(&file, "").unwrap();

        // What an undo in progress looks like from outside: the flag set and
        // the journal held.
        let undo = kk.begin_undo().unwrap();
        let journal = lock(&kk.0.journal);

        let (answer, answered) = mpsc::channel();
        let asked = kk.clone();
        let (renamed, created, parent) = (
            WirePath::from(file.as_path()),
            WirePath::from(base.as_path()),
            WirePath::from(base.as_path()),
        );
        std::thread::spawn(move || {
            let rename = asked.rename(renamed, "b".to_owned()).unwrap_err().kind;
            let dir = asked.create_dir(created, "d".to_owned()).unwrap_err().kind;
            let file = asked.create_file(parent, "f".to_owned()).unwrap_err().kind;
            let _ = answer.send((rename, dir, file));
        });
        let kinds = answered.recv_timeout(Duration::from_secs(20));
        // Released before any assertion, so a failure does not leave the
        // helper thread waiting on the journal for good.
        drop(journal);
        drop(undo);

        assert_eq!(
            kinds.unwrap(),
            ("Busy".to_owned(), "Busy".to_owned(), "Busy".to_owned())
        );
        // Nothing was renamed or made.
        assert!(file.exists());
        assert!(!base.join("b").exists() && !base.join("d").exists() && !base.join("f").exists());
    }

    fn fixture() -> (
        tauri::App<tauri::test::MockRuntime>,
        crate::state::Kernkraft<tauri::test::MockRuntime>,
        tempfile::TempDir,
        std::path::PathBuf,
    ) {
        let app = tauri::test::mock_app();
        let kk =
            crate::state::Kernkraft::new(app.handle().clone(), crate::state::Config::default());
        let root = tempfile::tempdir().unwrap();
        let base = root.path().canonicalize().unwrap();
        (app, kk, root, base)
    }

    /// The kinds the three edits answer with: `Ok` or the error kind.
    fn edits(
        kk: &crate::state::Kernkraft<tauri::test::MockRuntime>,
        base: &std::path::Path,
        tag: &str,
    ) -> [String; 3] {
        use crate::wire::WirePath;
        let kind = |r: Result<WirePath, crate::error::WireError>| match r {
            Ok(_) => "Ok".to_owned(),
            Err(e) => e.kind,
        };
        let from = base.join(format!("from-{tag}"));
        fs::write(&from, "").unwrap();
        [
            kind(kk.rename(WirePath::from(from.as_path()), format!("to-{tag}"))),
            kind(kk.create_dir(WirePath::from(base), format!("dir-{tag}"))),
            kind(kk.create_file(WirePath::from(base), format!("file-{tag}"))),
        ]
    }

    /// A copy, move, trash, delete or properties operation in the registry
    /// makes rename and create `Busy`, with nothing renamed or made; the
    /// moment it leaves they go ahead.
    #[test]
    fn rename_and_create_are_refused_while_a_file_operation_runs() {
        let (_app, kk, _root, base) = fixture();
        let running = kk.register(None).unwrap();
        assert_eq!(edits(&kk, &base, "busy"), ["Busy", "Busy", "Busy"]);
        assert!(base.join("from-busy").exists());
        assert!(!base.join("to-busy").exists());
        assert!(!base.join("dir-busy").exists() && !base.join("file-busy").exists());
        let message = kk
            .rename(
                crate::wire::WirePath::from(base.join("from-busy").as_path()),
                "x".to_owned(),
            )
            .unwrap_err()
            .message;
        assert!(message.contains("wait for it to finish"), "{message}");

        drop(running);
        assert_eq!(edits(&kk, &base, "free"), ["Ok", "Ok", "Ok"]);
    }

    /// Search and the Documents walk only read: they never block an edit.
    #[test]
    fn rename_and_create_are_not_refused_while_only_a_reader_runs() {
        let (_app, kk, _root, base) = fixture();
        let _reader = kk.register_as(None, true).unwrap();
        assert_eq!(edits(&kk, &base, "reader"), ["Ok", "Ok", "Ok"]);
    }

    /// Runs `f` on another thread and gives its answer, or `None` when it
    /// has not answered in twenty seconds (it is then blocked; the thread is
    /// left to finish when the lock it waits on is released).
    fn within_patience<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Option<T> {
        let (answer, answered) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let _ = answer.send(f());
        });
        answered
            .recv_timeout(std::time::Duration::from_secs(20))
            .ok()
    }

    /// An edit in progress (the registry's count, held by the guard) refuses
    /// the start of a file operation and of an undo or redo, and the guard
    /// lifts it, also when the edit unwinds.
    #[test]
    fn a_running_edit_refuses_an_operation_and_an_undo() {
        let (_app, kk, _root, _base) = fixture();
        let edit = kk.admit_edit().unwrap();
        assert_eq!(kk.register(None).err().unwrap().kind, "Busy");
        assert_eq!(kk.begin_undo().err().unwrap().kind, "Busy");
        // A reader never conflicts with an edit.
        drop(kk.register_as(None, true).unwrap());
        drop(edit);
        drop(kk.register(None).unwrap());
        drop(kk.begin_undo().unwrap());

        let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _edit = kk.admit_edit().unwrap();
            panic!("the edit fails");
        }));
        assert!(unwound.is_err());
        drop(kk.register(None).unwrap());
    }

    /// While an edit is under way nothing else in the registry waits for
    /// it: it may be stuck on a hung mount, and cancelling an operation,
    /// answering its conflict, registering a reader and the exit's wait for
    /// workers all go on.
    #[test]
    fn the_registry_does_not_wait_for_an_edit_in_progress() {
        let (_app, kk, _root, _base) = fixture();
        let reader = kk.register_as(None, true).unwrap();
        let op = reader.handle.id;
        let edit = kk.admit_edit().unwrap();
        lock(&kk.0.registry).workers += 1;

        let asked = kk.clone();
        let answers = within_patience(move || {
            let cancelled = asked.cancel_op(op).is_ok();
            let answered = asked
                .answer_conflict(op, 1, "skip", false)
                .unwrap_err()
                .kind;
            asked.cancel_from(Some("no-such-window"));
            let second_reader = asked.register_as(None, true).is_ok();
            let started = std::time::Instant::now();
            let all_done = asked.wait_for_workers(std::time::Duration::from_millis(200));
            (
                cancelled,
                answered,
                second_reader,
                all_done,
                started.elapsed(),
            )
        });
        drop(edit);
        lock(&kk.0.registry).workers -= 1;

        let (cancelled, answered, second_reader, all_done, waited) = answers.unwrap();
        assert!(cancelled && second_reader);
        assert_eq!(answered, "UnknownConflict");
        // The wait kept its bound: the worker did not finish, and it gave up
        // at the limit, not when the edit ended.
        assert!(!all_done);
        assert!(waited < std::time::Duration::from_secs(15), "{waited:?}");
    }
}
