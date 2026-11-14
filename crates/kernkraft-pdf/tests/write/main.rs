// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the full writer (`write/`). The files written are read back
//! through the document layer, and each object's bytes are compared with
//! the bytes the fixture wrote it as.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod encrypted;
mod objects;
mod robustness;
mod xref;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{Name, Object, ObjectId, PdfString, Value};
use kernkraft_pdf::write::{compact, WriteOptions};

pub use pdf_fixture::*;

pub const SEED: WriteOptions = WriteOptions { seed: [7; 32] };

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

pub fn string(bytes: &[u8]) -> Object {
    Object::new(Value::String(PdfString::new(bytes)))
}

pub fn name(text: &str) -> Object {
    Object::new(Value::Name(Name::new(text.as_bytes())))
}

/// `document` written and opened again with `password`.
pub fn rewritten(document: &Document, password: &str) -> (Vec<u8>, Document) {
    let bytes = compact(document, &SEED, &NoProgress).unwrap();
    let reopened = Document::from_bytes(bytes.clone(), &OPTIONS, password).unwrap();
    (bytes, reopened)
}

/// The bytes of object `id` as `bytes` holds it in the file.
pub fn file_bytes<'a>(document: &Document, bytes: &'a [u8], id: ObjectId) -> &'a [u8] {
    match document.required(id).unwrap().origin {
        kernkraft_pdf::doc::Origin::File {
            location: kernkraft_pdf::file::Location::File(span),
            ..
        } => &bytes[span.start as usize..span.end as usize],
        other => panic!("{id:?} is not in the file: {other:?}"),
    }
}
