// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Shared by the integration tests: reading the answers. Each test crate
//! uses part of it.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use serde_json::Value;

pub fn parse(answer: &str) -> Value {
    serde_json::from_str(answer).expect("an answer is JSON")
}

/// The `kind` of a failed answer; panics when the answer is a success.
pub fn failure_kind(answer: &str) -> String {
    let value = parse(answer);
    assert_eq!(
        value["ok"],
        Value::Bool(false),
        "expected a failure: {answer}"
    );
    value["kind"]
        .as_str()
        .expect("a failure has a kind")
        .to_owned()
}

/// A successful answer's value; panics, printing the answer, on a failure.
pub fn ok(answer: &str) -> Value {
    let value = parse(answer);
    assert_eq!(value["ok"], Value::Bool(true), "expected success: {answer}");
    value
}
