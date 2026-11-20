// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `renderSize`, `render` into a buffer, and `setSystemFonts`, on the A4
//! sample (595.28 by 841.89 points).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;

use common::{failure_kind, ok};
use fixture::{open, sample};
use kernkraft_jni::engine::Engine;
use serde_json::{json, Value};

const BYTES_PER_PIXEL: u64 = 4;
const PAPER: [u8; 4] = [255, 255, 255, 255];

fn size(engine: &Engine, handle: i64, extra: Value) -> Value {
    let mut request = json!({ "handle": handle, "page": 1, "width_px": 100 });
    request
        .as_object_mut()
        .unwrap()
        .extend(extra.as_object().unwrap().clone());
    ok(&engine.render_size(&request.to_string()))
}

fn pixels(answer: &Value) -> usize {
    (answer["width"].as_u64().unwrap() * answer["height"].as_u64().unwrap()) as usize
}

#[test]
fn the_picture_is_as_wide_as_asked_and_as_tall_as_the_page_is() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = size(&engine, handle, json!({}));

    assert_eq!(answer["width"], 100);
    // 100 pixels over 595.28 points is 0.168 pixels a point, so 841.89
    // points are 141.4 pixels, rounded up.
    let height = answer["height"].as_u64().unwrap();
    assert!((141..=142).contains(&height), "{height}");
    assert_eq!(answer["bytes"], 100 * height * BYTES_PER_PIXEL);
}

#[test]
fn a_region_is_as_wide_as_asked() {
    let engine = Engine::new();
    let handle = open(&engine);

    let answer = size(
        &engine,
        handle,
        json!({ "width_px": 50, "region": [0.5, 0.0, 1.0, 1.0] }),
    );

    assert_eq!(answer["width"], 50);
}

#[test]
fn a_turned_page_is_wider_than_it_is_tall() {
    let engine = Engine::new();
    let handle = open(&engine);
    ok(&engine.apply(
        &json!({ "handle": handle, "op": { "op": "rotate", "pages": [1], "degrees": 90 } })
            .to_string(),
    ));

    let answer = size(&engine, handle, json!({}));

    assert_eq!(answer["width"], 100);
    // 595.28 over 841.89 of 100 pixels is 70.7, rounded up.
    let height = answer["height"].as_u64().unwrap();
    assert!((70..=71).contains(&height), "{height}");
}

#[test]
fn render_fills_the_buffer_with_the_page() {
    let engine = Engine::new();
    let handle = open(&engine);
    let asked = size(&engine, handle, json!({}));
    let mut buffer = vec![0_u8; asked["bytes"].as_u64().unwrap() as usize];
    let request = json!({ "handle": handle, "page": 1, "width_px": 100 });

    let answer = ok(&engine.render(&request.to_string(), &mut buffer));

    assert_eq!(answer, asked_with_ok(&asked));
    assert_eq!(&buffer[..4], PAPER, "the corner of the page is paper");
    let dark = buffer
        .chunks(4)
        .filter(|pixel| pixel[0] < 128 && pixel[1] < 128 && pixel[2] < 128)
        .count();
    assert!(dark > 0, "the text of the page is drawn");
    assert!(
        buffer.chunks(4).all(|pixel| pixel[3] == 255),
        "every alpha is 255"
    );
    assert_eq!(buffer.len() / 4, pixels(&asked));
}

fn asked_with_ok(size: &Value) -> Value {
    let mut value = size.clone();
    value["ok"] = Value::Bool(true);
    value
}

#[test]
fn a_buffer_that_is_too_small_is_refused_and_left_as_it_was() {
    let engine = Engine::new();
    let handle = open(&engine);
    let asked = size(&engine, handle, json!({}));
    let mut buffer = vec![7_u8; asked["bytes"].as_u64().unwrap() as usize - 1];
    let request = json!({ "handle": handle, "page": 1, "width_px": 100 });

    let answer = engine.render(&request.to_string(), &mut buffer);

    assert_eq!(failure_kind(&answer), "BufferTooSmall");
    assert!(buffer.iter().all(|byte| *byte == 7));
}

#[test]
fn a_buffer_that_is_larger_is_filled_from_the_start() {
    let engine = Engine::new();
    let handle = open(&engine);
    let asked = size(&engine, handle, json!({}));
    let bytes = asked["bytes"].as_u64().unwrap() as usize;
    let mut buffer = vec![7_u8; bytes + 16];
    let request = json!({ "handle": handle, "page": 1, "width_px": 100 });

    ok(&engine.render(&request.to_string(), &mut buffer));

    assert!(buffer[bytes..].iter().all(|byte| *byte == 7));
    assert_eq!(&buffer[..4], PAPER);
}

#[test]
fn a_picture_with_no_width_is_bad() {
    let engine = Engine::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 1, "width_px": 0 });

    assert_eq!(
        failure_kind(&engine.render_size(&request.to_string())),
        "BadRequest"
    );
}

#[test]
fn a_picture_past_the_pixel_limit_is_refused() {
    let engine = Engine::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 1, "width_px": 40_000 });

    assert_eq!(
        failure_kind(&engine.render_size(&request.to_string())),
        "LimitExceeded"
    );
}

#[test]
fn files_that_are_not_fonts_are_skipped_and_counted() {
    let engine = Engine::new();

    let answer = ok(&engine.set_system_fonts(&json!({ "paths": [sample()] }).to_string()));

    assert_eq!(answer["faces"], 0);
    assert_eq!(answer["skipped"], 1);
    assert_eq!(
        ok(&engine.set_system_fonts(&json!({ "paths": [] }).to_string()))["faces"],
        0
    );
}
