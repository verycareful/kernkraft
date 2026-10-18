// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};

use kernkraft_fs::{
    create_dir, create_file, delete_permanently, rename, unique_name, validate_name, FsError,
    NoProgress, Progress,
};

#[test]
fn validate_rejects_slash_dot_dotdot_empty_nul() {
    for bad in ["", ".", "..", "a/b", "/", "a\0b"] {
        assert!(
            matches!(validate_name(bad), Err(FsError::InvalidName { .. })),
            "{bad:?}"
        );
    }
    for good in ["a", ".hidden", "...", "a b", "ü.txt"] {
        assert!(validate_name(good).is_ok(), "{good:?}");
    }
}

#[test]
fn unique_name_returns_free_name_unchanged() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(unique_name(dir.path(), "a.txt"), "a.txt");
}

#[test]
fn unique_name_counts_up() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("a.txt"), "").unwrap();
    fs::write(dir.path().join("a (2).txt"), "").unwrap();
    assert_eq!(unique_name(dir.path(), "a.txt"), "a (3).txt");
}

#[test]
fn unique_name_dotfile_keeps_whole_name() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join(".bashrc"), "").unwrap();
    assert_eq!(unique_name(dir.path(), ".bashrc"), ".bashrc (2)");
}

#[test]
fn unique_name_directory_keeps_dotted_name_whole() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("v1.2")).unwrap();
    assert_eq!(unique_name(dir.path(), "v1.2"), "v1.2 (2)");
}

#[test]
fn unique_name_counts_dangling_symlink_as_taken() {
    let dir = tempfile::tempdir().unwrap();
    symlink("nowhere", dir.path().join("a")).unwrap();
    assert_eq!(unique_name(dir.path(), "a"), "a (2)");
}

#[test]
fn create_dir_and_file() {
    let dir = tempfile::tempdir().unwrap();
    let d = create_dir(dir.path(), "d").unwrap();
    let f = create_file(&d, "f.txt").unwrap();
    assert!(d.is_dir());
    assert_eq!(fs::read(&f).unwrap(), b"");
}

#[test]
fn create_dir_refuses_existing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("d"), "file").unwrap();
    let err = create_dir(dir.path(), "d").unwrap_err();
    assert!(matches!(err, FsError::Exists { .. }), "{err:?}");
}

#[test]
fn create_file_refuses_existing() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("f"), "keep").unwrap();
    let err = create_file(dir.path(), "f").unwrap_err();
    assert!(matches!(err, FsError::Exists { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(dir.path().join("f")).unwrap(), "keep");
}

#[test]
fn create_rejects_invalid_name() {
    let dir = tempfile::tempdir().unwrap();
    let err = create_file(dir.path(), "../escape").unwrap_err();
    assert!(matches!(err, FsError::InvalidName { .. }), "{err:?}");
}

#[test]
fn rename_moves_within_directory() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    fs::write(&a, "x").unwrap();
    let b = rename(&a, "b").unwrap();
    assert_eq!(b, dir.path().join("b"));
    assert!(!a.exists());
    assert_eq!(fs::read_to_string(&b).unwrap(), "x");
}

#[test]
fn rename_refuses_existing_target() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    let b = dir.path().join("b");
    fs::write(&a, "A").unwrap();
    fs::write(&b, "B").unwrap();
    let err = rename(&a, "b").unwrap_err();
    assert!(matches!(err, FsError::Exists { .. }), "{err:?}");
    assert_eq!(fs::read_to_string(&a).unwrap(), "A");
    assert_eq!(fs::read_to_string(&b).unwrap(), "B");
}

#[test]
fn rename_rejects_invalid_name() {
    let dir = tempfile::tempdir().unwrap();
    let a = dir.path().join("a");
    fs::write(&a, "").unwrap();
    for bad in ["x/y", "..", ""] {
        let err = rename(&a, bad).unwrap_err();
        assert!(matches!(err, FsError::InvalidName { .. }), "{bad:?}");
    }
    assert!(a.exists());
}

#[test]
fn rename_missing_source_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let err = rename(&dir.path().join("nope"), "b").unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
}

#[test]
fn delete_removes_tree_but_not_symlink_target() {
    let root = tempfile::tempdir().unwrap();
    let outside = root.path().join("outside.txt");
    fs::write(&outside, "keep").unwrap();
    let tree = root.path().join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("sub/f"), "").unwrap();
    symlink(&outside, tree.join("link")).unwrap();
    let top_link = root.path().join("top-link");
    symlink(root.path().join("tree"), &top_link).unwrap();

    let report = delete_permanently(&[tree.clone(), top_link.clone()], &NoProgress);
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.done.len(), 2);
    assert!(!tree.exists());
    assert!(fs::symlink_metadata(&top_link).is_err());
    assert_eq!(fs::read_to_string(&outside).unwrap(), "keep");
}

#[test]
fn delete_reports_missing_path_as_failed() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    fs::write(&real, "").unwrap();
    let missing = dir.path().join("missing");
    let report = delete_permanently(&[missing.clone(), real.clone()], &NoProgress);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].0, missing);
    assert!(matches!(report.failed[0].1, FsError::NotFound { .. }));
    assert_eq!(report.done, vec![(real.clone(), None)]);
    assert!(!real.exists());
}

struct CancelAfter {
    reports: AtomicU32,
    limit: u32,
}

impl Progress for CancelAfter {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {
        self.reports.fetch_add(1, Ordering::SeqCst);
    }
    fn is_cancelled(&self) -> bool {
        self.reports.load(Ordering::SeqCst) >= self.limit
    }
}

#[test]
fn delete_stops_between_items_when_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<PathBuf> = (0..3).map(|i| dir.path().join(format!("f{i}"))).collect();
    for p in &paths {
        fs::write(p, "").unwrap();
    }
    let progress = CancelAfter {
        reports: AtomicU32::new(0),
        limit: 1,
    };
    let report = delete_permanently(&paths, &progress);
    assert!(report.cancelled);
    assert_eq!(report.done.len(), 1);
    assert!(!paths[0].exists());
    assert!(paths[1].exists() && paths[2].exists());
}

#[test]
fn rename_directory_and_missing_same_name() {
    let root = tempfile::tempdir().unwrap();
    let dir = root.path().join("d");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("f"), "v").unwrap();
    let renamed = rename(&dir, "e").unwrap();
    assert_eq!(fs::read_to_string(renamed.join("f")).unwrap(), "v");
    let missing = root.path().join("missing");
    assert!(matches!(
        rename(&missing, "missing"),
        Err(FsError::NotFound { .. })
    ));
}
