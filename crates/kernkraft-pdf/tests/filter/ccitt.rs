// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! CCITTFaxDecode, 7.4.6. The inputs are written out code by code from the
//! tables of ITU-T T.4 (terminating codes) and T.6 (two-dimensional modes),
//! so the expected pixels are stated first and the bits follow from them.

use kernkraft_pdf::filter::FilterKind;

use crate::{assert_parameters, assert_too_large, clean, decode, decode_capped, pipeline};

const G4_8: &str = "<</Filter /CCITTFaxDecode /DecodeParms <</K -1 /Columns 8 /Rows 2>>>>";
const G4_8_BLACK_IS_1: &str =
    "<</Filter /CCITTFaxDecode /DecodeParms <</K -1 /Columns 8 /Rows 2 /BlackIs1 true>>>>";

/// A string of `0` and `1` characters (spaces ignored) packed high bit
/// first, the last byte padded with zeros.
fn bits(text: &str) -> Vec<u8> {
    let digits: Vec<u8> = text
        .bytes()
        .filter(|b| *b != b' ')
        .map(|b| b - b'0')
        .collect();
    digits
        .chunks(8)
        .map(|chunk| {
            chunk
                .iter()
                .enumerate()
                .fold(0, |byte, (i, bit)| byte | bit << (7 - i))
        })
        .collect()
}

/// T.6: the end-of-facsimile-block, two end-of-line codes.
const EOFB: &str = "000000000001 000000000001";

/// Eight pixels: white 0 and 1, black 2 to 4, white 5 to 7.
const ROW_00111000: u8 = 0b1100_0111;

#[test]
fn group_4_rows_with_a_black_run() {
    // Row 1, against a white reference line: the first changing element
    // is at 2 and b1 is at the end of the line, too far for a vertical
    // mode, so horizontal mode (001) with the white run 2 (0111) and the
    // black run 3 (10), then V0 (1) for the end of the line.
    // Row 2 equals row 1: V0 at 2, V0 at 5, V0 at the end.
    let input = bits(&format!("001 0111 10 1  1 1 1  {EOFB}"));
    assert_eq!(decode(G4_8, &input), clean(&[ROW_00111000, ROW_00111000]));
}

#[test]
fn black_is_1_inverts_the_bits() {
    let input = bits(&format!("001 0111 10 1  1 1 1  {EOFB}"));
    let inverted = !ROW_00111000;
    assert_eq!(
        decode(G4_8_BLACK_IS_1, &input),
        clean(&[inverted, inverted])
    );
}

#[test]
fn group_4_all_white_rows_are_one_bit_each() {
    let input = bits(&format!("1 1 {EOFB}"));
    assert_eq!(decode(G4_8, &input), clean(&[0xFF, 0xFF]));
}

#[test]
fn group_3_one_dimensional_rows() {
    // White 2 (0111), black 3 (10), white 3 (1000), per T.4's terminating
    // codes; with no end-of-line codes and a known height the data stops
    // after Rows rows.
    let dictionary = "<</Filter /CCITTFaxDecode /DecodeParms \
                      <</K 0 /Columns 8 /Rows 2 /EndOfBlock false>>>>";
    let input = bits("0111 10 1000  0111 10 1000");
    assert_eq!(
        decode(dictionary, &input),
        clean(&[ROW_00111000, ROW_00111000])
    );
}

#[test]
fn group_3_rows_start_on_a_byte_boundary_when_aligned() {
    let dictionary = "<</Filter /CCITTFaxDecode /DecodeParms \
                      <</K 0 /Columns 8 /Rows 2 /EndOfBlock false /EncodedByteAlign true>>>>";
    // Each row's ten bits are followed by six fill bits.
    let input = bits("0111 10 1000 000000  0111 10 1000 000000");
    assert_eq!(
        decode(dictionary, &input),
        clean(&[ROW_00111000, ROW_00111000])
    );
}

/// `testdata/image/bars-g4.bin` is the Group 4 strip libtiff writes for a
/// 40 by 6 image in which the pixels at columns 3 to 9 and 20 and 21 are 0,
/// except row 2, which has none, and row 4, which is all 0, and the rest
/// are 1. The strip is read as TIFF reads CCITT data (a 0 is white), so
/// with `BlackIs1` false the decoded bit is 1 where the image had a 0.
/// mutool draws the same strip the same way.
#[test]
fn group_4_strip_of_an_independent_encoder() {
    let input = include_bytes!("../../../../testdata/image/bars-g4.bin");
    let dictionary = "<</Filter /CCITTFaxDecode /DecodeParms <</K -1 /Columns 40 /Rows 6>>>>";
    let mut expected = Vec::new();
    for row in 0..6 {
        for byte in 0..5 {
            let mut value = 0_u8;
            for bit in 0..8 {
                let x = byte * 8 + bit;
                let zero = match row {
                    2 => false,
                    4 => true,
                    _ => (3..10).contains(&x) || (20..22).contains(&x),
                };
                value = value << 1 | u8::from(zero);
            }
            expected.push(value);
        }
    }
    assert_eq!(decode(dictionary, input), clean(&expected));
}

#[test]
fn rows_not_given_end_at_the_end_of_block() {
    let dictionary = "<</Filter /CCITTFaxDecode /DecodeParms <</K -1 /Columns 8>>>>";
    let input = bits(&format!("1 1 1 {EOFB}"));
    assert_eq!(decode(dictionary, &input), clean(&[0xFF; 3]));
}

#[test]
fn damaged_data_gives_the_rows_before_the_damage() {
    // One good row, then a code that is in no table: seven 0 bits and a
    // 1 (an end-of-line is 11 zeros and a 1) after a horizontal prefix.
    let input = bits("1 001 00000001");
    let run = decode(G4_8, &input);
    assert!(!run.complete);
    assert_eq!(run.data, [0xFF]);
}

#[test]
fn the_output_cap_stops_a_row_that_does_not_fit() {
    let input = bits(&format!("1 1 {EOFB}"));
    assert_too_large(decode_capped(G4_8, &input, 1), 1);
    assert_eq!(decode_capped(G4_8, &input, 2).unwrap().data, [0xFF, 0xFF]);
}

#[test]
fn parameters_are_read_with_table_11_defaults() {
    // 1728 columns, 216 bytes a row, K 0: an all-white row is the white
    // run 1728 (make-up 1728 is 010011011, then terminating 0 is 00110101).
    let dictionary = "<</Filter /CCITTFaxDecode /DecodeParms <</Rows 1 /EndOfBlock false>>>>";
    let input = bits("010011011 00110101");
    let run = decode(dictionary, &input);
    assert_eq!(run.data, vec![0xFF; 216]);
}

#[test]
fn parameter_errors_name_the_key() {
    for (parms, key) in [
        ("/Columns 0", "Columns"),
        ("/Columns -5", "Columns"),
        ("/Rows -1", "Rows"),
        ("/K (a)", "K"),
        ("/BlackIs1 1", "BlackIs1"),
        ("/EncodedByteAlign (x)", "EncodedByteAlign"),
    ] {
        let dictionary = format!("<</Filter /CCITTFaxDecode /DecodeParms <<{parms}>>>>");
        assert_parameters(pipeline(&dictionary), "CCITTFaxDecode", key);
    }
}

#[test]
fn the_filter_is_named_as_in_table_6() {
    assert_eq!(FilterKind::Ccitt.name(), "CCITTFaxDecode");
}
