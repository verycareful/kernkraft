// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! FlateDecode, 7.4.4.1: the zlib wrapper of RFC 1950 around the blocks of
//! RFC 1951.

use kernkraft_pdf::filter::FilterKind;
use kernkraft_pdf::DiagnosticKind;

use crate::{
    adler32, assert_too_large, clean, damaged, decode, decode_capped, deflate, encode85, pattern,
    stored, stored_blocks, stored_in, Run, STORED_BLOCK_MAX, ZLIB_HEADER,
};

const FLATE: &str = "<</Filter /FlateDecode>>";
const KIND: FilterKind = FilterKind::Flate;

/// Whole data whose checksum is absent or wrong.
fn unchecked(data: &[u8]) -> Run {
    Run {
        data: data.to_vec(),
        complete: true,
        kinds: vec![DiagnosticKind::FlateChecksum],
    }
}

/// The FLG byte that makes `cmf` and it a multiple of 31, with `flags` set.
fn flg_for(cmf: u8, flags: u8) -> u8 {
    (0..31)
        .map(|check| flags | check)
        .find(|&flg| u16::from_be_bytes([cmf, flg]).is_multiple_of(31))
        .unwrap()
}

/// A zlib stream of one stored block under the header `cmf`, `flg`.
fn with_header(cmf: u8, flg: u8, bytes: &[u8]) -> Vec<u8> {
    let mut out = vec![cmf, flg];
    out.extend_from_slice(&stored_blocks(bytes, STORED_BLOCK_MAX));
    out.extend_from_slice(&adler32(bytes).to_be_bytes());
    out
}

#[test]
fn adler32_of_the_harness() {
    assert_eq!(adler32(b""), 1);
    // One byte 97: s1 = 1 + 97, s2 = 0 + 98.
    assert_eq!(adler32(b"a"), 0x0062_0062);
}

#[test]
fn stored_block() {
    assert_eq!(decode(FLATE, &stored(b"hello")), clean(b"hello"));
    assert_eq!(decode(FLATE, &stored(b"")), clean(b""));
}

#[test]
fn empty_fixed_block() {
    // RFC 1951, 3.2.3 and 3.2.6: BFINAL 1, BTYPE 01 (fixed codes), then
    // the seven zero bits of code 256, end of block. Bits fill a byte from
    // its low end: 1, then 1 0, then zeros: 03 00. The Adler-32 of nothing
    // is 1. 0x789C is 31 * 994.
    assert_eq!(
        decode(FLATE, &[0x78, 0x9C, 0x03, 0x00, 0x00, 0x00, 0x00, 0x01]),
        clean(b"")
    );
}

#[test]
fn several_stored_blocks() {
    let bytes = pattern(200_000);
    assert_eq!(decode(FLATE, &stored(&bytes)), clean(&bytes));
    assert_eq!(decode(FLATE, &stored_in(&bytes, 1)[..]), clean(&bytes));
}

#[test]
fn compressed_data() {
    let bytes = pattern(1 << 20);
    let encoded = deflate(&bytes);
    assert!(encoded.len() < bytes.len() / 10);
    assert_eq!(decode(FLATE, &encoded), clean(&bytes));
    let text = b"The quick brown fox jumps over the lazy dog. ".repeat(500);
    assert_eq!(decode(FLATE, &deflate(&text)), clean(&text));
    assert_eq!(decode(FLATE, &deflate(b"")), clean(b""));
}

#[test]
fn every_window_size_and_level_is_read() {
    // CINFO 0 to 7 with any FLEVEL bits: the stored block inside does not
    // depend on them.
    for info in 0..=7u8 {
        for level in 0..4u8 {
            let cmf = info << 4 | 8;
            let stream = with_header(cmf, flg_for(cmf, level << 6), b"abc");
            assert_eq!(decode(FLATE, &stream), clean(b"abc"), "CMF {cmf:#04x}");
        }
    }
}

#[test]
fn bytes_after_the_trailer_are_ignored() {
    let mut stream = stored(b"hello");
    stream.extend_from_slice(b"\r\nendstream");
    assert_eq!(decode(FLATE, &stream), clean(b"hello"));
}

#[test]
fn checksum_wrong() {
    let mut stream = stored(b"hello");
    *stream.last_mut().unwrap() ^= 1;
    assert_eq!(decode(FLATE, &stream), unchecked(b"hello"));
    let mut compressed = deflate(&pattern(5000));
    let first_of_trailer = compressed.len() - 4;
    compressed[first_of_trailer] ^= 0x80;
    assert_eq!(decode(FLATE, &compressed), unchecked(&pattern(5000)));
}

#[test]
fn checksum_absent_or_short() {
    let stream = stored(b"hello");
    for cut in 1..=4 {
        assert_eq!(
            decode(FLATE, &stream[..stream.len() - cut]),
            unchecked(b"hello"),
            "cut {cut}"
        );
    }
}

#[test]
fn truncated_before_the_final_block() {
    let bytes = pattern(20);
    // Two blocks of ten bytes, each five bytes of block header and its
    // data, after the two-byte zlib header.
    let stream = stored_in(&bytes, 10);
    assert_eq!(stream.len(), 2 + (5 + 10) + (5 + 10) + 4);
    // Cut after three bytes of the second block's data.
    assert_eq!(
        decode(FLATE, &stream[..2 + 15 + 5 + 3]),
        damaged(&bytes[..13], KIND)
    );
    // Cut at the end of the first block, and inside the second's header.
    assert_eq!(
        decode(FLATE, &stream[..2 + 15]),
        damaged(&bytes[..10], KIND)
    );
    assert_eq!(
        decode(FLATE, &stream[..2 + 15 + 2]),
        damaged(&bytes[..10], KIND)
    );
    // Header only.
    assert_eq!(decode(FLATE, &ZLIB_HEADER), damaged(b"", KIND));
}

#[test]
fn compressed_data_cut_short_keeps_a_prefix() {
    let bytes = pattern(100_000);
    let encoded = deflate(&bytes);
    let run = decode(FLATE, &encoded[..encoded.len() / 2]);
    assert!(!run.complete);
    assert_eq!(
        run.kinds,
        [DiagnosticKind::FilterDataDamaged { filter: KIND }]
    );
    assert!(!run.data.is_empty());
    assert_eq!(run.data, bytes[..run.data.len()]);
}

#[test]
fn header_refused() {
    let none = damaged(b"", KIND);
    // CM 7: not DEFLATE.
    assert_eq!(
        decode(FLATE, &with_header(0x77, flg_for(0x77, 0), b"abc")),
        none
    );
    // CINFO 8: a window of 64 KiB, which RFC 1950 does not allow.
    assert_eq!(
        decode(FLATE, &with_header(0x88, flg_for(0x88, 0), b"abc")),
        none
    );
    // The check bits one off.
    assert_eq!(decode(FLATE, &with_header(0x78, 0x02, b"abc")), none);
    // FDICT set.
    assert_eq!(
        decode(FLATE, &with_header(0x78, flg_for(0x78, 0x20), b"abc")),
        none
    );
    // One byte, and none.
    assert_eq!(decode(FLATE, &[0x78]), none);
    assert_eq!(decode(FLATE, &[]), none);
    // Raw DEFLATE with no wrapper: 01 is a final stored block's header and
    // 0x0103 is not a multiple of 31.
    assert_eq!(
        decode(FLATE, &stored_blocks(b"abc", STORED_BLOCK_MAX)),
        none
    );
}

#[test]
fn invalid_block() {
    // BFINAL 1 with BTYPE 11, which RFC 1951, 3.2.3 reserves.
    assert_eq!(
        decode(FLATE, &[0x78, 0x01, 0x07, 0, 0, 0, 0]),
        damaged(b"", KIND)
    );
    // A good block, then the reserved type.
    let mut stream = ZLIB_HEADER.to_vec();
    stream.extend_from_slice(&[0x00, 3, 0, !3, 0xFF, b'a', b'b', b'c']);
    stream.extend_from_slice(&[0x07, 0, 0, 0, 0]);
    assert_eq!(decode(FLATE, &stream), damaged(b"abc", KIND));
}

#[test]
fn stored_length_check_fails() {
    // NLEN repeats LEN where it should be its complement.
    let stream = [0x78, 0x01, 0x01, 5, 0, 5, 0, b'h', b'e', b'l', b'l', b'o'];
    assert_eq!(decode(FLATE, &stream), damaged(b"", KIND));
}

#[test]
fn bomb_stops_at_the_cap() {
    let encoded = deflate(&vec![0; 64 << 20]);
    assert!(encoded.len() < (64 << 20) / 500);
    assert_too_large(decode_capped(FLATE, &encoded, 1000), 1000);
}

#[test]
fn two_stages_stop_at_the_cap() {
    let twice = deflate(&deflate(&vec![0; 64 << 20]));
    let chain = "<</Filter [/FlateDecode /FlateDecode]>>";
    // The first stage gives a few tens of kilobytes, inside the cap; the
    // second would give 64 MiB.
    assert_too_large(decode_capped(chain, &twice, 1 << 20), 1 << 20);
    // The first stage alone passes a small cap.
    assert_too_large(decode_capped(chain, &twice, 100), 100);
}

#[test]
fn cap_met_and_passed() {
    let bytes = pattern(100_000);
    for encoded in [stored(&bytes), deflate(&bytes)] {
        assert_eq!(
            decode_capped(FLATE, &encoded, 100_000).unwrap(),
            clean(&bytes)
        );
        assert_too_large(decode_capped(FLATE, &encoded, 99_999), 99_999);
    }
    assert_eq!(decode_capped(FLATE, &stored(b""), 0).unwrap(), clean(b""));
}

#[test]
fn chained_with_ascii85() {
    let text = b"BT /F1 12 Tf (Unfiltered streams can be read easily) Tj ET";
    assert_eq!(
        decode(
            "<</Filter [/ASCII85Decode /FlateDecode]>>",
            &encode85(&stored(text))
        ),
        clean(text)
    );
}
