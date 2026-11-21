// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `appendSince`, `confirmAppended` and `compact` through the engine. What
//! they write is read back with `kernkraft-pdf`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;

use std::fs;
use std::path::Path;

use common::{failure_kind, ok};
use fixture::{open, pages_of, sample, sample_length, DATE};
use kernkraft_jni::bridge::Bridge;
use serde_json::{json, Value};

fn rotate_first_page(engine: &Bridge, handle: i64) {
    let request = json!({ "handle": handle,
                          "op": { "op": "rotate", "pages": [1], "degrees": 90 } });
    ok(&engine.apply(&request.to_string()));
}

fn append(engine: &Bridge, handle: i64, base_length: u64, out: &Path) -> String {
    let request = json!({ "handle": handle, "base_length": base_length,
                          "out": out, "modified": DATE });
    engine.append_since(&request.to_string())
}

fn rotation_in(bytes: Vec<u8>) -> u16 {
    let options = kernkraft_jni::open::OPEN_OPTIONS;
    let document = kernkraft_pdf::doc::Document::from_bytes(bytes, &options, "").unwrap();
    document.page_geometry(1).unwrap().rotate
}

#[test]
fn the_appended_bytes_after_the_original_are_the_edited_file() {
    let engine = Bridge::new();
    let handle = open(&engine);
    rotate_first_page(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("update.bin");

    let answer = ok(&append(&engine, handle, sample_length(), &out));

    let update = fs::read(&out).unwrap();
    assert_eq!(answer["appended"], update.len());
    assert_eq!(answer["length"], sample_length() + update.len() as u64);
    let mut whole = fs::read(sample()).unwrap();
    whole.extend_from_slice(&update);
    assert_eq!(rotation_in(whole), 90);
}

#[test]
fn the_document_counts_as_saved_only_once_the_append_is_confirmed() {
    let engine = Bridge::new();
    let handle = open(&engine);
    rotate_first_page(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let answer = ok(&append(
        &engine,
        handle,
        sample_length(),
        &dir.path().join("u.bin"),
    ));

    let undone = ok(&engine.undo(&json!({ "handle": handle }).to_string()));
    let redone = ok(&engine.redo(&json!({ "handle": handle }).to_string()));
    assert_eq!(
        undone["modified"], false,
        "undone, it is the file as opened"
    );
    assert_eq!(redone["modified"], true, "the append is not confirmed");

    let confirmed = ok(&engine
        .confirm_appended(&json!({ "handle": handle, "length": answer["length"] }).to_string()));
    assert_eq!(confirmed["modified"], false);
    let after = ok(&engine.undo(&json!({ "handle": handle }).to_string()));
    assert_eq!(after["modified"], true, "the file now holds the rotation");
}

#[test]
fn a_second_append_holds_only_what_came_after_the_first() {
    let engine = Bridge::new();
    let handle = open(&engine);
    rotate_first_page(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let first = ok(&append(
        &engine,
        handle,
        sample_length(),
        &dir.path().join("one.bin"),
    ));
    ok(&engine
        .confirm_appended(&json!({ "handle": handle, "length": first["length"] }).to_string()));
    ok(&engine.apply(
        &json!({ "handle": handle, "op": { "op": "rotate", "pages": [2], "degrees": 180 } })
            .to_string(),
    ));

    let second = ok(&append(
        &engine,
        handle,
        first["length"].as_u64().unwrap(),
        &dir.path().join("two.bin"),
    ));

    let mut whole = fs::read(sample()).unwrap();
    whole.extend(fs::read(dir.path().join("one.bin")).unwrap());
    whole.extend(fs::read(dir.path().join("two.bin")).unwrap());
    assert_eq!(second["length"], whole.len());
    assert_eq!(rotation_in(whole), 90);
}

#[test]
fn an_append_from_a_length_the_file_never_had_is_refused() {
    let engine = Bridge::new();
    let handle = open(&engine);
    rotate_first_page(&engine, handle);
    let dir = tempfile::tempdir().unwrap();

    let answer = append(&engine, handle, 12345, &dir.path().join("u.bin"));

    assert_eq!(failure_kind(&answer), "UnknownAppendPoint");
    assert!(!dir.path().join("u.bin").exists());
}

#[test]
fn with_nothing_changed_nothing_is_appended_and_no_file_is_made() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("u.bin");

    let answer = ok(&append(&engine, handle, sample_length(), &out));

    assert_eq!(answer["appended"], 0);
    assert_eq!(answer["length"], sample_length());
    assert!(!out.exists());
}

#[test]
fn an_append_never_replaces_a_file_that_is_there() {
    let engine = Bridge::new();
    let handle = open(&engine);
    rotate_first_page(&engine, handle);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("u.bin");
    fs::write(&out, b"the user's file").unwrap();

    let answer = append(&engine, handle, sample_length(), &out);

    assert_eq!(failure_kind(&answer), "Exists");
    assert_eq!(common::parse(&answer)["path"], out.to_str().unwrap());
    assert_eq!(fs::read(&out).unwrap(), b"the user's file");
}

#[test]
fn a_compacted_copy_holds_the_edits_and_leaves_the_original_alone() {
    let engine = Bridge::new();
    let handle = open(&engine);
    rotate_first_page(&engine, handle);
    ok(&engine
        .apply(&json!({ "handle": handle, "op": { "op": "remove", "pages": [2] } }).to_string()));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("copy.pdf");
    let original = fs::read(sample()).unwrap();

    let answer: Value = ok(&engine.compact(&json!({ "handle": handle, "out": out }).to_string()));

    assert_eq!(answer["path"], json!(out));
    assert_eq!(pages_of(&out), 1);
    assert_eq!(rotation_in(fs::read(&out).unwrap()), 90);
    assert_eq!(fs::read(sample()).unwrap(), original);
}

#[test]
fn a_compacted_copy_never_replaces_a_file_that_is_there() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("copy.pdf");
    fs::write(&out, b"the user's file").unwrap();

    let answer = engine.compact(&json!({ "handle": handle, "out": out }).to_string());

    assert_eq!(failure_kind(&answer), "Exists");
    assert_eq!(fs::read(&out).unwrap(), b"the user's file");
}
