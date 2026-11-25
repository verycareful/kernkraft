// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text that has no size (ISO 32000-1, 9.4.4: a font size of 0, or a text
//! matrix that leaves the glyph no height or no width) is text of the page
//! with no extent: it is listed, flagged, at the point its origin is at.

use super::*;
use kernkraft_pdf::text::SearchOptions;

/// Visible text at (50, 700), then the text of size 0 at (50, 650): displayed
/// 142 points from the top of a 792 point page.
const SEEN_AND_HIDDEN: &str =
    "BT /F1 12 Tf 50 700 Td (Seen) Tj ET BT /F1 0 Tf 50 650 Td (Size zero) Tj ET";

fn point(x: f64, y_from_top: f64) -> (f64, f64) {
    (x / PAGE_WIDTH, y_from_top / PAGE_HEIGHT)
}

fn is_point_at(rect: Rect, x: f64, y_from_top: f64, what: &str) {
    let (fx, fy) = point(x, y_from_top);
    assert_close(rect.left, fx, &format!("{what}: left"));
    assert_close(rect.right, fx, &format!("{what}: right"));
    assert_close(rect.top, fy, &format!("{what}: top"));
    assert_close(rect.bottom, fy, &format!("{what}: bottom"));
}

#[test]
fn text_of_size_zero_is_the_last_block_and_flagged() {
    let page = text_of(&one_page(SEEN_AND_HIDDEN));
    assert_eq!(page.blocks.len(), 2);
    assert_eq!(page.blocks[0].lines[0].text(), "Seen");
    assert!(page.blocks[0]
        .lines
        .iter()
        .flat_map(|line| &line.chars)
        .all(|ch| !ch.sizeless));
    let hidden = &page.blocks[1];
    assert_eq!(hidden.lines.len(), 1);
    assert_eq!(hidden.lines[0].text(), "Size zero");
    for ch in &hidden.lines[0].chars {
        assert!(ch.sizeless && !ch.synthetic && ch.source.is_some());
        is_point_at(ch.rect, 50.0, 142.0, "character");
        assert!(ch.quad.iter().all(|&corner| corner == point(50.0, 142.0)));
    }
    is_point_at(hidden.lines[0].rect, 50.0, 142.0, "line");
    is_point_at(hidden.rect, 50.0, 142.0, "block");
}

#[test]
fn a_text_matrix_that_flattens_the_glyph_leaves_it_no_size() {
    // Every entry zero, and a matrix that keeps the width and loses the
    // height: both are glyphs with no area. A Courier glyph advances 0.6 em,
    // so under the second matrix (font size 1, horizontal scale 10) each
    // glyph's origin is 6 points on from the last, and under the first none
    // is.
    for (matrix, step) in [("0 0 0 0 50 650 Tm", 0.0), ("10 0 0 0 50 650 Tm", 6.0)] {
        let content = format!("BT /F1 1 Tf {matrix} (Flat) Tj ET");
        let page = text_of(&one_page(&content));
        assert_eq!(page.blocks.len(), 1, "{matrix}");
        assert_eq!(page.blocks[0].lines[0].text(), "Flat", "{matrix}");
        for (index, ch) in page.blocks[0].lines[0].chars.iter().enumerate() {
            assert!(ch.sizeless, "{matrix}");
            is_point_at(ch.rect, 50.0 + step * index as f64, 142.0, matrix);
        }
    }
}

#[test]
fn each_showing_operation_is_a_line() {
    let page = text_of(&one_page("BT /F1 0 Tf 50 650 Td (One) Tj (Two) Tj ET"));
    assert_eq!(page.blocks.len(), 1);
    let lines: Vec<String> = page.blocks[0]
        .lines
        .iter()
        .map(|line| line.text())
        .collect();
    assert_eq!(lines, ["One", "Two"]);
}

#[test]
fn text_of_size_zero_whose_origin_is_off_the_page_is_not_listed() {
    let page = text_of(&one_page("BT /F1 0 Tf 700 650 Td (Away) Tj ET"));
    assert!(page.blocks.is_empty());
}

#[test]
fn a_search_finds_text_of_size_zero_and_draws_nothing_of_it() {
    let page = text_of(&one_page(SEEN_AND_HIDDEN));
    let matches = page.search("size zero", &SearchOptions::default());
    assert_eq!(matches.len(), 1);
    assert_eq!(matches[0].rects.len(), 1);
    is_point_at(matches[0].rects[0], 50.0, 142.0, "match");
}

#[test]
fn a_point_never_lands_on_text_of_size_zero() {
    let (x, y) = point(50.0, 142.0);
    // Visible text lies near that point: it is what the point names.
    let near = text_of(&one_page(
        "BT /F1 12 Tf 50 650 Td (Seen) Tj ET BT /F1 0 Tf 50 650 Td (Size zero) Tj ET",
    ));
    let nearest = near.nearest_char(x, y).expect("a character");
    assert!(!near.char(nearest).unwrap().sizeless);
    // With nothing else on the page there is no character to name.
    let alone = text_of(&one_page("BT /F1 0 Tf 50 650 Td (Size zero) Tj ET"));
    assert_eq!(alone.len(), 9);
    assert_eq!(alone.char_at(x, y), None);
    assert_eq!(alone.nearest_char(x, y), None);
}
