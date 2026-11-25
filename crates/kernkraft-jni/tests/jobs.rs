// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `cancelJob` and `jobProgress` through the engine, and the `job` field of
//! the long requests. A cancel that arrives before its request has begun is
//! kept for it, so each case cancels first and then makes the request.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;

use std::fs;

use common::{failure_kind, ok};
use fixture::{open, sample, DATE};
use kernkraft_jni::bridge::Bridge;
use kernkraft_jni::stateless;
use serde_json::json;

#[test]
fn a_cancelled_compact_makes_no_file() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("copy.pdf");
    let asked = ok(&engine.cancel_job(&json!({ "job": 1 }).to_string()));
    assert_eq!(asked["running"], false);

    let answer = engine.compact(&json!({ "handle": handle, "out": out, "job": 1 }).to_string());

    assert_eq!(failure_kind(&answer), "Cancelled");
    assert!(!out.exists());
}

#[test]
fn a_cancelled_append_writes_no_update() {
    let engine = Bridge::new();
    let handle = open(&engine);
    ok(&engine.apply(
        &json!({ "handle": handle,
                 "op": { "op": "rotate", "pages": [1], "degrees": 90 } })
        .to_string(),
    ));
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("update.bin");
    engine.cancel_job(&json!({ "job": 2 }).to_string());

    let answer = engine.append_since(
        &json!({ "handle": handle, "base_length": fs::metadata(sample()).unwrap().len(),
                 "out": out, "modified": DATE, "job": 2 })
        .to_string(),
    );

    assert_eq!(failure_kind(&answer), "Cancelled");
    assert!(!out.exists());
}

#[test]
fn a_cancelled_merge_makes_no_file_through_the_stateless_call() {
    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("merged.pdf");
    let engine = kernkraft_jni::bridge::bridge();
    engine.cancel_job(&json!({ "job": 3 }).to_string());

    let answer = stateless::merge_files(
        &json!({ "sources": [{ "path": sample() }, { "path": sample() }],
                 "out": out, "job": 3 })
        .to_string(),
    );

    assert_eq!(failure_kind(&answer), "Cancelled");
    assert!(!out.exists());
}

#[test]
fn a_cancelled_page_read_answers_cancelled() {
    let engine = Bridge::new();
    let handle = open(&engine);
    engine.cancel_job(&json!({ "job": 4 }).to_string());

    let answer = engine.text(&json!({ "handle": handle, "page": 1, "job": 4 }).to_string());

    assert_eq!(failure_kind(&answer), "Cancelled");
}

#[test]
fn a_job_that_is_not_running_has_nothing_counted() {
    let engine = Bridge::new();

    let answer = ok(&engine.job_progress(&json!({ "job": 99 }).to_string()));

    assert_eq!(answer["running"], false);
    assert_eq!(answer["done"], 0);
    assert_eq!(answer["total"], 0);
    assert_eq!(answer["stage"], "");
}

#[test]
fn a_job_request_with_a_field_it_does_not_know_is_refused() {
    let engine = Bridge::new();

    let cancel = engine.cancel_job(&json!({ "job": 1, "handle": 2 }).to_string());
    let progress = engine.job_progress("{}");

    assert_eq!(failure_kind(&cancel), "BadRequest");
    assert_eq!(failure_kind(&progress), "BadRequest");
}
