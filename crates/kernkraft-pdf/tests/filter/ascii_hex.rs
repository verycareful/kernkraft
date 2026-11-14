// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! ASCIIHexDecode, 7.4.2.

use kernkraft_pdf::filter::FilterKind;
use kernkraft_pdf::syntax::ObjectId;
use kernkraft_pdf::{Diagnostic, DiagnosticKind, Diagnostics};

use crate::{assert_too_large, clean, damaged, decode, decode_capped, pipeline, unmarked, OFFSET};

const HEX: &str = "<</Filter /ASCIIHexDecode>>";
const KIND: FilterKind = FilterKind::AsciiHex;

#[test]
fn pairs_and_both_cases() {
    // N is 4E and o is 6F.
    assert_eq!(decode(HEX, b"4E6f>"), clean(b"No"));
    assert_eq!(decode(HEX, b"00ffFF7f>"), clean(&[0x00, 0xFF, 0xFF, 0x7F]));
}

#[test]
fn white_space_is_ignored() {
    assert_eq!(
        decode(HEX, b" 4\x00E\t6\nF\x0c7\r6 >"),
        clean(&[0x4E, 0x6F, 0x76])
    );
}

#[test]
fn odd_digit_is_padded() {
    assert_eq!(decode(HEX, b"901FA>"), clean(&[0x90, 0x1F, 0xA0]));
    assert_eq!(decode(HEX, b"7>"), clean(&[0x70]));
}

#[test]
fn empty() {
    assert_eq!(decode(HEX, b">"), clean(b""));
    assert_eq!(decode(HEX, b" \n>"), clean(b""));
}

#[test]
fn bytes_after_the_marker_are_ignored() {
    assert_eq!(decode(HEX, b"41>zz 42"), clean(b"A"));
}

#[test]
fn missing_marker() {
    assert_eq!(decode(HEX, b"4142"), unmarked(b"AB", KIND));
    assert_eq!(decode(HEX, b"414"), unmarked(&[0x41, 0x40], KIND));
    assert_eq!(decode(HEX, b""), unmarked(b"", KIND));
}

#[test]
fn bad_byte_is_damage() {
    assert_eq!(decode(HEX, b"41X42>"), damaged(b"A", KIND));
    assert_eq!(decode(HEX, b"4X"), damaged(b"", KIND));
    assert_eq!(decode(HEX, b"41<42>"), damaged(b"A", KIND));
    assert_eq!(decode(HEX, b"4142\xFF"), damaged(b"AB", KIND));
}

#[test]
fn cap_met_and_passed() {
    assert_eq!(decode_capped(HEX, b"4142>", 2).unwrap(), clean(b"AB"));
    assert_too_large(decode_capped(HEX, b"414243>", 2), 2);
    // The padded half byte counts.
    assert_too_large(decode_capped(HEX, b"41424>", 2), 2);
    assert_eq!(decode_capped(HEX, b">", 0).unwrap(), clean(b""));
}

#[test]
fn diagnostic_carries_the_offset_and_object() {
    let object = ObjectId {
        number: 3,
        generation: 0,
    };
    let mut diagnostics = Diagnostics::new();
    diagnostics.set_object(Some(object));
    let decoded = pipeline(HEX)
        .unwrap()
        .decode(b"41", 10, OFFSET, &mut diagnostics)
        .unwrap();
    assert_eq!(decoded.data.as_ref(), b"A");
    assert_eq!(
        diagnostics.as_slice(),
        [Diagnostic {
            kind: DiagnosticKind::MissingEndOfData { filter: KIND },
            object: Some(object),
            offset: OFFSET,
        }]
    );
}
