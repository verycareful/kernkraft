// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Thin system-call helpers shared across modules.

use std::fs::{self, DirBuilder};
use std::io;
use std::os::unix::fs::{DirBuilderExt, MetadataExt};
use std::path::Path;

use rustix::fs::{linkat, renameat_with, AtFlags, RenameFlags, CWD};
use rustix::io::Errno;

/// Renames `from` to `to`, failing with `AlreadyExists` instead of replacing
/// anything at `to`. Nothing is ever replaced, whatever runs alongside.
///
/// Uses `renameat2(RENAME_NOREPLACE)` (rename(2)), where the kernel checks
/// and renames in one step. A filesystem that does not support the flag
/// (NFS, SMB, sshfs and other FUSE filesystems among them) answers `EINVAL`
/// (an old kernel `ENOSYS`); there the move goes through
/// [`move_without_noreplace`], which never replaces either. Every other
/// error, `EXDEV` among them, is returned as it is.
pub(crate) fn rename_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    match renameat_with(CWD, from, CWD, to, RenameFlags::NOREPLACE) {
        Ok(()) => Ok(()),
        Err(Errno::INVAL | Errno::NOSYS) => move_without_noreplace(from, to),
        Err(e) => Err(e.into()),
    }
}

/// Mode of a placeholder folder: owner only, as it holds nothing and is
/// replaced at once.
const PLACEHOLDER_MODE: u32 = 0o700;

/// [`rename_noreplace`] where the filesystem lacks `RENAME_NOREPLACE`: a
/// folder through an empty placeholder ([`claim_placeholder`], then
/// [`rename_into_placeholder`]), anything else by [`link_then_unlink`].
pub(crate) fn move_without_noreplace(from: &Path, to: &Path) -> io::Result<()> {
    if fs::symlink_metadata(from)?.is_dir() {
        claim_placeholder(to)?;
        rename_into_placeholder(from, to)
    } else {
        link_then_unlink(from, to)
    }
}

/// Claims the name `to` for a folder move by creating an empty directory
/// there (`mkdir(2)`, which fails with `EEXIST` rather than touch anything
/// already there). Its mode is owner-only while it stands in.
pub(crate) fn claim_placeholder(to: &Path) -> io::Result<()> {
    DirBuilder::new().mode(PLACEHOLDER_MODE).create(to)
}

/// Renames the folder `from` over the placeholder `to` with a plain
/// rename(2).
///
/// This cannot lose data. rename(2) replaces a directory only with a
/// directory, and only when the one replaced is empty: if anything was put
/// into the placeholder meanwhile, the rename fails with `ENOTEMPTY`; if
/// the placeholder was swapped for a file, it fails with `ENOTDIR`. At
/// worst it replaces an empty directory, which holds nothing.
///
/// On any failure the placeholder is removed with rmdir(2), which also only
/// removes an empty directory. If that fails too (someone filled it), the
/// returned error names the placeholder left at `to`.
pub(crate) fn rename_into_placeholder(from: &Path, to: &Path) -> io::Result<()> {
    let Err(e) = fs::rename(from, to) else {
        return Ok(());
    };
    match fs::remove_dir(to) {
        Ok(()) => Err(e),
        Err(cleanup) => Err(io::Error::new(
            e.kind(),
            format!(
                "{e}; the placeholder folder {} could not be removed: {cleanup}",
                to.display()
            ),
        )),
    }
}

/// Moves the non-directory `from` to `to` with `link(2)`, which fails with
/// `EEXIST` rather than replace anything, then removes `from`. A symlink is
/// linked as itself (`linkat` without `AT_SYMLINK_FOLLOW`).
///
/// A directory cannot be hard-linked and is refused as `Unsupported` (see
/// [`move_without_noreplace`] for folders); so is a filesystem that does
/// not support hard links (`EPERM`, `EOPNOTSUPP`, and `EXDEV` from some FUSE
/// filesystems). If `from` cannot be removed after the link,
/// [`take_back_link`] removes the new name again.
pub(crate) fn link_then_unlink(from: &Path, to: &Path) -> io::Result<()> {
    let unsupported = |what: &str| {
        io::Error::new(
            io::ErrorKind::Unsupported,
            format!("this filesystem cannot move {what} without the risk of replacing"),
        )
    };
    let meta = fs::symlink_metadata(from)?;
    if meta.is_dir() {
        return Err(unsupported("a folder by linking"));
    }
    match linkat(CWD, from, CWD, to, AtFlags::empty()) {
        Ok(()) => {}
        Err(Errno::EXIST) => return Err(io::ErrorKind::AlreadyExists.into()),
        Err(Errno::PERM | Errno::OPNOTSUPP | Errno::XDEV) => return Err(unsupported("an item")),
        Err(e) => return Err(e.into()),
    }
    match fs::remove_file(from) {
        Ok(()) => Ok(()),
        Err(e) => Err(take_back_link(&meta, to, e)),
    }
}

/// After `from` (whose metadata is `meta`) could not be removed with error
/// `e`, removes the second name `to` made for it, but only while `to` is
/// still the same inode, so nothing else that took the name is touched.
/// Returns `e`, extended if `to` could not be removed.
pub(crate) fn take_back_link(meta: &fs::Metadata, to: &Path, e: io::Error) -> io::Error {
    let ours =
        fs::symlink_metadata(to).is_ok_and(|m| m.dev() == meta.dev() && m.ino() == meta.ino());
    if !ours {
        return e;
    }
    match fs::remove_file(to) {
        Ok(()) => e,
        Err(cleanup) => io::Error::new(
            cleanup.kind(),
            format!(
                "{e}; the second name {} could not be removed: {cleanup}",
                to.display()
            ),
        ),
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

    use super::{
        claim_placeholder, link_then_unlink, move_without_noreplace, rename_into_placeholder,
        rename_noreplace, take_back_link,
    };
    use std::fs;
    use std::io::ErrorKind;
    use std::os::unix::fs::MetadataExt;
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

    #[test]
    fn fallback_moves_a_folder_with_its_contents() {
        let dir = tempfile::tempdir().unwrap();
        let (d, e) = (dir.path().join("d"), dir.path().join("e"));
        fs::create_dir(&d).unwrap();
        fs::write(d.join("f"), "f").unwrap();
        let ino = fs::metadata(&d).unwrap().ino();

        move_without_noreplace(&d, &e).unwrap();

        assert!(!d.exists());
        assert_eq!(fs::metadata(&e).unwrap().ino(), ino);
        assert_eq!(fs::read_to_string(e.join("f")).unwrap(), "f");
    }

    #[test]
    fn fallback_folder_never_replaces() {
        let dir = tempfile::tempdir().unwrap();
        let (d, e) = (dir.path().join("d"), dir.path().join("e"));
        fs::create_dir(&d).unwrap();
        fs::create_dir(&e).unwrap();

        let err = move_without_noreplace(&d, &e).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::AlreadyExists);
        assert!(d.is_dir());
        assert!(e.is_dir());
    }

    #[test]
    fn placeholder_filled_by_someone_else_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let (d, e) = (dir.path().join("d"), dir.path().join("e"));
        fs::create_dir(&d).unwrap();
        fs::write(d.join("mine"), "mine").unwrap();
        claim_placeholder(&e).unwrap();
        // Another program writes into the placeholder before the rename.
        fs::write(e.join("theirs"), "theirs").unwrap();

        let err = rename_into_placeholder(&d, &e).unwrap_err();

        // rename(2) only replaces an empty directory: ENOTEMPTY, and the
        // placeholder (no longer empty) stays, named in the error.
        assert_eq!(err.kind(), ErrorKind::DirectoryNotEmpty);
        assert!(err.to_string().contains(&e.display().to_string()), "{err}");
        assert_eq!(fs::read_to_string(d.join("mine")).unwrap(), "mine");
        assert_eq!(fs::read_to_string(e.join("theirs")).unwrap(), "theirs");
    }

    #[test]
    fn placeholder_removed_when_rename_fails() {
        let dir = tempfile::tempdir().unwrap();
        let (gone, e) = (dir.path().join("gone"), dir.path().join("e"));
        claim_placeholder(&e).unwrap();

        let err = rename_into_placeholder(&gone, &e).unwrap_err();

        assert_eq!(err.kind(), ErrorKind::NotFound);
        assert!(fs::symlink_metadata(&e).is_err());
    }

    #[test]
    fn link_taken_back_when_source_cannot_be_removed() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "a").unwrap();
        fs::hard_link(&a, &b).unwrap();
        let meta = fs::symlink_metadata(&a).unwrap();
        let cause = std::io::Error::from(ErrorKind::PermissionDenied);

        let err = take_back_link(&meta, &b, cause);

        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert!(fs::symlink_metadata(&b).is_err());
        assert_eq!(fs::read_to_string(&a).unwrap(), "a");
    }

    #[test]
    fn link_not_taken_back_when_name_was_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        fs::write(&a, "a").unwrap();
        fs::write(&b, "someone else's").unwrap();
        let meta = fs::symlink_metadata(&a).unwrap();
        let cause = std::io::Error::from(ErrorKind::PermissionDenied);

        let err = take_back_link(&meta, &b, cause);

        assert_eq!(err.kind(), ErrorKind::PermissionDenied);
        assert_eq!(fs::read_to_string(&b).unwrap(), "someone else's");
    }
}
