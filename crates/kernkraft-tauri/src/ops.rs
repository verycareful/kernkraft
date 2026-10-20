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
//! return `Busy`: the registry does not tell readers from writers, and a
//! properties walk is short next to a user reaching for undo. Operations
//! may run side by side; each item's failure, a source removed by another
//! operation included, is in that operation's report.

use std::collections::HashMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use kernkraft_fs::{
    copy, delete_permanently, measure, move_items, undoable_from_report, Conflict, Entry, FsError,
    IrreversibleReason, OpKind, OpReport, PriorStamps, Progress, Trash, TrashListing, TrashedItem,
    Undoable,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::{PluginError, WireError, WireItemError};
use crate::fs_cmds::{describe, WireEntry};
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

/// Payload of [`EVENT_OP_DONE`]. `report` is set for every operation but
/// properties, `properties` for properties. `error` is set instead of
/// either when the worker itself failed (a panic, which is a bug); nothing
/// of the operation's outcome is known then beyond the files themselves.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpDoneEvent {
    pub op: OpId,
    pub report: Option<WireReport>,
    pub properties: Option<WireProperties>,
    pub error: Option<WireError>,
}

/// The running operations, and whether an undo or redo runs.
#[derive(Default)]
pub(crate) struct Registry {
    last: OpId,
    ops: HashMap<OpId, Arc<OpHandle>>,
    undo_running: bool,
}

/// A running operation's cancel flag and conflict prompt.
pub(crate) struct OpHandle {
    id: OpId,
    /// Label of the window that started it, if any.
    origin: Option<String>,
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

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Copy,
    Move,
}

enum Job {
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
    },
    Properties {
        paths: Vec<PathBuf>,
    },
}

enum Outcome {
    Report(WireReport),
    Properties(WireProperties),
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
fn check_dest(dest: &Path) -> Result<(), WireError> {
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
                    Job::EmptyTrash { trash, listing }
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

    /// Registers an operation. Refused while an undo or redo runs.
    pub(crate) fn register(&self, origin: Option<String>) -> Result<Running<R>, WireError> {
        let mut registry = lock(&self.0.registry);
        if registry.undo_running {
            return Err(WireError::from(PluginError::Busy(
                "an undo or redo is running",
            )));
        }
        registry.last += 1;
        let handle = Arc::new(OpHandle {
            id: registry.last,
            origin,
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
        if !registry.ops.is_empty() {
            return Err(WireError::from(PluginError::Busy(
                "a file operation is running; undo and redo wait until it ends",
            )));
        }
        registry.undo_running = true;
        Ok(UndoGuard { kk: self.clone() })
    }

    fn spawn(&self, job: Job, origin: Option<String>) -> Result<OpId, WireError> {
        let running = self.register(origin)?;
        let id = running.handle.id;
        let kk = self.clone();
        // Detached: the worker reports through events, never a return value.
        drop(tauri::async_runtime::spawn_blocking(move || {
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
                error: None,
            };
            match result {
                Ok(Outcome::Report(r)) => done.report = Some(r),
                Ok(Outcome::Properties(p)) => done.properties = Some(p),
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
            Job::EmptyTrash { trash, listing } => {
                let progress = Throttle::new(self, op, "items");
                Outcome::Report(self.empty_trash(op, &progress, &trash, listing))
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
            replaced: Mutex::new(Vec::new()),
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
        let replaced = std::mem::take(&mut *lock(&asker.replaced));
        // Stamped before the journal is locked: stamping a large tree takes
        // a while, and a rename waits on the journal.
        let entry = undoable_from_report(kind, &report, &replaced);
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
        let entry = undoable_from_report(OpKind::Trash, &report, &[]);
        lock(&self.0.journal).record_after(entry, prior);
        WireReport::new(&report, &not_reached)
    }

    fn empty_trash(
        &self,
        op: &OpHandle,
        progress: &Throttle<'_, R>,
        trash: &Trash,
        listing: TrashListing,
    ) -> WireReport {
        let mut report = OpReport::default();
        let mut not_reached = Vec::new();
        // An info file that could not be read leaves its item in the trash;
        // the user is told the trash is not fully empty.
        report.failed.extend(listing.unreadable);
        let total = listing.items.len() as u64;
        for (i, item) in listing.items.iter().enumerate() {
            if op.is_cancelled() {
                report.cancelled = true;
                not_reached.extend(listing.items[i..].iter().map(|it| it.original.clone()));
                break;
            }
            progress.report(i as u64, total, &item.original.to_string_lossy());
            match trash.purge(item) {
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
    /// Existing paths answered `Replace`, for the journal.
    replaced: Mutex<Vec<PathBuf>>,
    /// Sources whose prompt a cancel released.
    cancel_skipped: Mutex<Vec<PathBuf>>,
}

impl<R: Runtime> Asker<'_, R> {
    fn ask(&self, src: &Path, existing: &Path) -> Conflict {
        let op = self.op;
        let mut prompt = lock(&op.prompt);
        if let Some(choice) = prompt.for_all {
            drop(prompt);
            return self.chose(choice, existing);
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
                self.chose(choice, existing)
            }
            None => {
                drop(prompt);
                lock(&self.cancel_skipped).push(src.to_path_buf());
                Conflict::Skip
            }
        }
    }

    fn chose(&self, choice: Conflict, existing: &Path) -> Conflict {
        if choice == Conflict::Replace {
            lock(&self.replaced).push(existing.to_path_buf());
        }
        choice
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
