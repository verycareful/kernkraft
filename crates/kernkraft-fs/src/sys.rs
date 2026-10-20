// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Thin system-call helpers shared across modules.

use std::fs;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use rustix::fs::{linkat, renameat_with, AtFlags, RenameFlags, CWD};
use rustix::io::Errno;

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing
/// anything at `to`. Nothing is ever replaced, whatever runs alongside.
///
/// Uses `renameat2(RENAME_NOREPLACE)` (rename(2)), where the kernel checks
/// and renames in one step. A filesystem that does not support the flag
/// answers `EINVAL` (an old kernel `ENOSYS`); there the move falls back to
/// [`link_then_unlink`], which never replaces either and refuses a
/// directory. Every other error, `EXDEV` among them, is returned as it is.
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS) => link_then_unlink(from, to),
        Err(e) => Err(e.into()),
    }
}

/// Moves the non-directory `from` to `to` with `link(2)`, which fails with
/// `EEXIST` rather than replace anything, then removes `from`. A symlink is
/// linked as itself (`linkat` without `AT_SYMLINK_FOLLOW`).
///
/// A directory cannot be hard-linked, and no other step moves one without
/// risk of replacing, so it is refused as `Unsupported`; so is a filesystem
/// that does not support hard links (`EPERM`, `EOPNOTSUPP`, and `EXDEV` from
/// some FUSE filesystems). If `from` cannot be removed after the link, the
/// new link is removed again (only while `to` is still the same inode) and
/// the error returned.
pub(crate) fn link_then_unlink(from: &Path, to: &Path) -> io::Result<()> {
    let unsupported = |what: &str| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("this filesystem cannot move {what} without the risk of replacing"),
        )
    };
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        return Err(unsupported("a folder"));
    }
    match linkat(CWD, from, CWD, to, AtFlags::empty()) {
        Ok(()) => {}
        Err(Errno::EXIST) => return Err(io::ErrorKind::AlreadyExists.into()),
        Err(Errno::PERM | Errno::OPNOTSUPP | Errno::XDEV) => return Err(unsupported("an item")),
        Err(e) => return Err(e.into()),
    }
    let Err(e) = fs::remove_file(from) else {
        return Ok(());
    };
    let ours =
        fs::symlink_metadata(to).is_ok_and(|m| m.dev() == meta.dev() && m.ino() == meta.ino());
    if ours {
        if let Err(cleanup) = fs::remove_file(to) {
            return Err(io::Error::new(
                cleanup.kind(),
                format!(
                    "{e}; the second name {} could not be removed: {cleanup}",
                    to.display()
                ),
            ));
        }
    }
    Err(e)
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

    use super::{link_then_unlink, rename_noreplace};
    use std::fs;
    use std::io::ErrorKind;
    use std::path::Path;

    #[test]
    fn rename_noreplace_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let err = rename_noreplace(&a, &b).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
        let c = dir.path().join("c");
        rename_noreplace(&a, &c).unwrap();
        assert_eq!(fs::read_to_string(&c).unwrap(), "a");
    }

    #[test]
    fn fallback_moves_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let (a, c) = (dir.path().join("a"), dir.path().join("c"));
        fs::write(&a, "a").unwrap();

        link_then_unlink(&a, &c).unwrap();

        assert!(!a.exists());
        assert_eq!(fs::read_to_string(&c).unwrap(), "a");
    }

    #[test]
    fn fallback_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "b").unwrap();

        let err = link_then_unlink(&a, &b).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert_eq!(fs::read_to_string(&a).unwrap(), "a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "b");
    }

    #[test]
    fn fallback_refuses_a_directory() {
        let dir = tempfile::tempdir().unwrap();
        let (d, e) = (dir.path().join("d"), dir.path().join("e"));
        fs::create_dir(&d).unwrap();

        let err = link_then_unlink(&d, &e).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::Unsupported);
        assert!(d.is_dir());
        assert!(!e.exists());
    }

    #[test]
    fn fallback_moves_a_symlink_as_itself() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target");
        fs::write(&target, "t").unwrap();
        let (link, moved) = (dir.path().join("link"), dir.path().join("moved"));
        std::os::unix::fs::symlink(&target, &link).unwrap();

        link_then_unlink(&link, &moved).unwrap();

        assert!(fs::symlink_metadata(&link).is_err());
        assert!(fs::symlink_metadata(&moved).unwrap().is_symlink());
        assert_eq!(fs::read_link(&moved).unwrap(), Path::new(&target));
        assert_eq!(fs::read_to_string(&target).unwrap(), "t");
    }
}
