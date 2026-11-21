// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of a file of fonts: every truncation and every
//! substitution of one byte by a byte that means something to the reader.
//! Each mutated file must open or be refused, and every font in it must
//! read or be refused and show a string or none: a panic or a hang fails
//! the test.

use super::*;

/// Bytes that change how the reader takes what follows.
const REPLACEMENTS: &[u8] = b"()<>[]/% \n\x0009Rnf\xFF";

/// A file with a simple font with a `Differences` encoding and ToUnicode, a
/// Type 0 font with an embedded CMap and widths, and a Type 3 font.
fn fonts() -> Vec<u8> {
    let cmap = b"begincmap 2 begincodespacerange <00> <7F> <8000> <FFFF> endcodespacerange \
                 1 begincidrange <20> <7E> 100 endcidrange 1 begincidchar <8142> 7 endcidchar \
                 1 beginnotdefrange <00> <1F> 1 endnotdefrange endcmap";
    let to_unicode = b"begincmap 1 begincodespacerange <00> <FF> endcodespacerange \
                       1 beginbfchar <41> <0058> endbfchar \
                       1 beginbfrange <42> <44> [<0059> <005A> <0061>] endbfrange endcmap";
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (
            60,
            b"<< /Type /Font /Subtype /Type1 /BaseFont /Foo /FirstChar 65 /LastChar 66 \
              /Widths [500 600] /Encoding 61 0 R /ToUnicode 62 0 R >>"
                .to_vec(),
        ),
        (
            61,
            b"<< /BaseEncoding /WinAnsiEncoding /Differences [65 /Euro /bullet 300 /x] >>".to_vec(),
        ),
        (62, stream("", to_unicode)),
        (
            63,
            b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding 64 0 R \
              /DescendantFonts [65 0 R] /ToUnicode 62 0 R >>"
                .to_vec(),
        ),
        (64, stream("/Type /CMap /UseCMap /90ms-RKSJ-H", cmap)),
        (
            65,
            b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo \
              /CIDSystemInfo << /Registry (Adobe) /Ordering (Japan1) /Supplement 2 >> \
              /DW 900 /W [1 [500 600] 10 12 700] /W2 [3 [-900 250 700] 5 6 -800 100 200] \
              /CIDToGIDMap 66 0 R >>"
                .to_vec(),
        ),
        (66, stream("", &[0, 0, 0, 7, 1, 2])),
        (
            67,
            b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 100 100] \
              /FontMatrix [0.01 0 0 0.01 0 0] /CharProcs << /a 68 0 R >> \
              /Encoding << /Differences [97 /a] >> /FirstChar 97 /LastChar 97 /Widths [50] >>"
                .to_vec(),
        ),
        (68, stream("", b"50 0 d0")),
    ];
    let objects: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let mut all = pages_document(1);
    for (number, body) in &objects {
        all.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&all), "/Root 1 0 R /Info 3 0 R").bytes
}

const STRING: &[u8] = &[0x41, 0x42, 0x20, 0x81, 0x42, 0x00, 0x01, 0xFF, 0x81, 0x30];

fn exercise(bytes: &[u8]) {
    let Ok(document) = Document::from_bytes(bytes.to_vec(), &OPTIONS, "") else {
        return;
    };
    for number in [60, 63, 67] {
        let Ok(font) = load(&document, number) else {
            continue;
        };
        for c in font.chars(STRING) {
            let _ = (c.unicode, c.width, c.vertical);
        }
        let _ = font.embedded_program(&document);
        let _ = font.cid_to_gid(&document);
        let _ = font.descriptor();
    }
}

#[test]
fn the_unmutated_file_reads() {
    let document = Document::from_bytes(fonts(), &OPTIONS, "").unwrap();
    for number in [60, 63, 67] {
        let font = load(&document, number).unwrap();
        assert!(font.chars(STRING).count() > 0);
    }
}

#[test]
fn every_truncation_reads_or_is_refused() {
    let fixture = fonts();
    for length in 0..fixture.len() {
        exercise(&fixture[..length]);
    }
}

#[test]
fn every_single_byte_substitution_reads_or_is_refused() {
    let fixture = fonts();
    let mut bytes = fixture.clone();
    for offset in 0..fixture.len() {
        for &replacement in REPLACEMENTS {
            bytes[offset] = replacement;
            exercise(&bytes);
        }
        bytes[offset] = fixture[offset];
    }
}
