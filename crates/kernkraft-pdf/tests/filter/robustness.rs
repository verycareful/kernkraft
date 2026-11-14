// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Damaged input: every truncation, substitution and splice of each
//! fixture decodes to a result or to `DecodedTooLarge`, never to a panic,
//! and never to more than the cap.

use kernkraft_pdf::{DiagnosticKind, PdfError};

use crate::{
    decode_capped, deflate, encode85, irregular, lzw_encode, png_rows, stored, tiff_rows,
    PNG_AVERAGE, PNG_PAETH, PNG_SUB, PNG_UP,
};

struct Fixture {
    dictionary: &'static str,
    encoded: Vec<u8>,
    /// The cap of every run: four times the decoded length.
    cap: usize,
}

/// The bytes a substitution puts in place of each byte of a fixture, with
/// the byte's own value with its low bit flipped.
const REPLACEMENTS: [u8; 9] = [0x00, 0x0A, 0x20, b'>', b'~', b'z', b'u', 0x80, 0xFF];

fn fixtures() -> Vec<Fixture> {
    let data = irregular(600);
    let hex: Vec<u8> = data
        .iter()
        .flat_map(|byte| format!("{byte:02X}").into_bytes())
        .chain(*b">")
        .collect();
    let mut runs = Vec::new();
    for chunk in data.chunks(100) {
        runs.push(99);
        runs.extend_from_slice(chunk);
        runs.extend_from_slice(&[200, 7]);
    }
    runs.push(128);
    let rows = png_rows(&data, 12, 3, &[PNG_SUB, PNG_UP, PNG_AVERAGE, PNG_PAETH]);
    // Six literal runs of 100 bytes, each followed by 257 - 200 repeats.
    let runs_len = data.len() + 6 * (257 - 200);
    let fixture_of = |dictionary, encoded, decoded_len: usize| Fixture {
        dictionary,
        encoded,
        cap: 4 * decoded_len,
    };
    let fixture = |dictionary, encoded| fixture_of(dictionary, encoded, data.len());
    vec![
        fixture("<</Filter /ASCIIHexDecode>>", hex.clone()),
        fixture("<</Filter /ASCII85Decode>>", encode85(&data)),
        fixture_of("<</Filter /RunLengthDecode>>", runs, runs_len),
        fixture("<</Filter /LZWDecode>>", lzw_encode(&data, true)),
        fixture(
            "<</Filter /LZWDecode /DecodeParms <</EarlyChange 0>>>>",
            lzw_encode(&data, false),
        ),
        fixture("<</Filter /FlateDecode>>", stored(&data)),
        fixture("<</Filter /FlateDecode>>", deflate(&data)),
        fixture(
            "<</Filter /FlateDecode /DecodeParms <</Predictor 15 /Colors 3 /Columns 4>>>>",
            deflate(&rows),
        ),
        fixture(
            "<</Filter /LZWDecode /DecodeParms <</Predictor 2 /Colors 3 /Columns 4>>>>",
            lzw_encode(&tiff_rows(&data, 3, 8, 4), true),
        ),
        fixture(
            "<</Filter /FlateDecode /DecodeParms <</Predictor 2 /BitsPerComponent 4 /Columns 7>>>>",
            stored(&tiff_rows(&data, 1, 4, 7)),
        ),
        fixture(
            "<</Filter [/ASCII85Decode /LZWDecode]>>",
            encode85(&lzw_encode(&data, true)),
        ),
        fixture("<</Filter [/ASCIIHexDecode /FlateDecode]>>", {
            deflate(&data)
                .iter()
                .flat_map(|byte| format!("{byte:02x}").into_bytes())
                .chain(*b">")
                .collect()
        }),
        fixture(
            "<</Filter [/FlateDecode /FlateDecode]>>",
            deflate(&deflate(&data)),
        ),
    ]
}

/// Decodes and checks what must hold of any input.
fn check(fixture: &Fixture, input: &[u8]) {
    match decode_capped(fixture.dictionary, input, fixture.cap) {
        Ok(run) => {
            assert!(run.data.len() <= fixture.cap);
            let damage = run
                .kinds
                .iter()
                .any(|kind| matches!(kind, DiagnosticKind::FilterDataDamaged { .. }));
            assert_eq!(run.complete, !damage, "{}", fixture.dictionary);
        }
        Err(PdfError::DecodedTooLarge { cap }) => assert_eq!(cap, fixture.cap as u64),
        Err(other) => panic!("{}: {other:?}", fixture.dictionary),
    }
}

#[test]
fn every_fixture_decodes_whole() {
    for fixture in fixtures() {
        let run = decode_capped(fixture.dictionary, &fixture.encoded, fixture.cap).unwrap();
        assert!(run.complete, "{}", fixture.dictionary);
        assert_eq!(run.kinds, [], "{}", fixture.dictionary);
        assert_eq!(run.data.len() * 4, fixture.cap, "{}", fixture.dictionary);
    }
}

#[test]
fn every_truncation_returns() {
    for fixture in fixtures() {
        for len in 0..fixture.encoded.len() {
            check(&fixture, &fixture.encoded[..len]);
        }
    }
}

#[test]
fn every_single_byte_substitution_returns() {
    for fixture in fixtures() {
        let mut input = fixture.encoded.clone();
        for offset in 0..input.len() {
            let original = input[offset];
            for replacement in REPLACEMENTS.into_iter().chain([original ^ 1]) {
                input[offset] = replacement;
                check(&fixture, &input);
            }
            input[offset] = original;
        }
    }
}

#[test]
fn every_splice_returns() {
    let fixtures = fixtures();
    for first in &fixtures {
        for second in &fixtures {
            // Every seventh cut keeps the test to seconds; the cuts still
            // fall at every position modulo each filter's unit.
            for cut in (0..first.encoded.len()).step_by(7) {
                let mut input = first.encoded[..cut].to_vec();
                input.extend_from_slice(&second.encoded);
                check(first, &input);
            }
        }
    }
}
