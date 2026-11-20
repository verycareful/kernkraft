// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `apply`, `undo` and `redo` through the engine, on the two-page sample.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;

use common::{failure_kind, ok};
use fixture::{open, DATE};
use kernkraft_jni::engine::Engine;
use serde_json::{json, Value};

fn apply(engine: &Engine, handle: i64, op: Value) -> String {
    engine.apply(&json!({ "handle": handle, "op": op }).to_string())
}

fn step(engine: &Engine, operation: fn(&Engine, &str) -> String, handle: i64) -> Value {
    ok(&operation(engine, &json!({ "handle": handle }).to_string()))
}

fn rotation_of(engine: &Engine, handle: i64, page: u32) -> u64 {
    let info = ok(&engine.page_info(&json!({ "handle": handle, "page": page }).to_string()));
    info["rotate"].as_u64().unwrap()
}

fn annotations(engine: &Engine, handle: i64) -> Vec<Value> {
    let answer = ok(&engine.annotations(&json!({ "handle": handle, "page": 1 }).to_string()));
    answer["annotations"].as_array().unwrap().clone()
}

#[test]
fn a_rotation_reports_the_page_and_undo_and_redo_move_between_the_states() {
    let engine = Engine::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [1], "degrees": 90 }),
    ));
    assert_eq!(done["revision"], 1);
    assert_eq!(done["changed_pages"], json!([1]));
    assert_eq!(done["pages"], 2);
    assert_eq!(done["can_undo"], true);
    assert_eq!(done["can_redo"], false);
    assert_eq!(done["modified"], true);
    assert_eq!(rotation_of(&engine, handle, 1), 90);
    assert_eq!(rotation_of(&engine, handle, 2), 0);

    let undone = step(&engine, Engine::undo, handle);
    assert_eq!(undone["revision"], 2);
    assert_eq!(undone["changed_pages"], json!([1]));
    assert_eq!(undone["modified"], false);
    assert_eq!(undone["can_redo"], true);
    assert_eq!(rotation_of(&engine, handle, 1), 0);

    let redone = step(&engine, Engine::redo, handle);
    assert_eq!(redone["revision"], 3);
    assert_eq!(redone["changed_pages"], json!([1]));
    assert_eq!(rotation_of(&engine, handle, 1), 90);
}

#[test]
fn undo_with_nothing_to_undo_changes_nothing() {
    let engine = Engine::new();
    let handle = open(&engine);

    let undone = step(&engine, Engine::undo, handle);

    assert_eq!(undone["revision"], 0);
    assert_eq!(undone["changed_pages"], json!([]));
    assert_eq!(undone["can_undo"], false);
    assert_eq!(
        step(&engine, Engine::redo, handle)["changed_pages"],
        json!([])
    );
}

#[test]
fn rotating_no_pages_rotates_them_all() {
    let engine = Engine::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [], "degrees": 180 }),
    ));

    assert_eq!(done["changed_pages"], json!([1, 2]));
    assert_eq!(rotation_of(&engine, handle, 2), 180);
}

#[test]
fn a_rotation_that_is_not_a_quarter_turn_is_bad() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [1], "degrees": 45 }),
    );

    assert_eq!(failure_kind(&answer), "BadRequest");
}

#[test]
fn removing_a_page_changes_the_page_tree_and_undo_brings_it_back() {
    let engine = Engine::new();
    let handle = open(&engine);

    let removed = ok(&apply(
        &engine,
        handle,
        json!({ "op": "remove", "pages": [2] }),
    ));
    assert_eq!(removed["pages"], 1);
    assert_eq!(removed["changed_pages"], json!([1]));

    let undone = step(&engine, Engine::undo, handle);
    assert_eq!(undone["pages"], 2);
    assert_eq!(undone["changed_pages"], json!([1, 2]));
}

#[test]
fn removing_every_page_is_refused_and_leaves_the_document() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = apply(&engine, handle, json!({ "op": "remove", "pages": [1, 2] }));

    assert_eq!(failure_kind(&answer), "Invalid");
    let after = step(&engine, Engine::undo, handle);
    assert_eq!(after["pages"], 2);
    assert_eq!(after["revision"], 0);
}

#[test]
fn reordering_reports_every_page() {
    let engine = Engine::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "reorder", "order": [2, 1] }),
    ));

    assert_eq!(done["changed_pages"], json!([1, 2]));
    assert_eq!(done["revision"], 1);
}

#[test]
fn a_new_change_after_an_undo_drops_what_could_be_redone() {
    let engine = Engine::new();
    let handle = open(&engine);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [1], "degrees": 90 }),
    ));
    step(&engine, Engine::undo, handle);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [2], "degrees": 270 }),
    ));

    assert_eq!(done["can_redo"], false);
    assert_eq!(done["changed_pages"], json!([2]));
    assert_eq!(rotation_of(&engine, handle, 1), 0);
    assert_eq!(rotation_of(&engine, handle, 2), 270);
}

#[test]
fn an_op_the_engine_does_not_know_is_bad() {
    let engine = Engine::new();
    let handle = open(&engine);

    assert_eq!(
        failure_kind(&apply(
            &engine,
            handle,
            json!({ "op": "addText", "page": 1 })
        )),
        "BadRequest"
    );
    assert_eq!(
        failure_kind(&apply(&engine, handle, json!({ "pages": [1] }))),
        "BadRequest"
    );
}

#[test]
fn a_highlight_is_listed_with_its_id_and_can_be_recoloured_and_deleted() {
    let engine = Engine::new();
    let handle = open(&engine);

    let added = ok(&apply(
        &engine,
        handle,
        json!({ "op": "addMarkup", "kind": "highlight", "page": 1,
                "quads": [[0.1, 0.1, 0.5, 0.15]], "colour": [1.0, 1.0, 0.0],
                "modified": DATE }),
    ));
    assert_eq!(added["changed_pages"], json!([1]));
    let id = added["annotation"].clone();

    let listed = annotations(&engine, handle);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["kind"], "Highlight");
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["colour"], json!([1.0, 1.0, 0.0]));

    ok(&apply(
        &engine,
        handle,
        json!({ "op": "editAnnotation", "page": 1, "id": id, "colour": [0.0, 1.0, 0.0],
                "modified": DATE }),
    ));
    assert_eq!(
        annotations(&engine, handle)[0]["colour"],
        json!([0.0, 1.0, 0.0])
    );

    ok(&apply(
        &engine,
        handle,
        json!({ "op": "deleteAnnotation", "page": 1, "id": id }),
    ));
    assert!(annotations(&engine, handle).is_empty());

    step(&engine, Engine::undo, handle);
    assert_eq!(annotations(&engine, handle).len(), 1);
}

#[test]
fn ink_and_a_note_are_listed_by_kind() {
    let engine = Engine::new();
    let handle = open(&engine);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "addInk", "page": 1, "strokes": [[[0.1, 0.1], [0.2, 0.2]]],
                "width": 2.0, "colour": [1.0, 0.0, 0.0], "modified": DATE }),
    ));
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "addNote", "page": 1, "at": [0.5, 0.5], "text": "check this",
                "colour": [1.0, 0.8, 0.0], "modified": DATE }),
    ));

    let listed = annotations(&engine, handle);

    let kinds: Vec<&str> = listed.iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["Ink", "Note"]);
    assert_eq!(listed[1]["contents"], "check this");
    assert_eq!(listed[0]["line_width"], 2.0);
}

#[test]
fn an_annotation_dated_wrongly_is_bad_and_adds_nothing() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "addNote", "page": 1, "at": [0.5, 0.5], "text": "x",
                "colour": [0.0, 0.0, 0.0], "modified": "yesterday" }),
    );

    assert_eq!(failure_kind(&answer), "BadRequest");
    assert!(annotations(&engine, handle).is_empty());
}

#[test]
fn deleting_an_annotation_that_is_not_there_is_refused() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "deleteAnnotation", "page": 1, "id": { "object": 9999, "generation": 0 } }),
    );

    assert_eq!(failure_kind(&answer), "AnnotationNotOnPage");
}

fn text_of(engine: &Engine, handle: i64) -> (u64, String) {
    let text = ok(&engine.text(&json!({ "handle": handle, "page": 1 }).to_string()));
    let all: Vec<&str> = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap())
        .collect();
    (text["revision"].as_u64().unwrap(), all.join("\n"))
}

fn heading_block(engine: &Engine, handle: i64) -> u64 {
    let text = ok(&engine.text(&json!({ "handle": handle, "page": 1 }).to_string()));
    let line = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["text"] == "Kern -- Project Architecture Brief")
        .unwrap();
    line["block"].as_u64().unwrap()
}

#[test]
fn editing_a_block_replaces_its_text() {
    let engine = Engine::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "Kern Brief" }),
    ));

    assert_eq!(done["changed_pages"], json!([1]));
    assert_eq!(done["lines"], 1);
    let (_, text) = text_of(&engine, handle);
    assert!(text.contains("Kern Brief"), "{text}");
    assert!(!text.contains("Project Architecture"), "{text}");
}

#[test]
fn a_block_read_before_a_change_is_stale() {
    let engine = Engine::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [2], "degrees": 90 }),
    ));

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision, "text": "X" }),
    );

    assert_eq!(failure_kind(&answer), "StaleBlock");
    assert!(text_of(&engine, handle).1.contains("Project Architecture"));
}

#[test]
fn a_block_past_the_last_is_bad() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": 9999, "revision": 0, "text": "X" }),
    );

    assert_eq!(failure_kind(&answer), "BadRequest");
}

#[test]
fn an_empty_text_removes_the_blocks_text() {
    let engine = Engine::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);

    ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": 0, "text": "" }),
    ));

    assert!(!text_of(&engine, handle).1.contains("Project Architecture"));
}
