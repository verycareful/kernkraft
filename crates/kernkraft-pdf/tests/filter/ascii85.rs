// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! ASCII85Decode, 7.4.3.

use kernkraft_pdf::filter::FilterKind;

use crate::{
    assert_too_large, clean, damaged, decode, decode_capped, digits85, encode85, encode85_unmarked,
    unmarked,
};

const A85: &str = "<</Filter /ASCII85Decode>>";
const KIND: FilterKind = FilterKind::Ascii85;

#[test]
fn full_group() {
    // `Man ` is 4D 61 6E 20, the number 0x4D616E20.
    let digits = digits85(0x4D61_6E20);
    // The digits are those of the relation of 7.4.3.
    let value = digits
        .iter()
        .fold(0u64, |value, &digit| value * 85 + u64::from(digit));
    assert_eq!(value, 0x4D61_6E20);
    let mut encoded: Vec<u8> = digits.iter().map(|digit| digit + b'!').collect();
    encoded.extend_from_slice(b"~>");
    assert_eq!(encode85(b"Man "), encoded);
    assert_eq!(decode(A85, &encoded), clean(b"Man "));
}

#[test]
fn zero_group_is_z() {
    assert_eq!(encode85(&[0; 4]), b"z~>");
    assert_eq!(decode(A85, b"z~>"), clean(&[0; 4]));
    assert_eq!(decode(A85, b"!!!!!~>"), clean(&[0; 4]));
    assert_eq!(decode(A85, b"zz!!!!!z~>"), clean(&[0; 16]));
}

#[test]
fn partial_groups() {
    for bytes in [
        &b"M"[..],
        b"Ma",
        b"Man",
        &[0xFF],
        &[0xFF, 0xFF],
        &[0xFF, 0xFF, 0xFF],
        &[0x00],
        &[0x00, 0x00],
        &[0x00, 0x00, 0x00],
        b"Man M",
        b"Man Ma",
        b"Man Man",
    ] {
        let encoded = encode85(bytes);
        // n bytes give n + 1 characters, a whole group five, then `~>`.
        let groups = bytes.len() / 4;
        assert_eq!(encoded.len(), groups * 5 + bytes.len() % 4 + 1 + 2);
        assert_eq!(decode(A85, &encoded), clean(bytes), "{bytes:?}");
    }
}

#[test]
fn every_byte_value_round_trips() {
    let bytes: Vec<u8> = (0..=u8::MAX).collect();
    assert_eq!(decode(A85, &encode85(&bytes)), clean(&bytes));
    for shift in 1..4 {
        assert_eq!(
            decode(A85, &encode85(&bytes[shift..])),
            clean(&bytes[shift..])
        );
    }
}

#[test]
fn white_space_anywhere() {
    let encoded = encode85(b"Man Ma");
    let mut spaced = Vec::new();
    for (index, &byte) in encoded[..encoded.len() - 2].iter().enumerate() {
        spaced.push(byte);
        spaced.push([0x00, 0x09, 0x0A, 0x0C, 0x0D, 0x20][index % 6]);
    }
    spaced.extend_from_slice(b"~>");
    assert_eq!(decode(A85, &spaced), clean(b"Man Ma"));
}

#[test]
fn largest_group() {
    // Digits 82 23 54 12 0 are 2^32 - 1; the last digit one higher is 2^32.
    assert_eq!(digits85(u32::MAX), [82, 23, 54, 12, 0]);
    assert_eq!(encode85(&[0xFF; 4]), b"s8W-!~>");
    assert_eq!(decode(A85, b"s8W-!~>"), clean(&[0xFF; 4]));
    assert_eq!(decode(A85, b"s8W-\"~>"), damaged(b"", KIND));
    assert_eq!(decode(A85, b"uuuuu~>"), damaged(b"", KIND));
    assert_eq!(decode(A85, b"zs8W-\"~>"), damaged(&[0; 4], KIND));
    // A final group whose filled value passes 2^32 - 1: `uu` is 84, 84 and
    // then 84, 84, 84.
    assert_eq!(decode(A85, b"uu~>"), damaged(b"", KIND));
}

#[test]
fn z_inside_a_group_is_damage() {
    assert_eq!(decode(A85, b"!!z~>"), damaged(b"", KIND));
    assert_eq!(decode(A85, b"z!z~>"), damaged(&[0; 4], KIND));
}

#[test]
fn one_character_final_group_is_damage() {
    assert_eq!(decode(A85, b"!~>"), damaged(b"", KIND));
    assert_eq!(decode(A85, b"z!"), damaged(&[0; 4], KIND));
}

#[test]
fn invalid_bytes() {
    let whole = encode85_unmarked(b"Man ");
    for bad in [&b"v"[..], b"{", b"~x", b"~", b"\x7F", b"\xFF", b"y", b">"] {
        let mut input = whole.clone();
        input.extend_from_slice(bad);
        input.extend_from_slice(b"~>");
        assert_eq!(decode(A85, &input), damaged(b"Man ", KIND), "{bad:?}");
    }
    // `<` is the digit 27, so `<~` is a group of one character followed by
    // a `~` that no `>` follows.
    let mut prefixed = b"<~".to_vec();
    prefixed.extend_from_slice(&encode85(b"Man "));
    assert_eq!(decode(A85, &prefixed), damaged(b"", KIND));
}

#[test]
fn missing_marker() {
    assert_eq!(
        decode(A85, &encode85_unmarked(b"Man ")),
        unmarked(b"Man ", KIND)
    );
    assert_eq!(
        decode(A85, &encode85_unmarked(b"Man Ma")),
        unmarked(b"Man Ma", KIND)
    );
    assert_eq!(decode(A85, b""), unmarked(b"", KIND));
}

#[test]
fn bytes_after_the_marker_are_ignored() {
    let mut input = encode85(b"Man");
    input.extend_from_slice(b"v{~>\xFF");
    assert_eq!(decode(A85, &input), clean(b"Man"));
}

#[test]
fn cap_met_and_passed() {
    assert_eq!(
        decode_capped(A85, &encode85(b"Man "), 4).unwrap(),
        clean(b"Man ")
    );
    assert_too_large(decode_capped(A85, &encode85(b"Man "), 3), 3);
    assert_eq!(
        decode_capped(A85, &encode85(b"Man Ma"), 6).unwrap(),
        clean(b"Man Ma")
    );
    assert_too_large(decode_capped(A85, &encode85(b"Man Ma"), 5), 5);
    assert_too_large(decode_capped(A85, b"z~>", 3), 3);
    // A thousand `z` are four thousand bytes.
    assert_too_large(decode_capped(A85, &[b'z'; 1000], 3999), 3999);
}
