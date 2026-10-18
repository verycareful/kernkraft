// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{symlink, PermissionsExt};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};

use kernkraft_fs::{list_dir, scan, Entry, EntryKind, NoProgress, Progress, ScanOptions};

fn all(_: &Entry) -> bool {
    true
}

fn names(entries: &[Entry]) -> Vec<&str> {
    entries.iter().map(|e| e.name.as_str()).collect()
}

#[test]
fn list_dir_hides_dotfiles_by_default() {
    let dir = tempfile::tempdir().unwrap();
    fs::write(dir.path().join("b"), "12").unwrap();
    fs::write(dir.path().join(".hidden"), "").unwrap();
    fs::create_dir(dir.path().join("a")).unwrap();
    let shown = list_dir(dir.path(), false).unwrap();
    assert_eq!(names(&shown), ["a", "b"]);
    assert_eq!(shown[0].kind, EntryKind::Dir);
    assert_eq!(shown[1].len, 2);
    let everything = list_dir(dir.path(), true).unwrap();
    assert_eq!(names(&everything), [".hidden", "a", "b"]);
    assert!(everything[0].hidden);
}

#[test]
fn list_dir_marks_symlink_targets() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join("d")).unwrap();
    symlink("d", dir.path().join("to-dir")).unwrap();
    symlink("nowhere", dir.path().join("dangling")).unwrap();
    let entries = list_dir(dir.path(), false).unwrap();
    let get = |n: &str| entries.iter().find(|e| e.name == n).unwrap();
    assert_eq!(get("to-dir").kind, EntryKind::Symlink);
    assert!(get("to-dir").target_is_dir);
    assert_eq!(get("dangling").kind, EntryKind::Symlink);
    assert!(!get("dangling").target_is_dir);
}

#[test]
fn list_dir_keeps_non_utf8_path() {
    use std::os::unix::ffi::OsStrExt;
    let dir = tempfile::tempdir().unwrap();
    let raw = std::ffi::OsStr::from_bytes(b"bad\xffname");
    fs::write(dir.path().join(raw), "").unwrap();
    let entries = list_dir(dir.path(), false).unwrap();
    assert_eq!(entries[0].path, dir.path().join(raw));
    assert_eq!(entries[0].name, "bad\u{fffd}name");
}

#[test]
fn scan_survives_symlink_loop() {
    let root = tempfile::tempdir().unwrap();
    let a = root.path().join("a");
    fs::create_dir(&a).unwrap();
    fs::write(a.join("f.txt"), "").unwrap();
    symlink(&a, a.join("link")).unwrap();
    let report = scan(
        &[root.path().to_path_buf()],
        &ScanOptions::default(),
        &|e: &Entry| e.kind == EntryKind::File,
        &NoProgress,
    );
    assert_eq!(names(&report.entries), ["f.txt"]);
    assert!(report.unreadable.is_empty());
}

#[test]
fn scan_follows_symlink_to_directory_once() {
    let root = tempfile::tempdir().unwrap();
    let lib = root.path().join("lib");
    fs::create_dir(&lib).unwrap();
    fs::write(lib.join("doc.pdf"), "").unwrap();
    let scanned = root.path().join("scanned");
    fs::create_dir(&scanned).unwrap();
    symlink(&lib, scanned.join("library")).unwrap();
    let report = scan(
        std::slice::from_ref(&scanned),
        &ScanOptions::default(),
        &|e: &Entry| e.kind == EntryKind::File,
        &NoProgress,
    );
    assert_eq!(report.entries.len(), 1);
    assert_eq!(report.entries[0].path, scanned.join("library/doc.pdf"));
}

#[test]
fn scan_respects_max_depth() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("d1/d2")).unwrap();
    fs::write(root.path().join("d1/two.txt"), "").unwrap();
    fs::write(root.path().join("d1/d2/three.txt"), "").unwrap();
    let opts = ScanOptions {
        max_depth: 2,
        include_hidden: false,
    };
    let report = scan(
        &[root.path().to_path_buf()],
        &opts,
        &|e: &Entry| e.kind == EntryKind::File,
        &NoProgress,
    );
    assert_eq!(names(&report.entries), ["two.txt"]);
}

#[test]
fn scan_skips_hidden_directories() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join(".git")).unwrap();
    fs::write(root.path().join(".git/x.txt"), "").unwrap();
    let report = scan(
        &[root.path().to_path_buf()],
        &ScanOptions::default(),
        &all,
        &NoProgress,
    );
    assert!(report.entries.is_empty());
}

#[test]
fn scan_records_unreadable_dir() {
    let root = tempfile::tempdir().unwrap();
    let locked = root.path().join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::read_dir(&locked).is_ok() {
        // Running as root: permissions do not stop the read.
        fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
        return;
    }
    let report = scan(
        &[root.path().to_path_buf()],
        &ScanOptions::default(),
        &all,
        &NoProgress,
    );
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(report.unreadable, vec![locked.clone()]);
    assert_eq!(names(&report.entries), ["locked"]);
}

#[test]
fn scan_missing_root_is_unreadable() {
    let root = tempfile::tempdir().unwrap();
    let missing = root.path().join("missing");
    let report = scan(
        std::slice::from_ref(&missing),
        &ScanOptions::default(),
        &all,
        &NoProgress,
    );
    assert_eq!(report.unreadable, vec![missing]);
}

#[test]
fn scan_keep_filters_entries() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir(root.path().join("sub")).unwrap();
    fs::write(root.path().join("a.pdf"), "").unwrap();
    fs::write(root.path().join("b.txt"), "").unwrap();
    fs::write(root.path().join("sub/c.pdf"), "").unwrap();
    let keep = |e: &Entry| e.path.extension().is_some_and(|x| x == "pdf");
    let report = scan(
        &[root.path().to_path_buf()],
        &ScanOptions::default(),
        &keep,
        &NoProgress,
    );
    let mut found: Vec<PathBuf> = report.entries.into_iter().map(|e| e.path).collect();
    found.sort();
    assert_eq!(
        found,
        vec![root.path().join("a.pdf"), root.path().join("sub/c.pdf")]
    );
}

struct CancelNow(AtomicBool);

impl Progress for CancelNow {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {}
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[test]
fn scan_stops_when_cancelled() {
    let root = tempfile::tempdir().unwrap();
    fs::write(root.path().join("a"), "").unwrap();
    let report = scan(
        &[root.path().to_path_buf()],
        &ScanOptions::default(),
        &all,
        &CancelNow(AtomicBool::new(true)),
    );
    assert!(report.cancelled);
    assert!(report.entries.is_empty());
}
