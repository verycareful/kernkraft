// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The box of an `editText` request: `left` and `width`, fractions of the
//! displayed page's width, as `addText`'s `at` and `width` are.

use std::path::{Path, PathBuf};

use kernkraft_pdf::doc::OpenOptions;
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf_engine::edit::{ApplyRequest, Op};
use kernkraft_pdf_engine::open::Source;
use kernkraft_pdf_engine::read::TextRequest;
use kernkraft_pdf_engine::Engine;
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

fn read(engine: &Engine, handle: i64) -> Value {
    Value::Object(
        engine
            .text(TextRequest {
                handle,
                page: 1,
                job: None,
            })
            .unwrap(),
    )
}

/// The block of the sample's heading and the revision it was read at.
fn heading(engine: &Engine, handle: i64) -> (u64, u64) {
    let text = read(engine, handle);
    let line = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["text"] == "Kern -- Project Architecture Brief")
        .unwrap();
    (
        line["block"].as_u64().unwrap(),
        text["revision"].as_u64().unwrap(),
    )
}

fn request(handle: i64, block: u64, revision: u64, text: &str, extra: Value) -> ApplyRequest {
    let mut op = json!({
        "op": "editText", "page": 1, "block": block, "revision": revision, "text": text
    });
    op.as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    serde_json::from_value(json!({ "handle": handle, "op": op })).unwrap()
}

#[test]
fn the_op_takes_a_left_and_a_width() {
    let parsed: ApplyRequest = serde_json::from_str(
        r#"{"handle":1,"op":{"op":"editText","page":1,"block":3,"revision":12,
            "text":"x","left":0.1,"width":0.5}}"#,
    )
    .unwrap();
    let Op::EditText { left, width, .. } = parsed.op else {
        panic!("not an editText");
    };
    assert_eq!((left, width), (Some(0.1), Some(0.5)));
}

#[test]
fn a_request_without_them_leaves_both_unset() {
    let parsed: ApplyRequest = serde_json::from_str(
        r#"{"handle":1,"op":{"op":"editText","page":1,"block":3,"revision":12,"text":"x"}}"#,
    )
    .unwrap();
    let Op::EditText { left, width, .. } = parsed.op else {
        panic!("not an editText");
    };
    assert_eq!((left, width), (None, None));
}

#[test]
fn a_field_the_op_does_not_have_is_refused() {
    for field in [r#""right":0.9"#, r#""top":0.1"#, r#""left":"far""#] {
        let text = format!(
            r#"{{"handle":1,"op":{{"op":"editText","page":1,"block":3,"revision":12,
                "text":"x",{field}}}}}"#
        );
        assert!(
            serde_json::from_str::<ApplyRequest>(&text).is_err(),
            "{field}"
        );
    }
}

#[test]
fn a_width_changes_how_many_lines_the_text_takes() {
    // Three two-letter words: all fit a line 0.95 of the page across, and a
    // line 0.01 across holds no more than one of them.
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (block, revision) = heading(&engine, handle);
    let wide = engine
        .apply(request(
            handle,
            block,
            revision,
            "ab cd ef",
            json!({ "width": 0.95 }),
        ))
        .unwrap();
    assert_eq!(wide["lines"], 1);
    engine
        .undo(kernkraft_pdf_engine::HandleRequest { handle })
        .unwrap();
    let (block, revision) = heading(&engine, handle);
    let narrow = engine
        .apply(request(
            handle,
            block,
            revision,
            "ab cd ef",
            json!({ "width": 0.01 }),
        ))
        .unwrap();
    assert!(narrow["lines"].as_u64().unwrap() >= 3, "{narrow:?}");
}

#[test]
fn a_left_puts_the_blocks_left_edge_there() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (block, revision) = heading(&engine, handle);
    engine
        .apply(request(
            handle,
            block,
            revision,
            "Kern Brief",
            json!({ "left": 0.5 }),
        ))
        .unwrap();
    let text = read(&engine, handle);
    let moved = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|line| line["text"] == "Kern Brief")
        .expect("the edited line is read back")["block"]
        .as_u64()
        .unwrap();
    let rect = &text["blocks"][moved as usize]["rect"];
    assert!((rect[0].as_f64().unwrap() - 0.5).abs() < 1e-6, "{rect}");
}

#[test]
fn a_box_that_is_not_on_the_page_is_refused_as_invalid() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (block, revision) = heading(&engine, handle);
    for extra in [
        json!({ "width": 0.0 }),
        json!({ "width": -0.5 }),
        json!({ "left": 1.5 }),
        json!({ "left": -2.0, "width": 1.0 }),
    ] {
        let refused = engine
            .apply(request(handle, block, revision, "X", extra.clone()))
            .unwrap_err();
        assert_eq!(refused.kind, "Invalid", "{extra}");
    }
    // Nothing was changed: the heading is still there to be read.
    assert_eq!(heading(&engine, handle), (block, revision));
}

#[test]
fn an_empty_text_ignores_the_box() {
    let engine = Engine::new(OPTIONS);
    let handle = open(&engine);
    let (block, revision) = heading(&engine, handle);
    let done = engine
        .apply(request(
            handle,
            block,
            revision,
            "",
            json!({ "left": 7.0, "width": -1.0 }),
        ))
        .unwrap();
    assert_eq!(done["lines"], 0);
}
