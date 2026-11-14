// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the filter layer, one module per source file. The helpers here
//! encode data the way each filter's clause describes, so a test states the
//! decoded bytes it starts from and never bytes copied from a decoder.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

mod ascii85;
mod ascii_hex;
mod flate;
mod lzw;
mod pipeline;
mod predictor;
mod robustness;
mod run_length;

use std::collections::HashMap;
use std::fmt::Debug;
use std::io::Write;

use kernkraft_pdf::filter::{FilterKind, Pipeline};
use kernkraft_pdf::syntax::limits::MAX_FILTERS;
use kernkraft_pdf::syntax::{Parser, Value};
use kernkraft_pdf::{DiagnosticKind, Diagnostics, Limit, MemorySource, PdfError};

/// The offset every test passes as the start of the stream's data.
pub const OFFSET: u64 = 77;

/// A cap no test's data comes near, for tests that are not about the cap.
pub const ROOMY: usize = 1 << 26;

/// A prime, so the pattern does not repeat on a power-of-two boundary.
const PATTERN_PERIOD: usize = 251;

/// `n` bytes where byte `i` is `i % PATTERN_PERIOD`.
pub fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % PATTERN_PERIOD) as u8).collect()
}

/// `n` bytes with no short period: byte `i` is the low byte of `i * i / 7`.
pub fn irregular(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * i / 7) as u8).collect()
}

/// The pipeline of a stream dictionary written as PDF text.
pub fn pipeline(dictionary: &str) -> Result<Pipeline, PdfError> {
    let source = MemorySource::new(dictionary.as_bytes().to_vec());
    let mut diagnostics = Diagnostics::new();
    // The parser's own diagnostics (an integer past a portable limit) are
    // not what these tests are about.
    let object = Parser::new(&source).parse_object(&mut diagnostics).unwrap();
    match object.value() {
        Value::Dictionary(dictionary) => Pipeline::from_dictionary(dictionary),
        other => panic!("not a dictionary: {other:?}"),
    }
}

/// What one decode gave.
#[derive(Debug, PartialEq)]
pub struct Run {
    pub data: Vec<u8>,
    pub complete: bool,
    pub kinds: Vec<DiagnosticKind>,
}

/// Decodes `input` through the pipeline of `dictionary`, allowing `cap`
/// bytes. Every diagnostic must carry [`OFFSET`] and no object.
pub fn decode_capped(dictionary: &str, input: &[u8], cap: usize) -> Result<Run, PdfError> {
    let mut diagnostics = Diagnostics::new();
    let decoded = pipeline(dictionary)
        .unwrap()
        .decode(input, cap, OFFSET, &mut diagnostics)?;
    for diagnostic in diagnostics.as_slice() {
        assert_eq!((diagnostic.offset, diagnostic.object), (OFFSET, None));
    }
    Ok(Run {
        data: decoded.data.into_owned(),
        complete: decoded.complete,
        kinds: diagnostics.into_vec().into_iter().map(|d| d.kind).collect(),
    })
}

pub fn decode(dictionary: &str, input: &[u8]) -> Run {
    decode_capped(dictionary, input, ROOMY).unwrap()
}

/// Whole data that ended at its end-of-data marker.
pub fn clean(data: &[u8]) -> Run {
    Run {
        data: data.to_vec(),
        complete: true,
        kinds: Vec::new(),
    }
}

/// Whole data that ended with no end-of-data marker.
pub fn unmarked(data: &[u8], filter: FilterKind) -> Run {
    Run {
        data: data.to_vec(),
        complete: true,
        kinds: vec![DiagnosticKind::MissingEndOfData { filter }],
    }
}

/// The data decoded before `filter` met damage.
pub fn damaged(data: &[u8], filter: FilterKind) -> Run {
    Run {
        data: data.to_vec(),
        complete: false,
        kinds: vec![DiagnosticKind::FilterDataDamaged { filter }],
    }
}

pub fn assert_too_large<T: Debug>(result: Result<T, PdfError>, expected_cap: usize) {
    match result {
        Err(PdfError::DecodedTooLarge { cap }) => assert_eq!(cap, expected_cap as u64),
        other => panic!("expected DecodedTooLarge at {expected_cap}, got {other:?}"),
    }
}

pub fn assert_parameters<T: Debug>(result: Result<T, PdfError>, filter: &str, key: &str) {
    match result {
        Err(PdfError::FilterParameters {
            filter: found_filter,
            key: found_key,
        }) => assert_eq!((found_filter, found_key), (filter, key)),
        other => panic!("expected FilterParameters {key} of {filter}, got {other:?}"),
    }
}

// ASCII base-85, 7.4.3.

const BASE_85: u32 = 85;
/// The character of digit 0.
const FIRST_DIGIT_85: u8 = b'!';

/// The five base-85 digits of a group's value, most significant first.
pub fn digits85(mut value: u32) -> [u8; 5] {
    let mut digits = [0; 5];
    for digit in digits.iter_mut().rev() {
        *digit = (value % BASE_85) as u8;
        value /= BASE_85;
    }
    digits
}

/// The encoding of 7.4.3, without the end-of-data marker.
pub fn encode85_unmarked(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for group in bytes.chunks(4) {
        let mut padded = [0; 4];
        padded[..group.len()].copy_from_slice(group);
        let value = u32::from_be_bytes(padded);
        if group.len() == 4 && value == 0 {
            out.push(b'z');
            continue;
        }
        let characters = digits85(value).map(|digit| digit + FIRST_DIGIT_85);
        out.extend_from_slice(&characters[..group.len() + 1]);
    }
    out
}

pub fn encode85(bytes: &[u8]) -> Vec<u8> {
    let mut out = encode85_unmarked(bytes);
    out.extend_from_slice(b"~>");
    out
}

// LZW, 7.4.4.2.

pub const LZW_CLEAR: u16 = 256;
pub const LZW_END: u16 = 257;
const LZW_FIRST_ENTRY: u16 = 258;
const LZW_FIRST_WIDTH: u8 = 9;
const LZW_LAST_WIDTH: u8 = 12;
/// One past entry 4095, the last of the table.
const LZW_TABLE_END: u16 = 4096;

/// Packs codes into bytes, high-order bit first, with the bits left over in
/// the last byte set to 0.
pub fn pack_codes(codes: &[(u16, u8)]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut bits: u32 = 0;
    let mut count: u32 = 0;
    for &(code, width) in codes {
        bits = bits << width | u32::from(code);
        count += u32::from(width);
        while count >= 8 {
            out.push((bits >> (count - 8)) as u8);
            count -= 8;
        }
        bits &= (1 << count) - 1;
    }
    if count > 0 {
        out.push((bits << (8 - count)) as u8);
    }
    out
}

/// The codes of the encoder of 7.4.4.2 with their widths, and how many
/// clear-table codes it issued after the first.
///
/// The code length grows after the creation of entry 511, 1023 and 2047
/// with `early_change`, and one code later without it. When the table is
/// full the encoder issues a clear-table code if `clear_when_full`;
/// otherwise it goes on with the full table.
pub fn lzw_codes(
    bytes: &[u8],
    early_change: bool,
    clear_when_full: bool,
) -> (Vec<(u16, u8)>, usize) {
    let mut codes = vec![(LZW_CLEAR, LZW_FIRST_WIDTH)];
    let mut clears = 0;
    let mut table: HashMap<Vec<u8>, u16> = HashMap::new();
    let mut next = LZW_FIRST_ENTRY;
    let mut width = LZW_FIRST_WIDTH;
    let code_of = |sequence: &[u8], table: &HashMap<Vec<u8>, u16>| match sequence {
        [byte] => u16::from(*byte),
        _ => table[sequence],
    };
    let mut sequence: Vec<u8> = Vec::new();
    for &byte in bytes {
        let mut longer = sequence.clone();
        longer.push(byte);
        if longer.len() == 1 || table.contains_key(&longer) {
            sequence = longer;
            continue;
        }
        codes.push((code_of(&sequence, &table), width));
        sequence = vec![byte];
        if next == LZW_TABLE_END {
            continue;
        }
        table.insert(longer, next);
        next += 1;
        if next == LZW_TABLE_END {
            if clear_when_full {
                codes.push((LZW_CLEAR, width));
                clears += 1;
                table.clear();
                next = LZW_FIRST_ENTRY;
                width = LZW_FIRST_WIDTH;
            }
        } else if width < LZW_LAST_WIDTH && next + u16::from(early_change) > 1 << width {
            width += 1;
        }
    }
    if !sequence.is_empty() {
        codes.push((code_of(&sequence, &table), width));
    }
    codes.push((LZW_END, width));
    (codes, clears)
}

pub fn lzw_encode(bytes: &[u8], early_change: bool) -> Vec<u8> {
    pack_codes(&lzw_codes(bytes, early_change, true).0)
}

// zlib, RFC 1950, and DEFLATE's stored blocks, RFC 1951, 3.2.4.

/// RFC 1950, 8.2: the largest prime below 65536.
const ADLER_MODULUS: u32 = 65_521;
/// CMF for DEFLATE with a 32 KiB window, and the FLG that makes the two a
/// multiple of 31 with no preset dictionary: 0x7801 is 31 * 991.
pub const ZLIB_HEADER: [u8; 2] = [0x78, 0x01];
/// The most bytes a stored block holds: LEN is 16 bits.
pub const STORED_BLOCK_MAX: usize = 65_535;

pub fn adler32(bytes: &[u8]) -> u32 {
    let (mut s1, mut s2) = (1u32, 0u32);
    for &byte in bytes {
        s1 = (s1 + u32::from(byte)) % ADLER_MODULUS;
        s2 = (s2 + s1) % ADLER_MODULUS;
    }
    s2 << 16 | s1
}

/// The stored blocks of `bytes`, `block_len` bytes to a block, the last one
/// marked final: a header byte holding BFINAL with BTYPE 00, then LEN, its
/// complement NLEN, and the bytes.
pub fn stored_blocks(bytes: &[u8], block_len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut blocks: Vec<&[u8]> = bytes.chunks(block_len).collect();
    if blocks.is_empty() {
        blocks.push(&[]);
    }
    for (index, block) in blocks.iter().enumerate() {
        let len = u16::try_from(block.len()).unwrap();
        out.push(u8::from(index + 1 == blocks.len()));
        out.extend_from_slice(&len.to_le_bytes());
        out.extend_from_slice(&(!len).to_le_bytes());
        out.extend_from_slice(block);
    }
    out
}

/// A zlib stream of stored blocks.
pub fn stored_in(bytes: &[u8], block_len: usize) -> Vec<u8> {
    let mut out = ZLIB_HEADER.to_vec();
    out.extend_from_slice(&stored_blocks(bytes, block_len));
    out.extend_from_slice(&adler32(bytes).to_be_bytes());
    out
}

pub fn stored(bytes: &[u8]) -> Vec<u8> {
    stored_in(bytes, STORED_BLOCK_MAX)
}

/// A zlib stream of compressed blocks, from flate2's encoder.
pub fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

// Predictors, 7.4.4.4.

pub const PNG_NONE: u8 = 0;
pub const PNG_SUB: u8 = 1;
pub const PNG_UP: u8 = 2;
pub const PNG_AVERAGE: u8 = 3;
pub const PNG_PAETH: u8 = 4;

/// RFC 2083, 6.6.
fn paeth(a: i32, b: i32, c: i32) -> i32 {
    let p = a + b - c;
    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
    if pa <= pb && pa <= pc {
        a
    } else if pb <= pc {
        b
    } else {
        c
    }
}

/// Applies a PNG filter to each row of `raw` (RFC 2083, section 6): row `r`
/// gets the tag `tags[r % tags.len()]` and each byte becomes its difference
/// from the tag's prediction, bytes outside the image being 0.
pub fn png_rows(raw: &[u8], row_bytes: usize, pixel_bytes: usize, tags: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let zeros = vec![0; row_bytes];
    let rows: Vec<&[u8]> = raw.chunks(row_bytes).collect();
    for (r, row) in rows.iter().enumerate() {
        let above = if r == 0 { &zeros[..] } else { rows[r - 1] };
        let tag = tags[r % tags.len()];
        out.push(tag);
        for i in 0..row.len() {
            let a = if i >= pixel_bytes {
                i32::from(row[i - pixel_bytes])
            } else {
                0
            };
            let b = i32::from(above[i]);
            let c = if i >= pixel_bytes {
                i32::from(above[i - pixel_bytes])
            } else {
                0
            };
            let prediction = match tag {
                PNG_NONE => 0,
                PNG_SUB => a,
                PNG_UP => b,
                PNG_AVERAGE => (a + b) / 2,
                PNG_PAETH => paeth(a, b, c),
                other => panic!("no PNG filter {other}"),
            };
            out.push((i32::from(row[i]) - prediction).rem_euclid(256) as u8);
        }
    }
    out
}

/// The component at `index` of a row of `bits`-wide components packed from
/// the high-order bit down.
fn component(row: &[u8], index: usize, bits: usize) -> u32 {
    (0..bits).fold(0, |value, k| {
        let bit = index * bits + k;
        value << 1 | u32::from(row[bit / 8] >> (7 - bit % 8) & 1)
    })
}

fn set_component(row: &mut [u8], index: usize, bits: usize, value: u32) {
    for k in 0..bits {
        let bit = index * bits + k;
        let mask = 1 << (7 - bit % 8);
        if value >> (bits - 1 - k) & 1 == 1 {
            row[bit / 8] |= mask;
        } else {
            row[bit / 8] &= !mask;
        }
    }
}

/// Applies TIFF Predictor 2 to each row of `raw`: every component becomes
/// its difference, modulo `2^bits`, from the same component of the sample
/// to its left.
pub fn tiff_rows(raw: &[u8], colors: usize, bits: usize, columns: usize) -> Vec<u8> {
    let row_bytes = (colors * bits * columns).div_ceil(8);
    let mut out = raw.to_vec();
    for (row, source) in out.chunks_mut(row_bytes).zip(raw.chunks(row_bytes)) {
        for index in colors..colors * columns {
            let left = component(source, index - colors, bits);
            let value = component(source, index, bits);
            set_component(
                row,
                index,
                bits,
                value.wrapping_sub(left) & ((1 << bits) - 1),
            );
        }
    }
    out
}

#[test]
fn filter_names_are_those_of_table_6() {
    let names = [
        (FilterKind::AsciiHex, "ASCIIHexDecode"),
        (FilterKind::Ascii85, "ASCII85Decode"),
        (FilterKind::Lzw, "LZWDecode"),
        (FilterKind::Flate, "FlateDecode"),
        (FilterKind::RunLength, "RunLengthDecode"),
    ];
    for (kind, name) in names {
        assert_eq!(kind.name(), name);
        assert_eq!(kind.to_string(), name);
    }
}

#[test]
fn filter_count_limit_matches_its_constant() {
    assert_eq!(Limit::FilterCount.max(), MAX_FILTERS as u64);
    assert_eq!(Limit::FilterCount.to_string(), "filter count");
}

#[test]
fn error_messages() {
    assert_eq!(
        PdfError::DecodedTooLarge { cap: 5 }.to_string(),
        "decoded data is larger than the 5 bytes allowed"
    );
    assert_eq!(
        PdfError::FilterParameters {
            filter: "FlateDecode",
            key: "Columns"
        }
        .to_string(),
        "the Columns of FlateDecode is not valid"
    );
    assert_eq!(
        PdfError::UnknownFilter {
            name: "DCTDecode".into()
        }
        .to_string(),
        "DCTDecode is not a filter this library decodes"
    );
}
