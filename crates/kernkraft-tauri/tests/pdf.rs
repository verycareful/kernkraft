// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! PDF documents through the plugin: its methods, and the commands as the
//! frontend calls them.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use common::{env, env_with_permissions, invoke, invoke_raw, path as to_path, w, Env};
use kernkraft_tauri::{WireExpect, WirePdfSource, WireStamp};
use serde_json::{json, Value};

/// The sample PDF kernkraft may redistribute: two pages, upright.
fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf")
}

/// A time to stamp edits and saves with: a PDF date.
const DATE: &str = "D:20261121103000Z";

fn source(path: &Path) -> WirePdfSource {
    WirePdfSource {
        path: w(path),
        password: None,
    }
}

/// A copy of the sample the test may replace.
fn work_copy(e: &Env) -> PathBuf {
    let path = e.dir("pdf").join("work.pdf");
    fs::copy(sample(), &path).unwrap();
    path
}

fn handle_of(opened: &serde_json::Map<String, Value>) -> i64 {
    opened["handle"].as_i64().unwrap()
}

fn stamp_of(opened: &serde_json::Map<String, Value>) -> WireStamp {
    serde_json::from_value(opened["stamp"].clone()).unwrap()
}

fn rotate(e: &Env, handle: i64, page: u32) {
    let op =
        serde_json::from_value(json!({ "op": "rotate", "pages": [page], "degrees": 90 })).unwrap();
    e.kk.pdf_apply(handle, op).unwrap();
}

fn rotation_of(e: &Env, path: &Path, page: u32) -> i64 {
    let opened = e.kk.pdf_open(source(path), None).unwrap();
    let info = e.kk.pdf_page_info(handle_of(&opened), page).unwrap();
    e.kk.pdf_close(handle_of(&opened)).unwrap();
    info["rotate"].as_i64().unwrap()
}

#[test]
fn open_answers_the_handle_the_pages_and_the_stamp_of_the_file() {
    let e = env();
    let path = work_copy(&e);

    let opened = e.kk.pdf_open(source(&path), None).unwrap();

    assert_eq!(opened["pages"], 2);
    assert_eq!(opened["revision"], 0);
    assert!(handle_of(&opened) > 0);
    assert_eq!(
        e.kk.text_check(w(&path), stamp_of(&opened)).unwrap(),
        "same"
    );
}

#[test]
fn a_save_writes_the_opened_bytes_and_an_update_and_answers_the_new_stamp() {
    let e = env();
    let path = work_copy(&e);
    let original = fs::read(&path).unwrap();
    let opened = e.kk.pdf_open(source(&path), None).unwrap();
    let handle = handle_of(&opened);
    rotate(&e, handle, 1);

    let saved =
        e.kk.pdf_save(
            handle,
            w(&path),
            WireExpect::Unchanged {
                stamp: stamp_of(&opened),
            },
            DATE.to_owned(),
        )
        .unwrap();

    let written = fs::read(&path).unwrap();
    assert!(written.starts_with(&original));
    assert!(written.len() > original.len());
    assert_eq!(rotation_of(&e, &path, 1), 90);
    assert_eq!(rotation_of(&e, &path, 2), 0);
    assert_eq!(e.kk.text_check(w(&path), saved).unwrap(), "same");
}

#[test]
fn a_file_changed_since_it_was_opened_is_not_replaced() {
    let e = env();
    let path = work_copy(&e);
    let opened = e.kk.pdf_open(source(&path), None).unwrap();
    let handle = handle_of(&opened);
    rotate(&e, handle, 1);
    let mut changed = fs::read(&path).unwrap();
    changed.push(b'\n');
    fs::write(&path, &changed).unwrap();

    let refused =
        e.kk.pdf_save(
            handle,
            w(&path),
            WireExpect::Unchanged {
                stamp: stamp_of(&opened),
            },
            DATE.to_owned(),
        )
        .unwrap_err();

    assert_eq!(refused.kind, "ExternalModification");
    assert_eq!(fs::read(&path).unwrap(), changed);
}

#[test]
fn a_save_as_does_not_replace_a_file_and_leaves_the_opened_one() {
    let e = env();
    let path = work_copy(&e);
    let original = fs::read(&path).unwrap();
    let handle = handle_of(&e.kk.pdf_open(source(&path), None).unwrap());
    rotate(&e, handle, 2);
    let taken = e.dir("pdf").join("taken.pdf");
    fs::write(&taken, b"mine").unwrap();

    let refused =
        e.kk.pdf_save(handle, w(&taken), WireExpect::Nothing, DATE.to_owned())
            .unwrap_err();
    let copy = e.dir("pdf").join("copy.pdf");
    e.kk.pdf_save(handle, w(&copy), WireExpect::Nothing, DATE.to_owned())
        .unwrap();

    assert_eq!(refused.kind, "Exists");
    assert_eq!(fs::read(&taken).unwrap(), b"mine");
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(rotation_of(&e, &copy, 2), 90);
}

#[test]
fn a_stamp_that_does_not_decode_is_refused_before_anything_is_written() {
    let e = env();
    let path = work_copy(&e);
    let original = fs::read(&path).unwrap();
    let handle = handle_of(&e.kk.pdf_open(source(&path), None).unwrap());
    rotate(&e, handle, 1);
    let bad: WireStamp = serde_json::from_value(json!("not a stamp")).unwrap();

    let refused =
        e.kk.pdf_save(
            handle,
            w(&path),
            WireExpect::Unchanged { stamp: bad },
            DATE.to_owned(),
        )
        .unwrap_err();

    assert_eq!(refused.kind, "InvalidArgument");
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn a_closed_document_is_unknown() {
    let e = env();
    let path = work_copy(&e);
    let handle = handle_of(&e.kk.pdf_open(source(&path), None).unwrap());

    e.kk.pdf_close(handle).unwrap();

    assert_eq!(
        e.kk.pdf_page_info(handle, 1).unwrap_err().kind,
        "UnknownHandle"
    );
    assert_eq!(e.kk.pdf_close(handle).unwrap_err().kind, "UnknownHandle");
}

#[test]
fn a_file_that_is_not_a_pdf_or_is_missing_is_refused_with_its_kind() {
    let e = env();
    let text = e.dir("pdf").join("notes.pdf");
    fs::write(&text, b"just text").unwrap();

    let not_pdf = e.kk.pdf_open(source(&text), None).unwrap_err();
    let missing =
        e.kk.pdf_open(source(&e.dir("pdf").join("none.pdf")), None)
            .unwrap_err();

    assert_eq!(not_pdf.kind, "MissingHeader");
    assert_eq!(missing.kind, "NotFound");
}

#[test]
fn a_file_name_that_is_not_utf8_opens_and_saves_over_itself() {
    let e = env();
    let path = e.dir("pdf").join(OsStr::from_bytes(b"caf\xE9.pdf"));
    fs::copy(sample(), &path).unwrap();
    let opened = e.kk.pdf_open(source(&path), None).unwrap();
    let handle = handle_of(&opened);
    rotate(&e, handle, 1);

    e.kk.pdf_save(
        handle,
        w(&path),
        WireExpect::Unchanged {
            stamp: stamp_of(&opened),
        },
        DATE.to_owned(),
    )
    .unwrap();

    assert_eq!(rotation_of(&e, &path, 1), 90);
}

#[test]
fn undo_and_redo_move_between_the_states_without_writing() {
    let e = env();
    let path = work_copy(&e);
    let original = fs::read(&path).unwrap();
    let handle = handle_of(&e.kk.pdf_open(source(&path), None).unwrap());
    rotate(&e, handle, 1);

    let undone = e.kk.pdf_undo(handle).unwrap();
    assert_eq!(undone["can_redo"], true);
    assert_eq!(undone["can_undo"], false);
    assert_eq!(undone["modified"], false);
    let redone = e.kk.pdf_redo(handle).unwrap();

    assert_eq!(redone["modified"], true);
    assert_eq!(redone["changed_pages"], json!([1]));
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn a_page_is_drawn_to_the_size_it_was_asked_for_on_a_white_page() {
    let e = env();
    let path = work_copy(&e);
    let handle = handle_of(&e.kk.pdf_open(source(&path), None).unwrap());

    let size = e.kk.pdf_render_size(handle, 1, None, 200).unwrap();
    let picture = e.kk.pdf_render(handle, 1, None, 200).unwrap();

    assert_eq!(size["width"], 200);
    let (width, height) = (
        size["width"].as_u64().unwrap(),
        size["height"].as_u64().unwrap(),
    );
    assert_eq!(size["bytes"], width * height * 4);
    assert_eq!(picture.len() as u64, width * height * 4);
    // A page is drawn on an opaque white sheet: every alpha is 255, and
    // the corner of a page with margins is white.
    assert!(picture.chunks(4).all(|pixel| pixel[3] == 255));
    assert_eq!(&picture[..4], &[255, 255, 255, 255]);
}

#[test]
fn the_picture_of_a_page_that_does_not_exist_is_refused() {
    let e = env();
    let path = work_copy(&e);
    let handle = handle_of(&e.kk.pdf_open(source(&path), None).unwrap());

    let refused = e.kk.pdf_render(handle, 9, None, 200).unwrap_err();

    assert_eq!(refused.kind, "PageOutOfRange");
}

#[test]
fn merge_extract_and_split_make_new_files_and_never_replace_one() {
    let e = env();
    let path = work_copy(&e);
    let dir = e.dir("out");

    let merged =
        e.kk.pdf_merge(
            vec![source(&path), source(&path)],
            w(&dir.join("merged.pdf")),
        )
        .unwrap();
    let extracted =
        e.kk.pdf_extract(source(&path), vec![2], w(&dir.join("page2.pdf")))
            .unwrap();
    let split =
        e.kk.pdf_split(source(&path), "1,2".to_owned(), w(&dir))
            .unwrap();

    let pages = |file: &Path| {
        let opened = e.kk.pdf_open(source(file), None).unwrap();
        let count = opened["pages"].as_u64().unwrap();
        e.kk.pdf_close(handle_of(&opened)).unwrap();
        count
    };
    assert_eq!(pages(&to_path(&merged)), 4);
    assert_eq!(pages(&to_path(&extracted)), 1);
    assert_eq!(
        split.iter().map(to_path).collect::<Vec<_>>(),
        vec![dir.join("work_1.pdf"), dir.join("work_2.pdf")]
    );
    let again =
        e.kk.pdf_extract(source(&path), vec![1], w(&dir.join("page2.pdf")))
            .unwrap_err();
    assert_eq!(again.kind, "Exists");
}

#[test]
fn the_font_folders_are_searched_by_extension_and_what_cannot_be_read_is_counted() {
    let e = env();
    let fonts = e.xdg.data.join("fonts");
    fs::create_dir_all(fonts.join("sub")).unwrap();
    fs::write(fonts.join("a.ttf"), b"not a font").unwrap();
    fs::write(fonts.join("sub/b.OTF"), b"not a font").unwrap();
    fs::write(fonts.join("notes.txt"), b"not a font file at all").unwrap();
    let locked = fonts.join("locked");
    fs::create_dir(&locked).unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o000)).unwrap();
    let unreadable = fs::read_dir(&locked).is_err();

    let answer = e.kk.pdf_set_system_fonts().unwrap();
    fs::set_permissions(&locked, fs::Permissions::from_mode(0o755)).unwrap();

    assert_eq!(answer["faces"], 0);
    // Two files with a font's extension, neither a font: skipped and counted.
    assert_eq!(answer["skipped"], 2);
    assert_eq!(answer["unreadable_dirs"], u64::from(unreadable));
}

#[test]
fn the_commands_take_their_arguments_as_the_frontend_writes_them() {
    let e = env();
    let view = e.webview("pdf");
    let file = work_copy(&e);
    let wp = |p: &Path| serde_json::to_value(w(p)).unwrap();

    let opened = invoke(&view, "pdf_open", json!({ "path": wp(&file) })).unwrap();
    let handle = opened["handle"].as_i64().unwrap();
    assert_eq!(opened["ok"], Value::Null);
    let info = invoke(
        &view,
        "pdf_page_info",
        json!({ "handle": handle, "page": 1 }),
    )
    .unwrap();
    assert_eq!(info["rotate"], 0);
    let applied = invoke(
        &view,
        "pdf_apply",
        json!({ "handle": handle, "op": { "op": "rotate", "pages": [1], "degrees": 90 } }),
    )
    .unwrap();
    assert_eq!(applied["changed_pages"], json!([1]));
    let saved = invoke(
        &view,
        "pdf_save",
        json!({ "handle": handle, "path": wp(&file),
                "expect": { "kind": "unchanged", "stamp": opened["stamp"] },
                "modified": DATE }),
    )
    .unwrap();
    assert!(saved.is_string());
    let size = invoke(
        &view,
        "pdf_render_size",
        json!({ "handle": handle, "page": 1, "widthPx": 120 }),
    )
    .unwrap();
    let picture = invoke_raw(
        &view,
        "pdf_render",
        json!({ "handle": handle, "page": 1, "region": [0.0, 0.0, 1.0, 1.0], "widthPx": 120 }),
    )
    .unwrap();
    assert_eq!(picture.len() as u64, size["bytes"].as_u64().unwrap());
    let found = invoke(
        &view,
        "pdf_search",
        json!({ "handle": handle, "page": 1, "query": "zzzzqq" }),
    )
    .unwrap();
    assert_eq!(found["matches"], json!([]));
    for command in ["pdf_text", "pdf_links", "pdf_annotations"] {
        let answer = invoke(&view, command, json!({ "handle": handle, "page": 1 })).unwrap();
        assert!(answer.is_object(), "{command}: {answer}");
    }
    invoke(&view, "pdf_undo", json!({ "handle": handle })).unwrap();
    invoke(&view, "pdf_redo", json!({ "handle": handle })).unwrap();
    invoke(&view, "pdf_close", json!({ "handle": handle })).unwrap();
}

#[test]
fn a_failure_is_a_wire_error_with_the_name_of_its_variant() {
    let e = env();
    let view = e.webview("pdf");

    let err = invoke(&view, "pdf_page_info", json!({ "handle": 99, "page": 1 })).unwrap_err();

    let mut keys: Vec<&str> = err
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    keys.sort_unstable();
    assert_eq!(keys, ["details", "kind", "message", "path"]);
    assert_eq!(err["kind"], "UnknownHandle");
}

#[test]
fn an_edit_the_engine_does_not_know_is_refused() {
    let e = env();
    let view = e.webview("pdf");
    let file = work_copy(&e);
    let opened = invoke(
        &view,
        "pdf_open",
        json!({ "path": serde_json::to_value(w(&file)).unwrap() }),
    )
    .unwrap();

    let refused = invoke(
        &view,
        "pdf_apply",
        json!({ "handle": opened["handle"], "op": { "op": "shred" } }),
    );

    assert!(refused.is_err(), "{refused:?}");
}

#[test]
fn a_pdf_command_is_granted_by_its_own_permission() {
    let e = env_with_permissions(&["kernkraft-tauri:allow-pdf-close"]);
    let view = e.webview("pdf");

    // Allowed: it reaches the plugin, which does not know the handle.
    let allowed = invoke(&view, "pdf_close", json!({ "handle": 1 })).unwrap_err();
    let refused = invoke(
        &view,
        "pdf_open",
        json!({ "path": serde_json::to_value(w(&sample())).unwrap() }),
    )
    .unwrap_err();

    assert_eq!(allowed["kind"], "UnknownHandle");
    assert!(
        refused.as_str().unwrap().contains("not allowed"),
        "{refused}"
    );
}
