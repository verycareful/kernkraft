// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Thin system-call helpers shared across modules.

use std::fs;
use std::io;
use std::path::Path;

use rustix::fs::{renameat_with, RenameFlags, CWD};
use rustix::io::Errno;

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing
/// anything at `to`.
///
/// Uses `renameat2(RENAME_NOREPLACE)`, which makes the check and the rename
/// one atomic step. A filesystem that does not support the flag answers
/// `EINVAL` (and an old kernel `ENOSYS`); there the check is a separate
/// `lstat` before a plain rename, which leaves a window in which another
/// process could create `to`.
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS) => {
            match fs::symlink_metadata(to) {
                Ok(_) => return Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                Err(e) if e.kind() == io::ErrorKind::NotFound => {}
                Err(e) => return Err(e),
            }
            fs::rename(from, to)
        }
        Err(e) => Err(e.into()),
    }
}

/// The effective user id of this process: the owner of everything it
/// creates, and so the uid ownership checks compare against.
pub(crate) fn uid() -> u32 {
    rustix::process::geteuid().as_raw()
}

/// The directory holding `path`; a bare file name lives in `.`.
pub(crate) fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    }
}
