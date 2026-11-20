// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the document layer, one module per source file. The files are
//! written by `pdf_fixture`, byte by byte.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod changes;
mod display;
mod open;
mod pages;
mod tree;

use kernkraft_pdf::syntax::{Name, Object, ObjectId, PdfString, Value};

pub use pdf_fixture::*;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

pub fn name(text: &str) -> Object {
    Object::new(Value::Name(Name::new(text.as_bytes())))
}

pub fn string(bytes: &[u8]) -> Object {
    Object::new(Value::String(PdfString::new(bytes)))
}

pub fn reference(number: u32) -> Object {
    Object::new(Value::Reference(id(number)))
}
