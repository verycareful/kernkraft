// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Fonts written by `FontEmbedder`, drawn by the renderer: the glyph chosen
//! by CID (ISO 32000-1:2008, 9.7.4.2) and the pen moved by the `W` array
//! (9.7.4.3).
//!
//! In the test fonts `A` is a rectangle 500 by 700 units with advance 700
//! and `B` a square of 200 with advance 450, on an em of 1000. At size 10
//! from the origin (1, 2), `A` covers user x 1 to 6 and y 2 to 9, rows 11 to
//! 18 of the 20 by 20 page; the pen then moves 7, and `B` covers x 8 to 10
//! and y 2 to 4, rows 16 to 18.

use kernkraft_pdf::font::FontEmbedder;

use super::*;

fn drawn(program: Vec<u8>, text: &str) -> Raster {
    let mut embedder = FontEmbedder::new(program, 0).unwrap();
    let codes = embedder.encode(text).unwrap();
    let (doc, _) = embedded_page(&embedder, show_codes(&codes, 10.0, 1.0, 2.0));
    render(&doc)
}

fn check_a_then_b(raster: &Raster) {
    assert_box(raster, 1, 11, 6, 18, BLACK);
    // The pen moved the 7 points of A's advance, and nothing is drawn
    // between the two.
    assert_box(raster, 6, 11, 8, 18, WHITE);
    assert_box(raster, 8, 16, 10, 18, BLACK);
    assert_box(raster, 8, 11, 10, 16, WHITE);
    assert_box(raster, 10, 0, SIDE, SIDE, WHITE);
    assert_box(raster, 0, 0, SIDE, 11, WHITE);
}

#[test]
fn an_embedded_truetype_subset_is_drawn_glyph_by_glyph() {
    check_a_then_b(&drawn(embed_truetype(true), "AB"));
}

#[test]
fn an_embedded_open_type_subset_with_cff_outlines_is_drawn_glyph_by_glyph() {
    check_a_then_b(&drawn(embed_opentype(), "AB"));
}

#[test]
fn the_glyph_numbers_follow_the_order_shown_not_the_font_s() {
    // B is shown first and takes the number 1; the font has A there.
    let mut embedder = FontEmbedder::new(embed_truetype(true), 0).unwrap();
    let codes = embedder.encode("BA").unwrap();
    assert_eq!(codes, [0, 1, 0, 2]);
    let (doc, _) = embedded_page(&embedder, show_codes(&codes, 10.0, 1.0, 2.0));
    let raster = render(&doc);
    // B's square: pen at 1, x 1 to 3, rows 16 to 18; A after 4.5 points.
    assert_box(&raster, 1, 16, 3, 18, BLACK);
    assert_box(&raster, 1, 11, 3, 16, WHITE);
    assert_box(&raster, 6, 11, 10, 18, BLACK);
}
