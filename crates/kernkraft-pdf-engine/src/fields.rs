// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The shape of a successful answer.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use crate::error::WireError;

/// What an operation returns on success: the fields of the answer object.
pub type Fields = Map<String, Value>;

/// A one-field answer.
pub fn field(name: &str, value: impl Into<Value>) -> Fields {
    let mut fields = Fields::new();
    fields.insert(name.to_owned(), value.into());
    fields
}

/// A one-field answer that is a path as text; `BadPath` when it is not UTF-8.
pub fn path_field(name: &str, path: &Path) -> Result<Fields, WireError> {
    Ok(field(name, text_of(path)?))
}

/// A one-field answer that lists paths as text. A path that is not UTF-8
/// cannot be one, so it is `BadPath`: an adapter whose callers hold paths
/// as bytes answers with the paths themselves instead.
pub fn paths_field(name: &str, paths: &[PathBuf]) -> Result<Fields, WireError> {
    let mut texts = Vec::with_capacity(paths.len());
    for path in paths {
        texts.push(json!(text_of(path)?));
    }
    Ok(field(name, Value::Array(texts)))
}

fn text_of(path: &Path) -> Result<&str, WireError> {
    path.to_str().ok_or_else(|| {
        WireError::new(
            "BadPath",
            format!(
                "{} is not UTF-8 and cannot be written as text",
                path.display()
            ),
        )
    })
}
