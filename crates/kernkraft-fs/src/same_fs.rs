// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Whether moving an item into a folder is a single rename.
//!
//! A move is a rename(2), and rename(2) works only within one mount. From
//! its ERRORS section, EXDEV: "oldpath and newpath are not on the same
//! mounted filesystem. (Linux permits a filesystem to be mounted at multiple
//! points, but rename() does not work across different mount points, even if
//! the same filesystem is mounted on both.)".
//!
//! The folders the item leaves and enters decide, not the item. The VFS
//! (`do_renameat2` in the kernel's `fs/namei.c`) looks up the parent folder
//! of the old name and of the new one and fails with EXDEV when they are on
//! different mounts. btrfs (`btrfs_rename` in `fs/btrfs/inode.c`) fails with
//! EXDEV when they are in different subvolumes, except that the root of a
//! subvolume may move. So a source is judged by its parent folder, followed
//! as the path leads to it, against the destination folder. The source
//! itself is looked at only to confirm that it exists (a dangling symlink
//! does), because a subvolume's root and an overlayfs file report a device
//! number other than their folder's although they move like any entry of it.
//!
//! The device number alone (`st_dev`, stat(2)) is not enough: a bind mount
//! has the device number of the folder it was made from and is another
//! mount. The mount ID (`stx_mnt_id`, statx(2) with `STATX_MNT_ID`, since
//! Linux 5.8, "the mount ID of the mount containing the file") tells mounts
//! apart, and the device number still has to match: btrfs gives each
//! subvolume a device number of its own inside one mount (`btrfs_getattr` in
//! `fs/btrfs/inode.c`), which is how subvolumes are told apart. A place is
//! the pair; two places are on one filesystem when both match.
//!
//! Both are read in one process at about the same time, so equal mount IDs
//! name the same mount (they are reused only after a mount goes away). A
//! mount changing between the calls and the move is not caught; the move
//! then fails with EXDEV and [`crate::move_items`] copies the item and then
//! deletes the source.
//!
//! # Where an answer can be wrong
//!
//! The mount and subvolume rules are those of the VFS and btrfs. A
//! filesystem that adds rules of its own, overlayfs and the FUSE filesystems
//! among them, can refuse a rename between two places called the same, or
//! allow one between two called different. Each wrong answer has a price:
//!
//! - Called the same, refused with EXDEV: [`crate::move_items`] falls back to
//!   copying the item and deleting the source. The result is right and only
//!   slower.
//! - Called different, though rename(2) would have done it: the caller copies
//!   where the user meant a move, so the item ends up in both places and the
//!   source stays. Moving the root of a btrfs subvolume into a folder of
//!   another subvolume is such a case: btrfs renames it, and this says
//!   different.
//!
//! Neither loses data.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use rustix::fs::{statx, AtFlags, StatxFlags, CWD};
use rustix::io::Errno;

use crate::sys::parent_dir;
use crate::FsError;

/// The device a path's filesystem reports and the mount that holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Place {
    dev_major: u32,
    dev_minor: u32,
    mnt_id: u64,
}

/// Why a place could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Unread {
    Errno(Errno),
    NoMountId,
}

impl Unread {
    /// The error for `path`, which is what was being read.
    fn error(self, path: &Path) -> FsError {
        match self {
            Unread::Errno(e) => FsError::not_found_or_io(path, e.into()),
            Unread::NoMountId => FsError::MountIdUnavailable {
                path: path.to_path_buf(),
            },
        }
    }
}

/// The place from the fields of a `statx` result. `mask` is `stx_mask`: from
/// statx(2), "If a filesystem does not support a field ... the mask bit
/// corresponding to that field will be cleared in stx_mask even if the user
/// asked for it", and a kernel older than 5.8 does not know the bit at all,
/// so a mount ID without its bit set is no mount ID.
fn place_from(mask: u32, dev_major: u32, dev_minor: u32, mnt_id: u64) -> Result<Place, Unread> {
    if mask & StatxFlags::MNT_ID.bits() == 0 {
        return Err(Unread::NoMountId);
    }
    Ok(Place {
        dev_major,
        dev_minor,
        mnt_id,
    })
}

/// The place of `path`, following a final symlink only when `follow`. The
/// final component is not automounted, as stat(2) and lstat(2) do not
/// (statx(2), `AT_NO_AUTOMOUNT`: "All of stat(2), lstat(2), and fstatat(2)
/// act as though AT_NO_AUTOMOUNT was set"), so asking never mounts anything.
fn place_of(path: &Path, follow: bool) -> Result<Place, Unread> {
    let mut flags = AtFlags::NO_AUTOMOUNT;
    if !follow {
        flags |= AtFlags::SYMLINK_NOFOLLOW;
    }
    let st = statx(CWD, path, flags, StatxFlags::MNT_ID).map_err(Unread::Errno)?;
    place_from(
        st.stx_mask,
        st.stx_dev_major,
        st.stx_dev_minor,
        st.stx_mnt_id,
    )
}

/// Confirms that `path` exists, as `lstat(2)` would: a symlink is itself, so
/// a dangling one exists. Nothing is automounted (see [`place_of`]).
fn exists(path: &Path) -> Result<(), Unread> {
    let flags = AtFlags::NO_AUTOMOUNT | AtFlags::SYMLINK_NOFOLLOW;
    statx(CWD, path, flags, StatxFlags::TYPE)
        .map(|_| ())
        .map_err(Unread::Errno)
}

/// For each of `sources`, whether moving it into the folder `dest` stays on
/// one filesystem, that is, is a single rename(2) rather than a copy and a
/// delete. One answer per source, in order.
///
/// The folder a source is in is compared with `dest`, because that is what
/// rename(2) compares (see the module documentation, which also says where
/// an answer can be wrong and what that costs). The source is only checked
/// to exist, `lstat(2)` style: a dangling symlink counts, and a symlink is
/// moved as itself. Both folders are looked at as `stat(2)` does, following
/// symlinks, so a symlink to a folder is that folder. Nothing is created,
/// opened or mounted.
///
/// The answer is `Ok(true)` when both the device number and the mount ID of
/// the two folders match. A source that is itself a mount point is judged by
/// the folder it is in; rename(2) does not move a mount point at all
/// (`EBUSY`), which is a failure of the move and not part of this answer.
/// `dest` is not required to be a folder; this tells where it is, not
/// whether an item can go in.
///
/// A failure is that source's own answer ([`FsError::NotFound`] or
/// [`FsError::Io`] for the source or for its folder; [`FsError::InvalidName`]
/// for a path with no name in a folder, such as `/` or one ending in `..`),
/// except that a `dest` that cannot be looked at makes every answer an error
/// naming `dest`. A kernel that reports no mount ID is
/// [`FsError::MountIdUnavailable`], never a guess.
pub fn same_filesystem(sources: &[PathBuf], dest: &Path) -> Vec<Result<bool, FsError>> {
    let to = place_of(dest, true);
    // Sources are usually many items of one folder: look at it once.
    let mut folders: HashMap<&Path, Result<Place, Unread>> = HashMap::new();
    sources
        .iter()
        .map(|source| {
            let to = to.map_err(|e| e.error(dest))?;
            exists(source).map_err(|e| e.error(source))?;
            if source.file_name().is_none() {
                return Err(FsError::InvalidName {
                    name: source.to_string_lossy().into_owned(),
                });
            }
            let folder = parent_dir(source);
            let from = *folders
                .entry(folder)
                .or_insert_with(|| place_of(folder, true));
            let from = from.map_err(|e| e.error(folder))?;
            Ok(from == to)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    const MNT_ID: u32 = StatxFlags::MNT_ID.bits();

    #[test]
    fn a_mount_id_without_its_mask_bit_is_no_mount_id() {
        // Everything else in the mask, but not the mount ID's bit: a kernel
        // before 5.8, or a result that left the field out.
        let others = StatxFlags::BASIC_STATS.bits();
        assert_eq!(MNT_ID & others, 0);
        assert_eq!(place_from(others, 0, 40, 7), Err(Unread::NoMountId));
        assert_eq!(
            place_from(others | MNT_ID, 0, 40, 7),
            Ok(Place {
                dev_major: 0,
                dev_minor: 40,
                mnt_id: 7
            })
        );
    }

    #[test]
    fn the_missing_mount_id_is_a_typed_error_naming_the_path() {
        let path = Path::new("/some/where");
        match Unread::NoMountId.error(path) {
            FsError::MountIdUnavailable { path: p } => assert_eq!(p, path),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn places_differ_by_either_part() {
        let place = |dev_minor, mnt_id| Place {
            dev_major: 0,
            dev_minor,
            mnt_id,
        };
        assert_eq!(place(40, 7), place(40, 7));
        // A bind mount: same device, other mount.
        assert_ne!(place(40, 7), place(40, 8));
        // A btrfs subvolume: same mount, other device number.
        assert_ne!(place(40, 7), place(41, 7));
    }
}
