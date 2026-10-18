// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs;
use std::os::unix::fs::{symlink, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use kernkraft_fs::{copy, move_items, Conflict, FsError, NoProgress, Progress};

const MIB: usize = 1 << 20;

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn always(c: Conflict) -> impl Fn(&Path, &Path) -> Conflict {
    move |_, _| c
}

fn never(_: &Path, _: &Path) -> Conflict {
    panic!("resolve must not be called")
}

struct Setup {
    _root: tempfile::TempDir,
    src: PathBuf,
    dest: PathBuf,
}

fn setup() -> Setup {
    let root = tempfile::tempdir().unwrap();
    let src = root.path().join("src");
    let dest = root.path().join("dest");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&dest).unwrap();
    Setup {
        _root: root,
        src,
        dest,
    }
}

#[test]
fn copies_tree_with_contents_mode_and_mtime() {
    let s = setup();
    let tree = s.src.join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("sub/f.txt"), "hello").unwrap();
    fs::set_permissions(tree.join("sub/f.txt"), fs::Permissions::from_mode(0o640)).unwrap();
    let mtime = SystemTime::UNIX_EPOCH + Duration::from_secs(1_700_000_000);
    fs::File::options()
        .write(true)
        .open(tree.join("sub/f.txt"))
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    fs::set_permissions(tree.join("sub"), fs::Permissions::from_mode(0o750)).unwrap();

    let report = copy(std::slice::from_ref(&tree), &s.dest, &never, &NoProgress).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(report.done, vec![(tree.clone(), Some(s.dest.join("tree")))]);

    let copied = s.dest.join("tree/sub/f.txt");
    assert_eq!(fs::read_to_string(&copied).unwrap(), "hello");
    let meta = fs::metadata(&copied).unwrap();
    assert_eq!(meta.mode() & 0o7777, 0o640);
    assert_eq!(meta.modified().unwrap(), mtime);
    let sub = fs::metadata(s.dest.join("tree/sub")).unwrap();
    assert_eq!(sub.mode() & 0o7777, 0o750);
    assert_eq!(fs::read_to_string(tree.join("sub/f.txt")).unwrap(), "hello");
}

#[test]
fn copy_keeps_symlink_as_symlink() {
    let s = setup();
    fs::write(s.src.join("real"), "r").unwrap();
    symlink("real", s.src.join("link")).unwrap();
    copy(&[s.src.join("link")], &s.dest, &never, &NoProgress).unwrap();
    let copied = s.dest.join("link");
    assert!(fs::symlink_metadata(&copied)
        .unwrap()
        .file_type()
        .is_symlink());
    assert_eq!(fs::read_link(&copied).unwrap(), Path::new("real"));
}

#[test]
fn copy_into_own_subfolder_is_rejected() {
    let s = setup();
    let tree = s.src.join("tree");
    fs::create_dir_all(tree.join("inner")).unwrap();
    fs::write(tree.join("f"), "").unwrap();
    let other = s.src.join("other.txt");
    fs::write(&other, "").unwrap();
    let err = copy(
        &[other.clone(), tree.clone()],
        &tree.join("inner"),
        &never,
        &NoProgress,
    )
    .unwrap_err();
    assert!(matches!(err, FsError::IntoItself { .. }), "{err:?}");
    assert!(names(&tree.join("inner")).is_empty());
}

#[test]
fn move_into_itself_is_rejected() {
    let s = setup();
    let tree = s.src.join("tree");
    fs::create_dir(&tree).unwrap();
    let err = move_items(std::slice::from_ref(&tree), &tree, &never, &NoProgress).unwrap_err();
    assert!(matches!(err, FsError::IntoItself { .. }), "{err:?}");
    assert!(tree.is_dir());
}

#[test]
fn missing_destination_is_an_error() {
    let s = setup();
    fs::write(s.src.join("f"), "").unwrap();
    let err = copy(&[s.src.join("f")], &s.dest.join("no"), &never, &NoProgress).unwrap_err();
    assert!(matches!(err, FsError::NotFound { .. }), "{err:?}");
}

#[test]
fn missing_source_is_reported_and_others_still_copy() {
    let s = setup();
    fs::write(s.src.join("ok"), "").unwrap();
    let report = copy(
        &[s.src.join("gone"), s.src.join("ok")],
        &s.dest,
        &never,
        &NoProgress,
    )
    .unwrap();
    assert_eq!(report.failed.len(), 1);
    assert!(matches!(report.failed[0].1, FsError::NotFound { .. }));
    assert_eq!(names(&s.dest), ["ok"]);
}

#[test]
fn conflict_skip_leaves_destination() {
    let s = setup();
    fs::write(s.src.join("a.txt"), "new").unwrap();
    fs::write(s.dest.join("a.txt"), "old").unwrap();
    let report = copy(
        &[s.src.join("a.txt")],
        &s.dest,
        &always(Conflict::Skip),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(report.skipped, vec![s.src.join("a.txt")]);
    assert_eq!(fs::read_to_string(s.dest.join("a.txt")).unwrap(), "old");
}

#[test]
fn conflict_keep_both_writes_suffixed_copy() {
    let s = setup();
    fs::write(s.src.join("a.txt"), "new").unwrap();
    fs::write(s.dest.join("a.txt"), "old").unwrap();
    let report = copy(
        &[s.src.join("a.txt")],
        &s.dest,
        &always(Conflict::KeepBoth),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(report.done[0].1, Some(s.dest.join("a (2).txt")));
    assert_eq!(names(&s.dest), ["a (2).txt", "a.txt"]);
    assert_eq!(fs::read_to_string(s.dest.join("a.txt")).unwrap(), "old");
    assert_eq!(fs::read_to_string(s.dest.join("a (2).txt")).unwrap(), "new");
}

#[test]
fn conflict_replace_on_file_replaces() {
    let s = setup();
    fs::write(s.src.join("a.txt"), "new").unwrap();
    fs::write(s.dest.join("a.txt"), "old").unwrap();
    copy(
        &[s.src.join("a.txt")],
        &s.dest,
        &always(Conflict::Replace),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(fs::read_to_string(s.dest.join("a.txt")).unwrap(), "new");
    assert_eq!(names(&s.dest), ["a.txt"]);
}

#[test]
fn conflict_replace_on_dir_merges() {
    let s = setup();
    fs::create_dir(s.src.join("d")).unwrap();
    fs::write(s.src.join("d/y"), "y").unwrap();
    fs::write(s.src.join("d/x"), "new x").unwrap();
    fs::create_dir(s.dest.join("d")).unwrap();
    fs::write(s.dest.join("d/x"), "old x").unwrap();
    fs::write(s.dest.join("d/z"), "z").unwrap();
    let asked = Mutex::new(Vec::new());
    let resolve = |src: &Path, _: &Path| {
        asked.lock().unwrap().push(src.to_path_buf());
        if src.ends_with("x") {
            Conflict::Skip
        } else {
            Conflict::Replace
        }
    };
    let report = copy(&[s.src.join("d")], &s.dest, &resolve, &NoProgress).unwrap();
    assert_eq!(
        *asked.lock().unwrap(),
        vec![s.src.join("d"), s.src.join("d/x")]
    );
    assert_eq!(names(&s.dest.join("d")), ["x", "y", "z"]);
    assert_eq!(fs::read_to_string(s.dest.join("d/x")).unwrap(), "old x");
    assert_eq!(report.skipped, vec![s.src.join("d/x")]);
}

#[test]
fn replace_file_with_dir_is_refused() {
    let s = setup();
    fs::create_dir(s.src.join("n")).unwrap();
    fs::write(s.dest.join("n"), "keep").unwrap();
    let report = copy(
        &[s.src.join("n")],
        &s.dest,
        &always(Conflict::Replace),
        &NoProgress,
    )
    .unwrap();
    assert!(matches!(report.failed[0].1, FsError::Exists { .. }));
    assert_eq!(fs::read_to_string(s.dest.join("n")).unwrap(), "keep");
}

#[test]
fn copy_into_own_directory_makes_numbered_duplicate() {
    let s = setup();
    fs::write(s.src.join("a.txt"), "x").unwrap();
    let report = copy(&[s.src.join("a.txt")], &s.src, &never, &NoProgress).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert_eq!(names(&s.src), ["a (2).txt", "a.txt"]);
}

struct CancelAfterFirst {
    reports: AtomicU32,
}

impl Progress for CancelAfterFirst {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {
        self.reports.fetch_add(1, Ordering::SeqCst);
    }
    fn is_cancelled(&self) -> bool {
        self.reports.load(Ordering::SeqCst) >= 1
    }
}

#[test]
fn cancel_mid_file_leaves_no_partial_destination() {
    let s = setup();
    fs::write(s.src.join("big"), vec![7u8; 3 * MIB]).unwrap();
    let progress = CancelAfterFirst {
        reports: AtomicU32::new(0),
    };
    let report = copy(&[s.src.join("big")], &s.dest, &never, &progress).unwrap();
    assert!(report.cancelled);
    assert!(report.done.is_empty());
    assert!(names(&s.dest).is_empty(), "{:?}", names(&s.dest));
}

#[test]
fn cancel_mid_directory_removes_the_new_directory() {
    let s = setup();
    let tree = s.src.join("tree");
    fs::create_dir(&tree).unwrap();
    fs::write(tree.join("a"), vec![1u8; MIB]).unwrap();
    fs::write(tree.join("b"), vec![2u8; 2 * MIB]).unwrap();
    let progress = CancelAfterFirst {
        reports: AtomicU32::new(0),
    };
    let report = copy(std::slice::from_ref(&tree), &s.dest, &never, &progress).unwrap();
    assert!(report.cancelled);
    assert!(names(&s.dest).is_empty(), "{:?}", names(&s.dest));
    assert_eq!(names(&tree), ["a", "b"]);
}

#[test]
fn replace_with_failing_copy_keeps_old_file() {
    let s = setup();
    let src = s.src.join("a.txt");
    fs::write(&src, "new").unwrap();
    fs::set_permissions(&src, fs::Permissions::from_mode(0o000)).unwrap();
    if fs::File::open(&src).is_ok() {
        // Running as root: permissions do not stop the read.
        return;
    }
    fs::write(s.dest.join("a.txt"), "old").unwrap();
    let report = copy(&[src], &s.dest, &always(Conflict::Replace), &NoProgress).unwrap();
    assert_eq!(report.failed.len(), 1);
    assert_eq!(fs::read_to_string(s.dest.join("a.txt")).unwrap(), "old");
    assert_eq!(names(&s.dest), ["a.txt"]);
}

#[test]
fn move_same_device_renames() {
    let s = setup();
    let src = s.src.join("f");
    fs::write(&src, "x").unwrap();
    let ino = fs::metadata(&src).unwrap().ino();
    let report = move_items(std::slice::from_ref(&src), &s.dest, &never, &NoProgress).unwrap();
    assert_eq!(report.done, vec![(src.clone(), Some(s.dest.join("f")))]);
    assert!(!src.exists());
    assert_eq!(fs::metadata(s.dest.join("f")).unwrap().ino(), ino);
}

#[test]
fn move_onto_existing_refuses_without_replace() {
    let s = setup();
    fs::write(s.src.join("f"), "new").unwrap();
    fs::write(s.dest.join("f"), "old").unwrap();
    let report = move_items(
        &[s.src.join("f")],
        &s.dest,
        &always(Conflict::Skip),
        &NoProgress,
    )
    .unwrap();
    assert_eq!(report.skipped.len(), 1);
    assert_eq!(fs::read_to_string(s.dest.join("f")).unwrap(), "old");
    assert_eq!(fs::read_to_string(s.src.join("f")).unwrap(), "new");
}

#[test]
fn move_merge_removes_emptied_source_dir() {
    let s = setup();
    fs::create_dir(s.src.join("d")).unwrap();
    fs::write(s.src.join("d/a"), "a").unwrap();
    fs::create_dir(s.dest.join("d")).unwrap();
    fs::write(s.dest.join("d/b"), "b").unwrap();
    let report = move_items(
        &[s.src.join("d")],
        &s.dest,
        &always(Conflict::Replace),
        &NoProgress,
    )
    .unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!s.src.join("d").exists());
    assert_eq!(names(&s.dest.join("d")), ["a", "b"]);
}

#[test]
fn move_merge_with_skip_keeps_source_dir() {
    let s = setup();
    fs::create_dir(s.src.join("d")).unwrap();
    fs::write(s.src.join("d/a"), "new").unwrap();
    fs::write(s.src.join("d/c"), "c").unwrap();
    fs::create_dir(s.dest.join("d")).unwrap();
    fs::write(s.dest.join("d/a"), "old").unwrap();
    let resolve = |src: &Path, _: &Path| {
        if src.ends_with("a") {
            Conflict::Skip
        } else {
            Conflict::Replace
        }
    };
    move_items(&[s.src.join("d")], &s.dest, &resolve, &NoProgress).unwrap();
    assert_eq!(names(&s.src.join("d")), ["a"]);
    assert_eq!(fs::read_to_string(s.dest.join("d/a")).unwrap(), "old");
    assert_eq!(fs::read_to_string(s.dest.join("d/c")).unwrap(), "c");
}

#[test]
fn move_into_own_directory_is_skipped() {
    let s = setup();
    fs::write(s.src.join("f"), "x").unwrap();
    let report = move_items(&[s.src.join("f")], &s.src, &never, &NoProgress).unwrap();
    assert_eq!(report.skipped, vec![s.src.join("f")]);
    assert_eq!(fs::read_to_string(s.src.join("f")).unwrap(), "x");
}

#[derive(Default)]
struct Recorder {
    last: Mutex<Option<(u64, u64)>>,
    any: AtomicBool,
}

impl Progress for Recorder {
    fn report(&self, done: u64, total: u64, _stage: &str) {
        self.any.store(true, Ordering::SeqCst);
        *self.last.lock().unwrap() = Some((done, total));
    }
}

#[test]
fn progress_reaches_total() {
    let s = setup();
    fs::create_dir(s.src.join("d")).unwrap();
    fs::write(s.src.join("d/a"), vec![0u8; MIB + 17]).unwrap();
    fs::write(s.src.join("b"), vec![0u8; 5]).unwrap();
    let progress = Recorder::default();
    copy(
        &[s.src.join("d"), s.src.join("b")],
        &s.dest,
        &never,
        &progress,
    )
    .unwrap();
    let total = (MIB + 17 + 5) as u64;
    assert!(progress.any.load(Ordering::SeqCst));
    assert_eq!(*progress.last.lock().unwrap(), Some((total, total)));
}

/// Moves between the system temp dir and Cargo's target temp dir, which sit
/// on different filesystems on a typical tmpfs-/tmp machine. Where they
/// share a device the rename path runs instead and the test proves nothing,
/// so it returns early.
#[test]
fn move_across_devices_copies_then_deletes() {
    let near = tempfile::tempdir().unwrap();
    let far = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    let near_dev = fs::metadata(near.path()).unwrap().dev();
    if near_dev == fs::metadata(far.path()).unwrap().dev() {
        eprintln!("skipped: temp dirs share a device");
        return;
    }
    let tree = near.path().join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("sub/f"), "data").unwrap();
    symlink("sub/f", tree.join("link")).unwrap();
    let report = move_items(std::slice::from_ref(&tree), far.path(), &never, &NoProgress).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    assert!(!tree.exists());
    assert_eq!(
        fs::read_to_string(far.path().join("tree/sub/f")).unwrap(),
        "data"
    );
    assert_eq!(
        fs::read_link(far.path().join("tree/link")).unwrap(),
        Path::new("sub/f")
    );
}
