// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Undo and redo of file operations: create, rename, move, copy and trash.
//!
//! Each completed operation is recorded with what reversing it needs. Undo
//! of a create or copy puts the created items in the trash (never a
//! permanent delete); undo of a rename or move moves the items back; undo
//! of a trash restores the items. Redo reverses the undo: it restores the
//! very trash entries the undo made, moves the items again, or trashes them
//! again.
//!
//! An undo never overwrites or deletes anything that differs from what was
//! recorded:
//!
//! - **Check before acting.** Before the first action, every item is
//!   compared with its recorded [`TreeStamp`], every destination must be
//!   free, and every trash entry must still be the entry recorded. On any
//!   difference the undo returns [`FsError::Changed`] and does nothing.
//! - **Each action refuses to replace.** Something can still appear between
//!   the check and the action, so moves and restores use
//!   `renameat2(RENAME_NOREPLACE)`, where the kernel checks and renames in
//!   one step, and fail rather than replace. A filesystem without that flag
//!   is refused, never raced.
//! - **A stop partway is reported exactly.** An action that fails, or a
//!   cancel, stops the run: [`FsError::Partial`] lists the items reversed
//!   and the items not reached, with the cause. The journal is split to
//!   match (the items reversed move to the other stack, the rest stay), so
//!   a retry carries on with exactly the items not yet reversed and checks
//!   them again first.
//!
//! A move back across filesystems is a copy then a delete. The source is
//! compared with its stamp again after the copy and deleted only if it
//! still matches; otherwise the fresh copy is removed and the undo stops.
//! What remains is the short window between that last comparison and the
//! delete, which no copy-then-delete can close.
//!
//! Whether another operation is running is not checked here; the caller
//! that runs operations refuses undo while one is in flight.

use std::collections::{HashSet, VecDeque};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use rustix::io::Errno;

use crate::ops::{copy_exact, delete_tree};
use crate::sys::{parent_dir, rename_exclusive};
use crate::trash::{item_at, restore_exclusive, verify_entry};
use crate::{FileStamp, FsError, OpReport, Progress, Trash, TrashedItem};

/// Entries kept on the undo stack: a session's worth of undo. When a new
/// entry would exceed it, the oldest drops.
pub const JOURNAL_DEPTH: usize = 100;

/// The error of [`Journal::undo`] and [`Journal::redo`]: the crate's
/// [`FsError`], whose `Changed`, `Irreversible`, `Partial`, `NothingToUndo`
/// and `NothingToRedo` variants are the undo-specific outcomes.
pub type UndoError = FsError;

/// State of one entry in a [`TreeStamp`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EntryStamp {
    /// A regular file: its [`FileStamp`] (length, times, identity).
    File(FileStamp),
    /// A directory, by identity. Its entry names are the paths below it in
    /// the same stamp.
    Dir { dev: u64, ino: u64 },
    /// A symbolic link, by its target. The link is never followed.
    Symlink { target: PathBuf },
    /// Anything else (FIFO, socket, device node), by identity.
    Other { dev: u64, ino: u64 },
    /// The entry could not be read when the stamp was taken, so nothing can
    /// later be compared against it.
    Unrecorded,
}

/// The state of a file, directory or symlink and, for a directory,
/// everything under it: each entry's path relative to the stamped path
/// (`""` for the path itself), sorted, with its [`EntryStamp`].
///
/// Symlinks are never followed, at the top or inside. Stamping a directory
/// costs one `lstat` per entry, one `readlink` per symlink and one
/// directory listing per directory below it, and holds one path and stamp
/// per entry in memory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TreeStamp(Vec<(PathBuf, EntryStamp)>);

impl TreeStamp {
    /// Stamps `path` and, for a directory, its whole tree.
    pub fn of(path: &Path) -> Result<TreeStamp, FsError> {
        let mut entries = Vec::new();
        let mut pending = vec![PathBuf::new()];
        while let Some(rel) = pending.pop() {
            let full = if rel.as_os_str().is_empty() {
                path.to_path_buf()
            } else {
                path.join(&rel)
            };
            let meta =
                fs::symlink_metadata(&full).map_err(|e| FsError::not_found_or_io(&full, e))?;
            let kind = meta.file_type();
            let stamp = if kind.is_file() {
                EntryStamp::File(FileStamp::from_metadata(&full, &meta)?)
            } else if kind.is_symlink() {
                let target =
                    fs::read_link(&full).map_err(|e| FsError::not_found_or_io(&full, e))?;
                EntryStamp::Symlink { target }
            } else if kind.is_dir() {
                for entry in fs::read_dir(&full).map_err(|e| FsError::not_found_or_io(&full, e))? {
                    let entry = entry.map_err(|e| FsError::io(&full, e))?;
                    pending.push(rel.join(entry.file_name()));
                }
                EntryStamp::Dir {
                    dev: meta.dev(),
                    ino: meta.ino(),
                }
            } else {
                EntryStamp::Other {
                    dev: meta.dev(),
                    ino: meta.ino(),
                }
            };
            entries.push((rel, stamp));
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        Ok(TreeStamp(entries))
    }

    /// Each entry's relative path and stamp, sorted by path.
    pub fn entries(&self) -> &[(PathBuf, EntryStamp)] {
        &self.0
    }

    /// [`TreeStamp::of`], or a stamp that matches nothing when `path`
    /// cannot be read: an item whose state is unknown is never reversed.
    fn of_or_unrecorded(path: &Path) -> TreeStamp {
        TreeStamp::of(path)
            .unwrap_or_else(|_| TreeStamp(vec![(PathBuf::new(), EntryStamp::Unrecorded)]))
    }
}

/// What an undo or redo found different from the journal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The item, or something inside it, was changed, added or replaced.
    Modified,
    /// The item, its folder or its trash entry is gone.
    Missing,
    /// Something else now has the name the item would go back to.
    Taken,
    /// The item could not be read when the operation was recorded, so it
    /// cannot be shown to be unchanged.
    Unverified,
}

impl std::fmt::Display for Change {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Change::Modified => "changed after the operation",
            Change::Missing => "is gone",
            Change::Taken => "is taken by another item",
            Change::Unverified => "could not be read when the operation finished",
        })
    }
}

/// Why an operation cannot be undone.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IrreversibleReason {
    /// It replaced an existing item, whose contents are gone.
    ReplacedExisting,
    /// It merged a folder into an existing one, so the two are mixed.
    MergedDirectory,
    /// It deleted items for good.
    PermanentDelete,
}

impl std::fmt::Display for IrreversibleReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            IrreversibleReason::ReplacedExisting => "it replaced an existing item",
            IrreversibleReason::MergedDirectory => "it merged into an existing folder",
            IrreversibleReason::PermanentDelete => "it deleted permanently",
        })
    }
}

/// A completed operation, with what reversing it needs. Each stamp is the
/// item's state right after the operation, at the path the item then had.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undoable {
    Created {
        path: PathBuf,
        stamp: TreeStamp,
    },
    Renamed {
        from: PathBuf,
        to: PathBuf,
        stamp: TreeStamp,
    },
    /// Each item: where it was, where it went, and its stamp there.
    Moved {
        items: Vec<(PathBuf, PathBuf, TreeStamp)>,
    },
    Copied {
        created: Vec<(PathBuf, TreeStamp)>,
    },
    Trashed {
        items: Vec<TrashedItem>,
    },
    Irreversible {
        what: String,
        reason: IrreversibleReason,
    },
}

/// The kind of report [`undoable_from_report`] reads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpKind {
    Copy,
    Move,
    /// A report whose `done` entries are `(original path, Some(<trash
    /// dir>/files/<name>))`, one per item trashed.
    Trash,
}

impl OpKind {
    fn verb(self) -> &'static str {
        match self {
            OpKind::Copy => "copy",
            OpKind::Move => "move",
            OpKind::Trash => "trash",
        }
    }
}

/// The journal entry for an operation that produced `report`.
///
/// Only `report.done` items are journalled; skipped and failed items were
/// never placed. `replaced` lists every existing path the conflict resolver
/// answered `Replace` for. Any entry there makes the operation
/// [`Undoable::Irreversible`]: [`IrreversibleReason::MergedDirectory`] when
/// one of them is a directory, else
/// [`IrreversibleReason::ReplacedExisting`]. An [`OpReport`] does not mark
/// a merge itself (a merged directory is in `done` as `(source,
/// Some(existing directory))`, like a placed one), so `replaced` is what
/// tells them apart; listing a path there that was not in the end replaced
/// only makes the operation irreversible.
///
/// Each placed item is stamped now, so this is called right after the
/// operation. An item that cannot be stamped is recorded as unverifiable
/// and its undo is refused with [`Change::Unverified`].
pub fn undoable_from_report(kind: OpKind, report: &OpReport, replaced: &[PathBuf]) -> Undoable {
    if let Some(first) = replaced.first() {
        let merged = replaced
            .iter()
            .find(|p| fs::symlink_metadata(p).is_ok_and(|m| m.is_dir()));
        let (reason, path) = match merged {
            Some(dir) => (IrreversibleReason::MergedDirectory, dir),
            None => (IrreversibleReason::ReplacedExisting, first),
        };
        return Undoable::Irreversible {
            what: format!("{} onto {}", kind.verb(), path.display()),
            reason,
        };
    }
    let placed = report
        .done
        .iter()
        .filter_map(|(src, dest)| dest.as_ref().map(|d| (src, d)));
    match kind {
        OpKind::Copy => Undoable::Copied {
            created: placed
                .map(|(_, d)| (d.clone(), TreeStamp::of_or_unrecorded(d)))
                .collect(),
        },
        OpKind::Move => Undoable::Moved {
            items: placed
                .map(|(s, d)| (s.clone(), d.clone(), TreeStamp::of_or_unrecorded(d)))
                .collect(),
        },
        OpKind::Trash => Undoable::Trashed {
            items: placed.map(|(s, d)| item_at(s, d)).collect(),
        },
    }
}

/// What a successful undo or redo did.
#[derive(Debug, Default)]
pub struct UndoOutcome {
    /// Each item reversed, by its path outside the trash: where it was
    /// trashed from, restored to, or moved to.
    pub done: Vec<PathBuf>,
    /// Items reversed with something not carried over: a move back across
    /// filesystems whose destination refused some extended attributes, or
    /// a restored item whose trash info file could not be removed.
    pub warnings: Vec<(PathBuf, FsError)>,
}

/// One reversible action. Running it yields the action that reverses it.
#[derive(Clone, Debug)]
enum Step {
    /// Put `path` in the trash.
    Trash { path: PathBuf, stamp: TreeStamp },
    /// Restore `item` from the trash.
    Restore { item: TrashedItem },
    /// Move the item at `at` to `dest`.
    Move {
        at: PathBuf,
        dest: PathBuf,
        stamp: TreeStamp,
    },
}

impl Step {
    /// The item's path outside the trash before the step runs.
    fn before(&self) -> &Path {
        match self {
            Step::Trash { path, .. } => path,
            Step::Restore { item } => &item.original,
            Step::Move { at, .. } => at,
        }
    }

    /// The item's path outside the trash after the step runs.
    fn after(&self) -> &Path {
        match self {
            Step::Trash { path, .. } => path,
            Step::Restore { item } => &item.original,
            Step::Move { dest, .. } => dest,
        }
    }
}

/// The reversible variants of [`Undoable`].
#[derive(Clone, Copy)]
enum Shape {
    Created,
    Renamed,
    Moved,
    Copied,
    Trashed,
}

/// The shape of `op` and the steps that undo it; `None` for
/// [`Undoable::Irreversible`].
fn undo_steps(op: &Undoable) -> Option<(Shape, Vec<Step>)> {
    let trash = |path: &PathBuf, stamp: &TreeStamp| Step::Trash {
        path: path.clone(),
        stamp: stamp.clone(),
    };
    let back = |from: &PathBuf, to: &PathBuf, stamp: &TreeStamp| Step::Move {
        at: to.clone(),
        dest: from.clone(),
        stamp: stamp.clone(),
    };
    Some(match op {
        Undoable::Created { path, stamp } => (Shape::Created, vec![trash(path, stamp)]),
        Undoable::Renamed { from, to, stamp } => (Shape::Renamed, vec![back(from, to, stamp)]),
        Undoable::Moved { items } => (
            Shape::Moved,
            items.iter().map(|(f, t, s)| back(f, t, s)).collect(),
        ),
        Undoable::Copied { created } => (
            Shape::Copied,
            created.iter().map(|(p, s)| trash(p, s)).collect(),
        ),
        Undoable::Trashed { items } => (
            Shape::Trashed,
            items
                .iter()
                .map(|item| Step::Restore { item: item.clone() })
                .collect(),
        ),
        Undoable::Irreversible { .. } => return None,
    })
}

/// The entry of shape `shape` that `steps` undo: the reverse of
/// [`undo_steps`]. A `Created` or `Renamed` holding other than one step is
/// returned as the equivalent `Copied` or `Moved`.
fn from_undo_steps(shape: Shape, steps: Vec<Step>) -> Undoable {
    let trashes = |steps: Vec<Step>| {
        steps
            .into_iter()
            .filter_map(|s| match s {
                Step::Trash { path, stamp } => Some((path, stamp)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let moves = |steps: Vec<Step>| {
        steps
            .into_iter()
            .filter_map(|s| match s {
                Step::Move { at, dest, stamp } => Some((dest, at, stamp)),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    match shape {
        Shape::Created | Shape::Copied => {
            let mut created = trashes(steps);
            match (shape, created.len()) {
                (Shape::Created, 1) => {
                    let (path, stamp) = created.remove(0);
                    Undoable::Created { path, stamp }
                }
                _ => Undoable::Copied { created },
            }
        }
        Shape::Renamed | Shape::Moved => {
            let mut items = moves(steps);
            match (shape, items.len()) {
                (Shape::Renamed, 1) => {
                    let (from, to, stamp) = items.remove(0);
                    Undoable::Renamed { from, to, stamp }
                }
                _ => Undoable::Moved { items },
            }
        }
        Shape::Trashed => Undoable::Trashed {
            items: steps
                .into_iter()
                .filter_map(|s| match s {
                    Step::Restore { item } => Some(item),
                    _ => None,
                })
                .collect(),
        },
    }
}

/// An undone operation: `shown` is the operation as recorded (what a redo
/// repeats), `steps` the actions that repeat it.
#[derive(Debug)]
struct Redo {
    shown: Undoable,
    steps: Vec<Step>,
}

/// The session's undo and redo stacks.
#[derive(Debug, Default)]
pub struct Journal {
    undo: VecDeque<Undoable>,
    redo: Vec<Redo>,
}

impl Journal {
    pub fn new() -> Journal {
        Journal::default()
    }

    /// Records a completed operation and clears the redo stack. An
    /// operation that placed nothing (an empty `Copied`, `Moved` or
    /// `Trashed`) is not recorded and leaves both stacks as they are.
    pub fn record(&mut self, op: Undoable) {
        let empty = match &op {
            Undoable::Moved { items } => items.is_empty(),
            Undoable::Copied { created } => created.is_empty(),
            Undoable::Trashed { items } => items.is_empty(),
            _ => false,
        };
        if empty {
            return;
        }
        self.redo.clear();
        self.push_undo(op);
    }

    /// The operation the next [`Journal::undo`] reverses.
    pub fn peek_undo(&self) -> Option<&Undoable> {
        self.undo.back()
    }

    /// The operation the next [`Journal::redo`] repeats.
    pub fn peek_redo(&self) -> Option<&Undoable> {
        self.redo.last().map(|r| &r.shown)
    }

    /// Reverses the most recent operation. `progress` counts items and is
    /// polled for cancellation before each one.
    ///
    /// Errors: [`FsError::NothingToUndo`]; [`FsError::Irreversible`] for an
    /// irreversible entry, which stays on top; [`FsError::Changed`] when the
    /// check before acting finds a difference, with nothing done; the cause
    /// itself when the first item fails, with nothing done;
    /// [`FsError::Partial`] when later items fail or the run is cancelled.
    pub fn undo(
        &mut self,
        trash: &Trash,
        progress: &dyn Progress,
    ) -> Result<UndoOutcome, UndoError> {
        let op = self.undo.back().ok_or(FsError::NothingToUndo)?;
        let Some((shape, steps)) = undo_steps(op) else {
            return Err(match op {
                Undoable::Irreversible { what, reason } => FsError::Irreversible {
                    what: what.clone(),
                    reason: *reason,
                },
                _ => FsError::NothingToUndo,
            });
        };
        check(&steps)?;
        let run = execute(steps, trash, progress);
        self.undo.pop_back();
        if !run.not_done.is_empty() {
            self.undo
                .push_back(from_undo_steps(shape, run.not_done.clone()));
        }
        if !run.done.is_empty() {
            let shown = from_undo_steps(shape, run.done.clone());
            self.push_redo(Redo {
                shown,
                steps: run.inverses.clone(),
            });
        }
        run.finish()
    }

    /// Repeats the most recently undone operation, with the same checks,
    /// errors and progress as [`Journal::undo`].
    pub fn redo(
        &mut self,
        trash: &Trash,
        progress: &dyn Progress,
    ) -> Result<UndoOutcome, UndoError> {
        let top = self.redo.last().ok_or(FsError::NothingToRedo)?;
        let Some((shape, shown_steps)) = undo_steps(&top.shown) else {
            return Err(FsError::NothingToRedo);
        };
        let steps = top.steps.clone();
        check(&steps)?;
        let run = execute(steps, trash, progress);
        self.redo.pop();
        if !run.not_done.is_empty() {
            let rest = shown_steps.into_iter().skip(run.done.len()).collect();
            self.push_redo(Redo {
                shown: from_undo_steps(shape, rest),
                steps: run.not_done.clone(),
            });
        }
        if !run.done.is_empty() {
            self.push_undo(from_undo_steps(shape, run.inverses.clone()));
        }
        run.finish()
    }

    fn push_undo(&mut self, op: Undoable) {
        self.undo.push_back(op);
        while self.undo.len() > JOURNAL_DEPTH {
            self.undo.pop_front();
        }
    }

    fn push_redo(&mut self, entry: Redo) {
        self.redo.push(entry);
        if self.redo.len() > JOURNAL_DEPTH {
            self.redo.remove(0);
        }
    }
}

/// The all-or-nothing check before the first action: every item matches
/// its stamp, every trash entry is the one recorded, every destination is
/// free and its folder exists, and no two items go to the same place.
fn check(steps: &[Step]) -> Result<(), FsError> {
    let changed = |(path, what)| FsError::Changed { path, what };
    let mut dests: HashSet<&Path> = HashSet::new();
    for step in steps {
        match step {
            Step::Trash { path, stamp } => {
                if let Some(c) = compare(path, stamp)? {
                    return Err(changed(c));
                }
            }
            Step::Move { at, dest, stamp } => {
                if let Some(c) = compare(at, stamp)? {
                    return Err(changed(c));
                }
                let folder = parent_dir(dest);
                match fs::metadata(folder) {
                    Ok(m) if m.is_dir() => {}
                    Ok(_) => return Err(changed((folder.to_path_buf(), Change::Modified))),
                    Err(e) if e.kind() == ErrorKind::NotFound => {
                        return Err(changed((folder.to_path_buf(), Change::Missing)))
                    }
                    Err(e) => return Err(FsError::io(folder, e)),
                }
                free(dest)?;
            }
            Step::Restore { item } => {
                if let Some(c) = verify_entry(item)? {
                    return Err(changed(c));
                }
                free(&item.original)?;
            }
        }
        let dest = step.after();
        if !dests.insert(dest) {
            return Err(changed((dest.to_path_buf(), Change::Taken)));
        }
    }
    Ok(())
}

/// [`FsError::Changed`] with [`Change::Taken`] unless nothing is at `path`.
fn free(path: &Path) -> Result<(), FsError> {
    match fs::symlink_metadata(path) {
        Ok(_) => Err(FsError::Changed {
            path: path.to_path_buf(),
            what: Change::Taken,
        }),
        Err(e) if e.kind() == ErrorKind::NotFound => Ok(()),
        Err(e) => Err(FsError::io(path, e)),
    }
}

/// The first difference between `path` now and `recorded`, by path:
/// an entry added or changed is [`Change::Modified`], one removed
/// [`Change::Missing`].
fn compare(path: &Path, recorded: &TreeStamp) -> Result<Option<(PathBuf, Change)>, FsError> {
    if recorded
        .0
        .iter()
        .any(|(_, e)| matches!(e, EntryStamp::Unrecorded))
    {
        return Ok(Some((path.to_path_buf(), Change::Unverified)));
    }
    let current = match TreeStamp::of(path) {
        Ok(stamp) => stamp,
        // The path itself is gone, or an entry vanished while it was read.
        Err(FsError::NotFound { path: gone }) => {
            let what = if gone == path {
                Change::Missing
            } else {
                Change::Modified
            };
            return Ok(Some((gone, what)));
        }
        Err(e) => return Err(e),
    };
    let (mut now, mut then) = (current.0.iter().peekable(), recorded.0.iter().peekable());
    loop {
        let diff = match (now.peek(), then.peek()) {
            (None, None) => return Ok(None),
            (Some((p, _)), None) => Some((p, Change::Modified)),
            (None, Some((p, _))) => Some((p, Change::Missing)),
            (Some((pn, sn)), Some((pt, st))) => match pn.cmp(pt) {
                std::cmp::Ordering::Less => Some((pn, Change::Modified)),
                std::cmp::Ordering::Greater => Some((pt, Change::Missing)),
                std::cmp::Ordering::Equal if sn != st => Some((pn, Change::Modified)),
                std::cmp::Ordering::Equal => None,
            },
        };
        if let Some((rel, what)) = diff {
            let at = if rel.as_os_str().is_empty() {
                path.to_path_buf()
            } else {
                path.join(rel)
            };
            return Ok(Some((at, what)));
        }
        now.next();
        then.next();
    }
}

/// The result of running a list of steps until one fails or a cancel.
struct Run {
    /// Steps that ran, in order.
    done: Vec<Step>,
    /// The step reversing each one in `done`, in the same order.
    inverses: Vec<Step>,
    /// Steps not reached, the failed one first.
    not_done: Vec<Step>,
    failure: Option<FsError>,
    warnings: Vec<(PathBuf, FsError)>,
}

impl Run {
    fn finish(self) -> Result<UndoOutcome, FsError> {
        let done = self.done.iter().map(|s| s.after().to_path_buf()).collect();
        match self.failure {
            None => Ok(UndoOutcome {
                done,
                warnings: self.warnings,
            }),
            Some(cause) if self.done.is_empty() => Err(cause),
            Some(cause) => Err(FsError::Partial {
                done,
                not_done: self
                    .not_done
                    .iter()
                    .map(|s| s.before().to_path_buf())
                    .collect(),
                cause: Box::new(cause),
                warnings: self.warnings,
            }),
        }
    }
}

fn execute(steps: Vec<Step>, trash: &Trash, progress: &dyn Progress) -> Run {
    let total = steps.len() as u64;
    let mut run = Run {
        done: Vec::new(),
        inverses: Vec::new(),
        not_done: Vec::new(),
        failure: None,
        warnings: Vec::new(),
    };
    let mut steps = steps.into_iter();
    while let Some(step) = steps.next() {
        let result = if progress.is_cancelled() {
            Err(FsError::Cancelled)
        } else {
            run_step(&step, trash, progress, &mut run.warnings)
        };
        match result {
            Ok(inverse) => {
                progress.report(
                    run.done.len() as u64 + 1,
                    total,
                    &step.after().to_string_lossy(),
                );
                run.inverses.push(inverse);
                run.done.push(step);
            }
            Err(e) => {
                run.failure = Some(e);
                run.not_done.push(step);
                run.not_done.extend(steps);
                break;
            }
        }
    }
    run
}

/// Runs one step and returns the step that reverses it.
fn run_step(
    step: &Step,
    trash: &Trash,
    progress: &dyn Progress,
    warnings: &mut Vec<(PathBuf, FsError)>,
) -> Result<Step, FsError> {
    match step {
        Step::Trash { path, .. } => Ok(Step::Restore {
            item: trash.put(path)?,
        }),
        Step::Restore { item } => {
            if let Some(w) = restore_exclusive(item)? {
                warnings.push((item.original.clone(), w));
            }
            Ok(Step::Trash {
                path: item.original.clone(),
                stamp: TreeStamp::of_or_unrecorded(&item.original),
            })
        }
        Step::Move { at, dest, stamp } => {
            match rename_exclusive(at, dest) {
                Ok(()) => {}
                Err(e) if e.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
                    move_across(at, dest, stamp, progress, warnings)?;
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    return Err(FsError::Exists { path: dest.clone() })
                }
                Err(e) => return Err(FsError::not_found_or_io(at, e)),
            }
            Ok(Step::Move {
                at: dest.clone(),
                dest: at.clone(),
                stamp: TreeStamp::of_or_unrecorded(dest),
            })
        }
    }
}

/// Moves `at` to `dest` on another filesystem: copy, compare `at` with its
/// stamp again, then delete it. If `at` changed during the copy, the copy is
/// removed and nothing else is touched.
fn move_across(
    at: &Path,
    dest: &Path,
    stamp: &TreeStamp,
    progress: &dyn Progress,
    warnings: &mut Vec<(PathBuf, FsError)>,
) -> Result<(), FsError> {
    let refused = copy_exact(at, dest, &CancelOnly(progress))?;
    match compare(at, stamp) {
        Ok(None) => {}
        Ok(Some((path, what))) => {
            delete_tree(dest)?;
            return Err(FsError::Changed { path, what });
        }
        Err(e) => {
            delete_tree(dest)?;
            return Err(e);
        }
    }
    delete_tree(at)?;
    warnings.extend(refused);
    Ok(())
}

/// Passes cancellation through to a copy whose byte counts would not fit
/// the item counts the undo reports.
struct CancelOnly<'a>(&'a dyn Progress);

impl Progress for CancelOnly<'_> {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {}

    fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}
