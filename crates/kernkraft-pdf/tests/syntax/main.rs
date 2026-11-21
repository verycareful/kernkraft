// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the syntax layer, one module per source file. The helpers here
//! build the objects a test expects and take apart the errors it asserts on.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

mod cursor;
mod lexer;
mod object;
mod parser;
mod robustness;
mod serialise;

use std::fmt::Debug;
use std::path::PathBuf;

use kernkraft_pdf::syntax::limits::{
    MAX_NAME_BYTES, MAX_NESTING_DEPTH, MAX_STRING_BYTES, MAX_TOKEN_BYTES,
};
use kernkraft_pdf::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Real, Value};
use kernkraft_pdf::{
    Diagnostic, DiagnosticKind, Diagnostics, FileSource, Limit, MemorySource, PdfError, Source,
};

/// A prime, so the pattern does not repeat on a power-of-two boundary.
const PATTERN_PERIOD: usize = 251;

/// `n` bytes where byte `i` is `i % PATTERN_PERIOD`.
pub fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % PATTERN_PERIOD) as u8).collect()
}

pub fn memory(bytes: &[u8]) -> MemorySource {
    MemorySource::new(bytes.to_vec())
}

pub struct Sources {
    // Keeps the file alive for as long as the sources are used.
    _dir: tempfile::TempDir,
    pub path: PathBuf,
    pub all: Vec<(&'static str, Box<dyn Source>)>,
}

/// A memory source and a file source over the same bytes.
pub fn both(bytes: &[u8]) -> Sources {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.bin");
    std::fs::write(&path, bytes).unwrap();
    let all: Vec<(&'static str, Box<dyn Source>)> = vec![
        ("memory", Box::new(memory(bytes))),
        ("file", Box::new(FileSource::open(&path).unwrap())),
    ];
    Sources {
        _dir: dir,
        path,
        all,
    }
}

pub fn null() -> Object {
    Object::new(Value::Null)
}

pub fn boolean(value: bool) -> Object {
    Object::new(Value::Boolean(value))
}

pub fn int(value: i64) -> Object {
    Object::new(Value::Integer(value))
}

pub fn real(value: f64) -> Object {
    Object::new(Value::Real(Real::new(value).unwrap()))
}

pub fn string(bytes: &[u8]) -> Object {
    Object::new(Value::String(PdfString::new(bytes)))
}

pub fn name(bytes: &str) -> Object {
    Object::new(Value::Name(Name::new(bytes)))
}

pub fn reference(number: u32, generation: u16) -> Object {
    Object::new(Value::Reference(ObjectId { number, generation }))
}

pub fn array(items: Vec<Object>) -> Object {
    Object::new(Value::Array(items))
}

pub fn dict(entries: Vec<(&str, Object)>) -> Object {
    let mut dictionary = Dictionary::new();
    for (key, value) in entries {
        assert!(dictionary.insert(Name::new(key), value).is_none());
    }
    Object::new(Value::Dictionary(dictionary))
}

/// The kind and offset of each diagnostic, in the order they were raised.
pub fn kinds_and_offsets(diagnostics: &Diagnostics) -> Vec<(DiagnosticKind, u64)> {
    diagnostics
        .as_slice()
        .iter()
        .map(|d: &Diagnostic| (d.kind.clone(), d.offset))
        .collect()
}

pub fn assert_unexpected_end<T: Debug>(result: Result<T, PdfError>, expected_offset: u64) {
    match result {
        Err(PdfError::UnexpectedEnd { offset }) => assert_eq!(offset, expected_offset),
        other => panic!("expected UnexpectedEnd at {expected_offset}, got {other:?}"),
    }
}

pub fn assert_unexpected_token<T: Debug>(
    result: Result<T, PdfError>,
    expected_offset: u64,
    expected_found: &str,
) {
    match result {
        Err(PdfError::UnexpectedToken { offset, found }) => {
            assert_eq!((offset, found.as_str()), (expected_offset, expected_found));
        }
        other => panic!(
            "expected UnexpectedToken {expected_found:?} at {expected_offset}, got {other:?}"
        ),
    }
}

pub fn assert_limit<T: Debug>(
    result: Result<T, PdfError>,
    expected_offset: u64,
    expected_limit: Limit,
) {
    match result {
        Err(PdfError::LimitExceeded { offset, limit }) => {
            assert_eq!((offset, limit), (expected_offset, expected_limit));
        }
        other => {
            panic!("expected LimitExceeded {expected_limit:?} at {expected_offset}, got {other:?}")
        }
    }
}

#[test]
fn limit_max_matches_its_constant() {
    assert_eq!(Limit::NestingDepth.max(), MAX_NESTING_DEPTH as u64);
    assert_eq!(Limit::NameBytes.max(), MAX_NAME_BYTES as u64);
    assert_eq!(Limit::TokenBytes.max(), MAX_TOKEN_BYTES as u64);
    assert_eq!(Limit::StringBytes.max(), MAX_STRING_BYTES as u64);
}

#[test]
fn limit_error_message_names_the_limit() {
    let error = PdfError::LimitExceeded {
        offset: 12,
        limit: Limit::NestingDepth,
    };
    assert_eq!(
        error.to_string(),
        format!("nesting depth at offset 12 is above the limit of {MAX_NESTING_DEPTH}")
    );
}
