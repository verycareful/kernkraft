// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name search and the Documents scan: streamed batches, loops, filters.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::os::unix::fs::symlink;
use std::path::PathBuf;

use common::{env, path, w, Env};
use kernkraft_tauri::{
    OpDoneEvent, OpId, SearchHitEvent, EVENT_OP_DONE, EVENT_SEARCH_HIT, SEARCH_BATCH,
};

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
    let sizes: Vec<usize> = batches.iter().map(|b| b.entries.len()).collect();
    assert_eq!(sizes, [SEARCH_BATCH, SEARCH_BATCH, 51]);
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
    let err =
        e.kk.search("k".to_owned(), vec![w(&e.base)], String::new(), false, None)
            .unwrap_err();
    assert_eq!(err.kind, "InvalidArgument");
}

#[test]
fn search_cancel_reports_cancelled() {
    let e = env();
    let root = e.dir("c");
    for i in 0..50 {
        let d = root.join(format!("d{i}"));
        fs::create_dir(&d).unwrap();
        for j in 0..20 {
            fs::write(d.join(format!("f{j}.txt")), "").unwrap();
        }
    }
    let op =
        e.kk.search("c".to_owned(), vec![w(&root)], "f".to_owned(), false, None)
            .unwrap();
    // A search only reads, so it does not hold undo back: with nothing
    // journalled, undo says so instead of Busy.
    assert_eq!(e.kk.undo().unwrap_err().kind, "NothingToUndo");
    // Cancelled at once; the walk polls before each folder. The op may
    // already have ended, which is the only way the cancel can fail.
    let cancelled = e.kk.cancel_op(op).is_ok();
    let (_, done) = collect(&e, op);
    let scan = done.scan.unwrap();
    if cancelled {
        assert!(scan.cancelled || scan.found == 1000);
    }
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
        e.kk.documents("docs".to_owned(), vec![w(&root)], None)
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
