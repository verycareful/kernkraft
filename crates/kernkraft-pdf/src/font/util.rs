// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading the entries of a font's dictionaries.

use crate::doc::Document;
use crate::syntax::{Dictionary, Object, Value};
use crate::PdfError;

/// The number a value is, an integer or a real.
pub(crate) fn as_number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    }
}

/// The number `object` is or refers to.
pub(crate) fn number(doc: &Document, object: &Object) -> Result<Option<f64>, PdfError> {
    Ok(as_number(doc.resolve(object)?.value()))
}

/// The number of entry `key`, `None` when it is absent or not a number.
pub(crate) fn entry_number(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<f64>, PdfError> {
    Ok(as_number(doc.get(dictionary, key)?.value()))
}

/// The integer of entry `key`.
pub(crate) fn entry_integer(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<i64>, PdfError> {
    Ok(match doc.get(dictionary, key)?.value() {
        Value::Integer(integer) => Some(*integer),
        _ => None,
    })
}

/// The name of entry `key`, as text.
pub(crate) fn entry_name(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<String>, PdfError> {
    Ok(match doc.get(dictionary, key)?.value() {
        Value::Name(name) => Some(String::from_utf8_lossy(name.as_bytes()).into_owned()),
        _ => None,
    })
}

/// The string of entry `key`, as text.
pub(crate) fn entry_string(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<String>, PdfError> {
    Ok(match doc.get(dictionary, key)?.value() {
        Value::String(string) => Some(String::from_utf8_lossy(string.as_bytes()).into_owned()),
        _ => None,
    })
}

/// The dictionary of entry `key`.
pub(crate) fn entry_dictionary(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<Dictionary>, PdfError> {
    Ok(doc.get(dictionary, key)?.dictionary().cloned())
}

/// The items of the array of entry `key`.
pub(crate) fn entry_array(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<Vec<Object>>, PdfError> {
    Ok(doc.get(dictionary, key)?.array().map(<[Object]>::to_vec))
}
