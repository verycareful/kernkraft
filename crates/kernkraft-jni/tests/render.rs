// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `renderSize`, `render` into a buffer, and `setSystemFonts`, on the A4
//! sample (595.28 by 841.89 points).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;

use common::{failure_kind, ok};
use fixture::{open, sample};
use kernkraft_jni::bridge::Bridge;
use serde_json::{json, Value};

const BYTES_PER_PIXEL: u64 = 4;
const PAPER: [u8; 4] = [255, 255, 255, 255];

fn size(engine: &Bridge, handle: i64, extra: Value) -> Value {
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
    let engine = Bridge::new();
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
    let engine = Bridge::new();
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
    let engine = Bridge::new();
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
    let engine = Bridge::new();
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
    let engine = Bridge::new();
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
    let engine = Bridge::new();
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
    let engine = Bridge::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 1, "width_px": 0 });

    assert_eq!(
        failure_kind(&engine.render_size(&request.to_string())),
        "BadRequest"
    );
}

#[test]
fn a_picture_past_the_pixel_limit_is_refused() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let request = json!({ "handle": handle, "page": 1, "width_px": 40_000 });

    assert_eq!(
        failure_kind(&engine.render_size(&request.to_string())),
        "LimitExceeded"
    );
}

#[test]
fn files_that_are_not_fonts_are_skipped_and_counted() {
    let engine = Bridge::new();

    let answer = ok(&engine.set_system_fonts(&json!({ "paths": [sample()] }).to_string()));

    assert_eq!(answer["faces"], 0);
    assert_eq!(answer["skipped"], 1);
    assert_eq!(
        ok(&engine.set_system_fonts(&json!({ "paths": [] }).to_string()))["faces"],
        0
    );
}

// The CMYK profile of the application (`setCmykProfile`).

/// A profile of a CMYK colour space whose lightness follows black ink
/// alone: full black ink is black, where the bundled profile
/// makes it a dark grey of about 43.
fn grey_by_black() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/icc/cmyk-grey-by-black.icc")
}

/// Black, as far as a profile transform can say: its lightness 0 comes out
/// within a few levels of 0.
fn assert_black(pixel: [u8; 4]) {
    assert!(pixel[..3].iter().all(|level| *level <= 4), "{pixel:?}");
    assert_eq!(pixel[3], 255);
}

/// A one-page file, 20 points square, that paints all of it in full black
/// ink, DeviceCMYK `0 0 0 1`, with its cross-reference table.
fn black_ink_page(directory: &std::path::Path) -> std::path::PathBuf {
    let content = "0 0 0 1 k 0 0 20 20 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Contents 4 0 R >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
    ];
    let mut file = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(file.len());
        file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let table = file.len();
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        xref.push_str(&format!("{offset:010} 00000 n \n"));
    }
    xref.push_str(&format!(
        "trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{table}\n%%EOF\n",
        objects.len() + 1
    ));
    file.extend_from_slice(xref.as_bytes());
    let path = directory.join("black-ink.pdf");
    std::fs::write(&path, file).unwrap();
    path
}

/// The pixel in the middle of the page of `path`, opened by `engine`.
fn middle_pixel(engine: &Bridge, path: &std::path::Path) -> [u8; 4] {
    let opened = ok(&engine.open(&json!({ "path": path }).to_string()));
    let handle = opened["handle"].as_i64().unwrap();
    let request = json!({ "handle": handle, "page": 1, "width_px": 20 });
    let asked = ok(&engine.render_size(&request.to_string()));
    let (width, height) = (
        asked["width"].as_u64().unwrap() as usize,
        asked["height"].as_u64().unwrap() as usize,
    );
    let mut buffer = vec![0_u8; asked["bytes"].as_u64().unwrap() as usize];
    ok(&engine.render(&request.to_string(), &mut buffer));
    let at = ((height / 2) * width + width / 2) * 4;
    [buffer[at], buffer[at + 1], buffer[at + 2], buffer[at + 3]]
}

#[test]
fn the_cmyk_profile_of_the_application_is_used_by_the_documents_opened_after() {
    let directory = tempfile::tempdir().unwrap();
    let page = black_ink_page(directory.path());
    let engine = Bridge::new();

    // The bundled profile: black ink is dark, not black.
    let bundled = middle_pixel(&engine, &page);
    assert!(
        bundled[..3].iter().all(|level| (1..=64).contains(level)),
        "{bundled:?}"
    );

    let answer = ok(&engine.set_cmyk_profile(&json!({ "path": grey_by_black() }).to_string()));
    assert_eq!(answer["source"], "application");
    assert_black(middle_pixel(&engine, &page));

    // No path takes it away.
    let answer = ok(&engine.set_cmyk_profile("{}"));
    assert_eq!(answer["source"], "bundled");
    assert_eq!(middle_pixel(&engine, &page), bundled);
}

#[test]
fn a_cmyk_profile_that_cannot_be_used_is_refused_and_the_one_in_force_stays() {
    let directory = tempfile::tempdir().unwrap();
    let page = black_ink_page(directory.path());
    let engine = Bridge::new();
    ok(&engine.set_cmyk_profile(&json!({ "path": grey_by_black() }).to_string()));

    // Not a profile at all, and a file that is missing.
    let not_a_profile = directory.path().join("not-a-profile.icc");
    std::fs::write(&not_a_profile, b"not a profile").unwrap();
    for path in [not_a_profile, directory.path().join("missing.icc")] {
        let answer = engine.set_cmyk_profile(&json!({ "path": path }).to_string());
        assert!(
            ["Invalid", "Io"].contains(&failure_kind(&answer).as_str()),
            "{answer}"
        );
    }
    // A request that names something else is not understood.
    assert_eq!(
        failure_kind(&engine.set_cmyk_profile(&json!({ "profile": "x" }).to_string())),
        "BadRequest"
    );

    assert_black(middle_pixel(&engine, &page));
}
