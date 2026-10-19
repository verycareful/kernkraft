// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use kernkraft_text::{decode, encode, LineEnding, ReadOnly};

const BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

#[test]
fn round_trip_is_identity() {
    let bodies: [&[u8]; 5] = [
        b"",
        b"one line",
        b"a\nb\nc\n",
        b"a\nb\nc",
        "caf\u{e9}\n\u{4e2d}\u{6587}\n".as_bytes(),
    ];
    for body in bodies {
        for ending in [LineEnding::Lf, LineEnding::CrLf] {
            for bom in [false, true] {
                let mut input = Vec::new();
                if bom {
                    input.extend_from_slice(BOM);
                }
                match ending {
                    LineEnding::Lf => input.extend_from_slice(body),
                    LineEnding::CrLf => {
                        for &b in body {
                            if b == b'\n' {
                                input.push(b'\r');
                            }
                            input.push(b);
                        }
                    }
                }
                let d = decode(&input);
                assert_eq!(d.read_only, None, "{input:?}");
                assert_eq!(d.bom, bom, "{input:?}");
                assert!(!d.text.contains('\r'), "{input:?}");
                assert_eq!(encode(&d.text, d.ending, d.bom), input, "{input:?}");
            }
        }
    }
}

#[test]
fn ending_is_detected() {
    assert_eq!(decode(b"a\r\nb\r\n").ending, LineEnding::CrLf);
    assert_eq!(decode(b"a\nb\n").ending, LineEnding::Lf);
    assert_eq!(decode(b"no break").ending, LineEnding::Lf);
    assert_eq!(decode(b"").ending, LineEnding::Lf);
}

#[test]
fn bom_is_not_in_text() {
    let d = decode(b"\xEF\xBB\xBFhi");
    assert!(d.bom);
    assert_eq!(d.text, "hi");
}

#[test]
fn mixed_endings_read_only() {
    let d = decode(b"a\nb\r\nc");
    assert_eq!(
        d.read_only,
        Some(ReadOnly::MixedLineEndings {
            lf: 1,
            crlf: 1,
            cr: 0
        })
    );
    assert_eq!(d.text, "a\nb\nc");
}

#[test]
fn lone_cr_read_only() {
    let d = decode(b"a\rb\nc");
    assert_eq!(
        d.read_only,
        Some(ReadOnly::MixedLineEndings {
            lf: 1,
            crlf: 0,
            cr: 1
        })
    );
    let only_cr = decode(b"a\rb");
    assert_eq!(
        only_cr.read_only,
        Some(ReadOnly::MixedLineEndings {
            lf: 0,
            crlf: 0,
            cr: 1
        })
    );
    assert_eq!(only_cr.text, "a\nb");
}

#[test]
fn invalid_utf8_reports_offset() {
    let d = decode(b"ab\xffcd");
    assert_eq!(d.read_only, Some(ReadOnly::NotUtf8 { offset: 2 }));
    assert_eq!(d.text, "ab\u{fffd}cd");
}

#[test]
fn invalid_utf8_offset_counts_the_bom() {
    let d = decode(b"\xEF\xBB\xBFab\xff");
    assert_eq!(d.read_only, Some(ReadOnly::NotUtf8 { offset: 5 }));
    assert!(d.bom);
}

#[test]
fn utf16_bom_read_only() {
    let le = decode(&[0xFF, 0xFE, b'h', 0, b'i', 0, b'\r', 0, b'\n', 0]);
    assert_eq!(le.read_only, Some(ReadOnly::Utf16));
    assert_eq!(le.text, "hi\n");
    let be = decode(&[0xFE, 0xFF, 0, b'h', 0, b'i']);
    assert_eq!(be.read_only, Some(ReadOnly::Utf16));
    assert_eq!(be.text, "hi");
    assert!(!be.bom);
}

#[test]
fn nul_bytes_in_valid_utf8_still_text() {
    let input = b"a\0b\nc\0";
    let d = decode(input);
    assert_eq!(d.read_only, None);
    assert_eq!(d.text, "a\0b\nc\0");
    assert_eq!(encode(&d.text, d.ending, d.bom), input);
}
