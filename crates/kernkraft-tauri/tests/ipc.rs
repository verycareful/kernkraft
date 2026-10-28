// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Every command through Tauri's IPC, as the frontend calls it: argument
//! names in camelCase, and the JSON shape of each reply and of an error.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::{env, invoke, w, Env};
use kernkraft_tauri::{EVENT_OP_DONE, EVENT_SEARCH_HIT, EVENT_THEME_CHANGED, EVENT_THUMB};
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
    assert_eq!(keys(&done), ["error", "op", "properties", "report", "scan"]);
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

/// Error kinds a call to the system bus or UDisks may end in on a machine
/// without either (a CI runner, a container): no bus address, no socket,
/// or no UDisks service on the bus.
const NO_UDISKS: &[&str] = &[
    "NoAddress",
    "NotUnix",
    "Malformed",
    "Connect",
    "Handshake",
    "Udisks",
    "Dbus",
];

#[test]
fn ipc_text_open() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    fs::write(&path, "x\r\n").unwrap();
    let reply = invoke(&v, "text_open", json!({"path": wp(&path)})).unwrap();
    assert_eq!(
        keys(&reply),
        ["bom", "ending", "path", "read_only", "stamp", "text"]
    );
    assert_eq!(reply["text"], "x\n");
    assert_eq!(reply["ending"], "crlf");
    assert_eq!(reply["read_only"], Value::Null);
    assert!(reply["stamp"].is_string());
    let latin1 = e.dir("d").join("l.txt");
    fs::write(&latin1, b"\xE9").unwrap();
    let ro = invoke(&v, "text_open", json!({"path": wp(&latin1)})).unwrap();
    assert_eq!(keys(&ro["read_only"]), ["detail", "kind"]);
    assert_wire_error(
        invoke(&v, "text_open", json!({"path": wp(&e.dir("d"))})),
        "Io",
    );
}

#[test]
fn ipc_text_save() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    fs::write(&path, "x").unwrap();
    let opened = invoke(&v, "text_open", json!({"path": wp(&path)})).unwrap();
    let stamp = invoke(
        &v,
        "text_save",
        json!({"path": wp(&path), "text": "y\n", "ending": "crlf", "bom": true, "stamp": opened["stamp"]}),
    )
    .unwrap();
    assert!(stamp.is_string());
    assert_eq!(fs::read(&path).unwrap(), b"\xEF\xBB\xBFy\r\n");
    // The stamp is stale now.
    assert_wire_error(
        invoke(
            &v,
            "text_save",
            json!({"path": wp(&path), "text": "z", "ending": "lf", "bom": false, "stamp": opened["stamp"]}),
        ),
        "ExternalModification",
    );
    let new = e.dir("d").join("new.txt");
    invoke(
        &v,
        "text_save",
        json!({"path": wp(&new), "text": "n", "ending": "lf", "bom": false, "stamp": null}),
    )
    .unwrap();
    assert_eq!(fs::read_to_string(&new).unwrap(), "n");
}

#[test]
fn ipc_text_check() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    fs::write(&path, "x").unwrap();
    let opened = invoke(&v, "text_open", json!({"path": wp(&path)})).unwrap();
    let check = |v: &WebviewWindow<MockRuntime>| {
        invoke(
            v,
            "text_check",
            json!({"path": wp(&path), "stamp": opened["stamp"]}),
        )
    };
    assert_eq!(check(&v).unwrap(), "same");
    fs::write(&path, "longer").unwrap();
    assert_eq!(check(&v).unwrap(), "changed");
    fs::remove_file(&path).unwrap();
    assert_eq!(check(&v).unwrap(), "deleted");
    assert_wire_error(
        invoke(
            &v,
            "text_check",
            json!({"path": wp(&path), "stamp": "1:2:3"}),
        ),
        "InvalidArgument",
    );
}

#[test]
fn ipc_thumb_request() {
    let (e, v) = setup();
    let svg = e.dir("d").join("a.svg");
    fs::write(
        &svg,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8"/></svg>"#,
    )
    .unwrap();
    let reply = invoke(
        &v,
        "thumb_request",
        json!({"key": "g", "paths": [wp(&svg)], "size": "large"}),
    )
    .unwrap();
    assert_eq!(reply, Value::Null);
    let ev = e.wait_event(EVENT_THUMB);
    assert_eq!(keys(&ev), ["error", "file", "key", "path"]);
    assert_eq!(ev["key"], "g");
    assert_eq!(ev["path"], wp(&svg));
    assert!(ev["file"]["v"]
        .as_str()
        .unwrap()
        .contains("/thumbnails/large/"));
    assert_wire_error(
        invoke(
            &v,
            "thumb_request",
            json!({"key": "g", "paths": [wp(&svg)], "size": "x-large"}),
        ),
        "InvalidArgument",
    );
}

#[test]
fn ipc_thumb_cancel() {
    // Cancelling a key with nothing queued is not an error: the queue may
    // have drained just before.
    let (_e, v) = setup();
    assert_eq!(
        invoke(&v, "thumb_cancel", json!({"key": "none"})).unwrap(),
        Value::Null
    );
}

fn theme_args() -> Value {
    json!({
        "follow": {"colours": true, "accent": true, "fonts": true, "radius": true},
        "user": {"mode": "dark", "accent": "teal", "density": "compact"}
    })
}

#[test]
fn ipc_theme_get() {
    let (_e, v) = setup();
    let reply = invoke(&v, "theme_get", theme_args()).unwrap();
    assert_eq!(keys(&reply), ["css", "fallbacks", "problems", "tokens"]);
    assert!(reply["css"].as_str().unwrap().contains("--"));
    // A pinned scheme (Mode::Dark) is reported as that scheme.
    assert_eq!(reply["tokens"]["scheme"], "dark");
    assert_eq!(reply["tokens"]["density"], "compact");
    assert!(reply["problems"].is_array());
}

#[test]
fn ipc_theme_set_prefs() {
    let (e, v) = setup();
    let reply = invoke(&v, "theme_set_prefs", theme_args()).unwrap();
    assert_eq!(reply["tokens"]["scheme"], "dark");
    let ev = e.wait_event(EVENT_THEME_CHANGED);
    assert_eq!(ev["css"], reply["css"]);
    assert_eq!(ev["tokens"], reply["tokens"]);
}

#[test]
fn ipc_apps_for() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    fs::write(&path, "x").unwrap();
    let reply = invoke(&v, "apps_for", json!({"path": wp(&path)})).unwrap();
    assert_eq!(reply, json!({"apps": [], "problems": []}));
}

#[test]
fn ipc_app_default() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    fs::write(&path, "x").unwrap();
    let reply = invoke(&v, "app_default", json!({"path": wp(&path)})).unwrap();
    assert_eq!(reply, json!({"app": null, "problems": []}));
}

#[test]
fn ipc_all_apps() {
    let (e, v) = setup();
    let apps = e.xdg.data.join("applications");
    fs::create_dir_all(&apps).unwrap();
    fs::write(
        apps.join("v.desktop"),
        "[Desktop Entry]\nType=Application\nName=V\nIcon=v\nExec=v %f\nMimeType=text/plain;\n",
    )
    .unwrap();
    let reply = invoke(&v, "all_apps", json!({})).unwrap();
    assert_eq!(
        reply["apps"][0],
        json!({"id": "v.desktop", "name": "V", "icon": "v", "terminal": false, "no_display": false, "mime_types": ["text/plain"]})
    );
    assert_eq!(reply["problems"], json!([]));
}

// The launching commands are called here only where they fail before any
// program starts; tests/apps.rs runs them against stub programs.

#[test]
fn ipc_launch_app() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    let err = assert_wire_error(
        invoke(
            &v,
            "launch_app",
            json!({"id": "none.desktop", "paths": [wp(&path)], "terminal": "foot"}),
        ),
        "UnknownApp",
    );
    assert!(err["message"].as_str().unwrap().contains("none.desktop"));
}

#[test]
fn ipc_open_default() {
    let (e, v) = setup();
    let path = e.dir("d").join("a.txt");
    fs::write(&path, "x").unwrap();
    let err = assert_wire_error(
        invoke(
            &v,
            "open_default",
            json!({"paths": [wp(&path)], "terminal": null}),
        ),
        "NoDefaultApp",
    );
    assert_eq!(err["path"], wp(&path));
}

#[test]
fn ipc_set_default_app() {
    let (_e, v) = setup();
    assert_wire_error(
        invoke(
            &v,
            "set_default_app",
            json!({"id": "none.desktop", "mime": "text/plain"}),
        ),
        "UnknownApp",
    );
}

#[test]
fn ipc_open_terminal() {
    let (e, v) = setup();
    assert_wire_error(
        invoke(
            &v,
            "open_terminal",
            json!({"dir": wp(&e.base.join("missing")), "setting": null}),
        ),
        "NotFound",
    );
    let file = e.dir("d").join("f");
    fs::write(&file, "").unwrap();
    assert_wire_error(
        invoke(
            &v,
            "open_terminal",
            json!({"dir": wp(&file), "setting": "kitty"}),
        ),
        "Io",
    );
}

#[test]
fn ipc_mime_of() {
    let (e, v) = setup();
    let dir = e.dir("d");
    // No glob matches; the content decides (sniffing per file).
    fs::write(dir.join("README"), "plain words").unwrap();
    fs::write(dir.join("blob"), b"\0\x01").unwrap();
    let mime = |name: &str| invoke(&v, "mime_of", json!({"path": wp(&dir.join(name))})).unwrap();
    assert_eq!(
        mime("README"),
        json!({"mime": "text/plain", "is_text": true})
    );
    assert_eq!(
        mime("blob"),
        json!({"mime": "application/octet-stream", "is_text": false})
    );
}

#[test]
fn ipc_user_dirs() {
    let (e, v) = setup();
    fs::create_dir_all(e.xdg.home.join("Documents")).unwrap();
    fs::create_dir_all(e.xdg.home.join("Bilder")).unwrap();
    fs::create_dir_all(&e.xdg.config).unwrap();
    fs::write(
        e.xdg.config.join("user-dirs.dirs"),
        "XDG_PICTURES_DIR=\"$HOME/Bilder\"\nXDG_DESKTOP_DIR=\"$HOME\"\n",
    )
    .unwrap();
    let reply = invoke(&v, "user_dirs", json!({})).unwrap();
    // Existing folders only, and none that is home itself (xdg-user-dirs
    // writes $HOME for a folder it disables).
    assert_eq!(
        reply,
        json!({
            "home": wp(&e.xdg.home),
            "dirs": [
                {"name": "documents", "path": wp(&e.xdg.home.join("Documents"))},
                {"name": "pictures", "path": wp(&e.xdg.home.join("Bilder"))},
            ]
        })
    );
}

#[test]
fn ipc_volumes_list() {
    // Reads this machine's own mounts; only the shape is checked.
    let (_e, v) = setup();
    let reply = invoke(&v, "volumes_list", json!({})).unwrap();
    assert_eq!(keys(&reply), ["mounted", "problems", "unmounted"]);
    for m in reply["mounted"].as_array().unwrap() {
        assert_eq!(
            keys(m),
            [
                "device",
                "fs_type",
                "identity",
                "label",
                "mount_point",
                "read_only",
                "removable"
            ]
        );
    }
}

#[test]
fn ipc_volume_mount() {
    // A device without an identity is refused before UDisks is asked
    // anything; one the listing does not hold is refused before UDisks is
    // asked to mount anything.
    let (_e, v) = setup();
    let device = |identity: serde_json::Value| {
        json!({"object_path": "/org/freedesktop/UDisks2/block_devices/kernkraft_test_none",
        "device": "/dev/none", "label": "", "fs_type": "ext4", "size": 0, "system": false,
        "uuid": "", "identity": identity})
    };
    let unknown = device(json!({"state": "unknown", "reason": "test"}));
    let err = invoke(&v, "volume_mount", json!({"device": unknown})).unwrap_err();
    assert_eq!(err["kind"], "IdentityUnknown", "{err}");

    let known = device(json!({"state": "known", "major": 8, "minor": 17, "diskseq": 1}));
    let err = invoke(&v, "volume_mount", json!({"device": known})).unwrap_err();
    let kind = err["kind"].as_str().unwrap();
    assert!(
        kind == "UnknownDevice" || NO_UDISKS.contains(&kind),
        "{err}"
    );
}

/// A volume is named by the identity it was listed with, and the identity
/// is required. One that does not match what is mounted at the path (here a
/// folder nothing is mounted on) is refused before UDisks is asked anything,
/// for unmount and eject alike; a call without it is refused as malformed.
#[test]
fn ipc_volume_identity_is_required_and_checked_first() {
    let (e, v) = setup();
    let mount_point = wp(&e.dir("m"));
    let disk = json!({"major": 0, "minor": 0, "diskseq": 1});
    for command in ["volume_unmount", "volume_eject"] {
        let err = invoke(
            &v,
            command,
            json!({"mountPoint": mount_point, "disk": disk}),
        )
        .unwrap_err();
        assert_eq!(err["kind"], "IdentityMismatch", "{command}: {err}");
        for args in [json!({"mountPoint": mount_point}), json!({"disk": disk})] {
            let err = invoke(&v, command, args.clone()).unwrap_err();
            assert_ne!(err["kind"], "IdentityMismatch", "{command} {args}: {err}");
            assert_ne!(err["kind"], "NotMounted", "{command} {args}: {err}");
        }
    }
}

#[test]
fn ipc_search() {
    let (e, v) = setup();
    let dir = e.dir("d");
    fs::write(dir.join("Needle.txt"), "").unwrap();
    let op = invoke(
        &v,
        "search",
        json!({"key": "s", "roots": [wp(&dir)], "query": "needle", "hidden": false}),
    )
    .unwrap();
    let hit = e.wait_event(EVENT_SEARCH_HIT);
    assert_eq!(keys(&hit), ["entries", "key", "op"]);
    assert_eq!(hit["op"], op);
    assert_eq!(hit["entries"][0]["path"], wp(&dir.join("Needle.txt")));
    let done = wait_done(&e, &op);
    assert_eq!(
        done["scan"],
        json!({"found": 1, "unreadable": [], "cancelled": false})
    );
    assert_wire_error(
        invoke(
            &v,
            "search",
            json!({"key": "s", "roots": [wp(&dir)], "query": "", "hidden": false}),
        ),
        "InvalidArgument",
    );
}

#[test]
fn ipc_documents() {
    let (e, v) = setup();
    let dir = e.dir("d");
    fs::write(dir.join("a.md"), "").unwrap();
    fs::write(dir.join("b.png"), "").unwrap();
    let op = invoke(&v, "documents", json!({"key": "docs", "roots": [wp(&dir)]})).unwrap();
    let hit = e.wait_event(EVENT_SEARCH_HIT);
    assert_eq!(hit["key"], "docs");
    assert_eq!(hit["entries"].as_array().unwrap().len(), 1);
    assert_eq!(hit["entries"][0]["mime"], "text/markdown");
    // A root that is not there is reported, not fatal.
    let missing = e.base.join("missing");
    wait_done(&e, &op);
    let op = invoke(
        &v,
        "documents",
        json!({"key": "docs", "roots": [wp(&missing)]}),
    )
    .unwrap();
    assert_eq!(
        wait_done(&e, &op)["scan"]["unreadable"],
        json!([wp(&missing)])
    );
}
