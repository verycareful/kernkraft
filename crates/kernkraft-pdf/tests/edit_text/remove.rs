// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `delete_text_block`: the glyphs of a block leave the stream and the text
//! after them stays where it was (ISO 32000-1, 9.4.3 and 9.4.4).
//!
//! A Courier glyph is 600 units wide, so at 10 points it advances 6 points
//! and a number `n` of a `TJ` array moves the position by `-n / 1000 * 10`
//! points: a removed 5-glyph word (30 points) is made up for by `-3000`.

use super::*;
use kernkraft_pdf::content::StreamId;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::ObjectId;
use kernkraft_pdf::{delete_text_block, EditBlocker, PdfError, Rect};

fn page_stream() -> StreamId {
    StreamId::Page(ObjectId {
        number: PAGE,
        generation: 0,
    })
}

/// The page's decoded content, with the writer's line feeds as spaces and
/// no space inside brackets, so that its spacing does not matter.
fn content(doc: &Document) -> String {
    let data = doc.content_stream(page_stream(), 1 << 20).unwrap().data;
    String::from_utf8(data)
        .unwrap()
        .replace('\n', " ")
        .replace("[ ", "[")
        .replace(" ]", "]")
}

/// A block's text and the boxes of its characters. The place in the
/// stream each came from is left out: an edit moves the bytes.
fn geometry(block: &kernkraft_pdf::text::TextBlock) -> (Rect, Vec<(String, Rect)>) {
    let chars = block
        .lines
        .iter()
        .flat_map(|line| line.chars.iter())
        .map(|ch| (ch.text.clone(), ch.rect))
        .collect();
    (block.rect, chars)
}

fn delete(fixture: &Fixture, block: usize) -> (Document, TextPage, TextPage) {
    let doc = fixture.open();
    let before = doc.text_page(1, &NoProgress).unwrap();
    let after = delete_text_block(&doc, 1, &before.blocks[block], &NoProgress).unwrap();
    let text = after.text_page(1, &NoProgress).unwrap();
    (after, before, text)
}

#[test]
fn a_removed_word_is_made_up_for_by_a_number() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj 0 -100 Td (World) Tj ET");
    let (doc, before, after) = delete(&fixture, 0);
    assert_eq!(before.blocks.len(), 2);
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 100 700 Td [-3000] TJ 0 -100 Td (World) Tj ET"
    );
    assert_eq!(after.blocks.len(), 1);
    assert_eq!(geometry(&after.blocks[0]), geometry(&before.blocks[1]));
}

#[test]
fn text_on_the_same_line_stays_where_it_was() {
    // The second word starts at 100 + 30 = 130. With the first taken out
    // it must still start there.
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj (World) Tj ET");
    let doc = fixture.open();
    let before = doc.text_page(1, &NoProgress).unwrap();
    // One line, one block: the whole line goes.
    assert_eq!(before.blocks.len(), 1);
    let after = delete_text_block(&doc, 1, &before.blocks[0], &NoProgress).unwrap();
    assert_eq!(
        content(&after),
        "BT /F1 10 Tf 100 700 Td [-3000] TJ [-3000] TJ ET"
    );
    assert!(after.text_page(1, &NoProgress).unwrap().blocks.is_empty());
}

#[test]
fn the_next_line_by_t_star_does_not_move() {
    // T* uses the line matrix (9.4.2), which the removal does not touch:
    // the second word stays 100 points below the first.
    let fixture = one_page("BT /F1 10 Tf 100 TL 100 700 Td (Hello) Tj T* (World) Tj ET");
    let (doc, before, after) = delete(&fixture, 0);
    assert_eq!(before.blocks.len(), 2);
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 100 TL 100 700 Td [-3000] TJ T* (World) Tj ET"
    );
    assert_eq!(geometry(&after.blocks[0]), geometry(&before.blocks[1]));
}

#[test]
fn a_removed_quote_keeps_its_line_feed() {
    // ' is T* then Tj (9.4.3): the move to the next line stays.
    let fixture = one_page("BT /F1 10 Tf 100 TL 100 700 Td (Hello) Tj (World) ' ET");
    let (doc, before, after) = delete(&fixture, 1);
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 100 TL 100 700 Td (Hello) Tj T* [-3000] TJ ET"
    );
    assert_eq!(geometry(&after.blocks[0]), geometry(&before.blocks[0]));
}

#[test]
fn a_removed_double_quote_keeps_its_spacing_and_line_feed() {
    // The operator \" sets Tw 5 and Tc 2 and then shows the string: each
    // glyph advances 0.6 * 10 + 2 = 8 points, five of them 40 points, which
    // a number of -4000 makes up for.
    let fixture = one_page("BT /F1 10 Tf 100 TL 100 700 Td (Hello) Tj 5 2 (World) \" ET");
    let (doc, _, _) = delete(&fixture, 1);
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 100 TL 100 700 Td (Hello) Tj 5 Tw 2 Tc T* [-4000] TJ ET"
    );
}

#[test]
fn the_number_does_not_depend_on_horizontal_scaling() {
    // At 50 percent a glyph advances 3 points, five of them 15; a number
    // moves the position by n / 1000 * 10 * 0.5, so -3000 again.
    let fixture = one_page("BT /F1 10 Tf 50 Tz 100 700 Td (Hello) Tj ET");
    let (doc, _, _) = delete(&fixture, 0);
    assert_eq!(content(&doc), "BT /F1 10 Tf 50 Tz 100 700 Td [-3000] TJ ET");
}

#[test]
fn a_block_that_is_part_of_an_array_leaves_the_rest_in_place() {
    // 40000 / 1000 * 10 = 400 points separate the two words, so the layout
    // makes two blocks of the one array.
    let fixture = one_page("BT /F1 10 Tf 50 700 Td [(Left) -40000 (Right)] TJ ET");
    let doc = fixture.open();
    let before = doc.text_page(1, &NoProgress).unwrap();
    assert_eq!(before.blocks.len(), 2);
    let after = delete_text_block(&doc, 1, &before.blocks[0], &NoProgress).unwrap();
    // Left is 4 glyphs, 24 points: -2400, then the array's own number.
    assert_eq!(
        content(&after),
        "BT /F1 10 Tf 50 700 Td [-2400 -40000 (Right)] TJ ET"
    );
    let text = after.text_page(1, &NoProgress).unwrap();
    assert_eq!(text.blocks.len(), 1);
    assert_eq!(geometry(&text.blocks[0]), geometry(&before.blocks[1]));
}

#[test]
fn only_the_stream_with_the_text_is_written_again() {
    let fixture = build(
        "BT /F1 10 Tf 100 700 Td",
        "",
        "",
        Some("[11 0 R 12 0 R]"),
        &[(12, &stream("", b"(Hello) Tj ET"))],
    );
    let doc = fixture.open();
    let before = doc.text_page(1, &NoProgress).unwrap();
    let after = delete_text_block(&doc, 1, &before.blocks[0], &NoProgress).unwrap();
    let old = doc.content_stream(page_stream(), 1 << 20).unwrap();
    let new = after.content_stream(page_stream(), 1 << 20).unwrap();
    assert_eq!(new.parts.len(), 2);
    let first = |content: &kernkraft_pdf::content::ContentStream| {
        content.data[..content.parts[0].len].to_vec()
    };
    assert_eq!(first(&old), first(&new));
    assert_eq!(
        String::from_utf8(new.data[new.parts[1].start..].to_vec())
            .unwrap()
            .replace('\n', " ")
            .replace("[ ", "[")
            .replace(" ]", "]"),
        "[-3000] TJ ET"
    );
}

#[test]
fn a_block_that_cannot_be_edited_is_refused() {
    let fixture = one_page("BT /F1 10 Tf 7 Tr 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let page = doc.text_page(1, &NoProgress).unwrap();
    match delete_text_block(&doc, 1, &page.blocks[0], &NoProgress) {
        Err(PdfError::TextNotEditable { reason }) => {
            assert_eq!(reason, EditBlocker::ClippingRenderMode)
        }
        other => panic!("not refused: {:?}", other.map(|_| ())),
    }
}

// The page is 612 points wide and a glyph is 6 points. A glyph whose box has
// no part on the page is not text of the page, so no block holds it: the rest
// of its line is taken with the block that is.

#[test]
fn the_glyphs_of_a_string_that_lie_past_the_right_edge_go_with_the_block() {
    // A 590..596, B 596..602, C 602..608, D 608..614 (the edge, 612, is
    // inside it), E 614..620 and on are off the page: the block is `ABCD`.
    let fixture = one_page("BT /F1 10 Tf 590 700 Td (ABCDEFGH) Tj ET");
    let (doc, before, _) = delete(&fixture, 0);
    assert_eq!(before.blocks.len(), 1);
    assert_eq!(before.blocks[0].lines[0].text(), "ABCD");
    // All eight glyphs leave, 48 points: -4800.
    assert_eq!(content(&doc), "BT /F1 10 Tf 590 700 Td [-4800] TJ ET");
}

#[test]
fn the_glyphs_of_a_string_that_lie_past_the_left_edge_go_with_the_block() {
    // A to D lie at -30..-6, wholly left of the page; E is -6..0, whose
    // right edge is the page's.
    let fixture = one_page("BT /F1 10 Tf 1 0 0 1 -30 700 Tm (ABCDEFGH) Tj ET");
    let (doc, before, _) = delete(&fixture, 0);
    assert_eq!(before.blocks[0].lines[0].text(), "EFGH");
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 1 0 0 1 -30 700 Tm [-4800] TJ ET"
    );
}

#[test]
fn every_string_of_a_tj_array_gives_up_its_off_page_glyphs() {
    // ABCD is 590..614. The kern of -200 moves the text 2 points to the
    // right, so EFGH starts at 616.
    let fixture = one_page("BT /F1 10 Tf 590 700 Td [(ABCD) -200 (EFGH)] TJ ET");
    let (doc, before, _) = delete(&fixture, 0);
    assert_eq!(before.blocks[0].lines[0].text(), "ABCD");
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 590 700 Td [-2400 -200 -2400] TJ ET"
    );
}

#[test]
fn an_off_page_glyph_of_another_operation_stays() {
    // The second `Tj` is its own showing operation and holds no glyph the
    // block has, so the edit does not reach it.
    let fixture = one_page("BT /F1 10 Tf 590 700 Td (ABCD) Tj (EFGH) Tj ET");
    let (doc, before, _) = delete(&fixture, 0);
    assert_eq!(before.blocks[0].lines[0].text(), "ABCD");
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 590 700 Td [-2400] TJ (EFGH) Tj ET"
    );
}

#[test]
fn the_off_page_words_of_the_same_array_go_with_the_block_on_the_page() {
    // One `TJ` shows two words 400 points apart; the second is off the page,
    // so only the first is a block, and the edit of it takes the second as
    // the rest of the operation. The first word is ABCD at 400..424, the
    // second starts at 424 + 400 = 824, wholly off the page.
    let fixture = one_page("BT /F1 10 Tf 400 700 Td [(ABCD) -40000 (EFGH)] TJ ET");
    let (doc, before, _) = delete(&fixture, 0);
    assert_eq!(before.blocks.len(), 1);
    assert_eq!(
        content(&doc),
        "BT /F1 10 Tf 400 700 Td [-2400 -40000 -2400] TJ ET"
    );
}

#[test]
fn an_edit_takes_the_off_page_glyphs_with_the_text_it_replaces() {
    let fixture = one_page("BT /F1 10 Tf 590 700 Td (ABCDEFGH) Tj ET");
    let doc = fixture.open();
    let before = doc.text_page(1, &NoProgress).unwrap();

    let edited = kernkraft_pdf::edit_text(
        &doc,
        1,
        &before.blocks[0],
        &kernkraft_pdf::TextEdit::new("X"),
        &NoProgress,
    )
    .unwrap();

    let content = content(&edited.document);
    assert!(content.starts_with("BT /F1 10 Tf 590 700 Td [-4800] TJ ET "));
    assert!(!content.contains("EFGH"));
}
