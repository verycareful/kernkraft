// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::cell::RefCell;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use kernkraft_fs::{
    copy, create_dir, create_file, move_items, rename, undoable_from_report, Change, Conflict,
    FsError, IrreversibleReason, Journal, NoProgress, OpKind, OpReport, Progress, Trash, TreeStamp,
    UndoError, Undoable, JOURNAL_DEPTH,
};

struct Fixture {
    _root: tempfile::TempDir,
    src: PathBuf,
    dest: PathBuf,
    trash: Trash,
}

fn fixture() -> Fixture {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let src = base.join("src");
    let dest = base.join("dest");
    fs::create_dir(&src).unwrap();
    fs::create_dir(&dest).unwrap();
    let trash = Trash::with_home_trash(base.join("trash"));
    Fixture {
        _root: root,
        src,
        dest,
        trash,
    }
}

fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn never(_: &Path, _: &Path) -> Conflict {
    panic!("resolve must not be called")
}

/// Originals of everything in the trash, sorted.
fn trashed_originals(trash: &Trash) -> Vec<PathBuf> {
    let listing = trash.list().unwrap();
    assert!(listing.unreadable.is_empty(), "{:?}", listing.unreadable);
    let mut v: Vec<PathBuf> = listing.items.into_iter().map(|i| i.original).collect();
    v.sort();
    v
}

/// Copies `files` (names under `src`, each holding its own name as content)
/// into `dest` and returns the journal entry for it.
fn copy_files(f: &Fixture, files: &[&str]) -> Undoable {
    let sources: Vec<PathBuf> = files
        .iter()
        .map(|n| {
            let p = f.src.join(n);
            fs::write(&p, n).unwrap();
            p
        })
        .collect();
    let report = copy(&sources, &f.dest, &never, &NoProgress).unwrap();
    assert_eq!(report.done.len(), files.len());
    undoable_from_report(OpKind::Copy, &report, &[])
}

/// Trashes each path as a trash operation would, reporting each item's
/// place in the trash's `files/`.
fn trash_paths(trash: &Trash, paths: &[PathBuf]) -> OpReport {
    let mut report = OpReport::default();
    for p in paths {
        let item = trash.put(p).unwrap();
        let in_trash = item.trash_dir.join("files").join(&item.name);
        report.done.push((p.clone(), Some(in_trash)));
    }
    report
}

#[test]
fn undo_rename_restores() {
    let f = fixture();
    let from = f.src.join("a.txt");
    fs::write(&from, "alpha").unwrap();
    let to = rename(&from, "b.txt").unwrap();
    let mut journal = Journal::new();
    journal.record(Undoable::Renamed {
        from: from.clone(),
        to: to.clone(),
        stamp: TreeStamp::of(&to).unwrap(),
    });

    let outcome = journal.undo(&f.trash, &NoProgress).unwrap();

    assert_eq!(outcome.done, std::slice::from_ref(&from));
    assert_eq!(fs::read_to_string(&from).unwrap(), "alpha");
    assert!(fs::symlink_metadata(&to).is_err());
    assert!(journal.peek_undo().is_none());
    assert!(matches!(
        journal.peek_redo(),
        Some(Undoable::Renamed { .. })
    ));
}

#[test]
fn undo_move_restores_across_dirs() {
    let f = fixture();
    let file = f.src.join("a.txt");
    let dir = f.src.join("d");
    fs::write(&file, "alpha").unwrap();
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("inner"), "inner").unwrap();
    let report = move_items(&[file.clone(), dir.clone()], &f.dest, &never, &NoProgress).unwrap();
    assert_eq!(report.done.len(), 2);
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Move, &report, &[]));

    journal.undo(&f.trash, &NoProgress).unwrap();

    assert_eq!(names(&f.src), ["a.txt", "d"]);
    assert_eq!(fs::read_to_string(&file).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(dir.join("inner")).unwrap(), "inner");
    assert!(names(&f.dest).is_empty());
}

#[test]
fn undo_copy_trashes_copies() {
    let f = fixture();
    let mut journal = Journal::new();
    journal.record(copy_files(&f, &["a", "b"]));

    journal.undo(&f.trash, &NoProgress).unwrap();

    assert!(names(&f.dest).is_empty());
    assert_eq!(names(&f.src), ["a", "b"]);
    assert_eq!(
        trashed_originals(&f.trash),
        [f.dest.join("a"), f.dest.join("b")]
    );
    // In the trash with their contents, not deleted.
    for item in f.trash.list().unwrap().items {
        let held = item.trash_dir.join("files").join(&item.name);
        assert_eq!(fs::read_to_string(held).unwrap(), item.name);
    }
}

#[test]
fn undo_trash_restores() {
    let f = fixture();
    let a = f.src.join("a");
    let d = f.src.join("d");
    fs::write(&a, "alpha").unwrap();
    fs::create_dir(&d).unwrap();
    fs::write(d.join("inner"), "inner").unwrap();
    let report = trash_paths(&f.trash, &[a.clone(), d.clone()]);
    let op = undoable_from_report(OpKind::Trash, &report, &[]);
    let Undoable::Trashed { items } = &op else {
        panic!("{op:?}")
    };
    assert_eq!(items.len(), 2);
    let mut journal = Journal::new();
    journal.record(op);

    journal.undo(&f.trash, &NoProgress).unwrap();

    assert_eq!(fs::read_to_string(&a).unwrap(), "alpha");
    assert_eq!(fs::read_to_string(d.join("inner")).unwrap(), "inner");
    assert!(trashed_originals(&f.trash).is_empty());
}

#[test]
fn undo_create_trashes() {
    let f = fixture();
    let file = create_file(&f.src, "new.txt").unwrap();
    let dir = create_dir(&f.src, "new dir").unwrap();
    let mut journal = Journal::new();
    for p in [&file, &dir] {
        journal.record(Undoable::Created {
            path: p.clone(),
            stamp: TreeStamp::of(p).unwrap(),
        });
    }

    journal.undo(&f.trash, &NoProgress).unwrap();
    journal.undo(&f.trash, &NoProgress).unwrap();

    assert!(names(&f.src).is_empty());
    assert_eq!(trashed_originals(&f.trash), [dir, file]);
}

#[test]
fn redo_after_undo_repeats() {
    let f = fixture();
    let mut journal = Journal::new();

    // Copy: undo trashes, redo restores the same trash entries.
    journal.record(copy_files(&f, &["a", "b"]));
    journal.undo(&f.trash, &NoProgress).unwrap();
    journal.redo(&f.trash, &NoProgress).unwrap();
    assert_eq!(names(&f.dest), ["a", "b"]);
    assert!(trashed_originals(&f.trash).is_empty());
    assert!(matches!(journal.peek_undo(), Some(Undoable::Copied { .. })));
    // The entry put back by redo is undoable again.
    journal.undo(&f.trash, &NoProgress).unwrap();
    assert!(names(&f.dest).is_empty());
    journal.redo(&f.trash, &NoProgress).unwrap();
    assert_eq!(names(&f.dest), ["a", "b"]);

    // Rename.
    let from = f.src.join("a");
    let to = rename(&from, "renamed").unwrap();
    journal.record(Undoable::Renamed {
        from: from.clone(),
        to: to.clone(),
        stamp: TreeStamp::of(&to).unwrap(),
    });
    journal.undo(&f.trash, &NoProgress).unwrap();
    journal.redo(&f.trash, &NoProgress).unwrap();
    assert_eq!(names(&f.src), ["b", "renamed"]);
    journal.undo(&f.trash, &NoProgress).unwrap();
    assert_eq!(names(&f.src), ["a", "b"]);
    journal.redo(&f.trash, &NoProgress).unwrap();

    // Trash: undo restores, redo trashes again, undo restores again.
    let report = trash_paths(&f.trash, std::slice::from_ref(&to));
    journal.record(undoable_from_report(OpKind::Trash, &report, &[]));
    journal.undo(&f.trash, &NoProgress).unwrap();
    assert!(to.exists());
    journal.redo(&f.trash, &NoProgress).unwrap();
    assert!(!to.exists());
    assert_eq!(trashed_originals(&f.trash), std::slice::from_ref(&to));
    assert!(matches!(
        journal.peek_undo(),
        Some(Undoable::Trashed { .. })
    ));
    journal.undo(&f.trash, &NoProgress).unwrap();
    assert_eq!(fs::read_to_string(&to).unwrap(), "a");
    assert!(trashed_originals(&f.trash).is_empty());
}

#[test]
fn record_clears_redo() {
    let f = fixture();
    let mut journal = Journal::new();
    journal.record(copy_files(&f, &["a"]));
    journal.undo(&f.trash, &NoProgress).unwrap();
    assert!(journal.peek_redo().is_some());

    let made = create_file(&f.src, "new").unwrap();
    journal.record(Undoable::Created {
        stamp: TreeStamp::of(&made).unwrap(),
        path: made,
    });

    assert!(journal.peek_redo().is_none());
    assert!(matches!(
        journal.redo(&f.trash, &NoProgress),
        Err(UndoError::NothingToRedo)
    ));
}

#[test]
fn undo_refused_when_copy_edited() {
    let f = fixture();
    let mut journal = Journal::new();
    journal.record(copy_files(&f, &["a", "b"]));
    let edited = f.dest.join("b");
    fs::OpenOptions::new()
        .append(true)
        .open(&edited)
        .unwrap()
        .write_all(b" edited")
        .unwrap();

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    match err {
        UndoError::Changed { path, what } => {
            assert_eq!(path, edited);
            assert_eq!(what, Change::Modified);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(names(&f.dest), ["a", "b"]);
    assert_eq!(fs::read_to_string(&edited).unwrap(), "b edited");
    assert!(trashed_originals(&f.trash).is_empty());
    assert!(matches!(journal.peek_undo(), Some(Undoable::Copied { .. })));
    assert!(journal.peek_redo().is_none());
}

#[test]
fn undo_refused_when_edit_is_inside_copied_dir() {
    let f = fixture();
    let tree = f.src.join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("sub/f"), "one").unwrap();
    let report = copy(std::slice::from_ref(&tree), &f.dest, &never, &NoProgress).unwrap();
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Copy, &report, &[]));
    let added = f.dest.join("tree/sub/new");
    fs::write(&added, "new").unwrap();

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    match err {
        UndoError::Changed { path, what } => {
            assert_eq!(path, added);
            assert_eq!(what, Change::Modified);
        }
        other => panic!("{other:?}"),
    }
    assert!(added.exists());
    assert!(trashed_originals(&f.trash).is_empty());
}

#[test]
fn undo_refused_when_name_taken() {
    let f = fixture();
    let from = f.src.join("a");
    fs::write(&from, "old").unwrap();
    let to = rename(&from, "b").unwrap();
    let mut journal = Journal::new();
    journal.record(Undoable::Renamed {
        from: from.clone(),
        to: to.clone(),
        stamp: TreeStamp::of(&to).unwrap(),
    });
    fs::write(&from, "someone else's").unwrap();

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    match err {
        UndoError::Changed { path, what } => {
            assert_eq!(path, from);
            assert_eq!(what, Change::Taken);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(fs::read_to_string(&from).unwrap(), "someone else's");
    assert_eq!(fs::read_to_string(&to).unwrap(), "old");
    assert!(journal.peek_undo().is_some());
}

#[test]
fn undo_trash_refused_when_original_taken() {
    let f = fixture();
    let a = f.src.join("a");
    let b = f.src.join("b");
    fs::write(&a, "a").unwrap();
    fs::write(&b, "b").unwrap();
    let report = trash_paths(&f.trash, &[a.clone(), b.clone()]);
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Trash, &report, &[]));
    fs::write(&b, "new b").unwrap();

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    assert!(
        matches!(&err, UndoError::Changed { path, what: Change::Taken } if *path == b),
        "{err:?}"
    );
    // All or nothing: `a` was not restored either.
    assert!(!a.exists());
    assert_eq!(fs::read_to_string(&b).unwrap(), "new b");
    assert_eq!(trashed_originals(&f.trash), [a, b]);
}

#[test]
fn replace_is_irreversible() {
    let f = fixture();
    fs::write(f.src.join("a"), "new").unwrap();
    fs::write(f.dest.join("a"), "old").unwrap();
    let replaced = RefCell::new(Vec::new());
    let resolve = |_: &Path, existing: &Path| {
        replaced.borrow_mut().push(existing.to_path_buf());
        Conflict::Replace
    };
    let report = copy(&[f.src.join("a")], &f.dest, &resolve, &NoProgress).unwrap();
    assert_eq!(report.done.len(), 1);

    let op = undoable_from_report(OpKind::Copy, &report, &replaced.borrow());
    assert!(
        matches!(
            op,
            Undoable::Irreversible {
                reason: IrreversibleReason::ReplacedExisting,
                ..
            }
        ),
        "{op:?}"
    );
    let mut journal = Journal::new();
    journal.record(op);

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    assert!(
        matches!(
            err,
            UndoError::Irreversible {
                reason: IrreversibleReason::ReplacedExisting,
                ..
            }
        ),
        "{err:?}"
    );
    assert_eq!(fs::read_to_string(f.dest.join("a")).unwrap(), "new");
    assert!(matches!(
        journal.peek_undo(),
        Some(Undoable::Irreversible { .. })
    ));
}

#[test]
fn merge_is_irreversible() {
    let f = fixture();
    fs::create_dir(f.src.join("d")).unwrap();
    fs::write(f.src.join("d/new"), "new").unwrap();
    fs::create_dir(f.dest.join("d")).unwrap();
    fs::write(f.dest.join("d/old"), "old").unwrap();
    let replaced = RefCell::new(Vec::new());
    let resolve = |_: &Path, existing: &Path| {
        replaced.borrow_mut().push(existing.to_path_buf());
        Conflict::Replace
    };
    let report = move_items(&[f.src.join("d")], &f.dest, &resolve, &NoProgress).unwrap();
    assert_eq!(report.done.len(), 1);

    let op = undoable_from_report(OpKind::Move, &report, &replaced.borrow());

    assert!(
        matches!(
            op,
            Undoable::Irreversible {
                reason: IrreversibleReason::MergedDirectory,
                ..
            }
        ),
        "{op:?}"
    );
}

/// Cancels once a report names a stage ending in `/<name>`.
struct CancelAtStage {
    name: &'static str,
    hit: AtomicBool,
}

impl Progress for CancelAtStage {
    fn report(&self, _done: u64, _total: u64, stage: &str) {
        if stage.ends_with(&format!("/{}", self.name)) {
            self.hit.store(true, Ordering::SeqCst);
        }
    }
    fn is_cancelled(&self) -> bool {
        self.hit.load(Ordering::SeqCst)
    }
}

#[test]
fn cancelled_copy_journals_done_only() {
    let f = fixture();
    let sources: Vec<PathBuf> = ["a", "b", "c"]
        .iter()
        .map(|n| {
            let p = f.src.join(n);
            fs::write(&p, n).unwrap();
            p
        })
        .collect();
    let progress = CancelAtStage {
        name: "b",
        hit: AtomicBool::new(false),
    };
    let report = copy(&sources, &f.dest, &never, &progress).unwrap();
    assert!(report.cancelled);
    assert_eq!(report.done.len(), 1);

    let op = undoable_from_report(OpKind::Copy, &report, &[]);
    match &op {
        Undoable::Copied { created } => {
            let paths: Vec<&PathBuf> = created.iter().map(|(p, _)| p).collect();
            assert_eq!(paths, [&f.dest.join("a")]);
        }
        other => panic!("{other:?}"),
    }
    let mut journal = Journal::new();
    journal.record(op);
    journal.undo(&f.trash, &NoProgress).unwrap();

    assert!(names(&f.dest).is_empty());
    assert_eq!(trashed_originals(&f.trash), [f.dest.join("a")]);
}

/// Cancels after its first report.
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
fn cancelled_undo_is_partial_and_retry_finishes() {
    let f = fixture();
    let mut journal = Journal::new();
    journal.record(copy_files(&f, &["a", "b", "c"]));
    let progress = CancelAfterFirst {
        reports: AtomicU32::new(0),
    };

    let err = journal.undo(&f.trash, &progress).unwrap_err();

    match err {
        UndoError::Partial {
            done,
            not_done,
            cause,
            ..
        } => {
            assert_eq!(done, [f.dest.join("a")]);
            assert_eq!(not_done, [f.dest.join("b"), f.dest.join("c")]);
            assert!(matches!(*cause, FsError::Cancelled), "{cause:?}");
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(names(&f.dest), ["b", "c"]);
    // The rest stays undoable; the part done is redoable.
    match journal.peek_undo() {
        Some(Undoable::Copied { created }) => assert_eq!(created.len(), 2),
        other => panic!("{other:?}"),
    }
    match journal.peek_redo() {
        Some(Undoable::Copied { created }) => assert_eq!(created.len(), 1),
        other => panic!("{other:?}"),
    }

    journal.undo(&f.trash, &NoProgress).unwrap();

    assert!(names(&f.dest).is_empty());
    assert_eq!(trashed_originals(&f.trash).len(), 3);
    journal.redo(&f.trash, &NoProgress).unwrap();
    journal.redo(&f.trash, &NoProgress).unwrap();
    assert_eq!(names(&f.dest), ["a", "b", "c"]);
}

#[test]
fn depth_capped_at_100() {
    let f = fixture();
    let mut journal = Journal::new();
    // The oldest entry would block every undo below it; it must drop out.
    journal.record(Undoable::Irreversible {
        what: "oldest".to_string(),
        reason: IrreversibleReason::PermanentDelete,
    });
    for i in 0..JOURNAL_DEPTH {
        let made = create_file(&f.src, &i.to_string()).unwrap();
        journal.record(Undoable::Created {
            stamp: TreeStamp::of(&made).unwrap(),
            path: made,
        });
    }

    let mut undone = 0;
    loop {
        match journal.undo(&f.trash, &NoProgress) {
            Ok(_) => undone += 1,
            Err(UndoError::NothingToUndo) => break,
            Err(e) => panic!("after {undone}: {e:?}"),
        }
    }

    assert_eq!(JOURNAL_DEPTH, 100);
    assert_eq!(undone, JOURNAL_DEPTH);
}

#[test]
fn symlink_copy_undo_trashes_link_not_target() {
    let f = fixture();
    let target = f.src.join("target.txt");
    fs::write(&target, "target").unwrap();
    let link = f.src.join("link");
    std::os::unix::fs::symlink(&target, &link).unwrap();
    let report = copy(&[link], &f.dest, &never, &NoProgress).unwrap();
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Copy, &report, &[]));

    journal.undo(&f.trash, &NoProgress).unwrap();

    assert!(names(&f.dest).is_empty());
    assert_eq!(fs::read_to_string(&target).unwrap(), "target");
    let items = f.trash.list().unwrap().items;
    assert_eq!(items.len(), 1);
    let held = items[0].trash_dir.join("files").join(&items[0].name);
    let meta = fs::symlink_metadata(&held).unwrap();
    assert!(meta.file_type().is_symlink());
    assert_eq!(fs::read_link(&held).unwrap(), target);
}

#[test]
fn tree_stamp_never_follows_symlinks() {
    let f = fixture();
    let dir = f.src.join("dir");
    fs::create_dir(&dir).unwrap();
    fs::write(dir.join("inner"), "x").unwrap();
    let link = f.src.join("link");
    std::os::unix::fs::symlink(&dir, &link).unwrap();

    let before = TreeStamp::of(&link).unwrap();
    fs::write(dir.join("inner"), "changed through the target").unwrap();
    fs::write(dir.join("added"), "y").unwrap();

    assert_eq!(TreeStamp::of(&link).unwrap(), before);
    assert_eq!(before.entries().len(), 1);
    // Retargeting the link itself is a change.
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(f.src.join("elsewhere"), &link).unwrap();
    assert_ne!(TreeStamp::of(&link).unwrap(), before);
}

#[test]
fn tree_stamp_sorted_relative_paths() {
    let f = fixture();
    let dir = f.src.join("d");
    fs::create_dir_all(dir.join("b/c")).unwrap();
    fs::write(dir.join("a"), "").unwrap();
    fs::write(dir.join("b/c/e"), "").unwrap();

    let stamp = TreeStamp::of(&dir).unwrap();

    let paths: Vec<&Path> = stamp.entries().iter().map(|(p, _)| p.as_path()).collect();
    assert_eq!(
        paths,
        [
            Path::new(""),
            Path::new("a"),
            Path::new("b"),
            Path::new("b/c"),
            Path::new("b/c/e")
        ]
    );
}

#[test]
fn nothing_to_undo_on_empty_journal() {
    let f = fixture();
    let mut journal = Journal::new();
    assert!(matches!(
        journal.undo(&f.trash, &NoProgress),
        Err(UndoError::NothingToUndo)
    ));
    assert!(matches!(
        journal.redo(&f.trash, &NoProgress),
        Err(UndoError::NothingToRedo)
    ));
}

#[test]
fn redo_refused_when_name_taken() {
    let f = fixture();
    let mut journal = Journal::new();
    journal.record(copy_files(&f, &["a", "b"]));
    journal.undo(&f.trash, &NoProgress).unwrap();
    let squatter = f.dest.join("b");
    fs::write(&squatter, "someone else's").unwrap();

    let err = journal.redo(&f.trash, &NoProgress).unwrap_err();

    assert!(
        matches!(&err, UndoError::Changed { path, what: Change::Taken } if *path == squatter),
        "{err:?}"
    );
    // Nothing restored, the squatter untouched, both copies still in the trash.
    assert_eq!(names(&f.dest), ["b"]);
    assert_eq!(fs::read_to_string(&squatter).unwrap(), "someone else's");
    assert_eq!(
        trashed_originals(&f.trash),
        [f.dest.join("a"), f.dest.join("b")]
    );
    assert!(journal.peek_redo().is_some());
}

#[test]
fn undo_trash_refused_when_entry_left_trash() {
    let f = fixture();
    let a = f.src.join("a");
    fs::write(&a, "a").unwrap();
    let report = trash_paths(&f.trash, std::slice::from_ref(&a));
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Trash, &report, &[]));
    let item = f.trash.list().unwrap().items.remove(0);
    f.trash.purge(&item).unwrap();

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    let held = item.trash_dir.join("files").join(&item.name);
    assert!(
        matches!(&err, UndoError::Changed { path, what: Change::Missing } if *path == held),
        "{err:?}"
    );
    assert!(!a.exists());
}

#[test]
fn undo_trash_refused_when_entry_is_another_item() {
    let f = fixture();
    let a = f.src.join("a");
    fs::write(&a, "first").unwrap();
    let report = trash_paths(&f.trash, std::slice::from_ref(&a));
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Trash, &report, &[]));
    // The entry is emptied from the trash and a different file of another
    // folder takes its trash name.
    let item = f.trash.list().unwrap().items.remove(0);
    f.trash.purge(&item).unwrap();
    let other_dir = f.dest.join("a-holder");
    fs::create_dir(&other_dir).unwrap();
    fs::write(other_dir.join("a"), "second").unwrap();
    let other = f.trash.put(&other_dir.join("a")).unwrap();
    assert_eq!(other.name, item.name);

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    assert!(
        matches!(
            &err,
            UndoError::Changed {
                what: Change::Modified,
                ..
            }
        ),
        "{err:?}"
    );
    assert!(!a.exists());
    assert_eq!(trashed_originals(&f.trash), [other_dir.join("a")]);
}

#[test]
fn unreadable_at_record_refuses_undo() {
    use std::os::unix::fs::PermissionsExt;
    let f = fixture();
    let tree = f.src.join("tree");
    fs::create_dir_all(tree.join("locked")).unwrap();
    let report = copy(std::slice::from_ref(&tree), &f.dest, &never, &NoProgress).unwrap();
    let locked = f.dest.join("tree/locked");
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let op = undoable_from_report(OpKind::Copy, &report, &[]);
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();
    let mut journal = Journal::new();
    journal.record(op);

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    assert!(
        matches!(&err, UndoError::Changed { path, what: Change::Unverified } if *path == f.dest.join("tree")),
        "{err:?}"
    );
    assert!(locked.exists());
    assert!(trashed_originals(&f.trash).is_empty());
}

#[test]
fn record_ignores_op_with_nothing_done() {
    let f = fixture();
    let mut journal = Journal::new();
    journal.record(copy_files(&f, &["a"]));
    journal.undo(&f.trash, &NoProgress).unwrap();

    journal.record(undoable_from_report(
        OpKind::Copy,
        &OpReport::default(),
        &[],
    ));

    assert!(journal.peek_undo().is_none());
    assert!(journal.peek_redo().is_some());
}

#[test]
fn undo_move_refused_when_folder_gone() {
    let f = fixture();
    let from_dir = f.src.join("sub");
    fs::create_dir(&from_dir).unwrap();
    let file = from_dir.join("a");
    fs::write(&file, "a").unwrap();
    let report = move_items(std::slice::from_ref(&file), &f.dest, &never, &NoProgress).unwrap();
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Move, &report, &[]));
    fs::remove_dir(&from_dir).unwrap();

    let err = journal.undo(&f.trash, &NoProgress).unwrap_err();

    assert!(
        matches!(&err, UndoError::Changed { path, what: Change::Missing } if *path == from_dir),
        "{err:?}"
    );
    assert_eq!(names(&f.dest), ["a"]);
}

/// Moves from the test temp dir to Cargo's target temp dir and back. Where
/// the two share a device the rename path runs instead and the test proves
/// nothing, so it returns early (as `move_across_devices_copies_then_deletes`
/// does).
#[test]
fn undo_move_back_across_devices() {
    let near = tempfile::tempdir().unwrap();
    let far = tempfile::tempdir_in(env!("CARGO_TARGET_TMPDIR")).unwrap();
    use std::os::unix::fs::MetadataExt;
    if fs::metadata(near.path()).unwrap().dev() == fs::metadata(far.path()).unwrap().dev() {
        eprintln!("skipped: temp dirs share a device");
        return;
    }
    let near = near.path().canonicalize().unwrap();
    let far_dir = far.path().canonicalize().unwrap();
    let tree = near.join("tree");
    fs::create_dir_all(tree.join("sub")).unwrap();
    fs::write(tree.join("sub/f"), "data").unwrap();
    std::os::unix::fs::symlink("sub/f", tree.join("link")).unwrap();
    let trash = Trash::with_home_trash(near.join("trash"));
    let report = move_items(std::slice::from_ref(&tree), &far_dir, &never, &NoProgress).unwrap();
    assert!(report.failed.is_empty(), "{:?}", report.failed);
    let mut journal = Journal::new();
    journal.record(undoable_from_report(OpKind::Move, &report, &[]));

    journal.undo(&trash, &NoProgress).unwrap();

    assert!(!far_dir.join("tree").exists());
    assert_eq!(fs::read_to_string(tree.join("sub/f")).unwrap(), "data");
    assert_eq!(
        fs::read_link(tree.join("link")).unwrap(),
        Path::new("sub/f")
    );

    journal.redo(&trash, &NoProgress).unwrap();

    assert!(!tree.exists());
    assert_eq!(
        fs::read_to_string(far_dir.join("tree/sub/f")).unwrap(),
        "data"
    );
}
