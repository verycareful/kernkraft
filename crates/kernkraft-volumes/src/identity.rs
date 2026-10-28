// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Which volume is which: the device number and the disk sequence number.
//!
//! Specification: the Linux kernel's `Documentation/ABI/stable/sysfs-block`,
//! `/sys/block/<disk>/diskseq` ("The disk sequence number, a monotonically
//! increasing 64-bit number the kernel assigns when a disk is created; it is
//! not reused", since Linux 5.15), and sysfs's `/sys/dev/block/<major>:<minor>`
//! links to the device directories. A partition's directory is inside its
//! disk's and has a `partition` file but no `diskseq`; the disk's is at
//! `<partition>/../diskseq`. proc(5), "/proc/pid/mountinfo", for the
//! `major:minor` of a mount and its source.
//!
//! The device number alone is not an identity: the kernel hands the lowest
//! free `sdX` index to the next disk plugged in, so unplugging a stick and
//! plugging another gives the same `major:minor`. The sequence number
//! changes with every new disk object, so the pair tells them apart.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::mountinfo::{read_mountinfo, MountInfoLine};
use crate::{parse_mountinfo_strict, VolumesError};

/// Where sysfs lists block devices by device number.
pub const SYS_DEV_BLOCK: &str = "/sys/dev/block";

/// One disk, or a partition of it, as the kernel numbers it: the block
/// device number and the sequence number of the disk it is on.
///
/// What it is unique over: `major:minor` is unique among the block devices
/// present now and is reused as soon as one is gone. `diskseq` is unique per
/// disk object since the machine booted (not across reboots), and every
/// partition of a disk shares it. Together they name one disk object's
/// partition. They do not notice a change that is not a new disk object: a
/// stick reformatted or repartitioned in place, or a card swapped in a
/// reader that reports no media change, keeps its sequence number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskId {
    pub major: u32,
    pub minor: u32,
    pub diskseq: u64,
}

/// A volume's identity, or why it has none. Never guessed: a volume without
/// one cannot be acted on through the commands that require it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Identity {
    Known(DiskId),
    Unknown { reason: String },
}

impl Identity {
    pub(crate) fn from_result(r: Result<DiskId, VolumesError>) -> Identity {
        match r {
            Ok(id) => Identity::Known(id),
            Err(e) => Identity::Unknown {
                reason: e.to_string(),
            },
        }
    }

    /// The identity, or [`VolumesError::IdentityUnknown`] with the reason.
    pub fn known(&self) -> Result<DiskId, VolumesError> {
        match self {
            Identity::Known(id) => Ok(*id),
            Identity::Unknown { reason } => Err(VolumesError::IdentityUnknown {
                reason: reason.clone(),
            }),
        }
    }
}

fn no_seq(major: u32, minor: u32, reason: impl Into<String>) -> VolumesError {
    VolumesError::NoDiskSeq {
        major,
        minor,
        reason: reason.into(),
    }
}

fn read_seq(path: &Path) -> Result<Option<u64>, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => text
            .trim()
            .parse()
            .map(Some)
            .map_err(|_| format!("{} holds {:?}, not a number", path.display(), text.trim())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// The identity of block device `major:minor`, read from `sys_dev_block`
/// (`/sys/dev/block`, or a directory shaped like it).
pub fn disk_id_in(sys_dev_block: &Path, major: u32, minor: u32) -> Result<DiskId, VolumesError> {
    let dir: PathBuf = sys_dev_block.join(format!("{major}:{minor}"));
    if std::fs::metadata(&dir).is_err() {
        return Err(no_seq(
            major,
            minor,
            format!(
                "no block device {major}:{minor} in {}",
                sys_dev_block.display()
            ),
        ));
    }
    let own = dir.join("diskseq");
    let seq = match read_seq(&own).map_err(|r| no_seq(major, minor, r))? {
        Some(seq) => Some(seq),
        // A partition: its disk's number, one directory up.
        None if dir.join("partition").exists() => {
            let disk = dir.join("..").join("diskseq");
            read_seq(&disk).map_err(|r| no_seq(major, minor, r))?
        }
        None => None,
    };
    seq.map(|diskseq| DiskId {
        major,
        minor,
        diskseq,
    })
    .ok_or_else(|| {
        no_seq(
            major,
            minor,
            format!(
                "{} has no diskseq (the kernel is older than 5.15, or the device has none)",
                dir.display()
            ),
        )
    })
}

/// [`disk_id_in`] on the real `/sys/dev/block`.
pub fn disk_id(major: u32, minor: u32) -> Result<DiskId, VolumesError> {
    disk_id_in(Path::new(SYS_DEV_BLOCK), major, minor)
}

/// Reads the identity of `expected`'s device again and refuses when its
/// sequence number is not the one it was listed with.
pub fn verify_disk_in(sys_dev_block: &Path, expected: &DiskId) -> Result<(), VolumesError> {
    let now = disk_id_in(sys_dev_block, expected.major, expected.minor)?;
    if now.diskseq == expected.diskseq {
        return Ok(());
    }
    Err(VolumesError::IdentityMismatch {
        reason: format!(
            "device {}:{} is now disk sequence number {}, not {}",
            expected.major, expected.minor, now.diskseq, expected.diskseq
        ),
    })
}

/// The block device a mount is on: its mountinfo `major:minor` when sysfs
/// lists that as a block device, else, for filesystems that report an
/// anonymous device (btrfs), the device node its source names.
pub(crate) fn mount_block_device(
    sys_dev_block: &Path,
    line: &MountInfoLine,
) -> Result<(u32, u32), VolumesError> {
    let (major, minor) = (line.major, line.minor);
    if std::fs::metadata(sys_dev_block.join(format!("{major}:{minor}"))).is_ok() {
        return Ok((major, minor));
    }
    let source = Path::new(&line.source);
    let why = |detail: String| {
        no_seq(
            major,
            minor,
            format!(
                "the mount is on device {major}:{minor}, which is not a block device, and its \
                 source {:?} {detail}",
                line.source
            ),
        )
    };
    if !source.starts_with("/dev/") {
        return Err(why("is not a device node".to_string()));
    }
    let st = rustix::fs::stat(source).map_err(|e| why(format!("cannot be read: {e}")))?;
    if rustix::fs::FileType::from_raw_mode(st.st_mode) != rustix::fs::FileType::BlockDevice {
        return Err(why("is not a block device".to_string()));
    }
    Ok((rustix::fs::major(st.st_rdev), rustix::fs::minor(st.st_rdev)))
}

/// The identity of the filesystem mounted on `line`.
pub(crate) fn mount_disk_id(
    sys_dev_block: &Path,
    line: &MountInfoLine,
) -> Result<DiskId, VolumesError> {
    let (major, minor) = mount_block_device(sys_dev_block, line)?;
    disk_id_in(sys_dev_block, major, minor)
}

/// Checks that what is mounted at `mount_point` in `lines` is still the
/// volume `expected` was listed as: the mount is there, on the same block
/// device, whose sequence number is read again now. [`IdentityMismatch`]
/// otherwise; [`NoDiskSeq`] when the identity cannot be read at all.
///
/// [`IdentityMismatch`]: VolumesError::IdentityMismatch
/// [`NoDiskSeq`]: VolumesError::NoDiskSeq
pub fn verify_mount_in(
    sys_dev_block: &Path,
    lines: &[MountInfoLine],
    mount_point: &Path,
    expected: &DiskId,
) -> Result<(), VolumesError> {
    let mismatch = |reason: String| VolumesError::IdentityMismatch { reason };
    // The last line is the one on top when a path is mounted over.
    let line = lines
        .iter()
        .rev()
        .find(|l| l.mount_point == mount_point)
        .ok_or_else(|| mismatch(format!("{} is not mounted", mount_point.display())))?;
    let (major, minor) = mount_block_device(sys_dev_block, line)?;
    if (major, minor) != (expected.major, expected.minor) {
        return Err(mismatch(format!(
            "{} is now on device {major}:{minor}, not {}:{}",
            mount_point.display(),
            expected.major,
            expected.minor
        )));
    }
    verify_disk_in(sys_dev_block, expected)
}

/// [`verify_mount_in`] on this process's mounts and the real sysfs.
pub fn verify_mount(mount_point: &Path, expected: &DiskId) -> Result<(), VolumesError> {
    let lines = parse_mountinfo_strict(read_mountinfo()?)?;
    verify_mount_in(Path::new(SYS_DEV_BLOCK), &lines, mount_point, expected)
}
