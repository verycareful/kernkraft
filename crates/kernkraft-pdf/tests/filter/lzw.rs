// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! LZWDecode, 7.4.4.2, and `EarlyChange`, 7.4.4.3.

use kernkraft_pdf::filter::FilterKind;

use crate::{
    assert_too_large, clean, damaged, decode, decode_capped, irregular, lzw_codes, lzw_encode,
    pack_codes, pattern, png_rows, unmarked, LZW_CLEAR, LZW_END, PNG_UP,
};

const LZW: &str = "<</Filter /LZWDecode>>";
const LZW_LATE: &str = "<</Filter /LZWDecode /DecodeParms <</EarlyChange 0>>>>";
const KIND: FilterKind = FilterKind::Lzw;

/// 7.4.4.2, Example 1: the input of Table 7.
const TABLE_7_INPUT: [u8; 10] = [45, 45, 45, 45, 45, 65, 45, 45, 45, 66];
/// Table 7's output codes, all 9 bits long.
const TABLE_7_CODES: [u16; 9] = [256, 45, 258, 258, 65, 259, 66, 257, 0];
/// 7.4.4.2, Example 2: those codes packed into bytes.
const EXAMPLE_2: [u8; 9] = [0x80, 0x0B, 0x60, 0x50, 0x22, 0x0C, 0x0C, 0x85, 0x01];

fn nine_bit(codes: &[u16]) -> Vec<u8> {
    pack_codes(&codes.iter().map(|&code| (code, 9)).collect::<Vec<_>>())
}

#[test]
fn example_2_of_7_4_4_2() {
    assert_eq!(decode(LZW, &EXAMPLE_2), clean(&TABLE_7_INPUT));
}

#[test]
fn the_harness_reproduces_table_7_and_example_2() {
    let (codes, clears) = lzw_codes(&TABLE_7_INPUT, true, true);
    let expected: Vec<(u16, u8)> = TABLE_7_CODES[..8].iter().map(|&code| (code, 9)).collect();
    assert_eq!(codes, expected);
    assert_eq!(clears, 0);
    assert_eq!(lzw_encode(&TABLE_7_INPUT, true), EXAMPLE_2);
}

#[test]
fn no_leading_clear_code() {
    assert_eq!(
        decode(LZW, &nine_bit(&TABLE_7_CODES[1..8])),
        clean(&TABLE_7_INPUT)
    );
}

#[test]
fn width_switch_with_early_change_1_and_0() {
    // No two-byte sequence repeats in the first 251 bytes, so each code
    // creates an entry and entry 511 is reached within 300 codes.
    let input = pattern(3000);
    let early = lzw_encode(&input, true);
    let late = lzw_encode(&input, false);
    assert!(lzw_codes(&input, true, true)
        .0
        .iter()
        .any(|&(_, width)| width == 10));
    assert_ne!(early, late);
    assert_eq!(decode(LZW, &early), clean(&input));
    assert_eq!(decode(LZW_LATE, &late), clean(&input));
    assert_ne!(decode(LZW_LATE, &early), clean(&input));
    assert_ne!(decode(LZW, &late), clean(&input));
}

#[test]
fn every_width_is_reached() {
    let input = irregular(20_000);
    for early_change in [true, false] {
        let (codes, _) = lzw_codes(&input, early_change, true);
        for width in 9..=12 {
            assert!(codes.iter().any(|&(_, w)| w == width), "width {width}");
        }
        let dictionary = if early_change { LZW } else { LZW_LATE };
        assert_eq!(decode(dictionary, &pack_codes(&codes)), clean(&input));
    }
}

#[test]
fn full_table_then_clear() {
    let input = irregular(100_000);
    for early_change in [true, false] {
        let (codes, clears) = lzw_codes(&input, early_change, true);
        assert!(clears >= 2, "{clears} clears");
        let dictionary = if early_change { LZW } else { LZW_LATE };
        assert_eq!(decode(dictionary, &pack_codes(&codes)), clean(&input));
    }
}

#[test]
fn full_table_without_a_clear() {
    let input = irregular(100_000);
    for early_change in [true, false] {
        let (codes, clears) = lzw_codes(&input, early_change, false);
        assert_eq!(clears, 0);
        // The table filled: more codes were written than it has entries.
        assert!(codes.len() > 2 * 4096);
        let dictionary = if early_change { LZW } else { LZW_LATE };
        assert_eq!(decode(dictionary, &pack_codes(&codes)), clean(&input));
    }
}

#[test]
fn every_byte_value() {
    let bytes: Vec<u8> = (0..=u8::MAX).collect();
    assert_eq!(decode(LZW, &lzw_encode(&bytes, true)), clean(&bytes));
    assert_eq!(decode(LZW_LATE, &lzw_encode(&bytes, false)), clean(&bytes));
}

#[test]
fn empty() {
    assert_eq!(decode(LZW, &nine_bit(&[LZW_CLEAR, LZW_END])), clean(b""));
    assert_eq!(decode(LZW, &lzw_encode(b"", true)), clean(b""));
    assert_eq!(decode(LZW, &nine_bit(&[LZW_END])), clean(b""));
    assert_eq!(decode(LZW, b""), unmarked(b"", KIND));
}

#[test]
fn missing_marker() {
    assert_eq!(
        decode(LZW, &nine_bit(&TABLE_7_CODES[..7])),
        unmarked(&TABLE_7_INPUT, KIND)
    );
}

#[test]
fn invalid_code_is_damage() {
    // After the clear and one code the table ends at 257: 300 names
    // nothing.
    assert_eq!(
        decode(LZW, &nine_bit(&[LZW_CLEAR, 45, 300, LZW_END])),
        damaged(b"-", KIND)
    );
}

#[test]
fn bytes_after_the_marker_are_ignored() {
    let mut input = EXAMPLE_2.to_vec();
    input.extend_from_slice(&[0xFF; 40]);
    assert_eq!(decode(LZW, &input), clean(&TABLE_7_INPUT));
}

#[test]
fn bomb_stops_at_the_cap() {
    let encoded = lzw_encode(&vec![0; 4 << 20], true);
    // 7.4.4.1, NOTE 2: zeros compress by about 1365 to 1.
    assert!(encoded.len() < (4 << 20) / 1000);
    assert_too_large(decode_capped(LZW, &encoded, 1000), 1000);
}

#[test]
fn cap_met_and_passed() {
    assert_eq!(
        decode_capped(LZW, &EXAMPLE_2, 10).unwrap(),
        clean(&TABLE_7_INPUT)
    );
    assert_too_large(decode_capped(LZW, &EXAMPLE_2, 9), 9);
    let input = pattern(100_000);
    let encoded = lzw_encode(&input, true);
    assert_eq!(
        decode_capped(LZW, &encoded, 100_000).unwrap(),
        clean(&input)
    );
    assert_too_large(decode_capped(LZW, &encoded, 99_999), 99_999);
}

#[test]
fn predictor_after_lzw() {
    let raw = [10, 20, 30, 40, 11, 19, 35, 250];
    let predicted = png_rows(&raw, 4, 1, &[PNG_UP]);
    // Up on the first row leaves it; the second row is the differences.
    assert_eq!(predicted, [2, 10, 20, 30, 40, 2, 1, 255, 5, 210]);
    assert_eq!(
        decode(
            "<</Filter /LZWDecode /DecodeParms <</Predictor 12 /Columns 4>>>>",
            &lzw_encode(&predicted, true)
        ),
        clean(&raw)
    );
}
