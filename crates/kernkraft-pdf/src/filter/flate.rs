// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! FlateDecode: ISO 32000-1:2008, 7.4.4.1. The data is a zlib stream, RFC
//! 1950: a two-byte header (section 2.2), DEFLATE blocks (RFC 1951) and the
//! Adler-32 checksum of the decoded bytes (sections 2.2 and 8.2).
//!
//! The header and the checksum are read here and the blocks by the `flate2`
//! crate, so that data whose blocks all decode but whose checksum is absent
//! or wrong is told apart from data that is damaged.

use flate2::{Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status};

use super::output::Output;
use super::Stop;
use crate::PdfError;

/// RFC 1950, 2.2: the header's two bytes, CMF and FLG.
const HEADER_BYTES: usize = 2;
/// CM, the low four bits of CMF: 8 is DEFLATE, the only method defined.
const METHOD_MASK: u8 = 0x0F;
const METHOD_DEFLATE: u8 = 8;
/// CINFO, the high four bits of CMF: the window is `2^(CINFO + 8)` bytes and
/// values above 7 are not allowed.
const WINDOW_SHIFT: u32 = 4;
const MAX_WINDOW_INFO: u8 = 7;
/// CMF and FLG read as one 16-bit number are a multiple of 31.
const HEADER_CHECK_MODULUS: u16 = 31;
/// FDICT, bit 5 of FLG: a preset dictionary follows the header. PDF defines
/// none, so data that asks for one cannot be decoded.
const PRESET_DICTIONARY: u8 = 0x20;

/// RFC 1950, 2.2: the checksum is four bytes, most significant first.
const CHECKSUM_BYTES: usize = 4;
/// RFC 1950, 8.2: the largest prime below 65536.
const ADLER_MODULUS: u64 = 65_521;
/// How far the checksum's high half is shifted.
const ADLER_SHIFT: u32 = 16;

/// The bytes inflated between two checks of the output's limit: the size of
/// the largest DEFLATE window (RFC 1951, section 2).
const BLOCK_BYTES: usize = 32 * 1024;

/// The Adler-32 of the bytes given so far (RFC 1950, 8.2).
struct Adler32 {
    s1: u64,
    s2: u64,
}

impl Adler32 {
    fn new() -> Self {
        Self { s1: 1, s2: 0 }
    }

    fn update(&mut self, bytes: &[u8]) {
        // Reducing once per block is enough: after BLOCK_BYTES bytes s1 is
        // below 2^24 and s2 below 2^40, far inside a u64.
        for block in bytes.chunks(BLOCK_BYTES) {
            for &byte in block {
                self.s1 += u64::from(byte);
                self.s2 += self.s1;
            }
            self.s1 %= ADLER_MODULUS;
            self.s2 %= ADLER_MODULUS;
        }
    }

    fn value(&self) -> u64 {
        self.s2 << ADLER_SHIFT | self.s1
    }
}

/// The header this library writes: DEFLATE with the largest window
/// (CINFO 7) and FLEVEL 2, the default level, without a preset dictionary
/// (RFC 1950, 2.2).
const WRITTEN_CMF: u8 = (MAX_WINDOW_INFO << WINDOW_SHIFT) | METHOD_DEFLATE;
const FLEVEL_DEFAULT: u8 = 2 << 6;
/// The compression level written: 6, zlib's default, which FLEVEL 2
/// describes.
const WRITTEN_LEVEL: u32 = 6;

/// `data` encoded for FlateDecode: a zlib stream whose header and checksum
/// are written here and whose blocks `flate2` writes. The same data always
/// gives the same bytes from one version of `flate2`.
pub(crate) fn encode(data: &[u8]) -> Result<Vec<u8>, PdfError> {
    let check = |flg: u8| (u16::from(WRITTEN_CMF) << 8 | u16::from(flg)) % HEADER_CHECK_MODULUS;
    let fcheck = (HEADER_CHECK_MODULUS - check(FLEVEL_DEFAULT)) % HEADER_CHECK_MODULUS;
    // FCHECK is below 31, which fits FLG's low five bits.
    let flg = FLEVEL_DEFAULT | fcheck as u8;
    let mut out = vec![WRITTEN_CMF, flg];
    let mut compress = Compress::new(Compression::new(WRITTEN_LEVEL), false);
    let mut input = data;
    loop {
        out.reserve(BLOCK_BYTES);
        let before_in = compress.total_in();
        let status = compress
            .compress_vec(input, &mut out, FlushCompress::Finish)
            .map_err(|_| PdfError::Invalid("DEFLATE compression failed".to_string()))?;
        let consumed = usize::try_from(compress.total_in() - before_in).unwrap_or(input.len());
        input = input.get(consumed..).unwrap_or_default();
        if status == Status::StreamEnd {
            break;
        }
    }
    let mut adler = Adler32::new();
    adler.update(data);
    // The checksum is 32 bits wide (RFC 1950, 8.2).
    out.extend_from_slice(&(adler.value() as u32).to_be_bytes());
    Ok(out)
}

/// Whether the two header bytes describe DEFLATE data this filter can read.
fn header_is_valid(cmf: u8, flg: u8) -> bool {
    cmf & METHOD_MASK == METHOD_DEFLATE
        && cmf >> WINDOW_SHIFT <= MAX_WINDOW_INFO
        && u16::from_be_bytes([cmf, flg]).is_multiple_of(HEADER_CHECK_MODULUS)
        && flg & PRESET_DICTIONARY == 0
}

pub(super) fn decode(input: &[u8], out: &mut Output) -> Result<Stop, PdfError> {
    let Some((&[cmf, flg], mut rest)) = input.split_first_chunk::<HEADER_BYTES>() else {
        return Ok(Stop::Damaged);
    };
    if !header_is_valid(cmf, flg) {
        return Ok(Stop::Damaged);
    }
    let mut inflater = Decompress::new(false);
    let mut adler = Adler32::new();
    let mut block = vec![0; BLOCK_BYTES];
    loop {
        let (in_before, out_before) = (inflater.total_in(), inflater.total_out());
        let result = inflater.decompress(rest, &mut block, FlushDecompress::None);
        // The differences are at most the lengths of the two slices.
        let consumed = usize::try_from(inflater.total_in() - in_before).unwrap_or(rest.len());
        let produced = usize::try_from(inflater.total_out() - out_before).unwrap_or(block.len());
        let decoded = block.get(..produced).unwrap_or(&block);
        adler.update(decoded);
        out.extend(decoded)?;
        rest = rest.get(consumed..).unwrap_or(&[]);
        match result {
            Ok(Status::StreamEnd) => break,
            Ok(Status::Ok | Status::BufError) if consumed > 0 || produced > 0 => {}
            // No progress with room to write: the input ended before the
            // final block. An error: a block that is not DEFLATE.
            Ok(Status::Ok | Status::BufError) | Err(_) => return Ok(Stop::Damaged),
        }
    }
    let stored = rest
        .first_chunk::<CHECKSUM_BYTES>()
        .map(|bytes| u64::from(u32::from_be_bytes(*bytes)));
    Ok(if stored == Some(adler.value()) {
        Stop::EndOfData
    } else {
        Stop::ChecksumMismatch
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn adler32_of_nothing_and_of_one_byte() {
        // 8.2: s1 starts at 1 and s2 at 0.
        assert_eq!(Adler32::new().value(), 1);
        // One byte 97: s1 = 1 + 97 = 98, s2 = 0 + 98 = 98.
        let mut adler = Adler32::new();
        adler.update(b"a");
        assert_eq!(adler.value(), 98 << 16 | 98);
    }

    #[test]
    fn adler32_does_not_depend_on_how_the_bytes_are_split() {
        let bytes: Vec<u8> = (0..3 * BLOCK_BYTES + 17).map(|i| (i % 251) as u8).collect();
        let mut whole = Adler32::new();
        whole.update(&bytes);
        let mut parts = Adler32::new();
        for part in bytes.chunks(1000) {
            parts.update(part);
        }
        assert_eq!(whole.value(), parts.value());
        // Byte by byte, reducing at every step, as 8.2 states it.
        let (mut s1, mut s2) = (1u64, 0u64);
        for &byte in &bytes {
            s1 = (s1 + u64::from(byte)) % ADLER_MODULUS;
            s2 = (s2 + s1) % ADLER_MODULUS;
        }
        assert_eq!(whole.value(), s2 << 16 | s1);
    }

    /// What `encode` writes decodes back to the data, with its checksum,
    /// for nothing, for less than a block and for several blocks; the
    /// header is the usual 78 9C (RFC 1950, 2.2: CINFO 7, CM 8, FLEVEL 2).
    #[test]
    fn encode_round_trips() {
        let long: Vec<u8> = (0..5 * BLOCK_BYTES + 3)
            .map(|i| (i * 7 % 256) as u8)
            .collect();
        for data in [Vec::new(), b"Hello, Flate".to_vec(), long] {
            let encoded = encode(&data).unwrap();
            assert_eq!(&encoded[..2], &[0x78, 0x9C]);
            let mut out = Output::new(data.len(), 0);
            assert!(matches!(
                decode(&encoded, &mut out).unwrap(),
                Stop::EndOfData
            ));
            assert_eq!(out.into_vec(), data);
        }
    }
}
