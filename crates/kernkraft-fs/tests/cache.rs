// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};

use kernkraft_fs::{FsError, PrivateCache};

fn mode(path: &std::path::Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o777
}

#[test]
fn open_creates_private_dir() {
    let home = tempfile::tempdir().unwrap();
    let cache = PrivateCache::open(home.path(), "kern").unwrap();
    assert_eq!(cache.dir(), home.path().join("kern/staging"));
    assert_eq!(mode(cache.dir()), 0o700);
}

#[test]
fn open_resets_loose_mode() {
    let home = tempfile::tempdir().unwrap();
    let staging = home.path().join("kern/staging");
    fs::create_dir_all(&staging).unwrap();
    fs::set_permissions(&staging, fs::Permissions::from_mode(0o755)).unwrap();
    PrivateCache::open(home.path(), "kern").unwrap();
    assert_eq!(mode(&staging), 0o700);
}

#[test]
fn open_sweeps_leftovers() {
    let home = tempfile::tempdir().unwrap();
    let staging = home.path().join("kern/staging");
    fs::create_dir_all(staging.join("old-dir")).unwrap();
    fs::write(staging.join("stale.pdf"), "").unwrap();
    fs::write(staging.join("old-dir/x"), "").unwrap();
    let cache = PrivateCache::open(home.path(), "kern").unwrap();
    assert_eq!(fs::read_dir(cache.dir()).unwrap().count(), 0);
}

#[test]
fn sweep_does_not_follow_symlinks() {
    let home = tempfile::tempdir().unwrap();
    let outside = home.path().join("outside");
    fs::create_dir(&outside).unwrap();
    fs::write(outside.join("keep"), "keep").unwrap();
    let staging = home.path().join("kern/staging");
    fs::create_dir_all(&staging).unwrap();
    symlink(&outside, staging.join("link")).unwrap();
    PrivateCache::open(home.path(), "kern").unwrap();
    assert_eq!(fs::read_to_string(outside.join("keep")).unwrap(), "keep");
}

#[test]
fn open_refuses_symlinked_staging() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = home.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    fs::create_dir(home.path().join("kern")).unwrap();
    symlink(&elsewhere, home.path().join("kern/staging")).unwrap();
    let err = PrivateCache::open(home.path(), "kern").unwrap_err();
    assert!(matches!(err, FsError::UnsafeCacheDir { .. }), "{err:?}");
}

#[test]
fn open_refuses_symlinked_app_dir() {
    let home = tempfile::tempdir().unwrap();
    let elsewhere = home.path().join("elsewhere");
    fs::create_dir(&elsewhere).unwrap();
    symlink(&elsewhere, home.path().join("kern")).unwrap();
    let err = PrivateCache::open(home.path(), "kern").unwrap_err();
    assert!(matches!(err, FsError::UnsafeCacheDir { .. }), "{err:?}");
}

#[test]
fn open_rejects_bad_app_name() {
    let home = tempfile::tempdir().unwrap();
    let err = PrivateCache::open(home.path(), "../x").unwrap_err();
    assert!(matches!(err, FsError::InvalidName { .. }), "{err:?}");
}

#[test]
fn staged_copy_is_owner_only() {
    let home = tempfile::tempdir().unwrap();
    let src = home.path().join("report.pdf");
    fs::write(&src, "pdf bytes").unwrap();
    fs::set_permissions(&src, fs::Permissions::from_mode(0o644)).unwrap();
    let cache = PrivateCache::open(home.path(), "kern").unwrap();
    let copy = cache.stage_copy(&src).unwrap();
    assert_eq!(mode(&copy), 0o600);
    assert_eq!(fs::read_to_string(&copy).unwrap(), "pdf bytes");
    assert_eq!(copy.parent().unwrap(), cache.dir());
    assert_eq!(copy.extension().unwrap(), "pdf");
}

#[test]
fn staged_copies_never_collide() {
    let home = tempfile::tempdir().unwrap();
    let src = home.path().join("a.txt");
    fs::write(&src, "x").unwrap();
    let cache = PrivateCache::open(home.path(), "kern").unwrap();
    let one = cache.stage_copy(&src).unwrap();
    let two = cache.stage_copy(&src).unwrap();
    assert_ne!(one, two);
    cache.sweep().unwrap();
    assert_eq!(fs::read_dir(cache.dir()).unwrap().count(), 0);
}

#[test]
fn staging_missing_source_is_not_found() {
    let home = tempfile::tempdir().unwrap();
    let cache = PrivateCache::open(home.path(), "kern").unwrap();
    let err = cache.stage_copy(&home.path().join("nope")).unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
    assert_eq!(fs::read_dir(cache.dir()).unwrap().count(), 0);
}

#[test]
fn stage_copy_of_directory_fails_and_leaves_nothing() {
    let root = tempfile::tempdir().unwrap();
    let cache = PrivateCache::open(&root.path().join("cache"), "kern").unwrap();
    let dir = root.path().join("d.pdf");
    fs::create_dir(&dir).unwrap();
    assert!(cache.stage_copy(&dir).is_err());
    assert_eq!(fs::read_dir(cache.dir()).unwrap().count(), 0);
}
