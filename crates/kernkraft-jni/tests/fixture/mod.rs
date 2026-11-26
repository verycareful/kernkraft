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

/// A one-page PDF of `lines` in 10 point Courier, 12 points apart, so that
/// they are one block of the text layer; each line is a string of the page's
/// content, which must not hold a parenthesis or a backslash.
pub fn lines_page(dir: &Path, lines: &[&str]) -> PathBuf {
    let mut content = String::from("BT /F1 10 Tf 100 700 Td");
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            content.push_str(" 0 -12 Td");
        }
        content.push_str(&format!(" ({line}) Tj"));
    }
    content.push_str(" ET");
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Courier /Encoding /WinAnsiEncoding >>"
            .to_owned(),
    ];
    let mut file = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(file.len());
        file.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let table = file.len();
    let mut xref = format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for offset in offsets {
        xref.push_str(&format!("{offset:010} 00000 n \n"));
    }
    xref.push_str(&format!(
        "trailer\n<< /Root 1 0 R /Size {} >>\nstartxref\n{table}\n%%EOF\n",
        objects.len() + 1
    ));
    file.extend_from_slice(xref.as_bytes());
    let path = dir.join("lines.pdf");
    std::fs::write(&path, file).unwrap();
    path
}
