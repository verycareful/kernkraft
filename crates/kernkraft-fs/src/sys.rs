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

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing
/// anything at `to`, with no fallback.
///
/// The kernel makes the check and the rename one step
/// (`renameat2(RENAME_NOREPLACE)`, rename(2)). A filesystem that does not
/// support the flag answers `EINVAL` (an old kernel `ENOSYS`); that is
/// reported as [`io::ErrorKind::Unsupported`] instead of falling back to a
/// separate check, so a caller that must never replace anything is never
/// exposed to the window [`rename_noreplace`] leaves there. (`EINVAL` also
/// answers a directory renamed into itself; the one caller, undo, only
/// moves an item back to where it was.) Every other error, `EXDEV` among
/// them, is returned as it is.
pub(crate) fn rename_exclusive(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS) => Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "this filesystem cannot rename without the risk of replacing (no RENAME_NOREPLACE)",
        )),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::rename_exclusive;
    use std::fs;
    use std::io::ErrorKind;

    #[test]
    fn rename_exclusive_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let err = rename_exclusive(&a, &b).err().map(|e| e.kind());

        assert_eq!(err, Some(ErrorKind::AlreadyExists));
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
        assert!(a.exists());
        let c = dir.path().join("c");
        rename_exclusive(&a, &c).unwrap();
        assert_eq!(fs::read_to_string(&c).unwrap(), "a");
    }
}
