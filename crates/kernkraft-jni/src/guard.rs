// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The envelope every export answers with, and the panic net under it.

use std::panic::{catch_unwind, AssertUnwindSafe};

use kernkraft_pdf_engine::{Fields, WireError};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};

/// Runs `body` and turns its result into the answer string: its fields with
/// `"ok":true`, or `{"ok":false,"kind":...,"message":...}`. A panic in
/// `body` is a `Panic` failure, never an unwind across the JNI boundary
/// (which would abort the application).
pub fn respond(body: impl FnOnce() -> Result<Fields, WireError>) -> String {
    let result = match catch_unwind(AssertUnwindSafe(body)) {
        Ok(result) => result,
        Err(payload) => Err(WireError::new("Panic", panic_message(payload.as_ref()))),
    };
    match result {
        Ok(mut fields) => {
            fields.insert("ok".to_owned(), Value::Bool(true));
            Value::Object(fields).to_string()
        }
        Err(error) => json!({
            "ok": false,
            "kind": error.kind,
            "message": error.message,
        })
        .to_string(),
    }
}

/// Parses `request` as the JSON object `R` and runs `body` on it, inside
/// [`respond`]. A request that does not parse, or has a field the operation
/// does not know, is `BadRequest`.
pub fn run<R: DeserializeOwned>(
    request: &str,
    body: impl FnOnce(R) -> Result<Fields, WireError>,
) -> String {
    respond(|| {
        let parsed: R = serde_json::from_str(request)
            .map_err(|error| WireError::bad_request(format!("the request: {error}")))?;
        body(parsed)
    })
}

fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(text) = payload.downcast_ref::<&str>() {
        (*text).to_owned()
    } else if let Some(text) = payload.downcast_ref::<String>() {
        text.clone()
    } else {
        "a panic with no message".to_owned()
    }
}
