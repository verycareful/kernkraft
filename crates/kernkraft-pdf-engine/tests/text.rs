// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The extractor and the pages of text an [`Engine`] keeps for a session: one
//! per text epoch, shared by the reads of every revision of that epoch. A
//! step that writes only annotations leaves the epoch as it was; any other
//! step, an undo or a redo across it included, gives the next read a new one.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::OpenOptions;
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::text::TextExtractor;
use kernkraft_pdf_engine::edit::{ApplyRequest, Op};
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::read::{SearchRequest, TextRequest};
use kernkraft_pdf_engine::{Engine, HandleRequest};
use serde_json::{json, Value};

const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 1 << 20,
        max_xref_entries: 1 << 24,
        max_object_stream_bytes: 1 << 26,
    },
    cached_objects: 64,
    cached_object_streams: 4,
    max_diagnostics: 100,
};

fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf")
}

fn open(engine: &Engine) -> i64 {
    let source = Source {
        path: sample(),
        password: String::new(),
    };
    engine.open(source).unwrap()["handle"].as_i64().unwrap()
}

fn rotate(engine: &Engine, handle: i64) {
    engine
        .apply(ApplyRequest {
            handle,
            op: Op::Rotate {
                pages: vec![1],
                degrees: 90,
            },
            job: None,
        })
        .unwrap();
}

#[test]
fn reads_of_one_revision_share_one_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (_, revision, first) = engine.text_extractor(handle).unwrap();
    engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    engine
        .search(SearchRequest {
            handle,
            page: 1,
            query: "a".to_owned(),
            case_sensitive: false,
            whole_word: false,
            job: None,
        })
        .unwrap();
    let (_, again, second) = engine.text_extractor(handle).unwrap();
    assert_eq!(revision, again);
    assert!(Arc::ptr_eq(&first, &second));
}

#[test]
fn a_step_gives_the_next_read_a_new_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (_, before, first) = engine.text_extractor(handle).unwrap();
    rotate(&engine, handle);
    let (_, after, second) = engine.text_extractor(handle).unwrap();
    assert_eq!(after, before + 1);
    assert!(!Arc::ptr_eq(&first, &second));
}

#[test]
fn an_undo_and_a_redo_each_give_a_new_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    rotate(&engine, handle);
    let (_, _, edited) = engine.text_extractor(handle).unwrap();
    engine.undo(HandleRequest { handle }).unwrap();
    let (_, undone_revision, undone) = engine.text_extractor(handle).unwrap();
    assert!(!Arc::ptr_eq(&edited, &undone));
    engine.redo(HandleRequest { handle }).unwrap();
    let (_, redone_revision, redone) = engine.text_extractor(handle).unwrap();
    assert!(redone_revision > undone_revision);
    assert!(!Arc::ptr_eq(&undone, &redone));
}

#[test]
fn two_sessions_do_not_share_an_extractor() {
    let engine = Engine::new(OPTIONS);
    let one = open(&engine);
    let other = open(&engine);
    let (_, _, a) = engine.text_extractor(one).unwrap();
    let (_, _, b) = engine.text_extractor(other).unwrap();
    assert!(!Arc::ptr_eq(&a, &b));
}

#[test]
fn the_text_read_through_the_kept_extractor_is_the_same_each_time() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let first = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    let second = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    assert_eq!(first, second);
    assert!(!first["lines"].as_array().unwrap().is_empty());
}

#[test]
fn an_unknown_handle_is_refused_before_anything_is_built() {
    let engine = Engine::new(OPTIONS);
    let error = engine.text_extractor(99).err().unwrap();
    assert_eq!(error.kind, "UnknownHandle");
}

const DATE: &str = "D:20261126120000Z";

/// Applies the change `op`, a request's `op` object, and gives the answer.
fn apply(engine: &Engine, handle: i64, op: Value) -> Value {
    let request: ApplyRequest = serde_json::from_value(json!({ "handle": handle, "op": op }))
        .expect("the request is well formed");
    Value::Object(engine.apply(request).unwrap())
}

fn ink(engine: &Engine, handle: i64) -> Value {
    apply(
        engine,
        handle,
        json!({
            "op": "addInk", "page": 1, "width": 3.0, "colour": [1.0, 0.0, 0.0],
            "strokes": [[[0.1, 0.1], [0.3, 0.2], [0.5, 0.1]]], "modified": DATE
        }),
    )
}

fn undo(engine: &Engine, handle: i64) {
    engine.undo(HandleRequest { handle }).unwrap();
}

fn redo(engine: &Engine, handle: i64) {
    engine.redo(HandleRequest { handle }).unwrap();
}

fn extractor(engine: &Engine, handle: i64) -> kernkraft_pdf_engine::engine::SharedExtractor {
    engine.text_extractor(handle).unwrap().2
}

/// The text of page 1 as a new extractor reads it from the document the
/// session stands on: what every kept read has to equal.
fn fresh_text(engine: &Engine, handle: i64) -> kernkraft_pdf::text::TextPage {
    let (document, _) = engine.snapshot(handle).unwrap();
    TextExtractor::new(&document)
        .unwrap()
        .page(1, &NoProgress)
        .unwrap()
}

#[test]
fn each_step_that_writes_only_annotations_keeps_the_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let first = extractor(&engine, handle);
    let mut revision = engine.snapshot(handle).unwrap().1;

    let added = ink(&engine, handle);
    let id = added["annotation"].clone();
    let steps = [
        added,
        apply(
            &engine,
            handle,
            json!({
                "op": "addMarkup", "kind": "highlight", "page": 1,
                "quads": [[0.1, 0.1, 0.4, 0.15]], "colour": [1.0, 1.0, 0.0], "modified": DATE
            }),
        ),
        apply(
            &engine,
            handle,
            json!({
                "op": "addNote", "page": 1, "at": [0.5, 0.5], "text": "a note",
                "colour": [1.0, 0.8, 0.0], "modified": DATE
            }),
        ),
        apply(
            &engine,
            handle,
            json!({
                "op": "editAnnotation", "page": 1, "id": id, "colour": [0.0, 0.0, 1.0],
                "modified": DATE
            }),
        ),
        apply(
            &engine,
            handle,
            json!({ "op": "deleteAnnotation", "page": 1, "id": id }),
        ),
    ];
    for step in steps {
        revision += 1;
        assert_eq!(step["revision"], revision);
        assert!(Arc::ptr_eq(&first, &extractor(&engine, handle)));
    }
    assert_eq!(engine.snapshot(handle).unwrap().1, revision);
}

#[test]
fn the_text_read_after_an_annotation_equals_that_of_a_new_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let before = engine.text_page(handle, 1, &NoProgress).unwrap().2;
    ink(&engine, handle);
    let (_, _, after) = engine.text_page(handle, 1, &NoProgress).unwrap();
    assert_eq!(*after, fresh_text(&engine, handle));
    assert_eq!(*after, *before);
    assert!(!after.blocks.is_empty());
}

#[test]
fn a_step_that_changes_content_gives_a_new_extractor_whatever_came_before() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let first = extractor(&engine, handle);
    ink(&engine, handle);
    rotate(&engine, handle);
    let rotated = extractor(&engine, handle);
    assert!(!Arc::ptr_eq(&first, &rotated));
    ink(&engine, handle);
    assert!(Arc::ptr_eq(&rotated, &extractor(&engine, handle)));
}

#[test]
fn an_undo_and_a_redo_of_an_annotation_keep_the_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let first = extractor(&engine, handle);
    ink(&engine, handle);
    undo(&engine, handle);
    assert!(Arc::ptr_eq(&first, &extractor(&engine, handle)));
    redo(&engine, handle);
    assert!(Arc::ptr_eq(&first, &extractor(&engine, handle)));
}

#[test]
fn an_undo_across_a_content_step_gives_a_new_extractor() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    ink(&engine, handle);
    rotate(&engine, handle);
    let rotated = extractor(&engine, handle);
    undo(&engine, handle);
    let inked = extractor(&engine, handle);
    assert!(!Arc::ptr_eq(&rotated, &inked));
    undo(&engine, handle);
    assert!(Arc::ptr_eq(&inked, &extractor(&engine, handle)));
}

#[test]
fn a_step_made_after_an_undo_does_not_take_the_epoch_of_the_one_undone() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    rotate(&engine, handle);
    let first_rotation = extractor(&engine, handle);
    undo(&engine, handle);
    rotate(&engine, handle);
    assert!(!Arc::ptr_eq(&first_rotation, &extractor(&engine, handle)));
}

#[test]
fn a_block_read_after_an_annotation_is_edited_at_the_revision_it_was_read_at() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    ink(&engine, handle);
    let text = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    let revision = text["revision"].as_u64().unwrap();
    assert_eq!(revision, engine.snapshot(handle).unwrap().1);
    let block = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["text"] == "Kern -- Project Architecture Brief")
        .unwrap()["block"]
        .as_u64()
        .unwrap();
    let answer = apply(
        &engine,
        handle,
        json!({
            "op": "editText", "page": 1, "block": block, "revision": revision,
            "text": "Kern: Project Architecture Brief"
        }),
    );
    assert_eq!(answer["revision"], revision + 1);
    let edited = engine
        .text(TextRequest {
            handle,
            page: 1,
            job: None,
        })
        .unwrap();
    assert!(edited["lines"]
        .as_array()
        .unwrap()
        .iter()
        .any(|line| line["text"] == "Kern: Project Architecture Brief"));
}

#[test]
fn a_page_is_read_once_for_the_reads_of_one_epoch() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (_, first_revision, first) = engine.text_page(handle, 1, &NoProgress).unwrap();
    let (_, _, again) = engine.text_page(handle, 1, &NoProgress).unwrap();
    assert!(Arc::ptr_eq(&first, &again));

    ink(&engine, handle);
    let (_, revision, after_ink) = engine.text_page(handle, 1, &NoProgress).unwrap();
    assert_eq!(revision, first_revision + 1);
    assert!(Arc::ptr_eq(&first, &after_ink));

    let (_, _, other) = engine.text_page(handle, 2, &NoProgress).unwrap();
    assert!(!Arc::ptr_eq(&first, &other));
    assert_eq!(other.page, 2);

    rotate(&engine, handle);
    let (_, _, after_rotation) = engine.text_page(handle, 1, &NoProgress).unwrap();
    assert!(!Arc::ptr_eq(&first, &after_rotation));
}

#[test]
fn an_undo_back_to_an_epoch_finds_its_pages_still_kept() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (_, _, first) = engine.text_page(handle, 1, &NoProgress).unwrap();
    rotate(&engine, handle);
    let (_, _, rotated) = engine.text_page(handle, 1, &NoProgress).unwrap();
    assert!(!Arc::ptr_eq(&first, &rotated));
    undo(&engine, handle);
    let (_, _, back) = engine.text_page(handle, 1, &NoProgress).unwrap();
    assert!(Arc::ptr_eq(&first, &back));
}

#[test]
fn the_answer_of_text_is_the_same_before_and_after_an_annotation_but_for_its_revision() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let read = |engine: &Engine| {
        engine
            .text(TextRequest {
                handle,
                page: 1,
                job: None,
            })
            .unwrap()
    };
    let before = read(&engine);
    ink(&engine, handle);
    let mut after = read(&engine);
    assert_eq!(after["revision"], before["revision"].as_u64().unwrap() + 1);
    after.insert("revision".to_owned(), before["revision"].clone());
    assert_eq!(before, after);
}
