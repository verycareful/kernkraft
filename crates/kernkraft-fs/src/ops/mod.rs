// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! File operations: create, rename and permanent delete. None of them ever
//! replaces an existing entry unless the caller asked for exactly that.

mod names;
mod transfer;

use std::fs::{self, OpenOptions};
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

pub use names::{unique_name, validate_name};
pub use transfer::{copy, move_items, Conflict};
pub(crate) use transfer::{copy_exact, delete_tree};

use crate::sys::{parent_dir, renameat_noreplace};
use crate::{FsError, Progress};

/// What a multi-item operation did, item by item.
#[derive(Debug, Default)]
pub struct OpReport {
    /// Each completed item: its source, and where it ended up for an
    /// operation that places it somewhere (`None` for a delete).
    pub done: Vec<(PathBuf, Option<PathBuf>)>,
    /// Items left alone by request (a conflict resolved as skip).
    pub skipped: Vec<PathBuf>,
    /// Items that failed, with the reason. The rest still ran.
    pub failed: Vec<(PathBuf, FsError)>,
    /// Items that completed with something not carried over: a copy whose
    /// destination would not take some extended attributes
    /// ([`FsError::XattrsNotKept`], naming them).
    pub warnings: Vec<(PathBuf, FsError)>,
    /// The operation stopped early because its [`Progress`] asked to.
    pub cancelled: bool,
    /// Existing entries that were in fact replaced: a file or symlink a
    /// source was put over, and a folder that a merge put at least one
    /// entry into. Listed when the replace happens, so an item that failed
    /// or was cancelled afterwards (a move across devices whose source
    /// could not then be removed, a merge cancelled partway) still has its
    /// replaces here. A conflict answered `Replace` that never got that far
    /// is not listed.
    pub replaced: Vec<PathBuf>,
}

/// Creates directory `parent/name`. [`FsError::Exists`] if anything already
/// has that name.
pub fn create_dir(parent: &Path, name: &str) -> Result<PathBuf, FsError> {
    validate_name(name)?;
    let path = parent.join(name);
    fs::create_dir(&path).map_err(|e| exists_or(&path, e))?;
    Ok(path)
}

/// Creates the empty file `parent/name`. [`FsError::Exists`] if anything
/// already has that name; an existing file is never truncated.
pub fn create_file(parent: &Path, name: &str) -> Result<PathBuf, FsError> {
    validate_name(name)?;
    let path = parent.join(name);
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| exists_or(&path, e))?;
    Ok(path)
}

/// Renames `path` to `new_name` in the same directory, never replacing an
/// existing entry. Renaming to the current name is a no-op.
pub fn rename(path: &Path, new_name: &str) -> Result<PathBuf, FsError> {
    validate_name(new_name)?;
    let dest = path.with_file_name(new_name);
    if dest == path {
        fs::symlink_metadata(path).map_err(|e| FsError::not_found_or_io(path, e))?;
        return Ok(dest);
    }
    renameat_noreplace(path, &dest).map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => FsError::Exists { path: dest.clone() },
        _ => FsError::not_found_or_io(path, e),
    })?;
    Ok(dest)
}

/// Moves `from` to `to` on one filesystem, failing with
/// [`FsError::Exists`] (naming `to`) instead of replacing anything there: a
/// file, a folder, or a link, dangling or not. Nothing is replaced whatever
/// runs alongside, because the kernel checks and renames in one step
/// (`renameat2` with `RENAME_NOREPLACE`, rename(2)). A symlink is moved as
/// itself, a folder with its contents.
///
/// A filesystem without that flag (NFS, SMB and other FUSE filesystems among
/// them) is handled without a window in which something could be replaced:
/// a folder through an empty placeholder that only an empty folder can be
/// renamed over, anything else by a hard link, which also refuses an
/// existing name, and then removing the original. A filesystem that cannot
/// hard-link either fails with [`FsError::Io`] of kind `Unsupported`; it is
/// never moved by a plain rename.
///
/// `from` and `to` must be on one filesystem: across two, and for a mount
/// point, the failure is [`FsError::Io`] (`EXDEV`, `EBUSY`), as for
/// rename(2). A missing `from` is [`FsError::NotFound`] naming it; when
/// `from` is there and `to` cannot be reached, the folder `to` is in is
/// named (its immediate folder, even when several levels are missing).
/// Moving a folder into its own descendant is refused by the kernel with
/// `EINVAL`; that is also what a filesystem without the flag answers, so the
/// folder route is tried: an empty placeholder folder is made at `to` inside
/// the source, the rename fails again and the placeholder is removed before
/// the call returns [`FsError::Io`] naming `from`. Nothing is left behind.
pub fn rename_noreplace(from: &Path, to: &Path) -> Result<(), FsError> {
    renameat_noreplace(from, to).map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => FsError::Exists {
            path: to.to_path_buf(),
        },
        ErrorKind::NotFound if fs::symlink_metadata(from).is_ok() => FsError::NotFound {
            path: parent_dir(to).to_path_buf(),
        },
        _ => FsError::not_found_or_io(from, e),
    })
}

/// Deletes each path for good. A directory is removed with its contents; a
/// symlink is removed as itself, and no symlink is followed, inside a tree
/// or at the top. Each item is reported on its own; a failure does not stop
/// the rest. Cancellation is checked before each item.
pub fn delete_permanently(paths: &[PathBuf], progress: &dyn Progress) -> OpReport {
    let mut report = OpReport::default();
    let total = paths.len() as u64;
    for (i, path) in paths.iter().enumerate() {
        if progress.is_cancelled() {
            report.cancelled = true;
            break;
        }
        match delete_one(path) {
            Ok(()) => report.done.push((path.clone(), None)),
            Err(e) => report.failed.push((path.clone(), e)),
        }
        progress.report(i as u64 + 1, total, &path.to_string_lossy());
    }
    report
}

fn delete_one(path: &Path) -> Result<(), FsError> {
    let meta = fs::symlink_metadata(path).map_err(|e| FsError::not_found_or_io(path, e))?;
    let removed = if meta.is_dir() {
        // std's remove_dir_all never follows symlinks and opens each level
        // relative to its parent, so a swapped-in link cannot redirect it.
        fs::remove_dir_all(path)
    } else {
        fs::remove_file(path)
    };
    removed.map_err(|e| FsError::io(path, e))
}

fn exists_or(path: &Path, e: std::io::Error) -> FsError {
    if e.kind() == ErrorKind::AlreadyExists {
        FsError::Exists {
            path: path.to_path_buf(),
        }
    } else {
        FsError::io(path, e)
    }
}
