// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Atomic save: the target either keeps its old contents or holds the new
//! ones in full, never a mix, whatever fails or crashes in between.
//!
//! The new contents go to a temp file in the target's own directory (so the
//! final step is a same-filesystem `rename(2)`, which POSIX makes atomic),
//! are flushed and `fsync`ed, take the old file's owner, group, extended
//! attributes (POSIX ACLs among them) and permission bits, and are renamed
//! over the target. The directory is then `fsync`ed so the rename
//! itself survives a crash.

use std::fs::{self, File, Permissions};
use std::io::{self, BufWriter, ErrorKind, Write};
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use rustix::fs::{fchown, Gid, Uid};
use rustix::io::Errno;

use crate::sys::parent_dir;
use crate::xattr::copy_all;
use crate::FsError;

/// Linux's `MAXSYMLINKS` (include/linux/namei.h): the kernel gives up with
/// `ELOOP` after this many links in one lookup, and so do we.
const MAX_SYMLINKS: u32 = 40;

/// Permission bits of a new file before the umask is applied, as `open(2)`
/// callers such as `touch` and text editors pass them.
const NEW_FILE_MODE: u32 = 0o666;

/// Mode of the temp file while it is written over an existing file: owner
/// only, so a file that was private is never briefly readable by others.
const TEMP_MODE: u32 = 0o600;

/// Permission bits `chmod(2)` sets: rwx for all three classes plus setuid,
/// setgid and sticky.
const PERMISSION_BITS: u32 = 0o7777;

/// Identity and state of a file when it was read, used to detect that
/// something else changed it before a save.
///
/// Length and modification time catch an in-place write; device and inode
/// catch a replacement by rename (another program's atomic save); the inode
/// change time catches a same-length write that restored the old mtime,
/// since no unprivileged process can set it.
///
/// The timestamps are only as fine as the filesystem keeps them. A
/// same-length in-place write that lands in the same timestamp tick as the
/// read (a few milliseconds on ext4) cannot be told apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FileStamp {
    pub len: u64,
    pub modified: SystemTime,
    pub dev: u64,
    pub ino: u64,
    pub changed_ns: i128,
}

impl FileStamp {
    /// The stamp of the file at `path`, following symlinks.
    pub fn of(path: &Path) -> Result<FileStamp, FsError> {
        let meta = fs::metadata(path).map_err(|source| FsError::io(path, source))?;
        Self::from_metadata(path, &meta)
    }

    fn from_metadata(path: &Path, meta: &fs::Metadata) -> Result<FileStamp, FsError> {
        const NANOS_PER_SEC: i128 = 1_000_000_000;
        Ok(FileStamp {
            len: meta.len(),
            modified: meta
                .modified()
                .map_err(|source| FsError::io(path, source))?,
            dev: meta.dev(),
            ino: meta.ino(),
            changed_ns: i128::from(meta.ctime()) * NANOS_PER_SEC + i128::from(meta.ctime_nsec()),
        })
    }
}

/// Atomically replaces (or creates) the file at `path` with what `write`
/// produces.
///
/// A symlink at `path` is followed, through any chain, and its final target
/// is written; the links stay as they are. With `expected = Some(stamp)` the
/// target must still match `stamp`, else nothing is written and the result
/// is [`FsError::ExternalModification`]. An existing target's owner, group,
/// extended attributes (POSIX ACLs among them) and permission bits carry
/// over; a new file gets `0o666` less the process umask. A save that would
/// change the owner or group (the file belongs to another user, or to a
/// group this process is not in) is refused with
/// [`FsError::OwnershipNotKept`], and one that cannot set an attribute with
/// [`FsError::XattrsNotKept`].
///
/// On any error, including one returned by `write`, the target and its
/// directory are left exactly as they were. Returns the stamp of the file
/// now on disk.
pub fn write_atomic(
    path: &Path,
    expected: Option<&FileStamp>,
    write: impl FnOnce(&mut dyn Write) -> io::Result<()>,
) -> Result<FileStamp, FsError> {
    let target = resolve_symlinks(path).map_err(|source| FsError::io(path, source))?;
    let existing = match fs::metadata(&target) {
        Ok(meta) => Some(meta),
        Err(e) if e.kind() == ErrorKind::NotFound => None,
        Err(source) => return Err(FsError::io(&target, source)),
    };

    if let Some(meta) = &existing {
        if meta.is_dir() {
            return Err(FsError::io(
                &target,
                io::Error::from(ErrorKind::IsADirectory),
            ));
        }
    }
    if let Some(stamp) = expected {
        let current = existing
            .as_ref()
            .map(|meta| FileStamp::from_metadata(&target, meta))
            .transpose()?;
        if current.as_ref() != Some(stamp) {
            return Err(FsError::ExternalModification { path: target });
        }
    }

    let dir = parent_dir(&target);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let create_mode = if existing.is_some() {
        TEMP_MODE
    } else {
        NEW_FILE_MODE
    };
    let temp = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .suffix(".kk-tmp")
        .permissions(Permissions::from_mode(create_mode))
        .tempfile_in(dir)
        .map_err(|source| FsError::io(dir, source))?;
    let temp_path = temp.path().to_path_buf();
    let fail = |source| FsError::io(&temp_path, source);

    let mut out = BufWriter::new(temp.as_file());
    write(&mut out).map_err(fail)?;
    out.flush().map_err(fail)?;
    drop(out);

    if let Some(meta) = &existing {
        // fchown(2) clears the setuid and setgid bits, so ownership goes
        // first and the mode is applied after it.
        keep_ownership(temp.as_file(), meta, &target, &temp_path)?;
        let refused = copy_all(&target, temp.as_file()).map_err(fail)?;
        if !refused.is_empty() {
            return Err(FsError::XattrsNotKept {
                path: target,
                names: refused,
            });
        }
        let mode = meta.permissions().mode() & PERMISSION_BITS;
        temp.as_file()
            .set_permissions(Permissions::from_mode(mode))
            .map_err(fail)?;
    }
    temp.as_file().sync_all().map_err(fail)?;
    temp.persist(&target)
        .map_err(|e| FsError::io(&target, e.error))?;
    File::open(dir)
        .and_then(|d| d.sync_all())
        .map_err(|source| FsError::io(dir, source))?;

    FileStamp::of(&target)
}

/// Gives `temp` the owner and group of the file `meta` describes, where
/// they differ from what the temp file was created with.
fn keep_ownership(
    temp: &File,
    meta: &fs::Metadata,
    target: &Path,
    temp_path: &Path,
) -> Result<(), FsError> {
    let now = temp.metadata().map_err(|e| FsError::io(temp_path, e))?;
    let uid = (now.uid() != meta.uid()).then(|| Uid::from_raw(meta.uid()));
    let gid = (now.gid() != meta.gid()).then(|| Gid::from_raw(meta.gid()));
    if uid.is_none() && gid.is_none() {
        return Ok(());
    }
    match fchown(temp, uid, gid) {
        Ok(()) => Ok(()),
        Err(Errno::PERM) => Err(FsError::OwnershipNotKept {
            path: target.to_path_buf(),
        }),
        Err(e) => Err(FsError::io(temp_path, e.into())),
    }
}

/// [`write_atomic`] with the contents given as bytes.
pub fn write_bytes_atomic(
    path: &Path,
    expected: Option<&FileStamp>,
    bytes: &[u8],
) -> Result<FileStamp, FsError> {
    write_atomic(path, expected, |w| w.write_all(bytes))
}

/// Follows `path` through every symlink to the path that is not one (or does
/// not exist yet). A relative link resolves against the directory holding it.
fn resolve_symlinks(path: &Path) -> io::Result<PathBuf> {
    let mut current = path.to_path_buf();
    for _ in 0..=MAX_SYMLINKS {
        match fs::symlink_metadata(&current) {
            Ok(meta) if meta.file_type().is_symlink() => {
                let link = fs::read_link(&current)?;
                current = parent_dir(&current).join(link);
            }
            Ok(_) => return Ok(current),
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(current),
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::from(rustix::io::Errno::LOOP))
}
