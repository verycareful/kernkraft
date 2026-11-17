// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A file that is cut, flipped or spliced is drawn as far as it can be or
//! refused with a typed error, never a panic or a hang.

use kernkraft_pdf::doc::Document;

use super::text::{truetype, truetype_font, type3_fonts};
use super::*;

/// Content that uses paths, clips, dashes, alpha and both kinds of font.
const RICH: &str = "q 0.5 0.6 0.7 rg 2 2 16 16 re W n [3 2] 0 d 2 w /G gs \
     1 0 0 RG 0 0 m 20 20 l S 0 0 1 rg 5 5 10 10 re f* \
     BT /F1 8 Tf 2 2 Td (AB) Tj 1 Tr 5 Ts (A) Tj 7 Tr (A) Tj ET \
     BT /T3 8 Tf 2 12 Td (ab) Tj ET Q 0 0 1 rg 0 0 3 3 re f";

fn rich_file() -> Fixture {
    let mut objects = truetype_font("/Encoding /WinAnsiEncoding", 32, &truetype("Test Sans"));
    let mut procedures = type3_fonts(&[
        ("a", "600 0 d0 0 0 500 700 re f"),
        ("b", "600 0 0 0 500 700 d1 q 1 0 0 rg 0 0 5 5 re f"),
    ]);
    // The Type 3 font is object 20 in its builder; move it to 40.
    let (_, font) = procedures.remove(0);
    objects.push((40, font));
    objects.extend(procedures);
    let extra: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    fixture(
        "/MediaBox [0 0 20 20]",
        RICH,
        "/Font << /F1 20 0 R /T3 40 0 R >> /ExtGState << /G << /ca 0.5 /CA 0.5 >> >>",
        &extra,
    )
}

fn render_bytes(bytes: Vec<u8>) -> Result<(), PdfError> {
    let doc = Document::from_bytes(bytes, &OPTIONS, "")?;
    render_with(&doc, &RenderOptions::default()).map(drop)
}

#[test]
fn the_rich_file_renders() {
    let doc = rich_file().open();
    let raster = render(&doc);
    assert_eq!(px(&raster, 1, 18), BLUE);
}

#[test]
fn every_truncation_is_drawn_or_refused() {
    let bytes = rich_file().bytes;
    let step = (bytes.len() / 80).max(1);
    for length in (0..bytes.len()).step_by(step) {
        let _ = render_bytes(bytes[..length].to_vec());
    }
}

#[test]
fn every_seventh_byte_substituted_is_drawn_or_refused() {
    let bytes = rich_file().bytes;
    for at in (0..bytes.len()).step_by(7) {
        for value in [0x00, 0xFF] {
            let mut damaged = bytes.clone();
            damaged[at] = value;
            let _ = render_bytes(damaged);
        }
    }
}

#[test]
fn a_type_3_glyph_that_shows_its_own_font_is_stopped() {
    // The description shows text in the font it belongs to, forever.
    let doc = square_page(
        "BT /T3 10 Tf 2 2 Td (a) Tj ET",
        "/Font << /T3 40 0 R >>",
        &[
            (
                40,
                b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
                  /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 41 0 R >> \
                  /Encoding << /Type /Encoding /Differences [97 /a] >> \
                  /FirstChar 97 /LastChar 97 /Widths [600] \
                  /Resources << /Font << /T3 40 0 R >> >> >>",
            ),
            (
                41,
                &stream("", b"600 0 d0 0 0 500 700 re f BT /T3 10 Tf (a) Tj ET"),
            ),
        ],
    );
    assert!(render_with(&doc, &RenderOptions::default()).is_err());
}
