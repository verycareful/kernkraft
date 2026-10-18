// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::Path;

use kernkraft_fs::{write_atomic, write_bytes_atomic, FileStamp, FsError};

const TIMESTAMP_TICK_TRIES: u32 = 200;
const TIMESTAMP_TICK_MS: u64 = 5;

fn entries(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

fn process_umask() -> u32 {
    let status = fs::read_to_string("/proc/self/status").unwrap();
    let line = status.lines().find(|l| l.starts_with("Umask:")).unwrap();
    u32::from_str_radix(line.split_whitespace().nth(1).unwrap(), 8).unwrap()
}

#[test]
fn replaces_contents() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    write_bytes_atomic(&path, None, b"new").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "new");
    assert_eq!(entries(dir.path()), ["f.txt"]);
}

#[test]
fn creates_missing_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt");
    write_bytes_atomic(&path, None, b"hello").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "hello");
}

#[test]
fn new_file_mode_follows_umask() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("new.txt");
    write_bytes_atomic(&path, None, b"x").unwrap();
    let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o666 & !process_umask());
}

#[test]
fn keeps_permission_bits() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
    write_bytes_atomic(&path, None, b"new").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o640
    );
}

#[test]
fn keeps_permission_bits_wider_than_umask() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.sh");
    fs::write(&path, "old").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o777)).unwrap();
    write_bytes_atomic(&path, None, b"new").unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o777
    );
}

#[test]
fn writes_through_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real.txt");
    let link = dir.path().join("link.txt");
    fs::write(&real, "old").unwrap();
    symlink("real.txt", &link).unwrap();
    write_bytes_atomic(&link, None, b"new").unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&link).unwrap(), Path::new("real.txt"));
    assert_eq!(fs::read_to_string(&real).unwrap(), "new");
}

#[test]
fn writes_through_symlink_chain_into_other_dir() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("sub")).unwrap();
    let real = dir.path().join("sub/real.txt");
    fs::write(&real, "old").unwrap();
    symlink("real.txt", dir.path().join("sub/hop")).unwrap();
    symlink("sub/hop", dir.path().join("link")).unwrap();
    write_bytes_atomic(&dir.path().join("link"), None, b"new").unwrap();
    assert_eq!(fs::read_to_string(&real).unwrap(), "new");
    assert_eq!(entries(&dir.path().join("sub")), ["hop", "real.txt"]);
}

#[test]
fn dangling_symlink_creates_its_target() {
    let dir = tempfile::tempdir().unwrap();
    let link = dir.path().join("link");
    symlink("target.txt", &link).unwrap();
    write_bytes_atomic(&link, None, b"made").unwrap();
    assert!(fs::symlink_metadata(&link)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(
        fs::read_to_string(dir.path().join("target.txt")).unwrap(),
        "made"
    );
}

#[test]
fn symlink_loop_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    symlink("b", dir.path().join("a")).unwrap();
    symlink("a", dir.path().join("b")).unwrap();
    let err = write_bytes_atomic(&dir.path().join("a"), None, b"x").unwrap_err();
    assert!(matches!(err, FsError::Io { .. }), "{err:?}");
    assert_eq!(entries(dir.path()), ["a", "b"]);
}

#[test]
fn stale_stamp_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    let stamp = FileStamp::of(&path).unwrap();
    // Same length, so only the timestamps can give it away. Filesystems with
    // coarse timestamps (ext4 ticks per jiffy) can keep them equal for a few
    // milliseconds, so rewrite until the stamp moves.
    for _ in 0..TIMESTAMP_TICK_TRIES {
        fs::write(&path, "ext").unwrap();
        if FileStamp::of(&path).unwrap() != stamp {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(TIMESTAMP_TICK_MS));
    }
    let err = write_bytes_atomic(&path, Some(&stamp), b"new").unwrap_err();
    assert!(
        matches!(err, FsError::ExternalModification { .. }),
        "{err:?}"
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), "ext");
    assert_eq!(entries(dir.path()), ["f.txt"]);
}

#[test]
fn replaced_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    let stamp = FileStamp::of(&path).unwrap();
    let other = dir.path().join("other");
    fs::write(&other, "ext").unwrap();
    fs::rename(&other, &path).unwrap();
    let err = write_bytes_atomic(&path, Some(&stamp), b"new").unwrap_err();
    assert!(
        matches!(err, FsError::ExternalModification { .. }),
        "{err:?}"
    );
}

#[test]
fn deleted_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    let stamp = FileStamp::of(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let err = write_bytes_atomic(&path, Some(&stamp), b"new").unwrap_err();
    assert!(
        matches!(err, FsError::ExternalModification { .. }),
        "{err:?}"
    );
    assert!(entries(dir.path()).is_empty());
}

#[test]
fn matching_stamp_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    let stamp = FileStamp::of(&path).unwrap();
    let next = write_bytes_atomic(&path, Some(&stamp), b"new").unwrap();
    write_bytes_atomic(&path, Some(&next), b"newer").unwrap();
    assert_eq!(fs::read_to_string(&path).unwrap(), "newer");
}

#[test]
fn failed_writer_leaves_original_and_no_temp() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    fs::write(&path, "old").unwrap();
    let err = write_atomic(&path, None, |w: &mut dyn Write| {
        w.write_all(b"partial")?;
        Err(std::io::Error::other("writer failed"))
    })
    .unwrap_err();
    assert!(matches!(err, FsError::Io { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(&path).unwrap(), "old");
    assert_eq!(entries(dir.path()), ["f.txt"]);
}

#[test]
fn missing_directory_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let err = write_bytes_atomic(&dir.path().join("no/f.txt"), None, b"x").unwrap_err();
    assert!(matches!(err, FsError::Io { .. }), "{err:?}");
}

#[test]
fn directory_target_is_refused_untouched() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("d");
    fs::create_dir(&target).unwrap();
    fs::write(target.join("inner"), "keep").unwrap();
    let err = write_bytes_atomic(&target, None, b"x").unwrap_err();
    assert!(matches!(err, FsError::Io { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(target.join("inner")).unwrap(), "keep");
    assert_eq!(entries(dir.path()), ["d"]);
}

#[test]
fn returned_stamp_matches_disk() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("f.txt");
    let stamp = write_bytes_atomic(&path, None, b"contents").unwrap();
    assert_eq!(FileStamp::of(&path).unwrap(), stamp);
    assert_eq!(stamp.len, 8);
}
