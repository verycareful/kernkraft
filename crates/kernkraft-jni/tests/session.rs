// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening, closing and reading a page through the engine. Expectations
//! come from the sample file: two A4 pages, `MediaBox [0 0 595.28 841.89]`,
//! the first headed `Kern -- Project Architecture Brief`, with the word
//! `Overview` once, and no links or annotations.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;

use common::{failure_kind, ok};
use fixture::{open, sample};
use kernkraft_jni::bridge::Bridge;
use serde_json::{json, Value};

const WIDTH_PT: f64 = 595.28;
const HEIGHT_PT: f64 = 841.89;
const TOLERANCE: f64 = 0.001;

fn call(engine: &Bridge, operation: fn(&Bridge, &str) -> String, request: Value) -> String {
    operation(engine, &request.to_string())
}

#[test]
fn open_gives_a_handle_and_the_page_count() {
    let engine = Bridge::new();
    let answer = ok(&engine.open(&json!({ "path": sample() }).to_string()));

    assert_eq!(answer["pages"], 2);
    assert_eq!(answer["revision"], 0);
    assert!(answer["handle"].as_i64().unwrap() > 0);
}

#[test]
fn two_opens_are_two_sessions() {
    let engine = Bridge::new();

    assert_ne!(open(&engine), open(&engine));
}

#[test]
fn a_closed_handle_is_unknown_to_every_operation() {
    let engine = Bridge::new();
    let handle = open(&engine);
    ok(&call(&engine, Bridge::close, json!({ "handle": handle })));

    let request = json!({ "handle": handle, "page": 1 });
    assert_eq!(
        failure_kind(&call(&engine, Bridge::close, json!({ "handle": handle }))),
        "UnknownHandle"
    );
    assert_eq!(
        failure_kind(&call(&engine, Bridge::page_info, request.clone())),
        "UnknownHandle"
    );
    assert_eq!(
        failure_kind(&call(&engine, Bridge::text, request.clone())),
        "UnknownHandle"
    );
    assert_eq!(
        failure_kind(&call(&engine, Bridge::links, request.clone())),
        "UnknownHandle"
    );
    assert_eq!(
        failure_kind(&call(&engine, Bridge::annotations, request)),
        "UnknownHandle"
    );
}

#[test]
fn a_file_that_is_not_there_does_not_open() {
    let engine = Bridge::new();
    let request = json!({ "path": "/no/such/file.pdf" });

    assert_eq!(failure_kind(&engine.open(&request.to_string())), "NotFound");
}

#[test]
fn page_info_is_the_displayed_page() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let info = ok(&call(
        &engine,
        Bridge::page_info,
        json!({ "handle": handle, "page": 1 }),
    ));

    assert!((info["width_pt"].as_f64().unwrap() - WIDTH_PT).abs() < TOLERANCE);
    assert!((info["height_pt"].as_f64().unwrap() - HEIGHT_PT).abs() < TOLERANCE);
    assert_eq!(info["rotate"], 0);
    assert_eq!(info["crop_box"], json!([0.0, 0.0, WIDTH_PT, HEIGHT_PT]));
}

#[test]
fn a_page_past_the_end_is_out_of_range() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 3 });

    assert_eq!(
        failure_kind(&call(&engine, Bridge::page_info, request.clone())),
        "PageOutOfRange"
    );
    assert_eq!(
        failure_kind(&call(&engine, Bridge::text, request)),
        "PageOutOfRange"
    );
}

#[test]
fn text_has_the_heading_with_a_box_for_every_glyph() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let text = ok(&call(
        &engine,
        Bridge::text,
        json!({ "handle": handle, "page": 1 }),
    ));

    let lines = text["lines"].as_array().unwrap();
    let heading = lines
        .iter()
        .find(|line| line["text"] == "Kern -- Project Architecture Brief")
        .expect("the heading is a line");
    assert_eq!(
        heading["glyphs"].as_array().unwrap().len(),
        "Kern -- Project Architecture Brief".chars().count()
    );
    assert_eq!(text["revision"], 0);
}

#[test]
fn every_line_belongs_to_a_block_that_says_whether_it_is_editable() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let text = ok(&call(
        &engine,
        Bridge::text,
        json!({ "handle": handle, "page": 1 }),
    ));

    let blocks = text["blocks"].as_array().unwrap();
    assert!(!blocks.is_empty());
    for (index, block) in blocks.iter().enumerate() {
        assert_eq!(block["id"], index);
        assert!(block["editable"].is_boolean());
        assert_eq!(block["editable"] == false, block.get("reason").is_some());
    }
    for line in text["lines"].as_array().unwrap() {
        assert!(line["block"].as_u64().unwrap() < blocks.len() as u64);
    }
}

#[test]
fn search_finds_a_word_with_the_box_it_covers() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let request = |query: &str, case_sensitive: bool, whole_word: bool| {
        call(
            &engine,
            Bridge::search,
            json!({ "handle": handle, "page": 1, "query": query,
                    "case_sensitive": case_sensitive, "whole_word": whole_word }),
        )
    };

    let found = ok(&request("Overview", false, false));
    assert_eq!(found["matches"].as_array().unwrap().len(), 1);
    assert!(!found["matches"][0]["rects"].as_array().unwrap().is_empty());

    assert_eq!(
        ok(&request("overview", false, false))["matches"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(ok(&request("overview", true, false))["matches"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(!ok(&request("Over", false, false))["matches"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(ok(&request("Over", false, true))["matches"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn a_page_with_no_links_or_annotations_lists_none() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 1 });

    assert!(ok(&call(&engine, Bridge::links, request.clone()))["links"]
        .as_array()
        .unwrap()
        .is_empty());
    assert!(
        ok(&call(&engine, Bridge::annotations, request))["annotations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn a_request_with_a_field_the_operation_does_not_know_is_bad() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 1, "pgae": 2 });

    assert_eq!(
        failure_kind(&call(&engine, Bridge::page_info, request)),
        "BadRequest"
    );
}
