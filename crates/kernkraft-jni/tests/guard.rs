// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! The answer envelope and the panic net.

mod common;

use common::parse;
use kernkraft_jni::guard::respond;
use std::path::PathBuf;

use kernkraft_pdf_engine::{field, WireDetails, WireError};
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
fn a_failure_with_a_path_and_details_carries_them() {
    let mut error = WireError::new("XattrsNotKept", "not kept");
    error.path = Some(PathBuf::from("/data/user/0/doc.pdf"));
    error.details = Some(WireDetails::XattrsNotKept {
        names: vec!["user.a".to_owned(), "user.b".to_owned()],
    });

    let answer = parse(&respond(|| Err(error)));

    assert_eq!(
        answer,
        json!({
            "ok": false,
            "kind": "XattrsNotKept",
            "message": "not kept",
            "path": "/data/user/0/doc.pdf",
            "details": { "k": "XattrsNotKept", "names": ["user.a", "user.b"] },
        })
    );
}

#[test]
fn a_failure_without_a_path_has_no_path_key() {
    let answer = parse(&respond(|| Err(WireError::bad_request("x"))));

    assert!(answer.get("path").is_none());
    assert!(answer.get("details").is_none());
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
