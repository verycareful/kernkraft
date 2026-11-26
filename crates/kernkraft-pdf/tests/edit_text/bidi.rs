// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Editing with right-to-left and mixed text: `src/edit_text/arrange.rs`
//! and `typeset.rs`. Unicode Standard Annex #9 gives the paragraph level
//! (P2, P3) and the order of a line (L2).
//!
//! The font is made here: every glyph is 500 units, 5 points at 10 points,
//! and it has `A`, `B`, the space, four Hebrew letters (shin, lamed, vav and
//! final mem) and the two round brackets. The block edited is `Hello` in
//! 10 point Courier at x 100, which is 30 points wide, so its right edge is
//! at x 130.

use super::*;
use crate::pdf_fixture::fontkit::sfnt::{build, Glyph, SfntSpec};
use kernkraft_font::FontList;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::{edit_text, PdfError, TextEdit, TextStyle};

const SHIN: &str = "\u{05e9}";
const LAMED: &str = "\u{05dc}";
const VAV: &str = "\u{05d5}";
const FINAL_MEM: &str = "\u{05dd}";
const PAGE_WIDTH: f64 = 612.0;

/// Points a glyph advances, at 10 points.
const ADVANCE: f64 = 5.0;
/// Where the block ends on the right.
const RIGHT_EDGE: f64 = 130.0;

fn bidi_font() -> Vec<u8> {
    let glyph = |name: &'static str, marks: bool| Glyph {
        name,
        advance: 500,
        contours: if marks {
            vec![vec![
                (0, 0, true),
                (300, 0, true),
                (300, 600, true),
                (0, 600, true),
            ]]
        } else {
            vec![]
        },
    };
    let glyphs = vec![
        glyph(".notdef", false),
        glyph("A", true),
        glyph("B", true),
        glyph("space", false),
        glyph("shin", true),
        glyph("lamed", true),
        glyph("vav", true),
        glyph("finalmem", true),
        glyph("parenleft", true),
        glyph("parenright", true),
    ];
    let mut spec = SfntSpec::new(glyphs);
    spec.unicode = vec![
        (0x41, 1),
        (0x42, 2),
        (0x20, 3),
        (0x05E9, 4),
        (0x05DC, 5),
        (0x05D5, 6),
        (0x05DD, 7),
        (0x28, 8),
        (0x29, 9),
    ];
    spec.family = "Bidi Sans";
    spec.full = "Bidi Sans";
    spec.postscript = "BidiSans-Regular";
    build(&spec)
}

fn font_list(dir: &tempfile::TempDir) -> FontList {
    let path = dir.path().join("bidi-sans.ttf");
    std::fs::write(&path, bidi_font()).unwrap();
    FontList::index(&[path], &NoProgress).unwrap()
}

/// The page of `Hello` edited to `text`, and the document it is in.
fn edited(text: &str, list: &FontList) -> Result<Document, PdfError> {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = doc.text_page(1, &NoProgress).unwrap().blocks.remove(0);
    let style = TextStyle::default().with_family("Bidi Sans");
    let edit = TextEdit::new(text).with_style(style).with_fonts(list);
    Ok(edit_text(&doc, 1, &block, &edit, &NoProgress)?.document)
}

fn left_of(page: &TextPage, index: usize) -> f64 {
    page.char(index).unwrap().rect.left * PAGE_WIDTH
}

fn assert_left(page: &TextPage, index: usize, points: f64) {
    let left = left_of(page, index);
    assert!(
        (left - points).abs() < 1e-6,
        "character {index} starts at {left}, not {points}"
    );
}

#[test]
fn a_hebrew_word_is_set_against_the_right_edge_of_the_block() {
    let dir = tempfile::tempdir().unwrap();
    let doc = edited(&format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}"), &font_list(&dir)).unwrap();
    let page = doc.text_page(1, &NoProgress).unwrap();
    // Four glyphs of 5 points end at 130, so the line starts at 110; the
    // first letter typed, shin, is the rightmost.
    assert_eq!(page.text(), format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}"));
    for (index, left) in [(0, 125.0), (1, 120.0), (2, 115.0), (3, 110.0)] {
        assert_left(&page, index, left);
    }
    assert_eq!(RIGHT_EDGE - 4.0 * ADVANCE, 110.0);
}

#[test]
fn a_left_to_right_line_with_hebrew_in_it_reads_back_as_typed() {
    // One Latin letter against three Hebrew ones: the majority is right to
    // left, and only the order the glyphs are written in tells that the
    // paragraph is left to right. `A`, a space, then the word reversed in
    // places 2 to 4 from x 100; the five glyphs fit the block's 30 points.
    let dir = tempfile::tempdir().unwrap();
    let typed = format!("A {SHIN}{LAMED}{VAV}");
    let doc = edited(&typed, &font_list(&dir)).unwrap();
    let page = doc.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.blocks[0].lines.len(), 1);
    assert_eq!(page.text(), typed);
    for (index, left) in [(0, 100.0), (1, 105.0)] {
        assert_left(&page, index, left);
    }
    // Shin is typed first of the word and drawn last of its three places.
    for (index, left) in [(2, 120.0), (3, 115.0), (4, 110.0)] {
        assert_left(&page, index, left);
    }
}

#[test]
fn a_bracket_in_a_right_to_left_run_is_set_mirrored_and_reads_back_as_typed() {
    // `(`, shin, lamed, `)` in a right-to-left paragraph: four glyphs of 5
    // points end at 130, so from the left the line is the glyph of `(` for
    // the typed `)` at 110, lamed at 115, shin at 120 and the glyph of `)`
    // for the typed `(` at 125. The glyphs are drawn in the order typed,
    // the brackets each in an `ActualText` of the character typed, and the
    // page reads back as typed.
    let dir = tempfile::tempdir().unwrap();
    let typed = format!("({SHIN}{LAMED})");
    let doc = edited(&typed, &font_list(&dir)).unwrap();
    let page = doc.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.text(), typed);
    for (index, left) in [(0, 125.0), (1, 120.0), (2, 115.0), (3, 110.0)] {
        assert_left(&page, index, left);
    }
    let stream = doc
        .content_stream(
            kernkraft_pdf::content::StreamId::Page(kernkraft_pdf::syntax::ObjectId {
                number: PAGE,
                generation: 0,
            }),
            1 << 20,
        )
        .unwrap()
        .data;
    let stream = String::from_utf8_lossy(&stream);
    assert_eq!(stream.matches("/ActualText").count(), 2, "{stream}");
    // A bracket in a left-to-right run is drawn as typed, in no span.
    let plain = edited("(AB)", &font_list(&dir)).unwrap();
    assert_eq!(plain.text_page(1, &NoProgress).unwrap().text(), "(AB)");
}

#[test]
fn the_result_of_a_right_to_left_edit_can_be_edited_again() {
    let dir = tempfile::tempdir().unwrap();
    let list = font_list(&dir);
    let doc = edited(&format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}"), &list).unwrap();
    let block = doc.text_page(1, &NoProgress).unwrap().blocks.remove(0);
    let style = TextStyle::default().with_family("Bidi Sans");
    let again = TextEdit::new("AB").with_style(style).with_fonts(&list);
    let second = edit_text(&doc, 1, &block, &again, &NoProgress).unwrap();
    assert_eq!(
        second.document.text_page(1, &NoProgress).unwrap().text(),
        "AB"
    );
    // The block's right edge is where the first edit's text ended, 130:
    // `AB` is left to right and starts where the block's leftmost glyph
    // did, at 110.
    let page = second.document.text_page(1, &NoProgress).unwrap();
    assert_left(&page, 0, 110.0);
}

/// `Hello` edited to `text` in a box: `left` and `width` as fractions of
/// the page, `None` for the block's own.
fn edited_in_box(text: &str, list: &FontList, left: Option<f64>, width: Option<f64>) -> Document {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let block = doc.text_page(1, &NoProgress).unwrap().blocks.remove(0);
    let style = TextStyle::default().with_family("Bidi Sans");
    let mut edit = TextEdit::new(text).with_style(style).with_fonts(list);
    if let Some(left) = left {
        edit = edit.with_left(left);
    }
    if let Some(width) = width {
        edit = edit.with_width(width);
    }
    edit_text(&doc, 1, &block, &edit, &NoProgress)
        .unwrap()
        .document
}

/// The four letters of a Hebrew word, whose line ends where the box does.
fn assert_word_ends_at(doc: &Document, right: f64) {
    let page = doc.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.text(), format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}"));
    for index in 0..4 {
        assert_left(&page, index, right - ADVANCE * (index + 1) as f64);
    }
}

#[test]
fn a_right_to_left_line_follows_the_right_edge_of_a_wider_box() {
    // The block starts at 100: a box 40 points wide ends at 140, where the
    // line of four 5 point glyphs ends; it was 130 without a width.
    let dir = tempfile::tempdir().unwrap();
    let word = format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}");
    let doc = edited_in_box(&word, &font_list(&dir), None, Some(40.0 / PAGE_WIDTH));
    assert_word_ends_at(&doc, 140.0);
}

#[test]
fn a_left_moves_a_right_to_left_block_as_a_whole() {
    // 50 points further: the right edge, 130, is 180.
    let dir = tempfile::tempdir().unwrap();
    let word = format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}");
    let doc = edited_in_box(&word, &font_list(&dir), Some(150.0 / PAGE_WIDTH), None);
    assert_word_ends_at(&doc, 180.0);
}

#[test]
fn a_left_and_a_width_set_the_box_a_right_to_left_line_ends_at() {
    // From 150 and 40 wide: the box ends at 190.
    let dir = tempfile::tempdir().unwrap();
    let word = format!("{SHIN}{LAMED}{VAV}{FINAL_MEM}");
    let doc = edited_in_box(
        &word,
        &font_list(&dir),
        Some(150.0 / PAGE_WIDTH),
        Some(40.0 / PAGE_WIDTH),
    );
    assert_word_ends_at(&doc, 190.0);
}

#[test]
fn a_narrower_box_breaks_a_right_to_left_paragraph_and_keeps_its_right_edge_in_it() {
    // Two words of two letters, each 10 points, with a 5 point space: 25
    // points, in a box 20 wide from 100. The words are on two lines, each
    // against 120, the box's right edge.
    let dir = tempfile::tempdir().unwrap();
    let text = format!("{SHIN}{LAMED} {VAV}{FINAL_MEM}");
    let doc = edited_in_box(&text, &font_list(&dir), None, Some(20.0 / PAGE_WIDTH));
    let page = doc.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.blocks[0].lines.len(), 2);
    let right = page.blocks[0].rect.right * PAGE_WIDTH;
    assert!((right - 120.0).abs() < 1e-6, "{right}");
}
