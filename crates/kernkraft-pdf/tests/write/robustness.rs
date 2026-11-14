// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of whole files, read through the document layer
//! and written again: every truncation and every substitution of one byte
//! by a byte that means something to the reader. Each mutated file must
//! open or be refused with an error, its pages must be read or refused,
//! and a copy that is written must open again: a panic, a hang or a
//! written file that cannot be read fails the test.

use super::*;
use kernkraft_fs::NoProgress;

/// Bytes that change how the reader takes what follows: delimiters, white
/// space, digits at each end of the range, the letters of `R`, `n` and
/// `f`, and the extreme values.
const REPLACEMENTS: &[u8] = b"()<>[]/% \n\x0009Rnf\xFF";

fn fixtures() -> Vec<(Vec<u8>, &'static str)> {
    let objects = pages_document(2);
    vec![
        (
            table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").bytes,
            "",
        ),
        (
            stream_file(
                &[(3, &b"<< /Type /Page /Parent 2 0 R >>"[..])],
                &[
                    (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
                    (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
                ],
                "/Root 1 0 R",
            )
            .bytes,
            "",
        ),
        (encrypted_fixture("rc4-128-object-streams"), "user"),
    ]
}

fn exercise(bytes: &[u8], password: &str) {
    let Ok(document) = Document::from_bytes(bytes.to_vec(), &OPTIONS, password) else {
        return;
    };
    if let Ok(count) = document.page_count() {
        for number in 1..=count {
            let _ = document.page(number);
        }
    }
    if let Ok(written) = compact(&document, &SEED, &NoProgress) {
        let reopened = Document::from_bytes(written, &OPTIONS, password);
        assert!(
            reopened.is_ok(),
            "a written file does not open: {reopened:?}"
        );
    }
}

#[test]
fn every_truncation_reads_or_is_refused() {
    for (fixture, password) in fixtures() {
        for length in 0..fixture.len() {
            exercise(&fixture[..length], password);
        }
    }
}

#[test]
fn every_single_byte_substitution_reads_or_is_refused() {
    for (fixture, password) in fixtures() {
        let mut bytes = fixture.clone();
        for offset in 0..fixture.len() {
            for &replacement in REPLACEMENTS {
                bytes[offset] = replacement;
                exercise(&bytes, password);
            }
            bytes[offset] = fixture[offset];
        }
    }
}
