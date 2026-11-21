// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The predictor functions of 7.4.4.4. Each fixture starts from the
//! samples, applies the predictor forwards with the harness and wraps the
//! result in stored Flate blocks; the samples are the expectation.

use kernkraft_pdf::filter::FilterKind;

use crate::{
    assert_too_large, clean, damaged, decode, decode_capped, png_rows, stored, tiff_rows, Run,
    PNG_AVERAGE, PNG_NONE, PNG_PAETH, PNG_SUB, PNG_UP,
};

const KIND: FilterKind = FilterKind::Flate;

fn parms(predictor: u32, colors: usize, bits: usize, columns: usize) -> String {
    format!(
        "<</Filter /FlateDecode /DecodeParms <</Predictor {predictor} /Colors {colors} \
         /BitsPerComponent {bits} /Columns {columns}>>>>"
    )
}

/// Samples that rise and fall across the whole byte range, so differences
/// wrap in both directions.
fn samples(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i * 89 + i * i * 3) as u8).collect()
}

fn decode_png(raw: &[u8], colors: usize, bits: usize, columns: usize, tags: &[u8]) -> Run {
    let row_bytes = (colors * bits * columns).div_ceil(8);
    let pixel_bytes = (colors * bits / 8).max(1);
    decode(
        &parms(15, colors, bits, columns),
        &stored(&png_rows(raw, row_bytes, pixel_bytes, tags)),
    )
}

fn decode_tiff(raw: &[u8], colors: usize, bits: usize, columns: usize) -> Run {
    decode(
        &parms(2, colors, bits, columns),
        &stored(&tiff_rows(raw, colors, bits, columns)),
    )
}

#[test]
fn hand_computed_png_rows() {
    let flate =
        |predicted: &[u8], columns: usize| decode(&parms(12, 1, 8, columns), &stored(predicted));
    // Sub: each byte adds the one to its left. 5, 5 + 250, 255 + 10 - 256.
    assert_eq!(flate(&[PNG_SUB, 5, 250, 10], 3), clean(&[5, 255, 9]));
    // Up: the second row adds the first. 200 + 100 - 256, 7 + 0.
    assert_eq!(
        flate(&[PNG_NONE, 200, 7, PNG_UP, 100, 0], 2),
        clean(&[200, 7, 44, 7])
    );
    // Average, second row: left 0 and above 9 give 4; then left 5 and
    // above 4 give 4.
    assert_eq!(
        flate(&[PNG_NONE, 9, 4, PNG_AVERAGE, 1, 1], 2),
        clean(&[9, 4, 5, 5])
    );
    // Paeth, second row: left 0, above 10, upper-left 0: the estimate 10
    // is above itself. Then left 13, above 20, upper-left 10: the estimate
    // 23 is nearest 20.
    assert_eq!(
        flate(&[PNG_NONE, 10, 20, PNG_PAETH, 3, 1], 2),
        clean(&[10, 20, 13, 21])
    );
}

#[test]
fn hand_computed_tiff_rows() {
    // 8 bits, 3 colours: each component adds the one three bytes back.
    assert_eq!(
        decode(&parms(2, 3, 8, 2), &stored(&[250, 1, 2, 10, 255, 3])),
        clean(&[250, 1, 2, 4, 0, 5])
    );
    // 16 bits, most significant byte first: FFFF + 0002 wraps to 0001.
    assert_eq!(
        decode(&parms(2, 1, 16, 2), &stored(&[0xFF, 0xFF, 0x00, 0x02])),
        clean(&[0xFF, 0xFF, 0x00, 0x01])
    );
    // 4 bits: 3, 3 + 5, 8 + 15 - 16, 7 + 2.
    assert_eq!(
        decode(&parms(2, 1, 4, 4), &stored(&[0x35, 0xF2])),
        clean(&[0x38, 0x79])
    );
    // 2 bits: 3, 3 + 1 - 4, 0 + 2, 2 + 3 - 4.
    assert_eq!(
        decode(&parms(2, 1, 2, 4), &stored(&[0b11_01_10_11])),
        clean(&[0b11_00_10_01])
    );
    // 1 bit: a running sum modulo 2 of 1 0 1 1 0 0 0 0.
    assert_eq!(
        decode(&parms(2, 1, 1, 8), &stored(&[0b1011_0000])),
        clean(&[0b1101_1111])
    );
    // Each row starts again: the second row's first sample has nothing to
    // its left.
    assert_eq!(
        decode(&parms(2, 1, 8, 2), &stored(&[1, 2, 3, 4])),
        clean(&[1, 3, 3, 7])
    );
}

#[test]
fn png_none_sub_up_average_paeth() {
    let raw = samples(3 * 4 * 3);
    for tag in [PNG_NONE, PNG_SUB, PNG_UP, PNG_AVERAGE, PNG_PAETH] {
        assert_eq!(decode_png(&raw, 3, 8, 4, &[tag]), clean(&raw), "tag {tag}");
    }
}

#[test]
fn png_rows_with_different_tags() {
    let raw = samples(10 * 7 * 3);
    let tags = [PNG_NONE, PNG_SUB, PNG_UP, PNG_AVERAGE, PNG_PAETH];
    assert_eq!(decode_png(&raw, 3, 8, 7, &tags), clean(&raw));
    assert_eq!(
        decode_png(&raw, 3, 8, 7, &[PNG_PAETH, PNG_AVERAGE, PNG_UP]),
        clean(&raw)
    );
}

#[test]
fn png_declared_function_need_not_match() {
    let raw = samples(4 * 5);
    let encoded = stored(&png_rows(&raw, 5, 1, &[PNG_PAETH]));
    for predictor in 10..=15 {
        assert_eq!(decode(&parms(predictor, 1, 8, 5), &encoded), clean(&raw));
    }
}

#[test]
fn png_first_row_and_first_pixel() {
    let raw = samples(12);
    // With nothing above, Up predicts 0 and Paeth predicts the left byte.
    assert_eq!(png_rows(&raw, 12, 3, &[PNG_UP])[1..], raw);
    assert_eq!(
        png_rows(&raw, 12, 3, &[PNG_PAETH])[1..],
        png_rows(&raw, 12, 3, &[PNG_SUB])[1..]
    );
    for tag in [PNG_UP, PNG_AVERAGE, PNG_PAETH, PNG_SUB] {
        assert_eq!(decode_png(&raw, 3, 8, 4, &[tag]), clean(&raw), "tag {tag}");
    }
    // One column: nothing is ever to the left.
    let column = samples(9);
    for tag in [PNG_SUB, PNG_UP, PNG_AVERAGE, PNG_PAETH] {
        assert_eq!(
            decode_png(&column, 3, 8, 1, &[tag]),
            clean(&column),
            "tag {tag}"
        );
    }
}

#[test]
fn png_bytes_per_pixel() {
    let tags = [PNG_SUB, PNG_AVERAGE, PNG_PAETH];
    // 16 bits, one colour: two bytes to a pixel.
    let raw = samples(3 * 5 * 2);
    assert_eq!(decode_png(&raw, 1, 16, 5, &tags), clean(&raw));
    // 16 bits, three colours: six.
    let raw = samples(3 * 4 * 6);
    assert_eq!(decode_png(&raw, 3, 16, 4, &tags), clean(&raw));
    // 1 bit, 12 columns: a row of two bytes, one byte to a pixel.
    let raw = samples(3 * 2);
    assert_eq!(decode_png(&raw, 1, 1, 12, &tags), clean(&raw));
    // 4 bits, three colours, five columns: 60 bits are eight bytes, and
    // the 12 bits of a pixel round down to one byte.
    let raw = samples(3 * 8);
    assert_eq!(decode_png(&raw, 3, 4, 5, &tags), clean(&raw));
    // 8 bits, four colours.
    let raw = samples(3 * 3 * 4);
    assert_eq!(decode_png(&raw, 4, 8, 3, &tags), clean(&raw));
}

#[test]
fn pixel_width_changes_the_result() {
    // The same predicted bytes read as one 16-bit sample per pixel and as
    // two 8-bit ones: Sub reaches two bytes back, or one.
    let predicted = [PNG_SUB, 1, 2, 3, 4];
    assert_eq!(
        decode(&parms(12, 1, 16, 2), &stored(&predicted)),
        clean(&[1, 2, 4, 6])
    );
    assert_eq!(
        decode(&parms(12, 1, 8, 4), &stored(&predicted)),
        clean(&[1, 3, 6, 10])
    );
}

#[test]
fn tiff_8_bit() {
    let raw = samples(4 * 6 * 3);
    assert_eq!(decode_tiff(&raw, 3, 8, 6), clean(&raw));
    assert_eq!(decode_tiff(&raw, 1, 8, 18), clean(&raw));
    assert_eq!(decode_tiff(&raw, 4, 8, 3), clean(&raw));
}

#[test]
fn tiff_16_bit() {
    let raw = samples(3 * 5 * 2 * 2);
    assert_eq!(decode_tiff(&raw, 2, 16, 5), clean(&raw));
    assert_eq!(decode_tiff(&raw, 1, 16, 10), clean(&raw));
}

#[test]
fn tiff_sub_byte() {
    // Rows that end inside a byte: 12 bits in 2 bytes, 18 in 3, 36 in 5.
    // The bits that pad a row are zero in the samples and stay so.
    let mask_row = |raw: &mut [u8], row_bytes: usize, row_bits: usize| {
        for row in raw.chunks_mut(row_bytes) {
            let last = row.len() - 1;
            row[last] &= 0xFF << (row_bytes * 8 - row_bits);
        }
    };
    for (colors, bits, columns) in [(1, 1, 12), (3, 2, 3), (3, 4, 3), (1, 4, 5), (2, 1, 9)] {
        let row_bits: usize = colors * bits * columns;
        let row_bytes = row_bits.div_ceil(8);
        let mut raw = samples(4 * row_bytes);
        mask_row(&mut raw, row_bytes, row_bits);
        assert_eq!(
            decode_tiff(&raw, colors, bits, columns),
            clean(&raw),
            "{colors} colours of {bits} bits, {columns} columns"
        );
    }
}

#[test]
fn predictor_1_changes_nothing() {
    let raw = samples(40);
    assert_eq!(decode(&parms(1, 3, 8, 4), &stored(&raw)), clean(&raw));
}

#[test]
fn bad_tag_is_damage() {
    let raw = samples(3 * 4);
    let mut predicted = png_rows(&raw, 4, 1, &[PNG_SUB]);
    predicted[5] = 5;
    assert_eq!(
        decode(&parms(12, 1, 8, 4), &stored(&predicted)),
        damaged(&raw[..4], KIND)
    );
    predicted[0] = 255;
    assert_eq!(
        decode(&parms(12, 1, 8, 4), &stored(&predicted)),
        damaged(b"", KIND)
    );
}

#[test]
fn short_last_row() {
    let raw = samples(3 * 4);
    let predicted = png_rows(&raw, 4, 1, &[PNG_PAETH]);
    assert_eq!(
        decode(
            &parms(12, 1, 8, 4),
            &stored(&predicted[..predicted.len() - 1])
        ),
        damaged(&raw[..11], KIND)
    );
    // A tag with no row after it.
    assert_eq!(
        decode(&parms(12, 1, 8, 4), &stored(&predicted[..11])),
        damaged(&raw[..8], KIND)
    );
    let differenced = tiff_rows(&raw, 1, 8, 4);
    assert_eq!(
        decode(&parms(2, 1, 8, 4), &stored(&differenced[..11])),
        damaged(&raw[..11], KIND)
    );
    // 16 bits, a row cut inside a sample: the whole samples are undone and
    // the odd byte is kept as it is.
    let wide = samples(2 * 6);
    let differenced = tiff_rows(&wide, 1, 16, 3);
    let run = decode(&parms(2, 1, 16, 3), &stored(&differenced[..11]));
    assert_eq!(
        run,
        damaged(&[&wide[..10], &differenced[10..11]].concat(), KIND)
    );
}

#[test]
fn damage_in_the_data_and_in_the_rows_is_reported_once() {
    let raw = samples(3 * 4);
    let predicted = png_rows(&raw, 4, 1, &[PNG_UP]);
    let stream = stored(&predicted);
    // The Flate data is cut inside the last row: 2 header bytes, 5 block
    // header bytes, then 15 bytes of rows less two.
    assert_eq!(
        decode(&parms(12, 1, 8, 4), &stream[..2 + 5 + 13]),
        damaged(&raw[..10], KIND)
    );
}

#[test]
fn no_rows() {
    assert_eq!(decode(&parms(12, 3, 8, 4), &stored(b"")), clean(b""));
    assert_eq!(decode(&parms(2, 3, 8, 4), &stored(b"")), clean(b""));
}

#[test]
fn cap_allows_the_tag_bytes() {
    let raw = samples(100 * 4);
    let encoded = stored(&png_rows(&raw, 4, 1, &[PNG_SUB]));
    let dictionary = parms(12, 1, 8, 4);
    assert_eq!(
        decode_capped(&dictionary, &encoded, 400).unwrap(),
        clean(&raw)
    );
    assert_too_large(decode_capped(&dictionary, &encoded, 399), 399);
    // TIFF rows carry no tags.
    let encoded = stored(&tiff_rows(&raw, 1, 8, 4));
    let dictionary = parms(2, 1, 8, 4);
    assert_eq!(
        decode_capped(&dictionary, &encoded, 400).unwrap(),
        clean(&raw)
    );
    assert_too_large(decode_capped(&dictionary, &encoded, 399), 399);
}

#[test]
fn the_tag_allowance_is_not_a_way_past_the_cap() {
    // One column of one byte: every row is a tag and a byte, so a result of
    // 500 bytes comes from 1000.
    let dictionary = parms(12, 1, 8, 1);
    let run = decode_capped(&dictionary, &stored(&[PNG_NONE; 1000]), 500).unwrap();
    assert_eq!(run, clean(&[0; 500]));
    assert_too_large(
        decode_capped(&dictionary, &stored(&[PNG_NONE; 1002]), 500),
        500,
    );
    // Rows of four bytes: 500 bytes of result come from 625.
    let dictionary = parms(12, 1, 8, 4);
    assert_too_large(
        decode_capped(&dictionary, &stored(&[PNG_NONE; 700]), 500),
        500,
    );
}

#[test]
fn xref_stream_shape() {
    // Four entries of a cross-reference stream with W [1 3 1]: type, a
    // three-byte offset and a generation, in rows of five bytes.
    let raw = [
        1, 0, 0, 17, 0, //
        1, 0, 0, 81, 0, //
        1, 0, 0, 146, 0, //
        1, 0, 1, 25, 0,
    ];
    let predicted = png_rows(&raw, 5, 1, &[PNG_UP]);
    // Up leaves the first row and stores differences below it.
    assert_eq!(predicted[..12], [2, 1, 0, 0, 17, 0, 2, 0, 0, 0, 64, 0]);
    assert_eq!(
        decode(
            "<</Filter /FlateDecode /DecodeParms <</Columns 5 /Predictor 12>>>>",
            &stored(&predicted)
        ),
        clean(&raw)
    );
}
