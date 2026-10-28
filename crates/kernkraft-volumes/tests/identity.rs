// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Volume identity (disk sequence number) against a sysfs-shaped directory
//! tree standing in for `/sys/dev/block`: `M:m` entries are symlinks to
//! device directories, a partition's directory sits inside its disk's and
//! has a `partition` file, and only a disk has `diskseq` (Linux 5.15,
//! Documentation/ABI/stable/sysfs-block, "diskseq").

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::{Path, PathBuf};

use kernkraft_volumes::{
    disk_id_in, parse_mountinfo, verify_disk_in, verify_mount_in, DiskId, VolumesError,
};

struct Sys {
    root: tempfile::TempDir,
}

impl Sys {
    fn new() -> Sys {
        let root = tempfile::tempdir().unwrap();
        fs::create_dir_all(root.path().join("dev/block")).unwrap();
        fs::create_dir_all(root.path().join("devices")).unwrap();
        Sys { root }
    }

    fn block(&self) -> PathBuf {
        self.root.path().join("dev/block")
    }

    /// A disk `M:m` with the given `diskseq` (none: a kernel without it).
    fn disk(&self, name: &str, majmin: &str, seq: Option<&str>) -> PathBuf {
        let dir = self.root.path().join("devices").join(name);
        fs::create_dir_all(&dir).unwrap();
        if let Some(seq) = seq {
            fs::write(dir.join("diskseq"), format!("{seq}\n")).unwrap();
        }
        symlink(&dir, self.block().join(majmin)).unwrap();
        dir
    }

    /// A partition of `disk`: a directory inside the disk's, with a
    /// `partition` file and no `diskseq`.
    fn partition(&self, disk: &Path, name: &str, majmin: &str) {
        let dir = disk.join(name);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("partition"), "1\n").unwrap();
        symlink(&dir, self.block().join(majmin)).unwrap();
    }

    fn set_seq(&self, disk: &Path, seq: &str) {
        fs::write(disk.join("diskseq"), format!("{seq}\n")).unwrap();
    }
}

fn id(major: u32, minor: u32, diskseq: u64) -> DiskId {
    DiskId {
        major,
        minor,
        diskseq,
    }
}

#[test]
fn a_disk_has_its_own_sequence_number() {
    let sys = Sys::new();
    sys.disk("sdb", "8:16", Some("42"));
    assert_eq!(disk_id_in(&sys.block(), 8, 16).unwrap(), id(8, 16, 42));
}

#[test]
fn a_partition_takes_its_disks_sequence_number() {
    let sys = Sys::new();
    let disk = sys.disk("sdb", "8:16", Some("42"));
    sys.partition(&disk, "sdb1", "8:17");
    sys.partition(&disk, "sdb2", "8:18");
    // Each partition keeps its own device number and shares the disk's seq.
    assert_eq!(disk_id_in(&sys.block(), 8, 17).unwrap(), id(8, 17, 42));
    assert_eq!(disk_id_in(&sys.block(), 8, 18).unwrap(), id(8, 18, 42));
}

#[test]
fn missing_sequence_number_is_a_typed_error_with_a_reason() {
    let sys = Sys::new();
    // A disk on a kernel without diskseq.
    let disk = sys.disk("sdb", "8:16", None);
    sys.partition(&disk, "sdb1", "8:17");
    for (major, minor, why) in [
        (8, 16, "diskseq"),
        (8, 17, "diskseq"),
        // Not a block device sysfs knows.
        (0, 19, "no block device"),
    ] {
        match disk_id_in(&sys.block(), major, minor) {
            Err(VolumesError::NoDiskSeq {
                major: ma,
                minor: mi,
                reason,
            }) => {
                assert_eq!((ma, mi), (major, minor));
                assert!(reason.contains(why), "{reason}");
            }
            other => panic!("{major}:{minor}: {other:?}"),
        }
    }
}

#[test]
fn unreadable_sequence_number_is_a_typed_error() {
    let sys = Sys::new();
    sys.disk("sdb", "8:16", Some("not a number"));
    assert!(matches!(
        disk_id_in(&sys.block(), 8, 16),
        Err(VolumesError::NoDiskSeq { .. })
    ));
}

#[test]
fn a_swapped_disk_is_refused_and_the_same_one_is_not() {
    let sys = Sys::new();
    let disk = sys.disk("sdb", "8:16", Some("42"));
    sys.partition(&disk, "sdb1", "8:17");
    let listed = disk_id_in(&sys.block(), 8, 17).unwrap();
    verify_disk_in(&sys.block(), &listed).unwrap();

    // Unplugged and another stick plugged in: same device number, next
    // sequence number.
    sys.set_seq(&disk, "43");
    match verify_disk_in(&sys.block(), &listed) {
        Err(VolumesError::IdentityMismatch { reason }) => {
            assert!(reason.contains("42") && reason.contains("43"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn verifying_a_vanished_disk_is_an_error() {
    let sys = Sys::new();
    sys.disk("sdb", "8:16", Some("42"));
    let listed = disk_id_in(&sys.block(), 8, 16).unwrap();
    fs::remove_file(sys.block().join("8:16")).unwrap();
    assert!(matches!(
        verify_disk_in(&sys.block(), &listed),
        Err(VolumesError::NoDiskSeq { .. })
    ));
}

const LINES: &str = "\
25 20 8:17 / /run/media/u/STICK rw - exfat /dev/sdb1 rw
26 20 0:40 / /mnt/anon rw - btrfs /dev/does/not/exist rw
";

#[test]
fn a_mount_is_verified_against_its_block_device() {
    let sys = Sys::new();
    let disk = sys.disk("sdb", "8:16", Some("42"));
    sys.partition(&disk, "sdb1", "8:17");
    let lines = parse_mountinfo(LINES);
    let stick = Path::new("/run/media/u/STICK");
    let listed = id(8, 17, 42);
    verify_mount_in(&sys.block(), &lines, stick, &listed).unwrap();

    // Another disk now (new sequence number), another device number, a path
    // that is not mounted: each refused.
    sys.set_seq(&disk, "43");
    for (mp, wanted) in [
        (stick, listed),
        (stick, id(8, 18, 43)),
        (Path::new("/run/media/u/GONE"), id(8, 17, 43)),
    ] {
        assert!(
            matches!(
                verify_mount_in(&sys.block(), &lines, mp, &wanted),
                Err(VolumesError::IdentityMismatch { .. })
            ),
            "{mp:?} {wanted:?}"
        );
    }
    verify_mount_in(&sys.block(), &lines, stick, &id(8, 17, 43)).unwrap();
}

/// A mount on an anonymous device (0:N) whose source is no block device has
/// no identity: an error that says so, never a pass.
#[test]
fn a_mount_without_a_block_device_has_no_identity() {
    let sys = Sys::new();
    let lines = parse_mountinfo(LINES);
    match verify_mount_in(&sys.block(), &lines, Path::new("/mnt/anon"), &id(0, 40, 1)) {
        Err(VolumesError::NoDiskSeq { reason, .. }) => {
            assert!(reason.contains("block device"), "{reason}");
        }
        other => panic!("{other:?}"),
    }
}
