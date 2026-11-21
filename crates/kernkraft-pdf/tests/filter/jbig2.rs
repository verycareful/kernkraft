// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! JBIG2Decode, 7.4.7. The stream is the one the clause itself gives as
//! Example 2: a 52 by 66 page whose symbol dictionary is in the globals.

use std::sync::Arc;

use kernkraft_pdf::syntax::ObjectId;
use kernkraft_pdf::{Diagnostics, PdfError};

use crate::{assert_too_large, pipeline, OFFSET, ROOMY};

/// 7.4.7, Example 1: the filter entries of the image object.
const DICTIONARY: &str = "<</Filter /JBIG2Decode /DecodeParms <</JBIG2Globals 6 0 R>>>>";

/// Example 2, part b: segment 0, the symbol dictionary, which has page
/// association 0 and so goes in the globals stream.
const GLOBALS: &str = "00 00 00 00 00 01 00 00 00 00 32 00 00 03 FF FD FF 02 FE FE FE 00 00 \
                       00 01 00 00 00 01 2A E2 25 AE A9 A5 A5 38 B4 D9 99 9C 5C 8E 56 EF 0F \
                       87 27 F2 B5 3D 4E 37 EF 79 5C C5 50 6D FF AC";

/// Example 2, part c: the page information segment (52 by 66) and the
/// immediate text region segment, with the end-of-page segment of part d
/// left out as the clause directs.
const PAGE: &str = "00 00 00 01 30 00 01 00 00 00 13 00 00 00 34 00 00 00 42 00 00 00 00 \
                    00 00 00 00 40 00 00 00 00 00 02 06 20 00 01 00 00 00 1E 00 00 00 34 \
                    00 00 00 42 00 00 00 00 00 00 00 00 02 00 10 00 00 00 02 31 DB 51 CE \
                    51 FF AC";

const WIDTH: usize = 52;
const HEIGHT: usize = 66;
const ROW_BYTES: usize = WIDTH.div_ceil(8);

fn hex(text: &str) -> Vec<u8> {
    text.split_whitespace()
        .map(|byte| u8::from_str_radix(byte, 16).unwrap())
        .collect()
}

fn globals() -> Arc<[u8]> {
    hex(GLOBALS).into()
}

/// Decodes the page, giving the pipeline the globals as a reader of the
/// file does once it has found object 6.
fn decode_page(page: &[u8], cap: usize) -> Result<(Vec<u8>, bool), PdfError> {
    let mut pipeline = pipeline(DICTIONARY).unwrap();
    assert_eq!(
        pipeline.pending_jbig2_globals(),
        Some(ObjectId {
            number: 6,
            generation: 0
        })
    );
    pipeline.provide_jbig2_globals(
        ObjectId {
            number: 6,
            generation: 0,
        },
        globals(),
    );
    assert_eq!(pipeline.pending_jbig2_globals(), None);
    let mut diagnostics = Diagnostics::new();
    let decoded = pipeline.decode(page, cap, OFFSET, &mut diagnostics)?;
    Ok((decoded.data.into_owned(), decoded.complete))
}

#[test]
fn the_page_of_example_2_is_one_bit_a_pixel_in_whole_bytes() {
    let (data, complete) = decode_page(&hex(PAGE), ROOMY).unwrap();
    assert!(complete);
    assert_eq!(data.len(), ROW_BYTES * HEIGHT);
    // The padding bits of each row (columns 52 to 55) are white, 1.
    for row in data.chunks(ROW_BYTES) {
        assert_eq!(row[ROW_BYTES - 1] & 0x0F, 0x0F);
    }
    // The page has marks (0 is black) and is mostly white.
    let black = data
        .chunks(ROW_BYTES)
        .flat_map(|row| (0..WIDTH).map(move |x| row[x / 8] >> (7 - x % 8) & 1 == 0))
        .filter(|black| *black)
        .count();
    assert!(
        black > 0 && black < WIDTH * HEIGHT / 2,
        "{black} black pixels"
    );
}

#[test]
fn a_cap_below_the_page_refuses_it_before_decoding() {
    let result = decode_page(&hex(PAGE), ROW_BYTES * HEIGHT - 1);
    assert_too_large(result, ROW_BYTES * HEIGHT - 1);
}

#[test]
fn globals_named_but_not_given_are_an_error() {
    let (page, mut diagnostics) = (hex(PAGE), Diagnostics::new());
    let result = pipeline(DICTIONARY)
        .unwrap()
        .decode(&page, ROOMY, OFFSET, &mut diagnostics);
    assert!(matches!(
        result,
        Err(PdfError::FilterParameters {
            filter: "JBIG2Decode",
            key: "JBIG2Globals"
        })
    ));
}

#[test]
fn a_page_decoded_without_its_globals_is_damaged_not_repaired() {
    // The text region refers to the dictionary in the globals. The
    // decoder runs in its strict mode, so the missing segment is damage and
    // not a blank page.
    let (page, mut diagnostics) = (hex(PAGE), Diagnostics::new());
    let decoded = pipeline("<</Filter /JBIG2Decode>>")
        .unwrap()
        .decode(&page, ROOMY, OFFSET, &mut diagnostics)
        .unwrap();
    assert!(!decoded.complete);
}

#[test]
fn garbage_is_damaged_data() {
    let mut diagnostics = Diagnostics::new();
    let decoded = pipeline("<</Filter /JBIG2Decode>>")
        .unwrap()
        .decode(
            b"not a jbig2 stream at all",
            ROOMY,
            OFFSET,
            &mut diagnostics,
        )
        .unwrap();
    assert!(!decoded.complete);
    assert!(decoded.data.is_empty());
}

#[test]
fn globals_must_be_a_reference() {
    crate::assert_parameters(
        pipeline("<</Filter /JBIG2Decode /DecodeParms <</JBIG2Globals 5>>>>"),
        "JBIG2Decode",
        "JBIG2Globals",
    );
}
