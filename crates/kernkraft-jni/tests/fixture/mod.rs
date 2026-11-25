// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The fixture PDF and a reader for the files the operations make. Each test
//! crate uses part of it.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};

use kernkraft_pdf::doc::Document;

/// A two-page PDF that kernkraft may redistribute.
pub fn sample() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/kern-sample.pdf")
}

/// The page count of the PDF at `path`, read by `kernkraft-pdf`.
pub fn pages_of(path: &Path) -> u32 {
    let options = kernkraft_jni::open::OPEN_OPTIONS;
    Document::open(path, &options, "")
        .expect("the output opens")
        .page_count()
        .expect("the output has a page tree")
}

/// A time to stamp edits with: a PDF date.
pub const DATE: &str = "D:20261121103000Z";

/// Opens the sample in `engine` and gives its handle.
pub fn open(engine: &kernkraft_jni::bridge::Bridge) -> i64 {
    let request = serde_json::json!({ "path": sample() });
    let answer: serde_json::Value =
        serde_json::from_str(&engine.open(&request.to_string())).unwrap();
    assert_eq!(answer["ok"], true, "{answer}");
    answer["handle"].as_i64().unwrap()
}

/// The size of the sample file in bytes.
pub fn sample_length() -> u64 {
    std::fs::metadata(sample()).unwrap().len()
}

/// A copy of the sample in `dir` whose `startxref` names an offset past the
/// end of the file, so that the file opens only after its cross-reference
/// is rebuilt. The pages are the sample's.
pub fn rebuilt_sample(dir: &Path) -> PathBuf {
    const MARKER: &[u8] = b"startxref\n";
    const PAST_THE_END: &[u8] = b"7777777\n%%EOF\n";
    let mut bytes = std::fs::read(sample()).unwrap();
    let at = bytes
        .windows(MARKER.len())
        .rposition(|window| window == MARKER)
        .unwrap();
    bytes.truncate(at + MARKER.len());
    bytes.extend_from_slice(PAST_THE_END);
    let path = dir.join("rebuilt.pdf");
    std::fs::write(&path, bytes).unwrap();
    path
}
