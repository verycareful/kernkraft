// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A failure is answered with its kind.

mod common;

use common::failure_kind;
use kernkraft_jni::guard::respond;
use kernkraft_pdf_engine::WireError;

#[test]
fn a_bad_request_is_named_for_the_application() {
    assert_eq!(
        failure_kind(&respond(|| Err(WireError::bad_request("x")))),
        "BadRequest"
    );
}
