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
