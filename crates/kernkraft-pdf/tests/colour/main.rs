// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the colour layer: `functions.rs` (ISO 32000-1:2008, 7.10),
//! `spaces.rs` (8.6 and 10.3), `icc.rs` (8.6.5.5) and `cmyk.rs` (14.11.5), with a mutation test
//! over them. Files are written by `pdf_fixture`, byte by byte, and each
//! test's expectations are derived from the specification's formulas and
//! from how its file is built, never from a run.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod cmyk;
mod functions;
mod icc;
mod robustness;
mod spaces;

use kernkraft_pdf::colour::{ColourSpace, Function, Rgb};
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{Name, Object, ObjectId, Value};
use kernkraft_pdf::{Diagnostic, DiagnosticKind, PdfError};

pub use pdf_fixture::*;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

pub fn reference(number: u32) -> Object {
    Object::new(Value::Reference(id(number)))
}

pub fn name(text: &str) -> Object {
    Object::new(Value::Name(Name::new(text.as_bytes().to_vec())))
}

/// A one-page document with `extra` objects beside its own. The objects
/// of the tests are numbered from 60.
pub fn document(extra: &[(u32, &[u8])]) -> Document {
    let mut objects = pages_document(1);
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open()
}

/// The function at object 60 of a document of `extra` objects.
pub fn function_of(extra: &[(u32, &[u8])]) -> Result<Function, PdfError> {
    document(extra).function(&reference(60))
}

/// The colour space at object 60 of a document of `extra` objects.
pub fn space_of(extra: &[(u32, &[u8])]) -> (Document, Result<ColourSpace, PdfError>) {
    let document = document(extra);
    let space = document.colour_space(&reference(60));
    (document, space)
}

pub fn of_kind(document: &Document, kind: &DiagnosticKind) -> Vec<Diagnostic> {
    document
        .diagnostics()
        .into_iter()
        .filter(|diagnostic| &diagnostic.kind == kind)
        .collect()
}

/// `value` is within `tolerance` of `expected`.
pub fn near(value: f64, expected: f64, tolerance: f64) {
    assert!(
        (value - expected).abs() <= tolerance,
        "{value} is not within {tolerance} of {expected}"
    );
}

pub fn assert_rgb(rgb: Rgb, expected: [f64; 3], tolerance: f64) {
    for (value, expected) in [rgb.r, rgb.g, rgb.b].into_iter().zip(expected) {
        assert!(
            (value - expected).abs() <= tolerance,
            "{rgb:?} is not within {tolerance} of {expected:?}"
        );
    }
}

/// The sRGB transfer function of IEC 61966-2-1, from its definition.
pub fn srgb_encode(linear: f64) -> f64 {
    if linear <= 0.0031308 {
        12.92 * linear
    } else {
        1.055 * linear.powf(1.0 / 2.4) - 0.055
    }
}
