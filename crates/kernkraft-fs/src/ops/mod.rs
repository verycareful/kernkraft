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

use crate::sys::rename_noreplace;
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
    rename_noreplace(path, &dest).map_err(|e| match e.kind() {
        ErrorKind::AlreadyExists => FsError::Exists { path: dest.clone() },
        _ => FsError::not_found_or_io(path, e),
    })?;
    Ok(dest)
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
