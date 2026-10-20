// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Every command through Tauri's IPC, as the frontend calls it: argument
//! names in camelCase, and the JSON shape of each reply and of an error.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{env, invoke, w, Env};
use kernkraft_tauri::EVENT_OP_DONE;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::WebviewWindow;

/// A WireError as JSON: exactly these four keys, with the given kind.
fn assert_wire_error(reply: Result<Value, Value>, kind: &str) -> Value {
    let err = reply.expect_err("the command must fail");
    let obj = err
        .as_object()
        .unwrap_or_else(|| panic!("not a WireError: {err}"));
    let mut keys: Vec<&str> = obj.keys().map(String::as_str).collect();
    keys.sort_unstable();
    assert_eq!(keys, ["details", "kind", "message", "path"], "{err}");
    assert_eq!(err["kind"], kind, "{err}");
    assert!(err["message"].is_string());
    err
}

fn keys(v: &Value) -> Vec<String> {
    let mut k: Vec<String> = v.as_object().unwrap().keys().cloned().collect();
    k.sort();
    k
}

fn wp(p: &std::path::Path) -> Value {
    serde_json::to_value(w(p)).unwrap()
}

fn setup() -> (Env, WebviewWindow<MockRuntime>) {
    let e = env();
    let view = e.webview("ipc");
    (e, view)
}

fn report_keys() -> Vec<&'static str> {
    vec![
        "cancelled",
        "done",
        "failed",
        "not_reached",
        "skipped",
        "warnings",
    ]
}

fn wait_done(e: &Env, op: &Value) -> Value {
    loop {
        let ev = e.next_event();
        if ev.name == EVENT_OP_DONE && ev.payload["op"] == *op {
            return ev.payload;
        }
    }
}

#[test]
fn ipc_list_dir() {
    let (e, v) = setup();
    let dir = e.dir("d");
    fs::write(dir.join("a.txt"), "12").unwrap();
    let reply = invoke(&v, "list_dir", json!({"path": wp(&dir), "hidden": false})).unwrap();
    let entry = &reply[0];
    assert_eq!(
        keys(entry),
        [
            "hidden",
            "kind",
            "len",
            "mime",
            "modified_ms",
            "name",
            "path",
            "target_is_dir"
        ]
    );
    assert_eq!(entry["path"], wp(&dir.join("a.txt")));
    assert_eq!(entry["kind"], "file");
    assert_eq!(entry["len"], 2);
    assert_eq!(entry["mime"], "text/plain");
    let err = assert_wire_error(
        invoke(
            &v,
            "list_dir",
            json!({"path": wp(&dir.join("no")), "hidden": true}),
        ),
        "NotFound",
    );
    assert_eq!(err["path"], wp(&dir.join("no")));
    assert_eq!(err["details"], Value::Null);
}

#[test]
fn ipc_create_dir() {
    let (e, v) = setup();
    let dir = e.dir("d");
    let reply = invoke(&v, "create_dir", json!({"parent": wp(&dir), "name": "new"})).unwrap();
    assert_eq!(
        reply,
        json!({"k": "Utf8", "v": dir.join("new").to_str().unwrap()})
    );
    assert!(dir.join("new").is_dir());
    assert_wire_error(
        invoke(&v, "create_dir", json!({"parent": wp(&dir), "name": "a/b"})),
        "InvalidName",
    );
}

#[test]
fn ipc_create_file() {
    let (e, v) = setup();
    let dir = e.dir("d");
    let reply = invoke(&v, "create_file", json!({"parent": wp(&dir), "name": "f"})).unwrap();
    assert_eq!(reply, wp(&dir.join("f")));
    assert_wire_error(
        invoke(&v, "create_file", json!({"parent": wp(&dir), "name": "f"})),
        "Exists",
    );
}

#[test]
fn ipc_rename() {
    let (e, v) = setup();
    let dir = e.dir("d");
    fs::write(dir.join("a"), "").unwrap();
    let reply = invoke(
        &v,
        "rename",
        json!({"path": wp(&dir.join("a")), "newName": "b"}),
    )
    .unwrap();
    assert_eq!(reply, wp(&dir.join("b")));
    assert_wire_error(
        invoke(
            &v,
            "rename",
            json!({"path": {"k": "Utf8", "v": "b"}, "newName": "c"}),
        ),
        "NotAbsolute",
    );
    assert!(dir.join("b").exists());
}

#[test]
fn ipc_start_op() {
    let (e, v) = setup();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a"), "a").unwrap();
    let op = invoke(
        &v,
        "start_op",
        json!({"kind": "copy", "sources": [wp(&src.join("a"))], "dest": wp(&dest)}),
    )
    .unwrap();
    assert!(op.is_u64());
    let done = wait_done(&e, &op);
    assert_eq!(keys(&done), ["error", "op", "properties", "report"]);
    let report = &done["report"];
    assert_eq!(keys(report), report_keys());
    assert_eq!(
        report["done"][0],
        json!({"source": wp(&src.join("a")), "dest": wp(&dest.join("a"))})
    );
    assert_wire_error(
        invoke(
            &v,
            "start_op",
            json!({"kind": "shred", "sources": [], "dest": null}),
        ),
        "InvalidArgument",
    );
}

#[test]
fn ipc_answer_conflict() {
    let (e, v) = setup();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a"), "new").unwrap();
    fs::write(dest.join("a"), "old").unwrap();
    let op = invoke(
        &v,
        "start_op",
        json!({"kind": "copy", "sources": [wp(&src.join("a"))], "dest": wp(&dest)}),
    )
    .unwrap();
    let conflict = e.wait_conflict(op.as_u64().unwrap());
    assert_eq!(
        keys(&conflict),
        [
            "conflict",
            "existing",
            "existing_entry",
            "op",
            "source",
            "source_entry"
        ]
    );
    let reply = invoke(
        &v,
        "answer_conflict",
        json!({"op": op, "conflict": conflict["conflict"], "choice": "keep_both", "applyToAll": false}),
    )
    .unwrap();
    assert_eq!(reply, Value::Null);
    wait_done(&e, &op);
    assert_wire_error(
        invoke(
            &v,
            "answer_conflict",
            json!({"op": op, "conflict": 1, "choice": "skip", "applyToAll": true}),
        ),
        "UnknownOp",
    );
}

#[test]
fn ipc_cancel_op() {
    let (e, v) = setup();
    let src = e.dir("src");
    let dest = e.dir("dest");
    fs::write(src.join("a"), "new").unwrap();
    fs::write(dest.join("a"), "old").unwrap();
    let op = invoke(
        &v,
        "start_op",
        json!({"kind": "copy", "sources": [wp(&src.join("a"))], "dest": wp(&dest)}),
    )
    .unwrap();
    e.wait_conflict(op.as_u64().unwrap());
    assert_eq!(
        invoke(&v, "cancel_op", json!({"op": op})).unwrap(),
        Value::Null
    );
    assert_eq!(wait_done(&e, &op)["report"]["cancelled"], true);
    assert_wire_error(invoke(&v, "cancel_op", json!({"op": op})), "UnknownOp");
}

/// Trashes `a` under `src` and waits for it.
fn trash_one(e: &Env, v: &WebviewWindow<MockRuntime>) -> std::path::PathBuf {
    let src = e.dir("src");
    fs::write(src.join("a"), "a").unwrap();
    let op = invoke(
        v,
        "start_op",
        json!({"kind": "trash", "sources": [wp(&src.join("a"))], "dest": null}),
    )
    .unwrap();
    wait_done(e, &op);
    src.join("a")
}

#[test]
fn ipc_trash_list() {
    let (e, v) = setup();
    let a = trash_one(&e, &v);
    let listing = invoke(&v, "trash_list", json!({})).unwrap();
    assert_eq!(keys(&listing), ["items", "unreadable"]);
    let item = &listing["items"][0];
    assert_eq!(
        keys(item),
        ["deleted", "id", "is_dir", "name", "original", "trash_dir"]
    );
    assert_eq!(item["original"], wp(&a));

    // A trash whose home directory is a file cannot be listed.
    let broken = env();
    fs::write(&broken.trash_dir, "").unwrap();
    let view = broken.webview("ipc");
    assert_wire_error(invoke(&view, "trash_list", json!({})), "Io");
}

#[test]
fn ipc_trash_restore() {
    let (e, v) = setup();
    let a = trash_one(&e, &v);
    let listing = invoke(&v, "trash_list", json!({})).unwrap();
    let item = listing["items"][0].clone();
    let mut forged = item.clone();
    forged["original"] = json!({"k": "Nope"});
    let report = invoke(&v, "trash_restore", json!({"items": [item, forged]})).unwrap();
    assert_eq!(keys(&report), report_keys());
    assert_eq!(report["done"][0]["dest"], wp(&a));
    assert_eq!(report["failed"][0]["error"]["kind"], "NotInTrash");
    assert!(a.exists());
}

#[test]
fn ipc_trash_purge() {
    let (e, v) = setup();
    let a = trash_one(&e, &v);
    let listing = invoke(&v, "trash_list", json!({})).unwrap();
    let item = listing["items"][0].clone();
    let mut forged = item.clone();
    forged["name"] = json!("../../x");
    let report = invoke(&v, "trash_purge", json!({"items": [forged, item]})).unwrap();
    assert_eq!(keys(&report), report_keys());
    assert_eq!(report["done"][0]["source"], wp(&a));
    assert_eq!(report["done"][0]["dest"], Value::Null);
    assert_eq!(report["failed"][0]["error"]["kind"], "NotInTrash");
    assert!(e.trashed().is_empty());
}

#[test]
fn ipc_undo() {
    let (e, v) = setup();
    assert_wire_error(invoke(&v, "undo", json!({})), "NothingToUndo");
    let dir = e.dir("d");
    invoke(&v, "create_file", json!({"parent": wp(&dir), "name": "f"})).unwrap();
    let outcome = invoke(&v, "undo", json!({})).unwrap();
    assert_eq!(
        outcome,
        json!({"done": [wp(&dir.join("f"))], "warnings": []})
    );
}

#[test]
fn ipc_redo() {
    let (e, v) = setup();
    assert_wire_error(invoke(&v, "redo", json!({})), "NothingToRedo");
    let dir = e.dir("d");
    invoke(&v, "create_file", json!({"parent": wp(&dir), "name": "f"})).unwrap();
    invoke(&v, "undo", json!({})).unwrap();
    let outcome = invoke(&v, "redo", json!({})).unwrap();
    assert_eq!(keys(&outcome), ["done", "warnings"]);
    assert!(dir.join("f").exists());
}

#[test]
fn ipc_undo_state() {
    // undo_state cannot fail: it only reads the journal.
    let (e, v) = setup();
    assert_eq!(
        invoke(&v, "undo_state", json!({})).unwrap(),
        json!({"undo": null, "redo": null})
    );
    let dir = e.dir("d");
    invoke(&v, "create_dir", json!({"parent": wp(&dir), "name": "x"})).unwrap();
    assert_eq!(
        invoke(&v, "undo_state", json!({})).unwrap(),
        json!({"undo": "Undo create of x", "redo": null})
    );
}

#[test]
fn ipc_watch_dir_and_unwatch_dir() {
    let (e, v) = setup();
    let dir = e.dir("d");
    assert_eq!(
        invoke(&v, "watch_dir", json!({"key": "k", "path": wp(&dir)})).unwrap(),
        Value::Null
    );
    fs::write(dir.join("x"), "").unwrap();
    let ev = e.next_event();
    assert_eq!(ev.payload, json!({"key": "k"}));
    assert_eq!(
        invoke(&v, "unwatch_dir", json!({"key": "k"})).unwrap(),
        Value::Null
    );
    assert_wire_error(
        invoke(
            &v,
            "watch_dir",
            json!({"key": "k", "path": wp(&dir.join("no"))}),
        ),
        "NotFound",
    );
    assert_wire_error(
        invoke(&v, "unwatch_dir", json!({"key": "k"})),
        "UnknownWatch",
    );
}

#[test]
fn ipc_properties() {
    let (e, v) = setup();
    let dir = e.dir("d");
    fs::write(dir.join("a"), "123").unwrap();
    let op = invoke(&v, "properties", json!({"paths": [wp(&dir)]})).unwrap();
    let done = wait_done(&e, &op);
    assert_eq!(
        done["properties"],
        json!({"files": 1, "dirs": 1, "bytes": 3, "unreadable": [], "cancelled": false})
    );
    assert_eq!(done["report"], Value::Null);
    assert_wire_error(
        invoke(
            &v,
            "properties",
            json!({"paths": [{"k": "Utf8", "v": "rel"}]}),
        ),
        "NotAbsolute",
    );
}

#[test]
fn ipc_malformed_paths_are_wire_errors() {
    let (_e, v) = setup();
    let bad = [
        json!({"k": "Nope", "v": "/x"}),
        json!({"k": "Utf8"}),
        json!({"k": "Utf8", "v": 7}),
        json!("/plain/string"),
        json!(null),
        json!(42),
    ];
    for b in &bad {
        let calls = [
            ("list_dir", json!({"path": b, "hidden": false})),
            ("create_dir", json!({"parent": b, "name": "n"})),
            ("create_file", json!({"parent": b, "name": "n"})),
            ("rename", json!({"path": b, "newName": "n"})),
            (
                "start_op",
                json!({"kind": "delete", "sources": [b], "dest": null}),
            ),
            (
                "start_op",
                json!({"kind": "copy", "sources": [], "dest": {"k": "X"}}),
            ),
            ("watch_dir", json!({"key": "k", "path": b})),
            ("properties", json!({"paths": [b]})),
        ];
        for (cmd, args) in calls {
            assert_wire_error(invoke(&v, cmd, args), "BadPath");
        }
    }
}
