// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The shape of a successful answer.

use serde_json::{Map, Value};

/// What an operation returns on success: the fields of the answer object.
pub type Fields = Map<String, Value>;

/// A one-field answer.
pub fn field(name: &str, value: impl Into<Value>) -> Fields {
    let mut fields = Fields::new();
    fields.insert(name.to_owned(), value.into());
    fields
}
