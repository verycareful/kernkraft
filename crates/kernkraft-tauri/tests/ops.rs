// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! File operation commands, driven through the plugin on a mock app, with
//! every event captured from the app's listeners.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::symlink;
use std::path::PathBuf;
use std::time::Duration;

use common::{env, names, path, w};
use kernkraft_tauri::{
    OpProgressEvent, WirePath, WireTrashedItem, EVENT_OP_CONFLICT, EVENT_OP_DONE,
    EVENT_OP_PROGRESS, PROGRESS_INTERVAL_MS,
};

/// 1 MiB, the copy chunk of kernkraft-fs: a file of a few of these is
/// reported in several steps.
const MIB: usize = 1 << 20;

#[test]
fn copy_emits_progress_then_done() {
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    let a = src.join("a.bin");
    let content: Vec<u8> = (0..3 * MIB).map(|i| (i % 251) as u8).collect();
    fs::write(&a, &content).unwrap();

    let op =
        e.kk.start_op("copy", vec![w(&a)], Some(w(&dest)), None)
            .unwrap();
    let (events, done) = e.wait_done(op);

    let progress: Vec<_> = events
        .iter()
        .filter(|ev| ev.name == EVENT_OP_PROGRESS)
        .collect();
    assert!(!progress.is_empty());
    assert_eq!(events.last().unwrap().name, EVENT_OP_DONE);
    let last: OpProgressEvent =
        serde_json::from_value(progress.last().unwrap().payload.clone()).unwrap();
    assert_eq!(
        (last.done, last.total),
        (content.len() as u64, content.len() as u64)
    );
    assert_eq!(last.unit, "bytes");
    // Every update but the final one is at least the interval after the one
    // before it.
    let interval = Duration::from_millis(PROGRESS_INTERVAL_MS);
    let throttled = &progress[..progress.len() - 1];
    for pair in throttled.windows(2) {
        assert!(pair[1].at.duration_since(pair[0].at) >= interval);
    }

    let report = done.report.unwrap();
    assert_eq!(report.done.len(), 1);
    assert_eq!(path(&report.done[0].source), a);
    assert_eq!(
        path(report.done[0].dest.as_ref().unwrap()),
        dest.join("a.bin")
    );
    assert!(report.failed.is_empty() && !report.cancelled);
    assert_eq!(fs::read(dest.join("a.bin")).unwrap(), content);
}

#[test]
fn conflict_round_trip() {
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a.txt"), "new").unwrap();
    fs::write(dest.join("a.txt"), "old").unwrap();

    let op =
        e.kk.start_op("copy", vec![w(&src.join("a.txt"))], Some(w(&dest)), None)
            .unwrap();
    let conflict = e.wait_conflict(op);
    assert_eq!(
        serde_json::from_value::<WirePath>(conflict["existing"].clone()).unwrap(),
        w(&dest.join("a.txt"))
    );
    assert_eq!(conflict["existing_entry"]["len"], 3);
    assert_eq!(conflict["source_entry"]["mime"], "text/plain");
    let id = conflict["conflict"].as_u64().unwrap();
    e.kk.answer_conflict(op, id, "keep_both", false).unwrap();
    let (_, done) = e.wait_done(op);

    // kernkraft-fs numbers a kept duplicate from 2: `a (2).txt`.
    let report = done.report.unwrap();
    assert_eq!(
        path(report.done[0].dest.as_ref().unwrap()),
        dest.join("a (2).txt")
    );
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "old");
    assert_eq!(fs::read_to_string(dest.join("a (2).txt")).unwrap(), "new");
}

#[test]
fn apply_to_all_asks_once() {
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    let mut sources = Vec::new();
    for n in ["a", "b", "c"] {
        fs::write(src.join(n), "new").unwrap();
        fs::write(dest.join(n), "old").unwrap();
        sources.push(w(&src.join(n)));
    }

    let op =
        e.kk.start_op("copy", sources, Some(w(&dest)), None)
            .unwrap();
    let conflict = e.wait_conflict(op);
    let id = conflict["conflict"].as_u64().unwrap();
    e.kk.answer_conflict(op, id, "skip", true).unwrap();
    let (events, done) = e.wait_done(op);

    assert!(events.iter().all(|ev| ev.name != EVENT_OP_CONFLICT));
    let report = done.report.unwrap();
    assert_eq!(report.skipped.len(), 3);
    assert!(report.done.is_empty());
    for n in ["a", "b", "c"] {
        assert_eq!(fs::read_to_string(dest.join(n)).unwrap(), "old");
    }
}

#[test]
fn cancel_reports_cancelled() {
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a"), "new").unwrap();
    fs::write(src.join("b"), "b").unwrap();
    fs::write(dest.join("a"), "old").unwrap();

    let op =
        e.kk.start_op(
            "copy",
            vec![w(&src.join("a")), w(&src.join("b"))],
            Some(w(&dest)),
            None,
        )
        .unwrap();
    // The worker is blocked on the prompt; cancel releases it.
    e.wait_conflict(op);
    e.kk.cancel_op(op).unwrap();
    let (_, done) = e.wait_done(op);

    let report = done.report.unwrap();
    assert!(report.cancelled);
    assert!(report.done.is_empty() && report.skipped.is_empty());
    assert_eq!(
        report.not_reached,
        vec![w(&src.join("a")), w(&src.join("b"))]
    );
    assert_eq!(names(&dest), ["a"]);
    assert_eq!(fs::read_to_string(dest.join("a")).unwrap(), "old");
    // The op is gone: its ids are unknown now.
    assert_eq!(e.kk.cancel_op(op).unwrap_err().kind, "UnknownOp");
}

#[test]
fn trash_symlink_keeps_target() {
    let e = env();
    let dir = e.dir("files");
    let target = dir.join("target.txt");
    fs::write(&target, "keep me").unwrap();
    let target_dir = e.dir("files/target-dir");
    fs::write(target_dir.join("inner"), "inner").unwrap();
    let link = dir.join("link");
    let dir_link = dir.join("dir-link");
    let dangling = dir.join("dangling");
    symlink(&target, &link).unwrap();
    symlink(&target_dir, &dir_link).unwrap();
    symlink(dir.join("nowhere"), &dangling).unwrap();

    let op =
        e.kk.start_op(
            "trash",
            vec![w(&link), w(&dir_link), w(&dangling)],
            None,
            None,
        )
        .unwrap();
    let (_, done) = e.wait_done(op);

    let report = done.report.unwrap();
    assert_eq!(report.done.len(), 3, "{report:?}");
    assert_eq!(fs::read_to_string(&target).unwrap(), "keep me");
    assert_eq!(
        fs::read_to_string(target_dir.join("inner")).unwrap(),
        "inner"
    );
    assert_eq!(names(&dir), ["target-dir", "target.txt"]);
    for item in &report.done {
        let held = path(item.dest.as_ref().unwrap());
        assert!(fs::symlink_metadata(&held)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    let mut expected = vec![link, dir_link, dangling];
    expected.sort();
    assert_eq!(e.trashed(), expected);
}

#[test]
fn concurrent_trash_during_copy_reports() {
    // The copy is held deterministically on a conflict prompt for its first
    // source while another operation trashes its third source.
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    for n in ["a", "b", "c"] {
        fs::write(src.join(n), n).unwrap();
    }
    fs::write(dest.join("a"), "old").unwrap();
    let sources: Vec<WirePath> = ["a", "b", "c"].iter().map(|n| w(&src.join(n))).collect();

    let copy =
        e.kk.start_op("copy", sources, Some(w(&dest)), None)
            .unwrap();
    let conflict = e.wait_conflict(copy);
    let trash =
        e.kk.start_op("trash", vec![w(&src.join("c"))], None, None)
            .unwrap();
    let (_, trashed) = e.wait_done(trash);
    assert_eq!(trashed.report.unwrap().done.len(), 1);
    let id = conflict["conflict"].as_u64().unwrap();
    e.kk.answer_conflict(copy, id, "keep_both", false).unwrap();
    let (_, done) = e.wait_done(copy);

    let report = done.report.unwrap();
    let placed: Vec<PathBuf> = report
        .done
        .iter()
        .map(|d| path(d.dest.as_ref().unwrap()))
        .collect();
    assert_eq!(placed, [dest.join("a (2)"), dest.join("b")]);
    assert_eq!(report.failed.len(), 1);
    assert_eq!(path(&report.failed[0].path), src.join("c"));
    assert_eq!(report.failed[0].error.kind, "NotFound");
    assert!(report.skipped.is_empty() && !report.cancelled);
    // Nothing lost: the sources still there, the trashed one in the trash.
    assert_eq!(names(&src), ["a", "b"]);
    assert_eq!(names(&dest), ["a", "a (2)", "b"]);
    assert_eq!(e.trashed(), [src.join("c")]);
}

#[test]
fn undo_refused_while_busy() {
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a"), "new").unwrap();
    fs::write(dest.join("a"), "old").unwrap();
    let made = e.kk.create_file(w(&src), "made".into()).unwrap();

    let op =
        e.kk.start_op("copy", vec![w(&src.join("a"))], Some(w(&dest)), None)
            .unwrap();
    e.wait_conflict(op);
    assert_eq!(e.kk.undo().unwrap_err().kind, "Busy");
    assert_eq!(e.kk.redo().unwrap_err().kind, "Busy");
    e.kk.cancel_op(op).unwrap();
    e.wait_done(op);

    // With nothing running, the undo of the create goes ahead.
    let outcome = e.kk.undo().unwrap();
    assert_eq!(outcome.done, vec![made.clone()]);
    assert!(fs::symlink_metadata(path(&made)).is_err());
}

#[test]
fn replace_then_undo_irreversible() {
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a.txt"), "new").unwrap();
    fs::write(dest.join("a.txt"), "old").unwrap();

    let op =
        e.kk.start_op("copy", vec![w(&src.join("a.txt"))], Some(w(&dest)), None)
            .unwrap();
    let conflict = e.wait_conflict(op);
    let id = conflict["conflict"].as_u64().unwrap();
    e.kk.answer_conflict(op, id, "replace", false).unwrap();
    e.wait_done(op);
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "new");
    assert_eq!(
        e.kk.undo_state().undo.unwrap(),
        format!("Undo copy onto {}", dest.join("a.txt").display())
    );

    let err = e.kk.undo().unwrap_err();
    assert_eq!(err.kind, "Irreversible");
    assert_eq!(
        serde_json::to_value(err.details.unwrap()).unwrap(),
        serde_json::json!({"k": "Irreversible", "reason": "ReplacedExisting"})
    );
    assert_eq!(fs::read_to_string(dest.join("a.txt")).unwrap(), "new");
    assert_eq!(e.kk.undo().unwrap_err().kind, "NothingToUndo");
}

#[test]
fn non_utf8_entry_lists_and_renames() {
    let e = env();
    let dir = e.dir("d");
    let raw = dir.join(OsStr::from_bytes(b"n\xffme.txt"));
    fs::write(&raw, "bytes").unwrap();

    let entries = e.kk.list_dir(w(&dir), false).unwrap();
    assert_eq!(entries.len(), 1);
    assert!(matches!(entries[0].path, WirePath::Bytes(_)));
    assert_eq!(path(&entries[0].path), raw);
    assert_eq!(entries[0].name, "n\u{fffd}me.txt");
    assert_eq!(entries[0].mime, "text/plain");

    // The entry's path crosses the wire as JSON and comes back intact.
    let json = serde_json::to_string(&entries[0].path).unwrap();
    let back: WirePath = serde_json::from_str(&json).unwrap();
    let renamed = e.kk.rename(back, "renamed.txt".into()).unwrap();
    assert_eq!(path(&renamed), dir.join("renamed.txt"));
    assert_eq!(
        fs::read_to_string(dir.join("renamed.txt")).unwrap(),
        "bytes"
    );
    assert!(fs::symlink_metadata(&raw).is_err());
    assert_eq!(
        e.kk.undo_state().undo.unwrap(),
        "Undo rename of n\u{fffd}me.txt"
    );

    // Undo puts the real bytes back.
    e.kk.undo().unwrap();
    assert_eq!(fs::read_to_string(&raw).unwrap(), "bytes");
}

#[test]
fn list_dir_types_by_name_and_kind() {
    let e = env();
    let dir = e.dir("d");
    fs::write(dir.join("a.txt"), "").unwrap();
    // Text content, but the name says nothing: typed by name only.
    fs::write(dir.join("README"), "plain text").unwrap();
    fs::create_dir(dir.join("sub")).unwrap();
    symlink(dir.join("sub"), dir.join("to-sub")).unwrap();
    fs::write(dir.join(".hidden"), "").unwrap();

    let entries = e.kk.list_dir(w(&dir), false).unwrap();
    let got: Vec<(&str, &str, &str)> = entries
        .iter()
        .map(|en| (en.name.as_str(), en.kind.as_str(), en.mime.as_str()))
        .collect();
    assert_eq!(
        got,
        [
            ("README", "file", "application/octet-stream"),
            ("a.txt", "file", "text/plain"),
            ("sub", "dir", "inode/directory"),
            ("to-sub", "symlink", "inode/directory"),
        ]
    );
    assert!(entries[3].target_is_dir);
    assert!(entries[0].modified_ms.is_some());
    assert_eq!(e.kk.list_dir(w(&dir), true).unwrap().len(), 5);
    let missing = e.kk.list_dir(w(&dir.join("missing")), false).unwrap_err();
    assert_eq!(missing.kind, "NotFound");
}

#[test]
fn move_and_rename_chain_undo() {
    // create, rename, move: undone in reverse, each finding the item as the
    // step after it left it.
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    let made = e.kk.create_dir(w(&src), "folder".into()).unwrap();
    let renamed = e.kk.rename(made.clone(), "renamed".into()).unwrap();
    let op =
        e.kk.start_op("move", vec![renamed.clone()], Some(w(&dest)), None)
            .unwrap();
    let (_, done) = e.wait_done(op);
    assert_eq!(done.report.unwrap().done.len(), 1);
    assert_eq!(names(&dest), ["renamed"]);

    assert_eq!(e.kk.undo_state().undo.unwrap(), "Undo move of renamed");
    e.kk.undo().unwrap();
    assert_eq!(names(&src), ["renamed"]);
    assert_eq!(e.kk.undo_state().redo.unwrap(), "Redo move of renamed");
    e.kk.undo().unwrap();
    assert_eq!(names(&src), ["folder"]);
    e.kk.undo().unwrap();
    assert!(names(&src).is_empty());
    assert_eq!(e.trashed(), [src.join("folder")]);
    let state = e.kk.undo_state();
    assert_eq!(state.undo, None);
    assert_eq!(state.redo.unwrap(), "Redo create of folder");
}

#[test]
fn trash_then_undo_and_redo() {
    let e = env();
    let src = e.dir("src");
    let a = src.join("a");
    fs::write(&a, "a").unwrap();
    let op = e.kk.start_op("trash", vec![w(&a)], None, None).unwrap();
    e.wait_done(op);
    assert_eq!(e.trashed(), std::slice::from_ref(&a));

    let outcome = e.kk.undo().unwrap();
    assert_eq!(outcome.done, vec![w(&a)]);
    assert_eq!(fs::read_to_string(&a).unwrap(), "a");
    e.kk.redo().unwrap();
    assert_eq!(e.trashed(), std::slice::from_ref(&a));
    assert!(fs::symlink_metadata(&a).is_err());
}

#[test]
fn delete_reports_and_undo_says_irreversible() {
    let e = env();
    let src = e.dir("src");
    let a = src.join("a");
    let d = e.dir("src/d");
    fs::write(&a, "a").unwrap();
    fs::write(d.join("inner"), "x").unwrap();
    let missing = src.join("missing");

    let op =
        e.kk.start_op("delete", vec![w(&a), w(&d), w(&missing)], None, None)
            .unwrap();
    let (_, done) = e.wait_done(op);
    let report = done.report.unwrap();
    assert_eq!(report.done.len(), 2);
    assert!(report.done.iter().all(|d| d.dest.is_none()));
    assert_eq!(report.failed.len(), 1);
    assert_eq!(report.failed[0].error.kind, "NotFound");
    assert!(names(&src).is_empty());

    assert_eq!(e.kk.undo_state().undo.unwrap(), "Undo delete of 2 items");
    assert_eq!(e.kk.undo().unwrap_err().kind, "Irreversible");
}

#[test]
fn trash_list_restore_and_purge() {
    let e = env();
    let src = e.dir("src");
    for n in ["a", "b", "c"] {
        fs::write(src.join(n), n).unwrap();
    }
    let sources: Vec<WirePath> = ["a", "b", "c"].iter().map(|n| w(&src.join(n))).collect();
    let op = e.kk.start_op("trash", sources, None, None).unwrap();
    e.wait_done(op);

    let listing = e.kk.trash_list().unwrap();
    assert!(listing.unreadable.is_empty());
    let item = |n: &str| -> WireTrashedItem {
        listing
            .items
            .iter()
            .find(|i| path(&i.original) == src.join(n))
            .unwrap()
            .clone()
    };

    // An item the webview made up is refused, not acted on.
    let mut forged = item("b");
    forged.name = "../../src/a".into();
    let report = e.kk.trash_purge(vec![forged]).unwrap();
    assert!(report.done.is_empty());
    assert_eq!(report.failed[0].error.kind, "NotInTrash");

    let report = e.kk.trash_restore(vec![item("a")]).unwrap();
    assert_eq!(path(report.done[0].dest.as_ref().unwrap()), src.join("a"));
    assert_eq!(fs::read_to_string(src.join("a")).unwrap(), "a");

    let report = e.kk.trash_purge(vec![item("b")]).unwrap();
    assert_eq!(report.done.len(), 1);
    assert_eq!(e.trashed(), [src.join("c")]);

    let op =
        e.kk.start_op("empty_trash", Vec::new(), None, None)
            .unwrap();
    let (_, done) = e.wait_done(op);
    let report = done.report.unwrap();
    assert_eq!(report.done.len(), 1);
    assert_eq!(path(&report.done[0].source), src.join("c"));
    assert!(e.trashed().is_empty());
    assert_eq!(e.kk.undo_state().undo.unwrap(), "Undo empty trash");
}

#[test]
fn properties_counts_recursively() {
    let e = env();
    let d = e.dir("d/sub");
    fs::write(d.join("a"), "12345").unwrap();
    fs::write(e.base.join("d/b"), "12").unwrap();
    symlink(&d, e.base.join("d/link")).unwrap();

    let op = e.kk.properties(vec![w(&e.base.join("d"))], None).unwrap();
    let (_, done) = e.wait_done(op);
    let props = done.properties.unwrap();
    // d and sub; a, b and the link (not followed).
    assert_eq!((props.files, props.dirs, props.bytes), (3, 2, 7));
    assert!(props.unreadable.is_empty() && !props.cancelled);
    assert!(done.report.is_none() && done.error.is_none());
}

#[test]
fn bad_requests_are_wire_errors() {
    let e = env();
    let src = e.dir("src");
    let kind = |r: Result<u64, kernkraft_tauri::WireError>| r.unwrap_err().kind;
    assert_eq!(
        kind(e.kk.start_op("shred", Vec::new(), None, None)),
        "InvalidArgument"
    );
    assert_eq!(
        kind(e.kk.start_op("copy", vec![w(&src)], None, None)),
        "InvalidArgument"
    );
    assert_eq!(
        kind(e.kk.start_op("trash", vec![w(&src)], Some(w(&src)), None)),
        "InvalidArgument"
    );
    assert_eq!(
        kind(e.kk.start_op("copy", vec![w(&src)], Some(w(&src.join("no"))), None)),
        "NotFound"
    );
    assert_eq!(
        kind(e.kk.start_op("trash", vec![WirePath::Bytes("!".into())], None, None)),
        "BadBase64"
    );
    assert_eq!(e.kk.cancel_op(999).unwrap_err().kind, "UnknownOp");
    assert_eq!(
        e.kk.answer_conflict(999, 1, "skip", false)
            .unwrap_err()
            .kind,
        "UnknownOp"
    );
    assert_eq!(
        e.kk.unwatch_dir("nothing").unwrap_err().kind,
        "UnknownWatch"
    );
    assert_eq!(
        e.kk.create_dir(w(&src), "a/b".into()).unwrap_err().kind,
        "InvalidName"
    );

    // An answer to a conflict that is not open, or with an unknown choice.
    fs::write(src.join("a"), "").unwrap();
    let dest = e.dir("dest");
    fs::write(dest.join("a"), "").unwrap();
    let op =
        e.kk.start_op("copy", vec![w(&src.join("a"))], Some(w(&dest)), None)
            .unwrap();
    let id = e.wait_conflict(op)["conflict"].as_u64().unwrap();
    assert_eq!(
        e.kk.answer_conflict(op, id + 1, "skip", false)
            .unwrap_err()
            .kind,
        "UnknownConflict"
    );
    assert_eq!(
        e.kk.answer_conflict(op, id, "overwrite", false)
            .unwrap_err()
            .kind,
        "InvalidArgument"
    );
    e.kk.answer_conflict(op, id, "skip", false).unwrap();
    let (_, done) = e.wait_done(op);
    assert_eq!(done.report.unwrap().skipped, vec![w(&src.join("a"))]);
}

#[test]
fn permissions_cover_every_command() {
    // Every command Tauri generated a permission for is granted by
    // `kernkraft:default`, and the default grants nothing else.
    let crate_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let mut generated: Vec<String> =
        fs::read_dir(crate_dir.join("permissions/autogenerated/commands"))
            .unwrap()
            .map(|e| {
                let name = e.unwrap().file_name().to_string_lossy().into_owned();
                format!("allow-{}", name.trim_end_matches(".toml").replace('_', "-"))
            })
            .collect();
    generated.sort();
    let default = fs::read_to_string(crate_dir.join("permissions/default.toml")).unwrap();
    let mut granted: Vec<String> = default
        .lines()
        .map(str::trim)
        .filter(|l| l.starts_with("\"allow-"))
        .map(|l| l.trim_end_matches(',').trim_matches('"').to_owned())
        .collect();
    granted.sort();
    assert_eq!(generated, granted);
    assert_eq!(generated.len(), kernkraft_tauri::COMMANDS.len());
}

#[test]
fn app_exit_releases_a_blocked_worker() {
    let e = env();
    let window = tauri::WebviewWindowBuilder::new(&e.app, "main", Default::default())
        .build()
        .unwrap();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a"), "new").unwrap();
    fs::write(dest.join("a"), "old").unwrap();
    let op =
        e.kk.start_op(
            "copy",
            vec![w(&src.join("a"))],
            Some(w(&dest)),
            Some("main".into()),
        )
        .unwrap();
    e.wait_conflict(op);

    // Destroying the last window while the event loop runs makes the app
    // exit; the plugin cancels every operation on exit, which releases the
    // worker waiting for an answer no window can give now.
    let common::Env { app, events, .. } = e;
    app.run_return(move |_, event| {
        if matches!(event, tauri::RunEvent::Ready) {
            window.destroy().unwrap();
        }
    });
    // The exit waited for the cancelled worker: its op-done is already
    // here, with no waiting.
    let done = events
        .try_iter()
        .find(|ev| ev.name == EVENT_OP_DONE)
        .expect("op-done sent before the exit returned")
        .payload;
    assert_eq!(done["op"], op);
    assert_eq!(done["report"]["cancelled"], true);
    assert_eq!(fs::read_to_string(dest.join("a")).unwrap(), "old");
}

#[test]
fn relative_and_empty_paths_are_refused() {
    let e = env();
    for rel in ["victim", "", "./victim", "../victim"] {
        let p = WirePath::Utf8(rel.to_owned());
        let kind = |r: Result<u64, kernkraft_tauri::WireError>| r.unwrap_err().kind;
        assert_eq!(
            kind(e.kk.start_op("delete", vec![p.clone()], None, None)),
            "NotAbsolute"
        );
        assert_eq!(
            kind(e.kk.start_op("trash", vec![p.clone()], None, None)),
            "NotAbsolute"
        );
        assert_eq!(
            e.kk.rename(p.clone(), "x".into()).unwrap_err().kind,
            "NotAbsolute"
        );
        assert_eq!(
            e.kk.list_dir(p.clone(), true).unwrap_err().kind,
            "NotAbsolute"
        );
    }
    // Refused before anything ran: no operation was started.
    assert!(e.events.try_recv().is_err());
    assert_eq!(e.kk.undo_state().undo, None);
}

#[test]
fn replace_answered_but_not_done_stays_undoable() {
    // b is copied; a's conflict is answered replace and the operation is
    // cancelled in the same breath, so a is never replaced. The copy of b
    // must stay undoable, not be marked as having replaced anything.
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("b"), "b").unwrap();
    fs::write(src.join("a"), "new").unwrap();
    fs::write(dest.join("a"), "old").unwrap();
    let kk = e.kk.clone();
    // Answered and cancelled from the listener, which runs on the worker
    // thread before it waits: the worker then sees the answer, and the copy
    // sees the cancel before its first chunk.
    tauri::Listener::listen_any(&e.app, EVENT_OP_CONFLICT, move |ev| {
        let c: serde_json::Value = serde_json::from_str(ev.payload()).unwrap();
        let (op, id) = (c["op"].as_u64().unwrap(), c["conflict"].as_u64().unwrap());
        kk.answer_conflict(op, id, "replace", false).unwrap();
        kk.cancel_op(op).unwrap();
    });
    let op =
        e.kk.start_op(
            "copy",
            vec![w(&src.join("b")), w(&src.join("a"))],
            Some(w(&dest)),
            None,
        )
        .unwrap();
    let (_, done) = e.wait_done(op);
    let report = done.report.unwrap();
    assert!(report.cancelled);
    assert_eq!(report.done.len(), 1);
    assert_eq!(report.not_reached, vec![w(&src.join("a"))]);
    assert_eq!(fs::read_to_string(dest.join("a")).unwrap(), "old");

    assert_eq!(e.kk.undo_state().undo.unwrap(), "Undo copy of b");
    e.kk.undo().unwrap();
    assert_eq!(names(&dest), ["a"]);
}

fn trash_and_list(e: &common::Env, name: &str) -> (PathBuf, WireTrashedItem) {
    let src = e.dir("src");
    let p = src.join(name);
    fs::write(&p, "first").unwrap();
    let op = e.kk.start_op("trash", vec![w(&p)], None, None).unwrap();
    e.wait_done(op);
    let item =
        e.kk.trash_list()
            .unwrap()
            .items
            .into_iter()
            .find(|i| path(&i.original) == p)
            .unwrap();
    (p, item)
}

#[test]
fn trash_item_with_a_stale_id_is_refused() {
    let e = env();
    let (a, item) = trash_and_list(&e, "a");
    let mut stale = item.clone();
    stale.id = "1:2:3:4/5:6:7:8".into();

    let report = e.kk.trash_purge(vec![stale.clone()]).unwrap();
    assert!(report.done.is_empty());
    assert_eq!(report.failed[0].error.kind, "Changed");
    let report = e.kk.trash_restore(vec![stale]).unwrap();
    assert!(report.done.is_empty());
    assert_eq!(report.failed[0].error.kind, "Changed");
    assert_eq!(e.trashed(), std::slice::from_ref(&a));
    assert!(!a.exists());

    // The item as listed still works.
    let report = e.kk.trash_restore(vec![item]).unwrap();
    assert_eq!(report.done.len(), 1);
    assert_eq!(fs::read_to_string(&a).unwrap(), "first");
}

#[test]
fn trash_item_replaced_under_the_same_fields_is_refused() {
    // Another program empties the item and trashes a new `a`, which takes
    // the freed name; its deletion date is set to the old one, as happens
    // when both fall in the same second. Every field the UI holds now
    // matches the new entry; only the id tells them apart.
    let e = env();
    let (a, old) = trash_and_list(&e, "a");
    let trash = kernkraft_fs::Trash::with_home_trash(e.trash_dir.clone());
    let listed = trash.list().unwrap().items.remove(0);
    trash.purge(&listed).unwrap();
    fs::write(&a, "second").unwrap();
    let new = trash.put(&a).unwrap();
    assert_eq!(new.name, old.name);
    let info = e
        .trash_dir
        .join("info")
        .join(format!("{}.trashinfo", new.name));
    let text = fs::read_to_string(&info).unwrap();
    fs::write(&info, text.replace(&new.deleted, &old.deleted)).unwrap();
    let now = e.kk.trash_list().unwrap().items.remove(0);
    assert_eq!((&now.name, &now.deleted), (&old.name, &old.deleted));
    assert_ne!(now.id, old.id);

    let report = e.kk.trash_purge(vec![old.clone()]).unwrap();
    assert!(report.done.is_empty());
    assert_eq!(report.failed[0].error.kind, "Changed");
    let report = e.kk.trash_restore(vec![old]).unwrap();
    assert!(report.done.is_empty());
    assert_eq!(e.trashed(), std::slice::from_ref(&a));
}

#[test]
fn replace_done_inside_a_cancelled_merge_is_irreversible() {
    // b is copied; folder d is merged into the existing d, x inside it is
    // replaced, and the operation is cancelled at y's prompt. The merge is
    // not in done, but x was replaced: the copy cannot be undone.
    let e = env();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("b"), "b").unwrap();
    for (dir, content) in [(&src, "new"), (&dest, "old")] {
        fs::create_dir(dir.join("d")).unwrap();
        fs::write(dir.join("d/x"), content).unwrap();
        fs::write(dir.join("d/y"), content).unwrap();
    }
    let kk = e.kk.clone();
    let y = dest.join("d/y");
    tauri::Listener::listen_any(&e.app, EVENT_OP_CONFLICT, move |ev| {
        let c: serde_json::Value = serde_json::from_str(ev.payload()).unwrap();
        let (op, id) = (c["op"].as_u64().unwrap(), c["conflict"].as_u64().unwrap());
        let existing: WirePath = serde_json::from_value(c["existing"].clone()).unwrap();
        if path(&existing) == y {
            kk.cancel_op(op).unwrap();
        } else {
            kk.answer_conflict(op, id, "replace", false).unwrap();
        }
    });
    let op =
        e.kk.start_op(
            "copy",
            vec![w(&src.join("b")), w(&src.join("d"))],
            Some(w(&dest)),
            None,
        )
        .unwrap();
    let (_, done) = e.wait_done(op);
    let report = done.report.unwrap();
    assert!(report.cancelled);
    assert_eq!(report.done.len(), 1);
    assert_eq!(fs::read_to_string(dest.join("d/x")).unwrap(), "new");
    assert_eq!(fs::read_to_string(dest.join("d/y")).unwrap(), "old");

    let err = e.kk.undo().unwrap_err();
    assert_eq!(err.kind, "Irreversible");
    assert!(dest.join("b").exists());
}
