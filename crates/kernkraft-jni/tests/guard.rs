// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The answer envelope and the panic net.

mod common;

use common::parse;
use kernkraft_jni::guard::respond;
use kernkraft_pdf_engine::{field, WireError};
use serde_json::{json, Value};

#[test]
fn a_success_carries_its_fields_and_ok() {
    let answer = parse(&respond(|| Ok(field("pages", 7))));

    assert_eq!(answer, json!({ "ok": true, "pages": 7 }));
}

#[test]
fn a_failure_carries_kind_and_message() {
    let answer = parse(&respond(|| {
        Err(WireError::new("PageOutOfRange", "page 9 of 3"))
    }));

    assert_eq!(
        answer,
        json!({ "ok": false, "kind": "PageOutOfRange", "message": "page 9 of 3" })
    );
}

#[test]
fn a_panic_is_an_answer_not_an_unwind() {
    let text = respond(|| panic!("boom {}", 7));
    let answer = parse(&text);

    assert_eq!(answer["ok"], Value::Bool(false));
    assert_eq!(answer["kind"], "Panic");
    assert_eq!(answer["message"], "boom 7");
}

#[test]
fn a_panic_with_a_literal_message_keeps_it() {
    let answer = parse(&respond(|| panic!("plain")));

    assert_eq!(answer["message"], "plain");
}
