// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text on a rotated or skewed matrix (ISO 32000-1, 9.4.2 and 8.3.4): a
//! block is measured and set in the text space of its first glyph, so a
//! line is as wide along its baseline as the block's widest line is and the
//! next line is `pitch` away along the text space's `-y`.
//!
//! A Courier glyph is 600 units wide, 6 points at 10 points, so `Hello` is
//! 30 points along its baseline and the lines of a block are 10 points
//! apart. A point (x, y) of text space is at (a x + c y + e, b x + d y + f)
//! under `a b c d e f`.

use super::*;
use kernkraft_pdf::content::StreamId;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::ObjectId;
use kernkraft_pdf::{delete_text_block, edit_text, Editability, TextEdit};

fn content(doc: &Document) -> String {
    let data = doc
        .content_stream(
            StreamId::Page(ObjectId {
                number: PAGE,
                generation: 0,
            }),
            1 << 20,
        )
        .unwrap()
        .data;
    String::from_utf8(data)
        .unwrap()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace("[ ", "[")
        .replace(" ]", "]")
}

/// The page's content after the block of `content_of_page` is edited to
/// `text`, and the number of lines the new text took.
fn edited(content_of_page: &str, text: &str) -> (String, usize) {
    let fixture = one_page(content_of_page);
    let doc = fixture.open();
    let block = text_of(&fixture).blocks.remove(0);
    assert_eq!(
        doc.text_block_editability(1, &block, &NoProgress).unwrap(),
        Editability::Editable
    );
    let done = edit_text(&doc, 1, &block, &TextEdit::new(text), &NoProgress).unwrap();
    (content(&done.document), done.lines)
}

/// The same-length replacement of `Hello` is where `Hello` was.
fn howdy_is_where_hello_was(content_of_page: &str) {
    let fixture = one_page(content_of_page);
    let doc = fixture.open();
    let before = text_of(&fixture);
    let done = edit_text(
        &doc,
        1,
        &before.blocks[0],
        &TextEdit::new("Howdy"),
        &NoProgress,
    )
    .unwrap();
    let after = done.document.text_page(1, &NoProgress).unwrap();
    assert_eq!(after.blocks.len(), 1);
    assert_eq!(after.blocks[0].lines[0].text(), "Howdy");
    assert_eq!(after.blocks[0].rect, before.blocks[0].rect);
}

const QUARTER_TURN: &str = "BT /F1 10 Tf 0 1 -1 0 100 700 Tm (Hello) Tj ET";
const SHEARED_X: &str = "BT /F1 10 Tf 1 0 0.5 1 100 700 Tm (Hello) Tj ET";
const SHEARED_Y: &str = "BT /F1 10 Tf 1 0.5 0 1 100 700 Tm (Hello) Tj ET";
const TURNED_BY_CM: &str = "q 0 1 -1 0 0 0 cm BT /F1 10 Tf 700 -100 Td (Hello) Tj ET Q";
/// Mirrored top to bottom: text space (x, y) is at (x + 100, 700 - y).
const MIRRORED_Y: &str = "BT /F1 10 Tf 1 0 0 -1 100 700 Tm (Hello) Tj ET";
/// Mirrored left to right: text space (x, y) is at (200 - x, y + 700), so
/// the baseline runs to the left from x = 200.
const MIRRORED_X: &str = "BT /F1 10 Tf -1 0 0 1 200 700 Tm (Hello) Tj ET";
/// Mirrored by `cm` about the line y = 700: user space (x, y) is at
/// (x, 1400 - y).
const MIRRORED_BY_CM: &str = "q 1 0 0 -1 0 1400 cm BT /F1 10 Tf 100 700 Td (Hello) Tj ET Q";

#[test]
fn a_quarter_turn_is_wrapped_by_the_length_along_the_baseline() {
    // Hello runs up the page: its box is 10 wide and 30 high, so the width
    // that counts is the 30, not the 10. "ab cd" fits it, "ab cde" does
    // not. The next line is at text space (0, -10): (-1 * -10 + 100, 700).
    assert_eq!(edited(QUARTER_TURN, "ab cd").1, 1);
    let (content, lines) = edited(QUARTER_TURN, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("0 1 -1 0 100 700 Tm (ab) Tj 0 1 -1 0 110 700 Tm (cde) Tj ET Q"));
}

#[test]
fn a_shear_across_the_baseline_keeps_the_text_space_width() {
    // The next line is at (0, -10): (0.5 * -10 + 100, 690).
    assert_eq!(edited(SHEARED_X, "ab cd").1, 1);
    let (content, lines) = edited(SHEARED_X, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("1 0 0.5 1 100 700 Tm (ab) Tj 1 0 0.5 1 95 690 Tm (cde) Tj ET Q"));
}

#[test]
fn a_shear_along_the_baseline_keeps_the_text_space_width() {
    // The baseline climbs by half a point a point; the next line is at
    // (0, -10): (100, 700 - 10).
    assert_eq!(edited(SHEARED_Y, "ab cd").1, 1);
    let (content, lines) = edited(SHEARED_Y, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("1 0.5 0 1 100 700 Tm (ab) Tj 1 0.5 0 1 100 690 Tm (cde) Tj ET Q"));
}

#[test]
fn a_turn_by_cm_is_wrapped_by_the_length_along_the_baseline() {
    // The new text is inside the same q, so its Tm is the block's in the
    // turned space: the next line is 10 below it.
    assert_eq!(edited(TURNED_BY_CM, "ab cd").1, 1);
    let (content, lines) = edited(TURNED_BY_CM, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("1 0 0 1 700 -100 Tm (ab) Tj 1 0 0 1 700 -110 Tm (cde) Tj ET Q Q"));
}

#[test]
fn a_mirror_top_to_bottom_is_wrapped_by_the_length_along_the_baseline() {
    // The baseline is unchanged in direction, so Hello is 30 points wide and
    // "ab cd" fits it. The next line is at text space (0, -10):
    // (100, 700 + 10).
    assert_eq!(edited(MIRRORED_Y, "ab cd").1, 1);
    let (content, lines) = edited(MIRRORED_Y, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("1 0 0 -1 100 700 Tm (ab) Tj 1 0 0 -1 100 710 Tm (cde) Tj ET Q"));
}

#[test]
fn a_mirror_left_to_right_is_wrapped_by_the_length_along_the_baseline() {
    // The baseline runs to the left; the width is the same 30 points. The
    // next line is at text space (0, -10): (200 - 0, -10 + 700).
    assert_eq!(edited(MIRRORED_X, "ab cd").1, 1);
    let (content, lines) = edited(MIRRORED_X, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("-1 0 0 1 200 700 Tm (ab) Tj -1 0 0 1 200 690 Tm (cde) Tj ET Q"));
}

#[test]
fn a_mirror_by_cm_is_wrapped_by_the_length_along_the_baseline() {
    // The new text is inside the same q, so its Tm is the block's in the
    // mirrored space: the next line is 10 below it in that space.
    assert_eq!(edited(MIRRORED_BY_CM, "ab cd").1, 1);
    let (content, lines) = edited(MIRRORED_BY_CM, "ab cde");
    assert_eq!(lines, 2);
    assert!(content.ends_with("1 0 0 1 100 700 Tm (ab) Tj 1 0 0 1 100 690 Tm (cde) Tj ET Q Q"));
}

#[test]
fn replaced_text_is_where_the_block_was() {
    for page in [
        QUARTER_TURN,
        SHEARED_X,
        SHEARED_Y,
        TURNED_BY_CM,
        MIRRORED_Y,
        MIRRORED_X,
        MIRRORED_BY_CM,
    ] {
        howdy_is_where_hello_was(page);
    }
}

#[test]
fn deleting_does_not_depend_on_the_matrix() {
    // A removed 5-glyph word is made up for by -3000 (30 points), in text
    // space whatever the matrix is.
    for page in [
        QUARTER_TURN,
        SHEARED_X,
        SHEARED_Y,
        TURNED_BY_CM,
        MIRRORED_Y,
        MIRRORED_X,
        MIRRORED_BY_CM,
    ] {
        let fixture = one_page(page);
        let doc = fixture.open();
        let block = text_of(&fixture).blocks.remove(0);
        let done = delete_text_block(&doc, 1, &block, &NoProgress).unwrap();
        assert!(content(&done).contains("[-3000] TJ"), "{page}");
        assert!(done.text_page(1, &NoProgress).unwrap().blocks.is_empty());
    }
}
