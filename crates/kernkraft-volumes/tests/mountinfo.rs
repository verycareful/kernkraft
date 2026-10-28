// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! proc(5) `/proc/pid/mountinfo` parsing and the user-relevant filter. The
//! text is built here in the documented field order:
//! `id parent major:minor root mount-point options [optional...] - fstype source super-options`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::ffi::OsStr;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use kernkraft_volumes::{
    decode_udev_name, parse_mountinfo, parse_mountinfo_report, parse_mountinfo_strict,
    resolve_mount_in, user_mounts, ListProblem, Mount, MountKey, VolumesError,
};

const USER: &str = "sricharan";
const UID: u32 = 1000;

fn sample() -> String {
    [
        "20 1 0:19 /@ / rw,noatime shared:1 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@",
        "21 20 0:19 /@home /home rw,noatime shared:2 - btrfs /dev/nvme0n1p2 rw,ssd,subvol=/@home",
        "22 20 0:5 / /dev rw,nosuid shared:3 - devtmpfs devtmpfs rw,size=4k",
        "23 20 0:22 / /tmp rw,nosuid,nodev shared:4 - tmpfs tmpfs rw",
        "24 20 259:1 / /boot rw,relatime shared:5 - vfat /dev/nvme0n1p1 rw,fmask=0022",
        r"25 20 8:17 / /run/media/sricharan/USB\040STICK rw,nosuid,nodev,relatime shared:6 - exfat /dev/sdb1 rw,uid=1000",
        "26 20 8:33 / /mnt/data rw,relatime shared:7 - ext4 /dev/sdc1 rw",
        "27 20 0:30 / /run/user/1000/doc rw,nosuid,nodev,relatime shared:8 - fuse.portal portal rw,user_id=1000,group_id=1000",
    ]
    .join("\n")
        + "\n"
}

fn mounts(text: &str) -> Vec<Mount> {
    let lines = parse_mountinfo(text);
    user_mounts(&lines, UID, USER, &|_| false)
}

fn points(m: &[Mount]) -> Vec<PathBuf> {
    m.iter().map(|m| m.mount_point.clone()).collect()
}

#[test]
fn decodes_octal_escapes() {
    let lines = parse_mountinfo(&sample());
    assert_eq!(lines.len(), 8);
    let usb = &lines[5];
    assert_eq!(usb.mount_id, 25);
    assert_eq!((usb.major, usb.minor), (8, 17));
    // \040 is a space (proc(5): the kernel escapes space, tab, newline and backslash as octal).
    assert_eq!(usb.mount_point, Path::new("/run/media/sricharan/USB STICK"));
    assert_eq!(usb.fs_type, "exfat");
    assert_eq!(usb.source, "/dev/sdb1");
    assert_eq!(usb.optional_fields, vec!["shared:6".to_string()]);
    assert_eq!(lines[0].optional_fields, vec!["shared:1".to_string()]);
    assert_eq!(lines[0].root, Path::new("/@"));

    // \011 tab, \012 newline, \134 backslash; \377 is a byte that is not UTF-8.
    let odd =
        parse_mountinfo("30 20 8:1 / /mnt/a\\011b\\012c\\134d\\377e rw - ext4 /dev/sda1 rw\n");
    // No optional fields: the separator follows the options directly.
    assert!(odd[0].optional_fields.is_empty());
    assert_eq!(
        odd[0].mount_point.as_os_str().as_bytes(),
        b"/mnt/a\tb\nc\\d\xffe"
    );
}

#[test]
fn keeps_removable_media() {
    let m = mounts(&sample());
    let usb = m
        .iter()
        .find(|m| m.mount_point == Path::new("/run/media/sricharan/USB STICK"))
        .expect("usb stick kept");
    assert!(usb.removable);
    assert!(!usb.read_only);
    assert_eq!(usb.device, "/dev/sdb1");
    assert_eq!(usb.fs_type, "exfat");
    // The pure filter does no by-label lookup: the label is the mount point's name.
    assert_eq!(usb.label, "USB STICK");
}

#[test]
fn drops_root_home_and_pseudo() {
    let p = points(&mounts(&sample()));
    for gone in ["/", "/home", "/dev", "/tmp", "/boot", "/run/user/1000/doc"] {
        assert!(
            !p.contains(&PathBuf::from(gone)),
            "{gone} should be dropped"
        );
    }
    // Only the stick and /mnt/data survive.
    assert_eq!(p.len(), 2, "{p:?}");
}

#[test]
fn keeps_writable_mnt() {
    let m = mounts(&sample());
    let data = m
        .iter()
        .find(|m| m.mount_point == Path::new("/mnt/data"))
        .expect("/mnt/data kept without asking is_writable");
    assert!(!data.removable);

    // A mount elsewhere is kept only when the user can write to it.
    let text = "40 20 8:49 / /srv/games rw - ext4 /dev/sdd1 rw\n\
                41 20 8:50 / /opt/vendor ro - ext4 /dev/sdd2 ro\n";
    let lines = parse_mountinfo(text);
    let kept = user_mounts(&lines, UID, USER, &|p| p == Path::new("/srv/games"));
    assert_eq!(points(&kept), vec![PathBuf::from("/srv/games")]);
}

#[test]
fn read_only_from_either_option_set() {
    let text = "40 20 8:49 / /mnt/a ro - ext4 /dev/sdd1 rw\n\
                41 20 8:50 / /mnt/b rw - ext4 /dev/sdd2 ro\n\
                42 20 8:51 / /mnt/c rw - ext4 /dev/sdd3 rw\n";
    let m = mounts(text);
    assert_eq!(
        m.iter().map(|m| m.read_only).collect::<Vec<_>>(),
        vec![true, true, false]
    );
}

#[test]
fn other_users_media_and_run_are_dropped() {
    let text = "40 20 8:49 / /run/media/other/X rw - ext4 /dev/sdd1 rw\n\
                41 20 8:50 / /run/foo rw - ext4 /dev/sdd2 rw\n\
                42 20 8:51 / /media/cd rw - iso9660 /dev/sr0 ro\n\
                43 20 8:52 / /sys/fs/x rw - ext4 /dev/sdd4 rw\n\
                44 20 8:53 / /efi rw - vfat /dev/sdd5 rw\n\
                45 20 8:54 / /boot/efi rw - vfat /dev/sdd6 rw\n";
    let lines = parse_mountinfo(text);
    // Even an everywhere-writable closure cannot rescue the excluded trees.
    let kept = user_mounts(&lines, UID, USER, &|_| true);
    assert_eq!(points(&kept), vec![PathBuf::from("/media/cd")]);
}

#[test]
fn fuse_mounts_of_other_users_are_dropped() {
    // Outside /run/media, /media and /mnt the owner decides.
    let text = "40 20 0:40 / /srv/mine rw - fuse.rclone me:/ rw,user_id=1000,group_id=1000\n\
                41 20 0:41 / /srv/theirs rw - fuse.rclone x:/ rw,user_id=1001,group_id=1001\n";
    let lines = parse_mountinfo(text);
    let kept = user_mounts(&lines, UID, USER, &|_| true);
    assert_eq!(points(&kept), vec![PathBuf::from("/srv/mine")]);
}

#[test]
fn malformed_lines_never_panic_and_are_reported() {
    let bad = [
        "garbage",
        "1 2 3",
        "20 1 0:19 / / rw",
        "x 1 0:19 / / rw - btrfs /dev/x rw",
        "20 1 0:19 / / rw btrfs /dev/x rw",
        "20 1 0:19 / / rw - btrfs /dev/x",
        "20 1 0:19 / /a\\9 rw - btrfs /dev/x rw",
        "20 1 0:19 / /a\\777 rw - btrfs /dev/x rw",
        "20 1 0:19 / /a\\ rw - btrfs /dev/x rw",
        "20 1 019 / / rw - btrfs /dev/x rw",
    ];
    for line in bad {
        // A well-formed line after the bad one must still parse.
        let text = format!("{line}\n20 1 0:19 / / rw - btrfs /dev/x rw\n");
        assert_eq!(parse_mountinfo(&text).len(), 1, "{line:?}");
        match parse_mountinfo_strict(&text) {
            Err(VolumesError::MalformedMountinfo { line: n, .. }) => assert_eq!(n, 1, "{line:?}"),
            other => panic!("{line:?}: {other:?}"),
        }
    }
    // Blank lines are not entries.
    assert_eq!(parse_mountinfo_strict("\n\n").unwrap().len(), 0);
    // Arbitrary bytes must not panic either.
    let junk = String::from_utf8_lossy(&(0u8..=255).collect::<Vec<_>>()).into_owned();
    let _ = parse_mountinfo(&junk);
}

#[test]
fn udev_label_escapes_are_decoded() {
    // udev writes bytes outside its safe set as \xNN (lower-case hex).
    assert_eq!(decode_udev_name(OsStr::new(r"USB\x20STICK")), "USB STICK");
    assert_eq!(decode_udev_name(OsStr::new(r"a\x5cb")), "a\\b");
    assert_eq!(decode_udev_name(OsStr::new(r"caf\xc3\xa9")), "caf\u{e9}");
    // A malformed escape is kept as written, never dropped.
    assert_eq!(decode_udev_name(OsStr::new(r"a\xZZb\x4")), r"a\xZZb\x4");
    assert_eq!(decode_udev_name(OsStr::new("plain")), "plain");
}

#[test]
fn system_subvolume_dropped() {
    // /var/tmp is a subvolume of the root's device (0:19 here), writable by the user.
    let text = "20 1 0:19 /@ / rw - btrfs /dev/nvme0n1p2 rw\n\
                21 20 0:19 /@home /home rw - btrfs /dev/nvme0n1p2 rw\n\
                22 20 0:19 /@tmp /var/tmp rw - btrfs /dev/nvme0n1p2 rw\n\
                23 20 0:19 /@games /srv/games rw - btrfs /dev/nvme0n1p2 rw\n\
                24 20 8:33 / /srv/other rw - ext4 /dev/sdc1 rw\n";
    let lines = parse_mountinfo(text);
    let kept = user_mounts(&lines, UID, USER, &|_| true);
    assert_eq!(points(&kept), vec![PathBuf::from("/srv/other")]);
}

#[test]
fn root_device_bind_under_mnt_kept() {
    let text = "20 1 0:19 /@ / rw - btrfs /dev/nvme0n1p2 rw\n\
                21 20 0:19 /@data /mnt/data rw - btrfs /dev/nvme0n1p2 rw\n\
                22 20 0:19 /@m /media/m rw - btrfs /dev/nvme0n1p2 rw\n\
                23 20 0:19 /@u /run/media/sricharan/u rw - btrfs /dev/nvme0n1p2 rw\n";
    let kept = mounts(text);
    assert_eq!(kept.len(), 3, "{:?}", points(&kept));
}

#[test]
fn no_root_or_home_line_excludes_nothing_by_device() {
    let text = "22 20 0:19 /@tmp /var/tmp rw - btrfs /dev/nvme0n1p2 rw\n";
    let lines = parse_mountinfo(text);
    let kept = user_mounts(&lines, UID, USER, &|_| true);
    assert_eq!(points(&kept), vec![PathBuf::from("/var/tmp")]);
}

#[test]
fn overmounted_root_uses_the_last_line() {
    // The first / (0:19) is covered by a second one (0:20).
    let text = "20 1 0:19 / / rw - ext4 /dev/a rw\n\
                21 20 0:20 / / rw - ext4 /dev/b rw\n\
                22 21 0:19 / /srv/a rw - ext4 /dev/a rw\n\
                23 21 0:20 / /srv/b rw - ext4 /dev/b rw\n";
    let lines = parse_mountinfo(text);
    let kept = user_mounts(&lines, UID, USER, &|_| true);
    assert_eq!(points(&kept), vec![PathBuf::from("/srv/a")]);
}

#[test]
fn fuseblk_owned_by_root_kept_under_media() {
    // ntfs-3g runs as root, so its mountinfo says user_id=0.
    let text =
        "40 20 8:17 / /run/media/sricharan/NTFS rw - fuseblk /dev/sdb1 rw,user_id=0,group_id=0\n\
                41 20 8:18 / /mnt/n rw - fuseblk /dev/sdb2 rw,user_id=0,group_id=0\n\
                42 20 8:19 / /srv/n rw - fuseblk /dev/sdb3 rw,user_id=0,group_id=0\n";
    let lines = parse_mountinfo(text);
    let kept = user_mounts(&lines, UID, USER, &|_| true);
    assert_eq!(
        points(&kept),
        vec![
            PathBuf::from("/run/media/sricharan/NTFS"),
            PathBuf::from("/mnt/n")
        ]
    );
}

#[test]
fn remote_filesystems_are_not_probed_or_listed_outside_the_named_trees() {
    let text = "40 20 0:40 / /srv/share rw - nfs4 host:/x rw\n\
                41 20 0:41 / /srv/smb rw - cifs //h/s rw\n\
                42 20 0:42 / /mnt/share rw - nfs4 host:/y rw\n";
    let lines = parse_mountinfo(text);
    let probe = |p: &Path| -> bool {
        assert!(!p.starts_with("/srv"), "probed {p:?}");
        true
    };
    let kept = user_mounts(&lines, UID, USER, &probe);
    assert_eq!(points(&kept), vec![PathBuf::from("/mnt/share")]);
}

#[test]
fn mount_point_containing_separator_text() {
    // " - " inside a name arrives as \040-\040 and must not end the optional fields.
    let text = "30 20 8:1 / /mnt/a\\040-\\040b rw shared:1 - ext4 /dev/sda1 rw\n";
    let lines = parse_mountinfo_strict(text).unwrap();
    assert_eq!(lines[0].mount_point, Path::new("/mnt/a - b"));
    assert_eq!(lines[0].optional_fields, vec!["shared:1".to_string()]);
    assert_eq!(lines[0].fs_type, "ext4");
}

#[test]
fn report_lists_skipped_lines() {
    let text = "garbage\n20 1 0:19 / / rw - btrfs /dev/x rw\n1 2 3\n";
    let (lines, problems) = parse_mountinfo_report(text);
    assert_eq!(lines.len(), 1);
    assert_eq!(problems.len(), 2);
    assert!(matches!(
        problems[0],
        ListProblem::MalformedLine { line: 1, .. }
    ));
    assert!(matches!(
        problems[1],
        ListProblem::MalformedLine { line: 3, .. }
    ));
}

#[test]
fn mount_identity_is_the_mountinfo_ids() {
    let m = mounts(&sample());
    let stick = m
        .iter()
        .find(|m| m.mount_point.ends_with("USB STICK"))
        .unwrap();
    // "25 20 8:17": mount ID 25, device 8:17.
    assert_eq!(
        stick.key(),
        MountKey {
            mount_id: 25,
            major: 8,
            minor: 17
        }
    );
    let data = m.iter().find(|m| m.mount_point.ends_with("data")).unwrap();
    assert_eq!(
        (data.mount_id, data.major, data.minor),
        (26, 8, 33),
        "fields are public"
    );
}

fn key(mount_id: u32, major: u32, minor: u32) -> MountKey {
    MountKey {
        mount_id,
        major,
        minor,
    }
}

#[test]
fn identity_resolves_to_the_mount_point() {
    let lines = parse_mountinfo(&sample());
    let stick = Path::new("/run/media/sricharan/USB STICK");
    // Identity alone, and identity with its own mount point.
    assert_eq!(
        resolve_mount_in(&lines, None, Some(key(25, 8, 17))).unwrap(),
        stick
    );
    assert_eq!(
        resolve_mount_in(&lines, Some(stick), Some(key(25, 8, 17))).unwrap(),
        stick
    );
    // The path alone is taken as it is.
    assert_eq!(resolve_mount_in(&lines, Some(stick), None).unwrap(), stick);
}

#[test]
fn swapped_device_is_refused() {
    let lines = parse_mountinfo(&sample());
    let stick = Path::new("/run/media/sricharan/USB STICK");
    for (mp, k) in [
        // The ID is gone.
        (None, key(99, 8, 17)),
        // The ID now belongs to another device.
        (None, key(25, 8, 18)),
        // The ID is a different mount than the path says.
        (Some(Path::new("/mnt/data")), key(25, 8, 17)),
        (Some(stick), key(26, 8, 33)),
    ] {
        match resolve_mount_in(&lines, mp, Some(k)) {
            Err(VolumesError::IdentityMismatch { .. }) => {}
            other => panic!("{mp:?} {k:?}: {other:?}"),
        }
    }
    // Neither a path nor an identity.
    assert!(matches!(
        resolve_mount_in(&lines, None, None),
        Err(VolumesError::IdentityMismatch { .. })
    ));
}
