// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name search and the Documents scan: streamed batches, loops, filters.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::sync::{mpsc, Mutex};

use common::{env, path, w, Env, EVENT_WAIT};
use kernkraft_tauri::WirePath;
use kernkraft_tauri::{
    OpDoneEvent, OpId, SearchHitEvent, EVENT_OP_DONE, EVENT_SEARCH_HIT, SEARCH_BATCH,
};
use tauri::Listener;

/// Every search-hit batch of `op`, in order, and its op-done.
fn collect(e: &Env, op: OpId) -> (Vec<SearchHitEvent>, OpDoneEvent) {
    let mut batches = Vec::new();
    loop {
        let ev = e.next_event();
        if ev.name == EVENT_SEARCH_HIT && ev.payload["op"] == op {
            batches.push(serde_json::from_value(ev.payload).unwrap());
        } else if ev.name == EVENT_OP_DONE && ev.payload["op"] == op {
            return (batches, serde_json::from_value(ev.payload).unwrap());
        }
    }
}

fn found(batches: &[SearchHitEvent]) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = batches
        .iter()
        .flat_map(|b| b.entries.iter().map(|e| path(&e.path)))
        .collect();
    v.sort();
    v
}

#[test]
fn search_streams_batches() {
    let e = env();
    let root = e.dir("s");
    // Two full batches and a partial one of matches, among files that do
    // not match.
    let matches = 2 * SEARCH_BATCH + 50;
    let mut expected = Vec::new();
    for i in 0..matches {
        let p = root.join(format!("Report-{i:04}.txt"));
        fs::write(&p, "").unwrap();
        expected.push(p);
    }
    for i in 0..30 {
        fs::write(root.join(format!("other-{i}.txt")), "").unwrap();
    }
    fs::create_dir(root.join("sub")).unwrap();
    let deep = root.join("sub/old REPORT.md");
    fs::write(&deep, "").unwrap();
    expected.push(deep);
    expected.sort();

    let op =
        e.kk.search(
            "find".to_owned(),
            vec![w(&root)],
            "report".to_owned(),
            false,
            None,
        )
        .unwrap();
    let (batches, done) = collect(&e, op);

    assert!(batches.iter().all(|b| b.key == "find"));
    // A batch goes out when it is full or after SEARCH_FLUSH, so on a slow
    // machine it may be smaller: never larger, and nothing lost or repeated.
    let sizes: Vec<usize> = batches.iter().map(|b| b.entries.len()).collect();
    assert!(
        sizes.iter().all(|&n| (1..=SEARCH_BATCH).contains(&n)),
        "{sizes:?}"
    );
    assert!(batches.len() >= 3, "{sizes:?}");
    assert_eq!(sizes.iter().sum::<usize>(), expected.len());
    // Case-insensitive, at any depth.
    assert_eq!(found(&batches), expected);
    let scan = done.scan.unwrap();
    assert_eq!(scan.found, expected.len() as u64);
    assert!(!scan.cancelled);
    assert!(scan.unreadable.is_empty());
    assert_eq!(done.error, None);
}

#[test]
fn search_terminates_on_loop() {
    let e = env();
    let root = e.dir("loop");
    fs::create_dir(root.join("a")).unwrap();
    fs::write(root.join("a/target.txt"), "").unwrap();
    // a/back -> the root, and a/self -> a: each folder is walked once.
    symlink(&root, root.join("a/back")).unwrap();
    symlink(root.join("a"), root.join("a/self")).unwrap();

    let op =
        e.kk.search(
            "l".to_owned(),
            vec![w(&root)],
            "target".to_owned(),
            false,
            None,
        )
        .unwrap();
    let (batches, done) = collect(&e, op);
    assert_eq!(found(&batches), [root.join("a/target.txt")]);
    assert_eq!(done.scan.unwrap().found, 1);
}

#[test]
fn search_hidden_only_when_asked() {
    let e = env();
    let root = e.dir("h");
    fs::create_dir(root.join(".cfg")).unwrap();
    fs::write(root.join(".cfg/key.txt"), "").unwrap();
    fs::write(root.join("key.md"), "").unwrap();
    let run = |hidden| {
        let op =
            e.kk.search(
                "h".to_owned(),
                vec![w(&root)],
                "key".to_owned(),
                hidden,
                None,
            )
            .unwrap();
        found(&collect(&e, op).0)
    };
    assert_eq!(run(false), [root.join("key.md")]);
    assert_eq!(run(true), [root.join(".cfg/key.txt"), root.join("key.md")]);
}

#[test]
fn search_refuses_empty_query() {
    let e = env();
    for q in ["", "   ", "\t\n"] {
        let err =
            e.kk.search("k".to_owned(), vec![w(&e.base)], q.to_owned(), false, None)
                .unwrap_err();
        assert_eq!(err.kind, "InvalidArgument", "{q:?}");
    }
}

#[test]
fn search_trims_the_query() {
    let e = env();
    let root = e.dir("trim");
    fs::write(root.join("Budget.txt"), "").unwrap();
    // Surrounding spaces are not part of what is searched for.
    let op =
        e.kk.search(
            "t".to_owned(),
            vec![w(&root)],
            "  budget ".to_owned(),
            false,
            None,
        )
        .unwrap();
    assert_eq!(found(&collect(&e, op).0), [root.join("Budget.txt")]);
}

#[test]
fn search_hits_keep_non_utf8_names() {
    let e = env();
    let root = e.dir("bytes");
    // 0xE9 alone is not UTF-8 (RFC 3629 section 4).
    let odd = root.join(OsStr::from_bytes(b"caf\xE9-needle.txt"));
    fs::write(&odd, "").unwrap();
    let op =
        e.kk.search(
            "b".to_owned(),
            vec![w(&root)],
            "needle".to_owned(),
            false,
            None,
        )
        .unwrap();
    let (batches, _) = collect(&e, op);
    let entry = &batches[0].entries[0];
    // Shown with a replacement character, acted on with the real bytes.
    assert_eq!(entry.name, "caf\u{FFFD}-needle.txt");
    assert!(matches!(entry.path, WirePath::Bytes(_)));
    assert_eq!(path(&entry.path), odd);
}

#[test]
fn search_cancel_reports_cancelled() {
    let e = env();
    let root = e.dir("c");
    const DIRS: usize = 50;
    const PER_DIR: usize = 20;
    for i in 0..DIRS {
        let d = root.join(format!("d{i:02}"));
        fs::create_dir(&d).unwrap();
        for j in 0..PER_DIR {
            fs::write(d.join(format!("f{j}.txt")), "").unwrap();
        }
    }
    // Event listeners run on the emitting thread, and the walk's next hit
    // waits for the batch being sent. The first batch's listener cancels the
    // op itself, so the cancel lands while the walk still has folders left
    // whichever of the walk and the flush timer sent that batch, then holds
    // it until the test has looked at undo.
    let (release, held) = mpsc::channel::<()>();
    let held = Mutex::new(Some(held));
    let (reached, at_batch) = mpsc::channel::<()>();
    let reached = Mutex::new(reached);
    let (op_of, op_in) = mpsc::channel::<OpId>();
    let op_in = Mutex::new(op_in);
    let canceller = e.kk.clone();
    e.app.listen_any(EVENT_SEARCH_HIT, move |_| {
        if let Some(rx) = held.lock().unwrap().take() {
            let op = op_in.lock().unwrap().recv_timeout(EVENT_WAIT).unwrap();
            canceller.cancel_op(op).unwrap();
            let _ = reached.lock().unwrap().send(());
            let _ = rx.recv_timeout(EVENT_WAIT);
        }
    });

    let op =
        e.kk.search("c".to_owned(), vec![w(&root)], "f".to_owned(), false, None)
            .unwrap();
    op_of.send(op).unwrap();
    at_batch.recv_timeout(EVENT_WAIT).unwrap();
    // A search only reads, so it does not hold undo back: with nothing
    // journalled, undo says so instead of Busy.
    assert_eq!(e.kk.undo().unwrap_err().kind, "NothingToUndo");
    release.send(()).unwrap();

    let (batches, done) = collect(&e, op);
    let scan = done.scan.unwrap();
    assert!(scan.cancelled);
    let total = (DIRS * PER_DIR) as u64;
    // A batch went out; the walk stopped before the end.
    assert!(scan.found >= 1, "{}", scan.found);
    assert!(scan.found < total, "{} of {total}", scan.found);
    assert_eq!(found(&batches).len() as u64, scan.found);
}

#[test]
fn documents_lists_text_only() {
    let e = env();
    let root = e.dir("docs");
    fs::create_dir(root.join("sub")).unwrap();
    for name in ["a.txt", "b.md", "sub/run.sh", "c.pdf", "d.png", "noext"] {
        fs::write(root.join(name), "text").unwrap();
    }
    // A folder named like a text file is not a document.
    fs::create_dir(root.join("folder.txt")).unwrap();
    // A link to a text file is listed; a dangling one is not.
    symlink(root.join("a.txt"), root.join("link.txt")).unwrap();
    symlink(root.join("gone.txt"), root.join("dangling.txt")).unwrap();
    fs::write(root.join(".hidden.txt"), "").unwrap();

    let op =
        e.kk.documents("docs".to_owned(), vec![w(&root)], false, None)
            .unwrap();
    let (batches, done) = collect(&e, op);
    // Text by name: text/plain, text/markdown, and a subclass of
    // text/plain (application/x-shellscript). Never by content: `noext`
    // holds text and is still left out.
    assert_eq!(
        found(&batches),
        [
            root.join("a.txt"),
            root.join("b.md"),
            root.join("link.txt"),
            root.join("sub/run.sh"),
        ]
    );
    assert_eq!(done.scan.unwrap().found, 4);
}

#[test]
fn documents_reach_dotfiles_and_dot_folders_only_when_asked() {
    let e = env();
    let root = e.dir("dot-docs");
    fs::create_dir(root.join(".config")).unwrap();
    fs::write(root.join("a.txt"), "").unwrap();
    fs::write(root.join(".hidden.txt"), "").unwrap();
    fs::write(root.join(".config/inside.md"), "").unwrap();
    // Not text, so never a document, hidden or not.
    fs::write(root.join(".hidden.png"), "").unwrap();

    let walk = |hidden: bool| {
        let op =
            e.kk.documents("docs".to_owned(), vec![w(&root)], hidden, None)
                .unwrap();
        let (batches, done) = collect(&e, op);
        (found(&batches), done.scan.unwrap().found)
    };
    assert_eq!(walk(false), (vec![root.join("a.txt")], 1));
    assert_eq!(
        walk(true),
        (
            vec![
                root.join(".config/inside.md"),
                root.join(".hidden.txt"),
                root.join("a.txt"),
            ],
            3
        )
    );
}

// Search only reads: a rename or a create goes ahead while one runs.
#[test]
fn rename_and_create_go_ahead_while_a_search_runs() {
    let e = env();
    let root = e.dir("busy-search");
    for i in 0..20 {
        fs::write(root.join(format!("f{i}")), "").unwrap();
    }
    fs::write(root.join("keep"), "").unwrap();
    // Hold the walk right after its first batch: the search is registered
    // and running while the edits are made.
    let (release, held) = std::sync::mpsc::channel::<()>();
    let held = std::sync::Mutex::new(Some(held));
    let (reached, at_batch) = std::sync::mpsc::channel::<()>();
    let reached = std::sync::Mutex::new(reached);
    e.app.listen_any(EVENT_SEARCH_HIT, move |_| {
        if let Some(rx) = held.lock().unwrap().take() {
            let _ = reached.lock().unwrap().send(());
            let _ = rx.recv_timeout(EVENT_WAIT);
        }
    });
    let search =
        e.kk.search("s".to_owned(), vec![w(&root)], "f".to_owned(), false, None)
            .unwrap();
    at_batch.recv_timeout(EVENT_WAIT).unwrap();

    e.kk.rename(w(&root.join("keep")), "kept".to_owned())
        .unwrap();
    e.kk.create_dir(w(&root), "made".to_owned()).unwrap();
    e.kk.create_file(w(&root), "made.txt".to_owned()).unwrap();
    assert!(root.join("kept").exists() && root.join("made").is_dir());

    // Cancelled while still held, so the search is certainly registered
    // when the cancel arrives; released after, it ends cancelled.
    e.kk.cancel_op(search).unwrap();
    release.send(()).unwrap();
    e.wait_done(search);
}
