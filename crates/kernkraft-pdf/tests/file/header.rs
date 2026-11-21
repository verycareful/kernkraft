// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The file header (7.5.2) and where it is looked for.

use kernkraft_pdf::file::{Version, HEADER_SEARCH_BYTES};
use kernkraft_pdf::{DiagnosticKind, PdfError};

use crate::{open, small_file, Builder, Row, HEAD, HEADER};

/// The file of [`small_file`] with `prefix` in place of its header line.
/// Offsets are counted from the first byte of the file.
fn with_prefix(prefix: &[u8]) -> Vec<u8> {
    let mut b = Builder {
        bytes: prefix.to_vec(),
    };
    let catalog = b.object(1, "<</Type /Catalog>>");
    let xref = b.table(&[(0, &[HEAD, Row::N(catalog, 0)])], "/Size 2 /Root 1 0 R");
    b.startxref(xref);
    b.bytes
}

#[test]
fn every_version_of_7_5_2_and_2_0_is_read() {
    for (major, minor) in (0..=7).map(|minor| (1, minor)).chain([(2, 0)]) {
        let bytes = with_prefix(format!("%PDF-{major}.{minor}\n").as_bytes());
        let file = open(&bytes).clean();
        assert_eq!(file.version(), Version { major, minor });
        assert_eq!(file.header_offset(), 0);
    }
}

#[test]
fn the_builder_header_is_version_1_7() {
    assert!(HEADER.starts_with(b"%PDF-1.7"));
    let (b, _) = small_file();
    assert_eq!(b.open().clean().version(), Version { major: 1, minor: 7 });
}

#[test]
fn bytes_before_the_header_are_reported() {
    let junk = b"\xEF\xBB\xBFsome mail header\r\n";
    let mut prefix = junk.to_vec();
    prefix.extend_from_slice(b"%PDF-1.4\n");
    let (file, diagnostics) = open(&with_prefix(&prefix)).with_diagnostics();
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::HeaderNotAtStart, junk.len() as u64)]
    );
    assert_eq!(file.header_offset(), junk.len() as u64);
    assert_eq!(file.version(), Version { major: 1, minor: 4 });
    assert!(!file.xref_rebuilt());
}

#[test]
fn the_header_is_found_up_to_the_end_of_the_search_window() {
    const HEADER_TEXT: &[u8] = b"%PDF-1.4";
    let last_whole = HEADER_SEARCH_BYTES - HEADER_TEXT.len();
    for (padding, found) in [(last_whole, true), (last_whole + 1, false)] {
        let mut prefix = vec![b' '; padding];
        prefix.extend_from_slice(HEADER_TEXT);
        prefix.push(b'\n');
        let opened = open(&with_prefix(&prefix));
        if found {
            let (file, diagnostics) = opened.with_diagnostics();
            assert_eq!(
                diagnostics,
                [(DiagnosticKind::HeaderNotAtStart, padding as u64)]
            );
            assert_eq!(file.version(), Version { major: 1, minor: 4 });
        } else {
            assert!(matches!(opened.result, Err(PdfError::MissingHeader)));
        }
    }
}

#[test]
fn a_file_without_a_header_is_refused() {
    for prefix in [
        &b""[..],
        b"%PD\n",
        b"%PDF-\n",
        b"%PDF-1\n",
        b"%PDF-1.\n",
        b"%PDF-x.4\n",
        b"%PDF-1.x\n",
        b"%PDF-300.1\n",
        b"%PDF-1.300\n",
        b"PDF-1.4\n",
    ] {
        let opened = open(&with_prefix(prefix));
        assert!(
            matches!(opened.result, Err(PdfError::MissingHeader)),
            "{:?}",
            String::from_utf8_lossy(prefix)
        );
    }
    assert!(matches!(open(b"").result, Err(PdfError::MissingHeader)));
}

#[test]
fn a_marker_with_no_version_is_passed_over_for_a_later_header() {
    let prefix = b"%PDF-none %PDF-1.3\n";
    let (file, diagnostics) = open(&with_prefix(prefix)).with_diagnostics();
    let second = prefix.len() as u64 - b"%PDF-1.3\n".len() as u64;
    assert_eq!(diagnostics, [(DiagnosticKind::HeaderNotAtStart, second)]);
    assert_eq!(file.version(), Version { major: 1, minor: 3 });
}
