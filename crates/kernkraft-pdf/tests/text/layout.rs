// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Characters, lines, blocks and reading order from glyph geometry:
//! `text/collect.rs` and `text/layout.rs`.

use super::*;
use kernkraft_pdf::text::TextExtractor;
use kernkraft_pdf::{DiagnosticKind, Limit, PdfError, TextOptions};

/// Hello at (100, 700) in 10 point Courier: the baseline is 792 - 700 = 92
/// points from the top, so each glyph's box reaches from 92 - 8 = 84 to
/// 92 + 2 = 94 and the glyph `i` from 100 + 6 i to 106 + 6 i.
const HELLO: &str = "BT /F1 10 Tf 100 700 Td (Hello) Tj ET";

fn texts(page: &TextPage) -> Vec<String> {
    page.blocks
        .iter()
        .map(|block| {
            block
                .lines
                .iter()
                .map(|line| line.text())
                .collect::<Vec<_>>()
                .join("\n")
        })
        .collect()
}

#[test]
fn a_string_is_a_line_of_characters_with_their_boxes() {
    let page = page(HELLO);
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].lines.len(), 1);
    assert_eq!(page.text(), "Hello");
    assert!(!page.tagged);
    assert_eq!(page.len(), 5);
    for (index, ch) in page.blocks[0].lines[0].chars.iter().enumerate() {
        let left = 100.0 + 6.0 * index as f64;
        assert_rect(ch.rect, left, 84.0, left + 6.0, 94.0, "glyph");
        assert!(!ch.synthetic);
    }
    assert_rect(
        page.blocks[0].lines[0].rect,
        100.0,
        84.0,
        130.0,
        94.0,
        "line",
    );
    assert_rect(page.blocks[0].rect, 100.0, 84.0, 130.0, 94.0, "block");
}

#[test]
fn a_page_turned_a_quarter_flows_down_the_displayed_page() {
    // Rotate 90 (7.7.3.3) turns the page clockwise: the user point (x, y)
    // is at (y, x) on the displayed page, 792 wide and 612 high. The first
    // glyph's box, x 100 to 106 and y 698 to 708 in user space, is at
    // x 698 to 708 and y 100 to 106.
    let fixture = document(
        &[PageSpec {
            content: HELLO,
            entries: "/Rotate 90",
        }],
        "",
        "",
        &[],
    );
    let page = text_of(&fixture);
    assert_eq!(page.text(), "Hello");
    let first = &page.blocks[0].lines[0].chars[0];
    assert_close(first.rect.left, 698.0 / 792.0, "left");
    assert_close(first.rect.right, 708.0 / 792.0, "right");
    assert_close(first.rect.top, 100.0 / 612.0, "top");
    assert_close(first.rect.bottom, 106.0 / 612.0, "bottom");
    let last = &page.blocks[0].lines[0].chars[4];
    assert_close(last.rect.top, 124.0 / 612.0, "last top");
}

#[test]
fn the_crop_box_is_the_displayed_page() {
    // The crop box [50 600 350 750] is 300 by 150; the baseline at 700 is
    // 50 below its top and the glyph starts 50 from its left.
    let fixture = document(
        &[PageSpec {
            content: HELLO,
            entries: "/CropBox [50 600 350 750]",
        }],
        "",
        "",
        &[],
    );
    let page = text_of(&fixture);
    let first = &page.blocks[0].lines[0].chars[0];
    assert_close(first.rect.left, 50.0 / 300.0, "left");
    assert_close(first.rect.right, 56.0 / 300.0, "right");
    assert_close(first.rect.top, 42.0 / 150.0, "top");
    assert_close(first.rect.bottom, 52.0 / 150.0, "bottom");
}

#[test]
fn a_gap_wider_than_a_tenth_of_an_em_is_a_space() {
    // The second word starts 134 - 130 = 4 points (0.4 em) after the
    // first ends.
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 34 0 Td (World) Tj ET");
    assert_eq!(page.text(), "Hello World");
    let space = &page.blocks[0].lines[0].chars[5];
    assert!(space.synthetic);
    assert_eq!(space.text, " ");
    assert_rect(space.rect, 130.0, 84.0, 134.0, 94.0, "the space");
}

#[test]
fn a_narrower_gap_is_no_space() {
    // 0.5 point is 0.05 em.
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 30.5 0 Td (World) Tj ET");
    assert_eq!(page.text(), "HelloWorld");
}

#[test]
fn a_space_glyph_is_a_character_and_adds_no_second_space() {
    let page = page("BT /F1 10 Tf 100 700 Td (Hello World) Tj ET");
    assert_eq!(page.text(), "Hello World");
    assert!(page.blocks[0].lines[0].chars.iter().all(|ch| !ch.synthetic));
}

#[test]
fn a_tj_adjustment_makes_a_gap() {
    // -300 moves the next glyph right by 300 / 1000 * 10 = 3 points: 0.3
    // em is a space, 50 (0.5 point, 0.05 em) is not.
    assert_eq!(
        page("BT /F1 10 Tf 100 700 Td [(Hel) -300 (lo)] TJ ET").text(),
        "Hel lo"
    );
    assert_eq!(
        page("BT /F1 10 Tf 100 700 Td [(Hel) -50 (lo)] TJ ET").text(),
        "Hello"
    );
}

#[test]
fn glyphs_further_apart_than_an_em_are_not_one_line() {
    // 20 points between Hello (ending at 130) and World is 2 ems.
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 50 0 Td (World) Tj ET");
    assert_eq!(texts(&page), ["Hello", "World"]);
}

#[test]
fn text_drawn_twice_in_nearly_the_same_place_is_read_once() {
    // A bold made by overprinting: the copy is 0.3 points (0.03 em) over.
    let page =
        page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET BT /F1 10 Tf 100.3 700 Td (Hello) Tj ET");
    assert_eq!(page.text(), "Hello");
}

#[test]
fn the_same_letters_one_glyph_apart_are_not_a_duplicate() {
    let page = page("BT /F1 10 Tf 100 700 Td (AA) Tj ET");
    assert_eq!(page.text(), "AA");
}

#[test]
fn text_drawn_out_of_order_is_read_in_order_along_the_line() {
    let page = page("BT /F1 10 Tf 134 700 Td (World) Tj ET BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    assert_eq!(page.text(), "Hello World");
}

#[test]
fn lines_at_a_leading_make_a_block() {
    // 12 points of leading is 1.2 em.
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 0 -12 Td (World) Tj ET");
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].lines.len(), 2);
    assert_eq!(page.text(), "Hello\nWorld");
    assert_rect(page.blocks[0].rect, 100.0, 84.0, 130.0, 106.0, "block");
}

#[test]
fn a_gap_of_more_than_a_paragraph_break_starts_a_block() {
    // 24 points is 2.4 em, more than the 1.7 em a block's leading allows.
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 0 -24 Td (World) Tj ET");
    assert_eq!(page.blocks.len(), 2);
    assert_eq!(page.text(), "Hello\n\nWorld");
}

#[test]
fn a_heading_is_not_part_of_the_paragraph_below_it() {
    // 22 points is 1.1 em of the 20 point heading, which would stack, but
    // the sizes differ by a factor of 2.
    let page = page("BT /F1 20 Tf 100 700 Td (Title) Tj /F1 10 Tf 0 -22 Td (Body) Tj ET");
    assert_eq!(texts(&page), ["Title", "Body"]);
}

#[test]
fn a_line_that_does_not_overlap_the_one_above_is_no_part_of_its_block() {
    // The second line is further right than the first ends: no block.
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET BT /F1 10 Tf 300 688 Td (World) Tj ET");
    assert_eq!(page.blocks.len(), 2);
}

#[test]
fn two_columns_are_read_one_after_the_other() {
    // Left column at x 50, right at x 320, lines drawn across.
    let mut content = String::new();
    for (row, y) in [700, 688, 676].iter().enumerate() {
        content.push_str(&format!(
            "BT /F1 10 Tf 1 0 0 1 50 {y} Tm (L{n}) Tj ET BT /F1 10 Tf 1 0 0 1 320 {y} Tm (R{n}) Tj ET ",
            n = row + 1
        ));
    }
    let page = page(&content);
    assert_eq!(page.text(), "L1\nL2\nL3\n\nR1\nR2\nR3");
}

#[test]
fn a_title_over_two_columns_comes_first() {
    let content = "BT /F1 10 Tf 1 0 0 1 50 750 Tm (T) Tj ET \
                   BT /F1 10 Tf 1 0 0 1 50 700 Tm (L1) Tj ET \
                   BT /F1 10 Tf 1 0 0 1 320 700 Tm (R1) Tj ET";
    assert_eq!(page(content).text(), "T\n\nL1\n\nR1");
}

#[test]
fn blocks_one_below_another_are_read_top_down() {
    // Three blocks 40 points apart, one under the next.
    let content = "BT /F1 10 Tf 1 0 0 1 50 700 Tm (A) Tj ET \
                   BT /F1 10 Tf 1 0 0 1 50 650 Tm (B) Tj ET \
                   BT /F1 10 Tf 1 0 0 1 50 600 Tm (C) Tj ET";
    assert_eq!(page(content).text(), "A\n\nB\n\nC");
}

#[test]
fn text_in_the_invisible_render_mode_is_text() {
    let page = page("BT 3 Tr /F1 10 Tf 100 700 Td (Scanned) Tj ET");
    assert_eq!(page.text(), "Scanned");
}

#[test]
fn text_wholly_off_the_page_is_not_text() {
    assert!(page("BT /F1 10 Tf -500 700 Td (Hi) Tj ET").is_empty());
    assert!(page("BT /F1 10 Tf 100 5000 Td (Hi) Tj ET").is_empty());
    // Partly on is text.
    assert_eq!(page("BT /F1 10 Tf -3 700 Td (Hi) Tj ET").text(), "Hi");
}

#[test]
fn text_of_no_size_is_not_text() {
    assert!(page("BT /F1 0 Tf 100 700 Td (Hi) Tj ET").is_empty());
}

#[test]
fn text_that_optional_content_hides_is_not_text() {
    let fixture = document(
        &[PageSpec::of(
            "/OC /L1 BDC BT /F1 10 Tf 100 700 Td (Hidden) Tj ET EMC \
             /OC /L0 BDC BT /F1 10 Tf 100 600 Td (Shown) Tj ET EMC",
        )],
        "/OCProperties << /OCGs [30 0 R 31 0 R] /D << /OFF [31 0 R] >> >>",
        "/Properties << /L0 30 0 R /L1 31 0 R >>",
        &[
            (30, b"<< /Type /OCG /Name (On) >>"),
            (31, b"<< /Type /OCG /Name (Off) >>"),
        ],
    );
    assert_eq!(text_of(&fixture).text(), "Shown");
}

#[test]
fn a_glyph_with_no_unicode_text_keeps_its_place_as_the_replacement_character() {
    // Code 2 of F2 is named `zzz`.
    let fixture = document(
        &[PageSpec::of("BT /F2 10 Tf 100 700 Td (A\\002B) Tj ET")],
        "",
        "",
        &[],
    );
    let document = open(&fixture);
    let page = document.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.text(), "A\u{FFFD}B");
    let unmapped: Vec<_> = document
        .diagnostics()
        .into_iter()
        .filter(|d| d.kind == DiagnosticKind::TextUnmapped)
        .collect();
    assert_eq!(unmapped.len(), 1);
    assert_eq!(unmapped[0].object, Some(id(page_object(0))));
}

#[test]
fn a_glyph_the_page_shows_beyond_the_limit_is_refused() {
    let fixture = one_page(HELLO);
    let document = open(&fixture);
    let options = TextOptions {
        max_glyphs: 4,
        ..TextOptions::default()
    };
    let mut extractor = TextExtractor::with_options(&document, options).unwrap();
    let error = extractor.page(1, &NoProgress).unwrap_err();
    assert!(matches!(
        error,
        PdfError::LimitExceeded {
            limit: Limit::TextGlyphs { max: 4 },
            ..
        }
    ));
    // Exactly the limit is allowed.
    let options = TextOptions {
        max_glyphs: 5,
        ..TextOptions::default()
    };
    let mut extractor = TextExtractor::with_options(&document, options).unwrap();
    assert_eq!(extractor.page(1, &NoProgress).unwrap().text(), "Hello");
}

#[test]
fn text_in_a_form_is_text_of_the_page() {
    let fixture = document(
        &[PageSpec::of("/Fm0 Do")],
        "",
        "/XObject << /Fm0 80 0 R >>",
        &[(
            80,
            &stream(
                " /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 60 0 R >> >>",
                b"BT /F1 10 Tf 100 700 Td (In a form) Tj ET",
            ),
        )],
    );
    assert_eq!(text_of(&fixture).text(), "In a form");
}

#[test]
fn a_selection_is_the_text_between_two_characters() {
    let page = page("BT /F1 10 Tf 100 700 Td (Hello) Tj 0 -12 Td (World) Tj ET");
    // Characters 0 to 4 are Hello, 5 to 9 World.
    let selection = page.selection(3, 6).unwrap();
    assert_eq!(selection.text, "lo\nWo");
    // The parts of two lines: l and o, then W and o.
    assert_eq!(selection.rects.len(), 2);
    assert_rect(selection.rects[0], 118.0, 84.0, 130.0, 94.0, "first line");
    assert_rect(selection.rects[1], 100.0, 96.0, 112.0, 106.0, "second line");
    // Either end first.
    assert_eq!(page.selection(6, 3).unwrap(), selection);
    assert!(page.selection(0, 10).is_none());
}

#[test]
fn a_point_finds_the_character_it_is_in_or_the_nearest() {
    let page = page(HELLO);
    // The `e` spans x 106 to 112 and y 84 to 94.
    let inside = (109.0 / PAGE_WIDTH, 90.0 / PAGE_HEIGHT);
    assert_eq!(page.char_at(inside.0, inside.1), Some(1));
    assert_eq!(page.nearest_char(inside.0, inside.1), Some(1));
    // Above the line and right of it: no character holds it, the last is
    // nearest.
    let outside = (200.0 / PAGE_WIDTH, 10.0 / PAGE_HEIGHT);
    assert_eq!(page.char_at(outside.0, outside.1), None);
    assert_eq!(page.nearest_char(outside.0, outside.1), Some(4));
}

#[test]
fn text_turned_by_the_text_matrix_is_still_lines_and_blocks() {
    // 45 degrees counter-clockwise: Hello along the diagonal, and a second
    // line one leading below it in text space.
    let page = page(
        "BT /F1 10 Tf 0.70710678 0.70710678 -0.70710678 0.70710678 100 100 Tm \
         (Hello) Tj 0 -12 Td (World) Tj ET",
    );
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.blocks[0].lines.len(), 2);
    assert_eq!(page.text(), "Hello\nWorld");
}

#[test]
fn a_vertical_font_flows_down_the_page() {
    // Identity-V with DW 1000 (see tests/content/text.rs): the glyphs are
    // drawn at (-0.5, -0.88) em from the position, so with Tfs 10 at
    // (100, 700) the first glyph's origin is (95, 691.2); its box is 1 em
    // wide and 1 em (its displacement of 10) down from there, and the
    // second glyph's box starts where the first ends.
    let tounicode = stream(
        "",
        b"/CIDInit /ProcSet findresource begin 12 dict begin begincmap \
          /CMapName /X def 1 begincodespacerange <0000> <FFFF> endcodespacerange \
          2 beginbfchar <0001> <0041> <0002> <0042> endbfchar endcmap end end",
    );
    let fixture = document(
        &[PageSpec::of("BT /FV 10 Tf 100 700 Td <00010002> Tj ET")],
        "",
        "/Font << /FV 70 0 R >>",
        &[
            (
                70,
                b"<< /Type /Font /Subtype /Type0 /BaseFont /Foo /Encoding /Identity-V \
                  /DescendantFonts [71 0 R] /ToUnicode 73 0 R >>",
            ),
            (
                71,
                b"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /Foo \
                  /CIDSystemInfo << /Registry (Adobe) /Ordering (Identity) /Supplement 0 >> \
                  /FontDescriptor 72 0 R /DW 1000 >>",
            ),
            (72, b"<< /Type /FontDescriptor /FontName /Foo /Flags 4 >>"),
            (73, &tounicode),
        ],
    );
    let page = text_of(&fixture);
    assert_eq!(page.text(), "AB");
    assert_eq!(page.blocks[0].lines.len(), 1);
    let chars = &page.blocks[0].lines[0].chars;
    assert_rect(chars[0].rect, 90.0, 100.8, 100.0, 110.8, "first glyph");
    assert_rect(chars[1].rect, 90.0, 110.8, 100.0, 120.8, "second glyph");
}
