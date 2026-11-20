// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of PDF fonts, read side: one module for each of `simple.rs`,
//! `composite.rs` and `cmap.rs` of `src/font/`, and a mutation test over
//! them. Files are written by `pdf_fixture`, byte by byte, and each
//! test's expectations come from how its file is built and from the
//! specification's tables and Adobe's metrics and CMap files, never from
//! a run.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod cmap;
mod composite;
mod embed;
mod robustness;
mod simple;
mod type1;

use kernkraft_pdf::doc::Document;
use kernkraft_pdf::font::Font;
use kernkraft_pdf::syntax::{Object, ObjectId, Value};
use kernkraft_pdf::{Diagnostic, DiagnosticKind, PdfError};

pub use pdf_fixture::*;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

/// A one-page document with `extra` objects beside its own. Fonts are
/// numbered from 60.
pub fn font_document(extra: &[(u32, &[u8])]) -> Fixture {
    let mut objects = pages_document(1);
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

pub fn load(document: &Document, number: u32) -> Result<Font, PdfError> {
    document.font(&Object::new(Value::Reference(id(number))))
}

/// The font at object `number` of a document of `extra` objects.
pub fn font_of(extra: &[(u32, &[u8])], number: u32) -> (Document, Font) {
    let document = font_document(extra).open();
    let font = load(&document, number).expect("the font reads");
    (document, font)
}

/// The diagnostics of one kind.
pub fn of_kind(document: &Document, kind: &DiagnosticKind) -> Vec<Diagnostic> {
    document
        .diagnostics()
        .into_iter()
        .filter(|diagnostic| &diagnostic.kind == kind)
        .collect()
}

/// The text of a string's characters, one entry for each character.
pub fn texts(font: &Font, bytes: &[u8]) -> Vec<Option<String>> {
    font.chars(bytes)
        .map(|c| c.unicode.map(|text| text.into_owned()))
        .collect()
}

pub fn widths(font: &Font, bytes: &[u8]) -> Vec<f64> {
    font.chars(bytes).map(|c| c.width).collect()
}

/// A ToUnicode CMap stream object body whose mappings are `body`.
pub fn to_unicode(body: &str) -> Vec<u8> {
    stream(
        "",
        format!(
            "/CIDInit /ProcSet findresource begin 12 dict begin begincmap\n\
             /CMapName /Adobe-Identity-UCS def /CMapType 2 def\n\
             1 begincodespacerange <0000> <FFFF> endcodespacerange\n{body}\n\
             endcmap CMapName currentdict /CMap defineresource pop end end"
        )
        .as_bytes(),
    )
}
