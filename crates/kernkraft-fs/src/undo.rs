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
//!   the check and the action, so moves and restores rename with
//!   `renameat2(RENAME_NOREPLACE)`, where the kernel checks and renames in
//!   one step. Where a filesystem lacks that flag, a file or symlink is
//!   moved by `link(2)` then `unlink(2)`, and a folder by renaming it over
//!   an empty placeholder folder made for it; neither can replace anything. A trash entry is matched by its identity (device,
//!   inode and stamp), not only by name, path and deletion date.
//! - **A stop partway is reported exactly.** An action that fails, or a
//!   cancel, stops the run: [`FsError::Partial`] lists the items reversed
//!   and the items not reached, with the cause. The journal is split to
//!   match (the items reversed move to the other stack, the rest stay), so
//!   a retry carries on with exactly the items not yet reversed and checks
//!   them again first.
//!
//! **Chains of operations on one item.** Renaming, trashing and restoring
//! change an item's ctime, so an older entry's stamp of the item no longer
//! matches once a newer entry is undone, even though nothing changed it.
//! Each step therefore remembers the stamp the item had at its destination
//! before the step that the step reverses. When the step runs, its own
//! check has just matched the item exactly, and the item lands with nothing
//! changed but its ctime; then every entry on either stack that expects the
//! item there with exactly that remembered stamp is updated to the new one.
//! For an operation recorded from outside, the remembered stamps come from
//! [`Journal::prior_stamps`], taken just before the operation and passed to
//! [`Journal::record_after`]. A stamp that differs in anything else is never
//! carried over, so a change made from outside always stops the undo.
//!
//! A move back across filesystems is a copy then a delete. The source is
//! compared with its stamp again after the copy and deleted only if it
//! still matches; otherwise the fresh copy is removed and the undo stops.
//! What remains is the short window between that last comparison and the
//! delete, which no copy-then-delete can close. The copy has new inodes, so
//! a chain through such a move is not carried over.
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
use crate::sys::{parent_dir, rename_noreplace};
use crate::trash::{held_path, item_at, restore_for_undo, verify_entry};
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

    fn has_unrecorded(&self) -> bool {
        self.0
            .iter()
            .any(|(_, e)| matches!(e, EntryStamp::Unrecorded))
    }

    /// Equal in everything but the ctime of a file at the top, which is all
    /// that rename(2) changes; never true for a stamp holding an unrecorded
    /// entry.
    fn same_but_root_ctime(&self, other: &TreeStamp) -> bool {
        if self.has_unrecorded() || other.has_unrecorded() || self.0.len() != other.0.len() {
            return false;
        }
        self.0.iter().zip(&other.0).all(|((pa, a), (pb, b))| {
            pa == pb
                && match (a, b) {
                    (EntryStamp::File(x), EntryStamp::File(y)) if pa.as_os_str().is_empty() => {
                        FileStamp {
                            changed_ns: 0,
                            ..*x
                        } == FileStamp {
                            changed_ns: 0,
                            ..*y
                        }
                    }
                    _ => a == b,
                }
        })
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
/// never placed. A report with nothing placed gives an empty entry, whatever
/// `replaced` holds. `replaced` lists every existing path the conflict resolver
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
    let placed_any = report.done.iter().any(|(_, dest)| dest.is_some());
    if !placed_any {
        // Nothing was placed, so nothing was replaced either, whatever the
        // resolver answered; an empty entry, which `record` ignores.
        return match kind {
            OpKind::Copy => Undoable::Copied {
                created: Vec::new(),
            },
            OpKind::Move => Undoable::Moved { items: Vec::new() },
            OpKind::Trash => Undoable::Trashed { items: Vec::new() },
        };
    }
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

/// Stamps of an operation's sources taken just before it runs, for
/// [`Journal::record_after`]. Only sources some journal entry expects are
/// stamped; a source that cannot be read is left out, which only means the
/// older entries naming it are not carried over.
#[derive(Clone, Debug, Default)]
pub struct PriorStamps(Vec<(PathBuf, TreeStamp)>);

impl PriorStamps {
    fn get(&self, path: &Path) -> Option<&TreeStamp> {
        self.0.iter().find(|(p, _)| p == path).map(|(_, s)| s)
    }
}

/// One reversible action. Running it yields the action that reverses it.
///
/// `was`, on an action that puts an item back at a path, is the stamp the
/// item had at that path before the action this one reverses; see the
/// module documentation on chains.
#[derive(Clone, Debug)]
enum Step {
    /// Put the item at `path`, matching `stamp`, in the trash.
    Trash { path: PathBuf, stamp: TreeStamp },
    /// Restore `item`, whose trash file has identity `ident` (device,
    /// inode) and stamp `held`.
    Restore {
        item: TrashedItem,
        held: TreeStamp,
        ident: (u64, u64),
        was: Option<TreeStamp>,
    },
    /// Move the item at `at`, matching `stamp`, to `dest`.
    Move {
        at: PathBuf,
        dest: PathBuf,
        stamp: TreeStamp,
        was: Option<TreeStamp>,
    },
}

impl Step {
    /// The item's path outside the trash before the step runs.
    fn before(&self) -> &Path {
        match self {
            Step::Trash { path, .. } => path,
            Step::Restore { item, .. } => &item.original,
            Step::Move { at, .. } => at,
        }
    }

    /// The item's path outside the trash after the step runs.
    fn after(&self) -> &Path {
        match self {
            Step::Trash { path, .. } => path,
            Step::Restore { item, .. } => &item.original,
            Step::Move { dest, .. } => dest,
        }
    }

    /// The path and stamp the step's check expects, for a step that
    /// expects an item outside the trash.
    fn expects_mut(&mut self) -> Option<(&Path, &mut TreeStamp)> {
        match self {
            Step::Trash { path, stamp } => Some((path, stamp)),
            Step::Move { at, stamp, .. } => Some((at, stamp)),
            Step::Restore { .. } => None,
        }
    }

    fn expected_path(&self) -> Option<&Path> {
        match self {
            Step::Trash { path, .. } => Some(path),
            Step::Move { at, .. } => Some(at),
            Step::Restore { .. } => None,
        }
    }
}

/// Device and inode of `path`, not following a symlink; `(0, 0)` when it
/// cannot be read, in which case its stamp is unrecorded too and the entry
/// is never restored.
fn ident_of(path: &Path) -> (u64, u64) {
    fs::symlink_metadata(path)
        .map(|m| (m.dev(), m.ino()))
        .unwrap_or((0, 0))
}

/// The variants of [`Undoable`].
#[derive(Clone, Copy, Debug)]
enum Shape {
    Created,
    Renamed,
    Moved,
    Copied,
    Trashed,
    Irreversible,
}

/// The shape of `op` and the steps that undo it, with `prior` giving the
/// remembered stamps of moved sources. A Trashed item's trash file is
/// stamped now, so this runs right after the operation.
fn undo_steps(op: &Undoable, prior: &PriorStamps) -> (Shape, Vec<Step>) {
    let chain = |at: &Path, after: &TreeStamp| {
        prior
            .get(at)
            .filter(|before| before.same_but_root_ctime(after))
            .cloned()
    };
    let trash = |path: &PathBuf, stamp: &TreeStamp| Step::Trash {
        path: path.clone(),
        stamp: stamp.clone(),
    };
    let back = |from: &PathBuf, to: &PathBuf, stamp: &TreeStamp| Step::Move {
        at: to.clone(),
        dest: from.clone(),
        stamp: stamp.clone(),
        was: chain(from, stamp),
    };
    match op {
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
                .map(|item| {
                    let held_at = held_path(item);
                    let held = TreeStamp::of_or_unrecorded(&held_at);
                    Step::Restore {
                        item: item.clone(),
                        ident: ident_of(&held_at),
                        was: chain(&item.original, &held),
                        held,
                    }
                })
                .collect(),
        ),
        Undoable::Irreversible { .. } => (Shape::Irreversible, Vec::new()),
    }
}

/// The entry of shape `shape` that `steps` undo: the reverse of
/// [`undo_steps`]. A `Created` or `Renamed` holding other than one step is
/// returned as the equivalent `Copied` or `Moved`.
fn from_undo_steps(shape: Shape, steps: Vec<Step>) -> Undoable {
    match shape {
        Shape::Created | Shape::Copied | Shape::Irreversible => {
            let mut created: Vec<_> = steps
                .into_iter()
                .filter_map(|s| match s {
                    Step::Trash { path, stamp } => Some((path, stamp)),
                    _ => None,
                })
                .collect();
            match (shape, created.len()) {
                (Shape::Created, 1) => {
                    let (path, stamp) = created.remove(0);
                    Undoable::Created { path, stamp }
                }
                _ => Undoable::Copied { created },
            }
        }
        Shape::Renamed | Shape::Moved => {
            let mut items: Vec<_> = steps
                .into_iter()
                .filter_map(|s| match s {
                    Step::Move {
                        at, dest, stamp, ..
                    } => Some((dest, at, stamp)),
                    _ => None,
                })
                .collect();
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
                    Step::Restore { item, .. } => Some(item),
                    _ => None,
                })
                .collect(),
        },
    }
}

/// `op` without its first `k` items.
fn tail(op: &Undoable, k: usize) -> Undoable {
    match op {
        Undoable::Moved { items } => Undoable::Moved {
            items: items.iter().skip(k).cloned().collect(),
        },
        Undoable::Copied { created } => Undoable::Copied {
            created: created.iter().skip(k).cloned().collect(),
        },
        Undoable::Trashed { items } => Undoable::Trashed {
            items: items.iter().skip(k).cloned().collect(),
        },
        other => other.clone(),
    }
}

/// A journal entry: `shown` is the operation as recorded (what an undo
/// reverses or a redo repeats), `steps` the actions that do it.
#[derive(Debug)]
struct Entry {
    shape: Shape,
    shown: Undoable,
    steps: Vec<Step>,
}

impl Entry {
    /// An undo-stack entry, shown as the operation its steps undo.
    fn undo(shape: Shape, steps: Vec<Step>) -> Entry {
        Entry {
            shape,
            shown: from_undo_steps(shape, steps.clone()),
            steps,
        }
    }
}

/// The session's undo and redo stacks.
#[derive(Debug, Default)]
pub struct Journal {
    undo: VecDeque<Entry>,
    redo: Vec<Entry>,
}

impl Journal {
    pub fn new() -> Journal {
        Journal::default()
    }

    /// Records a completed operation and clears the redo stack; the same as
    /// [`Journal::record_after`] with no prior stamps.
    pub fn record(&mut self, op: Undoable) {
        self.record_after(op, PriorStamps::default());
    }

    /// Stamps, before an operation runs, those of its `sources` that an
    /// entry on either stack expects to find. Pass the result to
    /// [`Journal::record_after`] once the operation is done, so older
    /// entries naming those items stay undoable after the new one is undone.
    pub fn prior_stamps(&self, sources: &[PathBuf]) -> PriorStamps {
        let expected: HashSet<&Path> = self
            .undo
            .iter()
            .chain(&self.redo)
            .flat_map(|e| e.steps.iter())
            .filter_map(Step::expected_path)
            .collect();
        PriorStamps(
            sources
                .iter()
                .filter(|s| expected.contains(s.as_path()))
                .filter_map(|s| TreeStamp::of(s).ok().map(|stamp| (s.clone(), stamp)))
                .collect(),
        )
    }

    /// Records a completed operation, with `prior` from
    /// [`Journal::prior_stamps`] taken just before it, and clears the redo
    /// stack. An operation that placed nothing (an empty `Copied`, `Moved`
    /// or `Trashed`) is not recorded and leaves both stacks as they are.
    pub fn record_after(&mut self, op: Undoable, prior: PriorStamps) {
        let empty = match &op {
            Undoable::Moved { items } => items.is_empty(),
            Undoable::Copied { created } => created.is_empty(),
            Undoable::Trashed { items } => items.is_empty(),
            _ => false,
        };
        if empty {
            return;
        }
        let (shape, steps) = undo_steps(&op, &prior);
        self.redo.clear();
        self.push_undo(Entry {
            shape,
            shown: op,
            steps,
        });
    }

    /// The operation the next [`Journal::undo`] reverses.
    pub fn peek_undo(&self) -> Option<&Undoable> {
        self.undo.back().map(|e| &e.shown)
    }

    /// The operation the next [`Journal::redo`] repeats.
    pub fn peek_redo(&self) -> Option<&Undoable> {
        self.redo.last().map(|e| &e.shown)
    }

    /// Reverses the most recent operation. `progress` counts items and is
    /// polled for cancellation before each one.
    ///
    /// Errors: [`FsError::NothingToUndo`]; [`FsError::Irreversible`] when
    /// the most recent entry cannot be reversed: it is reported once and
    /// taken off the stack, so the next undo goes on to the older entries,
    /// each still guarded by its own check; [`FsError::Changed`] when the
    /// check before acting finds a difference, with nothing done; the cause
    /// itself when the first item fails, with nothing done;
    /// [`FsError::Partial`] when later items fail or the run is cancelled.
    pub fn undo(
        &mut self,
        trash: &Trash,
        progress: &dyn Progress,
    ) -> Result<UndoOutcome, UndoError> {
        let top = self.undo.back().ok_or(FsError::NothingToUndo)?;
        if let Undoable::Irreversible { what, reason } = &top.shown {
            let err = FsError::Irreversible {
                what: what.clone(),
                reason: *reason,
            };
            self.undo.pop_back();
            return Err(err);
        }
        check(&top.steps)?;
        let (shape, steps) = (top.shape, top.steps.clone());
        let run = execute(steps, trash, progress);
        self.undo.pop_back();
        if !run.not_done.is_empty() {
            self.undo
                .push_back(Entry::undo(shape, run.not_done.clone()));
        }
        if !run.done.is_empty() {
            self.push_redo(Entry {
                shape,
                shown: from_undo_steps(shape, run.done.clone()),
                steps: run.inverses.clone(),
            });
        }
        self.carry(&run.carried);
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
        check(&top.steps)?;
        let (shape, steps) = (top.shape, top.steps.clone());
        let shown = top.shown.clone();
        let run = execute(steps, trash, progress);
        self.redo.pop();
        if !run.not_done.is_empty() {
            self.push_redo(Entry {
                shape,
                shown: tail(&shown, run.done.len()),
                steps: run.not_done.clone(),
            });
        }
        if !run.done.is_empty() {
            self.push_undo(Entry::undo(shape, run.inverses.clone()));
        }
        self.carry(&run.carried);
        run.finish()
    }

    fn push_undo(&mut self, entry: Entry) {
        self.undo.push_back(entry);
        while self.undo.len() > JOURNAL_DEPTH {
            self.undo.pop_front();
        }
    }

    fn push_redo(&mut self, entry: Entry) {
        self.redo.push(entry);
        if self.redo.len() > JOURNAL_DEPTH {
            self.redo.remove(0);
        }
    }

    /// For each `(path, old, new)`: every step on either stack that expects
    /// the item at `path` with exactly `old` now expects `new`.
    fn carry(&mut self, carried: &[(PathBuf, TreeStamp, TreeStamp)]) {
        if carried.is_empty() {
            return;
        }
        let update = |entry: &mut Entry| {
            let mut changed = false;
            for step in &mut entry.steps {
                if let Some((at, stamp)) = step.expects_mut() {
                    for (path, old, new) in carried {
                        if at == path && stamp == old {
                            *stamp = new.clone();
                            changed = true;
                        }
                    }
                }
            }
            changed
        };
        for entry in &mut self.undo {
            if update(entry) {
                entry.shown = from_undo_steps(entry.shape, entry.steps.clone());
            }
        }
        for entry in &mut self.redo {
            update(entry);
        }
    }
}

/// The all-or-nothing check before the first action: every item matches
/// its stamp, every trash entry is the one recorded, every destination is
/// free and its folder exists as a real folder, and no two items go to the
/// same place.
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
            Step::Move {
                at, dest, stamp, ..
            } => {
                if let Some(c) = compare(at, stamp)? {
                    return Err(changed(c));
                }
                folder(parent_dir(dest))?;
                free(dest)?;
            }
            Step::Restore {
                item, held, ident, ..
            } => {
                if let Some(c) = verify_entry(item)? {
                    return Err(changed(c));
                }
                let held_at = held_path(item);
                if ident_of(&held_at) != *ident {
                    return Err(changed((held_at, Change::Modified)));
                }
                if let Some(c) = compare(&held_at, held)? {
                    return Err(changed(c));
                }
                folder(parent_dir(&item.original))?;
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

/// [`FsError::Changed`] unless `path` is a folder: [`Change::Missing`] when
/// nothing is there, [`Change::Modified`] when something else is, a symlink
/// included (an item is only put back into the real folder it left).
fn folder(path: &Path) -> Result<(), FsError> {
    let what = match fs::symlink_metadata(path) {
        Ok(m) if m.is_dir() => return Ok(()),
        Ok(_) => Change::Modified,
        Err(e) if e.kind() == ErrorKind::NotFound => Change::Missing,
        Err(e) => return Err(FsError::io(path, e)),
    };
    Err(FsError::Changed {
        path: path.to_path_buf(),
        what,
    })
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
    if recorded.has_unrecorded() {
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
    /// Stamps to carry over to other entries: `(path, old, new)`.
    carried: Vec<(PathBuf, TreeStamp, TreeStamp)>,
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
        carried: Vec::new(),
    };
    let mut steps = steps.into_iter();
    while let Some(step) = steps.next() {
        let result = if progress.is_cancelled() {
            Err(FsError::Cancelled)
        } else {
            run_step(&step, trash, progress, &mut run)
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

/// Runs one step and returns the step that reverses it. Each step's check
/// has just matched the item exactly; a landed item whose stamp differs
/// from that in anything but the top ctime is not chained.
fn run_step(
    step: &Step,
    trash: &Trash,
    progress: &dyn Progress,
    run: &mut Run,
) -> Result<Step, FsError> {
    match step {
        Step::Trash { path, stamp } => {
            let item = trash.put(path)?;
            let held_at = held_path(&item);
            let held = TreeStamp::of_or_unrecorded(&held_at);
            Ok(Step::Restore {
                ident: ident_of(&held_at),
                was: stamp.same_but_root_ctime(&held).then(|| stamp.clone()),
                held,
                item,
            })
        }
        Step::Restore {
            item, held, was, ..
        } => {
            if let Some(w) = restore_for_undo(item)? {
                run.warnings.push((item.original.clone(), w));
            }
            let now = TreeStamp::of_or_unrecorded(&item.original);
            if let Some(was) = was.as_ref().filter(|_| held.same_but_root_ctime(&now)) {
                run.carried
                    .push((item.original.clone(), was.clone(), now.clone()));
            }
            Ok(Step::Trash {
                path: item.original.clone(),
                stamp: now,
            })
        }
        Step::Move {
            at,
            dest,
            stamp,
            was,
        } => {
            match rename_noreplace(at, dest) {
                Ok(()) => {}
                Err(e) if e.raw_os_error() == Some(Errno::XDEV.raw_os_error()) => {
                    move_across(at, dest, stamp, progress, &mut run.warnings)?;
                }
                Err(e) if e.kind() == ErrorKind::AlreadyExists => {
                    return Err(FsError::Exists { path: dest.clone() })
                }
                Err(e) => return Err(FsError::not_found_or_io(at, e)),
            }
            let now = TreeStamp::of_or_unrecorded(dest);
            let same = stamp.same_but_root_ctime(&now);
            if let Some(was) = was.as_ref().filter(|_| same) {
                run.carried.push((dest.clone(), was.clone(), now.clone()));
            }
            Ok(Step::Move {
                at: dest.clone(),
                dest: at.clone(),
                stamp: now,
                was: same.then(|| stamp.clone()),
            })
        }
    }
}

/// Moves `at` to `dest` on another filesystem: copy, compare `at` with its
/// stamp again, then delete it. If `at` changed during the copy, the copy is
/// removed and nothing else is touched. If `at` cannot be deleted after a
/// complete copy, both remain and [`FsError::SourceNotRemoved`] says so.
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
    delete_tree(at).map_err(|cause| FsError::SourceNotRemoved {
        copy: dest.to_path_buf(),
        left: at.to_path_buf(),
        cause: Box::new(cause),
    })?;
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
