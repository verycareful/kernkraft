// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the annotations the library lists, adds, edits and deletes
//! (`src/annot/`). Files are written by `pdf_fixture`, byte by byte, and
//! each test's expectations come from how its file is built and from the
//! arithmetic of ISO 32000-1 (7.7.3.3 for the displayed page, 12.5.6 for
//! the annotation entries, 8.5 and 11.3 for what an appearance draws),
//! never from a run.
//!
//! Pages are 612 by 792 points unless a test says otherwise. A point `(x,
//! y)` of user space on an unrotated page is at the fractions `(x / 612,
//! (792 - y) / 792)` of the displayed page, so the box `[100 700 160 720]`
//! of user space is the box from `(100, 72)` to `(160, 92)` points
//! measured from the top left corner.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod add;
mod delete;
mod edit;
mod list;
mod render;
mod session;

use kernkraft_pdf::doc::{Date, Document};
use kernkraft_pdf::syntax::{Dictionary, ObjectId, Value};
use kernkraft_pdf::Rect;

pub use pdf_fixture::*;

pub const PAGE_WIDTH: f64 = 612.0;
pub const PAGE_HEIGHT: f64 = 792.0;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

/// The time every change of these tests is made at, and how it is written
/// (7.9.4).
pub const WHEN: Date = Date {
    year: 2026,
    month: 11,
    day: 20,
    hour: 10,
    minute: 30,
    second: 0,
    offset_minutes: Some(0),
};
pub const WHEN_TEXT: &[u8] = b"D:20261120103000Z";

/// Object `10 + 2i` is page `i` (counted from 0) and `11 + 2i` its
/// content.
pub fn page_object(index: u32) -> u32 {
    10 + 2 * index
}

/// A document of one page 612 by 792 points with `content`, `entries`
/// added to the page's dictionary, `catalog` added to the catalog's, and
/// `extra` objects besides.
pub fn document(content: &str, entries: &str, catalog: &str, extra: &[(u32, &[u8])]) -> Fixture {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (
            1,
            format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
        ),
        (
            2,
            b"<< /Type /Pages /Kids [10 0 R] /Count 1 /MediaBox [0 0 612 792] \
              /Resources << /Font << /F1 60 0 R >> >> >>"
                .to_vec(),
        ),
        (3, b"<< /Producer (kernkraft tests) >>".to_vec()),
        (
            10,
            format!("<< /Type /Page /Parent 2 0 R /Contents 11 0 R {entries} >>").into_bytes(),
        ),
        (11, stream("", content.as_bytes())),
        (
            60,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
                .to_vec(),
        ),
    ];
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

/// A page with `content`, and annotations `annots` listed in its `Annots`
/// (written as `n 0 R` references) with `extra` as their objects.
pub fn annotated(content: &str, annots: &[u32], extra: &[(u32, &[u8])]) -> Fixture {
    let list: Vec<String> = annots.iter().map(|n| format!("{n} 0 R")).collect();
    document(content, &format!("/Annots [{}]", list.join(" ")), "", extra)
}

/// An empty page.
pub fn blank() -> Fixture {
    document("", "", "", &[])
}

pub fn assert_close(actual: f64, expected: f64, what: &str) {
    assert!(
        (actual - expected).abs() < 1e-9,
        "{what}: {actual} is not {expected}"
    );
}

/// `rect` is the box given in points of the displayed page, measured from
/// its top left corner, as fractions of the page.
pub fn assert_rect(rect: Rect, left: f64, top: f64, right: f64, bottom: f64, what: &str) {
    assert_close(rect.left, left / PAGE_WIDTH, &format!("{what}: left"));
    assert_close(rect.top, top / PAGE_HEIGHT, &format!("{what}: top"));
    assert_close(rect.right, right / PAGE_WIDTH, &format!("{what}: right"));
    assert_close(
        rect.bottom,
        bottom / PAGE_HEIGHT,
        &format!("{what}: bottom"),
    );
}

/// The box from `(left, top)` to `(right, bottom)`, in points of the
/// displayed 612 by 792 page measured from its top left corner, as
/// fractions of it.
pub fn points_rect(left: f64, top: f64, right: f64, bottom: f64) -> Rect {
    Rect {
        left: left / PAGE_WIDTH,
        top: top / PAGE_HEIGHT,
        right: right / PAGE_WIDTH,
        bottom: bottom / PAGE_HEIGHT,
    }
}

/// The point `(x, y)` in points of the displayed 612 by 792 page, as
/// fractions of it.
pub fn point(x: f64, y: f64) -> (f64, f64) {
    (x / PAGE_WIDTH, y / PAGE_HEIGHT)
}

/// The dictionary of object `number`.
pub fn dict(document: &Document, number: u32) -> Dictionary {
    document
        .object(id(number))
        .unwrap()
        .expect("the object exists")
        .dictionary()
        .expect("the object is a dictionary")
        .clone()
}

/// The numbers of the array `key` of `dictionary`.
pub fn numbers(document: &Document, dictionary: &Dictionary, key: &str) -> Vec<f64> {
    let resolved = document.get(dictionary, key.as_bytes()).unwrap();
    resolved
        .array()
        .unwrap_or_else(|| panic!("{key} is not an array"))
        .iter()
        .map(|item| match document.resolve(item).unwrap().value() {
            Value::Integer(n) => *n as f64,
            Value::Real(r) => r.get(),
            other => panic!("{key} holds {other:?}"),
        })
        .collect()
}

pub fn assert_numbers(actual: &[f64], expected: &[f64], what: &str) {
    assert_eq!(actual.len(), expected.len(), "{what}: {actual:?}");
    for (a, e) in actual.iter().zip(expected) {
        assert_close(*a, *e, what);
    }
}

/// The name `key` of `dictionary`, as text.
pub fn name_of(document: &Document, dictionary: &Dictionary, key: &str) -> Option<String> {
    match document.get(dictionary, key.as_bytes()).unwrap().value() {
        Value::Name(name) => Some(String::from_utf8_lossy(name.as_bytes()).into_owned()),
        _ => None,
    }
}

/// The integer `key` of `dictionary`.
pub fn integer_of(document: &Document, dictionary: &Dictionary, key: &str) -> Option<i64> {
    match document.get(dictionary, key.as_bytes()).unwrap().value() {
        Value::Integer(n) => Some(*n),
        _ => None,
    }
}

/// The objects of the array `Annots` of page 1, as the numbers they refer
/// to.
pub fn annots_of(document: &Document) -> Vec<u32> {
    let page = document.page(1).unwrap();
    document
        .get(&page.dictionary, b"Annots")
        .unwrap()
        .array()
        .unwrap_or_default()
        .iter()
        .map(|item| match item.value() {
            Value::Reference(id) => id.number,
            other => panic!("Annots holds {other:?}"),
        })
        .collect()
}

/// The object number `AP`'s `N` of annotation `number` refers to.
pub fn appearance_of(document: &Document, number: u32) -> u32 {
    let annotation = dict(document, number);
    let ap = document.get(&annotation, b"AP").unwrap();
    let ap = ap.dictionary().expect("AP is a dictionary");
    match ap.get(b"N").expect("AP has N").value() {
        Value::Reference(id) => id.number,
        other => panic!("N is {other:?}"),
    }
}

/// The decoded data of stream `number`, as text.
pub fn stream_text(document: &Document, number: u32) -> String {
    let object = document.required(id(number)).unwrap();
    let stream = object.stream().expect("a stream");
    let decoded = document.decode_stream(id(number), stream, 1 << 20).unwrap();
    String::from_utf8(decoded.data.into_owned()).unwrap()
}

/// The dictionary of the state `GS` an appearance sets.
pub fn graphics_state(document: &Document, number: u32) -> Dictionary {
    let appearance = dict(document, number);
    let resources = document.get(&appearance, b"Resources").unwrap();
    let resources = resources.dictionary().unwrap().clone();
    let states = document.get(&resources, b"ExtGState").unwrap();
    let states = states.dictionary().unwrap().clone();
    let state = document.get(&states, b"GS").unwrap();
    state.dictionary().unwrap().clone()
}

pub fn real_of(document: &Document, dictionary: &Dictionary, key: &str) -> Option<f64> {
    match document.get(dictionary, key.as_bytes()).unwrap().value() {
        Value::Integer(n) => Some(*n as f64),
        Value::Real(r) => Some(r.get()),
        _ => None,
    }
}

/// The text string `key` of `dictionary`, as the bytes it holds.
pub fn string_of(document: &Document, dictionary: &Dictionary, key: &str) -> Option<Vec<u8>> {
    match document.get(dictionary, key.as_bytes()).unwrap().value() {
        Value::String(string) => Some(string.as_bytes().to_vec()),
        _ => None,
    }
}
