// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! RunLengthDecode, 7.4.5.

use kernkraft_pdf::filter::FilterKind;

use crate::{assert_too_large, clean, damaged, decode, decode_capped, pattern, unmarked};

const RL: &str = "<</Filter /RunLengthDecode>>";
const KIND: FilterKind = FilterKind::RunLength;
/// The length byte that ends the data.
const END: u8 = 128;

#[test]
fn literal_runs() {
    assert_eq!(decode(RL, &[2, b'a', b'b', b'c', END]), clean(b"abc"));
    assert_eq!(decode(RL, &[0, b'a', END]), clean(b"a"));
    // Length 127 copies 128 bytes.
    let bytes = pattern(128);
    let mut input = vec![127];
    input.extend_from_slice(&bytes);
    input.push(END);
    assert_eq!(decode(RL, &input), clean(&bytes));
}

#[test]
fn repeat_runs() {
    // 257 - 255 = 2, 257 - 129 = 128.
    assert_eq!(decode(RL, &[255, b'a', END]), clean(b"aa"));
    assert_eq!(decode(RL, &[129, b'a', END]), clean(&[b'a'; 128]));
    assert_eq!(decode(RL, &[254, 0, END]), clean(&[0; 3]));
}

#[test]
fn mixed_runs() {
    assert_eq!(
        decode(RL, &[1, b'x', b'y', 253, b'z', 0, END, END]),
        clean(b"xyzzzz\x80")
    );
}

#[test]
fn marker_ends_the_data() {
    assert_eq!(decode(RL, &[0, b'a', END, 0, b'b', 5]), clean(b"a"));
}

#[test]
fn empty() {
    assert_eq!(decode(RL, &[END]), clean(b""));
    assert_eq!(decode(RL, &[]), unmarked(b"", KIND));
}

#[test]
fn missing_marker() {
    assert_eq!(decode(RL, &[0, b'a']), unmarked(b"a", KIND));
    assert_eq!(decode(RL, &[0, b'a', 255, b'b']), unmarked(b"abb", KIND));
}

#[test]
fn ends_inside_a_literal_run() {
    assert_eq!(decode(RL, &[5, b'a', b'b']), damaged(b"ab", KIND));
    assert_eq!(decode(RL, &[0]), damaged(b"", KIND));
}

#[test]
fn ends_before_the_repeated_byte() {
    assert_eq!(decode(RL, &[254]), damaged(b"", KIND));
    assert_eq!(decode(RL, &[0, b'a', 129]), damaged(b"a", KIND));
}

#[test]
fn cap_met_and_passed() {
    assert_eq!(
        decode_capped(RL, &[129, 7, END], 128).unwrap(),
        clean(&[7; 128])
    );
    assert_too_large(decode_capped(RL, &[129, 7, 0, 8, END], 128), 128);
    assert_too_large(decode_capped(RL, &[129, 7, END], 127), 127);
    assert_too_large(decode_capped(RL, &[3, 1, 2, 3, 4, END], 3), 3);
}

#[test]
fn long_repeats_stop_at_the_cap() {
    let input: Vec<u8> = [129, 0].repeat(1_000_000);
    assert_too_large(decode_capped(RL, &input, 1000), 1000);
}
