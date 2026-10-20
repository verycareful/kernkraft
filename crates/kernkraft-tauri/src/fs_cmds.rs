// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Listing, create, rename, properties, and undo and redo.
//!
//! Entries are typed by name alone (Shared MIME-info Database 0.21, glob
//! matching) and by kind, never by reading their content: a listing of a
//! large folder must not open every file in it. Folders, and symlinks that
//! resolve to one, are `inode/directory`, and FIFOs, sockets and device
//! nodes their `inode/*` types (Shared MIME-info Database 0.21,
//! "Non-regular files"). A file whose name matches no glob is
//! `application/octet-stream`.

use std::fs;
use std::os::unix::fs::FileTypeExt;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use kernkraft_fs::{
    create_dir, create_file, list_dir, rename, undoable_from_report, Entry, EntryKind, FsError,
    NoProgress, OpKind, OpReport, TreeStamp, UndoOutcome, Undoable,
};
use kernkraft_xdg_apps::MimeDb;
use serde::{Deserialize, Serialize};
use tauri::Runtime;

use crate::error::{WireError, WireItemError};
use crate::state::{lock, Kernkraft};
use crate::wire::WirePath;

/// Type of a file whose name matches no glob (Shared MIME-info Database
/// 0.21, "Recommended checking order": the fallback for data).
const UNKNOWN_MIME: &str = "application/octet-stream";
const DIRECTORY_MIME: &str = "inode/directory";

/// One directory entry as the UI lists it. `name` is for display (bytes
/// that are not UTF-8 shown as U+FFFD); `path` is what every command takes
/// back. For a symlink, `kind` is `"symlink"`, `len` and `modified_ms`
/// describe the link, and `target_is_dir` tells whether it resolves to a
/// folder. `modified_ms` is milliseconds since the Unix epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireEntry {
    pub path: WirePath,
    pub name: String,
    /// `"file"`, `"dir"`, `"symlink"` or `"other"`.
    pub kind: String,
    pub target_is_dir: bool,
    pub len: u64,
    pub modified_ms: Option<i64>,
    pub hidden: bool,
    pub mime: String,
}

impl WireEntry {
    pub(crate) fn new(e: Entry, mime: &MimeDb) -> WireEntry {
        let kind = match e.kind {
            EntryKind::File => "file",
            EntryKind::Dir => "dir",
            EntryKind::Symlink => "symlink",
            EntryKind::Other => "other",
        };
        WireEntry {
            mime: mime_of(&e, mime),
            path: WirePath::from(e.path.as_path()),
            name: e.name,
            kind: kind.to_owned(),
            target_is_dir: e.target_is_dir,
            len: e.len,
            modified_ms: e.modified.map(epoch_ms),
            hidden: e.hidden,
        }
    }
}

fn mime_of(e: &Entry, db: &MimeDb) -> String {
    if e.target_is_dir {
        return DIRECTORY_MIME.to_owned();
    }
    if e.kind == EntryKind::Other {
        let special = fs::symlink_metadata(&e.path).ok().and_then(|m| {
            let t = m.file_type();
            if t.is_fifo() {
                Some("inode/fifo")
            } else if t.is_socket() {
                Some("inode/socket")
            } else if t.is_block_device() {
                Some("inode/blockdevice")
            } else if t.is_char_device() {
                Some("inode/chardevice")
            } else {
                None
            }
        });
        return special.unwrap_or(UNKNOWN_MIME).to_owned();
    }
    db.by_name(&e.name).unwrap_or(UNKNOWN_MIME).to_owned()
}

/// Milliseconds since the Unix epoch, negative before it, saturating at
/// the ends of `i64`.
fn epoch_ms(t: SystemTime) -> i64 {
    match t.duration_since(UNIX_EPOCH) {
        Ok(d) => i64::try_from(d.as_millis()).unwrap_or(i64::MAX),
        Err(e) => i64::try_from(e.duration().as_millis()).map_or(i64::MIN, |ms| -ms),
    }
}

/// What a successful undo or redo did: each item reversed, by its path
/// outside the trash, and the items reversed with something not carried
/// over.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireUndoOutcome {
    pub done: Vec<WirePath>,
    pub warnings: Vec<WireItemError>,
}

impl From<UndoOutcome> for WireUndoOutcome {
    fn from(o: UndoOutcome) -> WireUndoOutcome {
        WireUndoOutcome {
            done: o.done.iter().map(WirePath::from).collect(),
            warnings: o
                .warnings
                .iter()
                .map(|(p, e)| WireItemError::new(p, e))
                .collect(),
        }
    }
}

/// Labels for the Edit menu: what undo and redo would do now, such as
/// `"Undo rename of notes.md"`, or `None` when there is nothing.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireUndoState {
    pub undo: Option<String>,
    pub redo: Option<String>,
}

/// A path's name for a label, or the whole path when it has none.
fn name(p: &Path) -> String {
    p.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| p.to_string_lossy().into_owned())
}

/// `notes.md` for one path, `3 items` for several.
pub(crate) fn describe(paths: &[PathBuf]) -> String {
    match paths {
        [one] => name(one),
        _ => format!("{} items", paths.len()),
    }
}

/// `rename of notes.md`, `move of 3 items`, or the description an
/// irreversible entry carries.
fn label(op: &Undoable) -> String {
    match op {
        Undoable::Created { path, .. } => format!("create of {}", name(path)),
        Undoable::Renamed { from, .. } => format!("rename of {}", name(from)),
        Undoable::Moved { items } => {
            let from: Vec<PathBuf> = items.iter().map(|(f, _, _)| f.clone()).collect();
            format!("move of {}", describe(&from))
        }
        Undoable::Copied { created } => {
            let made: Vec<PathBuf> = created.iter().map(|(p, _)| p.clone()).collect();
            format!("copy of {}", describe(&made))
        }
        Undoable::Trashed { items } => {
            let from: Vec<PathBuf> = items.iter().map(|i| i.original.clone()).collect();
            format!("trash of {}", describe(&from))
        }
        Undoable::Irreversible { what, .. } => what.clone(),
    }
}

/// The journal entry for a single item placed at `to` from `from` (`from`
/// equal to `to` for a create), stamped now. A stamp that cannot be taken
/// gives the same entry through `undoable_from_report`, which records the
/// item as unverifiable so its undo is refused rather than guessed.
fn single(kind: OpKind, from: &Path, to: &Path) -> Undoable {
    match (kind, TreeStamp::of(to)) {
        (OpKind::Copy, Ok(stamp)) => Undoable::Created {
            path: to.to_path_buf(),
            stamp,
        },
        (OpKind::Move, Ok(stamp)) => Undoable::Renamed {
            from: from.to_path_buf(),
            to: to.to_path_buf(),
            stamp,
        },
        _ => {
            let report = OpReport {
                done: vec![(from.to_path_buf(), Some(to.to_path_buf()))],
                ..OpReport::default()
            };
            undoable_from_report(kind, &report, &[])
        }
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// The entries of `path`, sorted by name; dotfiles only with `hidden`.
    pub fn list_dir(&self, path: WirePath, hidden: bool) -> Result<Vec<WireEntry>, WireError> {
        let dir = PathBuf::try_from(path)?;
        let mime = self.mime()?;
        Ok(list_dir(&dir, hidden)?
            .into_iter()
            .map(|e| WireEntry::new(e, mime))
            .collect())
    }

    /// Creates folder `parent/name`, refusing an existing name, and
    /// journals it.
    pub fn create_dir(&self, parent: WirePath, name: String) -> Result<WirePath, WireError> {
        self.create(parent, &name, create_dir)
    }

    /// Creates the empty file `parent/name`, refusing an existing name, and
    /// journals it.
    pub fn create_file(&self, parent: WirePath, name: String) -> Result<WirePath, WireError> {
        self.create(parent, &name, create_file)
    }

    fn create(
        &self,
        parent: WirePath,
        name: &str,
        make: fn(&Path, &str) -> Result<PathBuf, FsError>,
    ) -> Result<WirePath, WireError> {
        let parent = PathBuf::try_from(parent)?;
        // Held across the create and its record, so an undo cannot run in
        // between.
        let mut journal = lock(&self.0.journal);
        let made = make(&parent, name)?;
        journal.record(single(OpKind::Copy, &made, &made));
        Ok(WirePath::from(&made))
    }

    /// Renames `path` to `new_name` in its folder, never replacing anything,
    /// and journals it. The item's prior stamps are taken right before the
    /// rename, so older entries naming it stay undoable.
    pub fn rename(&self, path: WirePath, new_name: String) -> Result<WirePath, WireError> {
        let path = PathBuf::try_from(path)?;
        let mut journal = lock(&self.0.journal);
        let prior = journal.prior_stamps(std::slice::from_ref(&path));
        let to = rename(&path, &new_name)?;
        if to != path {
            journal.record_after(single(OpKind::Move, &path, &to), prior);
        }
        Ok(WirePath::from(&to))
    }

    /// Reverses the most recent operation. `Busy` while any operation runs.
    pub fn undo(&self) -> Result<WireUndoOutcome, WireError> {
        let trash = self.trash()?;
        let _busy = self.begin_undo()?;
        let outcome = lock(&self.0.journal).undo(trash, &NoProgress)?;
        Ok(outcome.into())
    }

    /// Repeats the most recently undone operation. `Busy` while any
    /// operation runs.
    pub fn redo(&self) -> Result<WireUndoOutcome, WireError> {
        let trash = self.trash()?;
        let _busy = self.begin_undo()?;
        let outcome = lock(&self.0.journal).redo(trash, &NoProgress)?;
        Ok(outcome.into())
    }

    /// What undo and redo would do now.
    pub fn undo_state(&self) -> WireUndoState {
        let journal = lock(&self.0.journal);
        WireUndoState {
            undo: journal.peek_undo().map(|op| format!("Undo {}", label(op))),
            redo: journal.peek_redo().map(|op| format!("Redo {}", label(op))),
        }
    }
}
