// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `add_text`: new text on a page that follows the text already there (ISO
//! 32000-1, 9.3, 9.4 and 7.8.2).
//!
//! The page is 612 by 792 points. Courier glyphs are 600 units wide, 6
//! points at 10 points, and the fonts have no descriptor, so the ascent is
//! the fallback of `text/collect.rs`, 800 units, and a line is 1000 units
//! apart (8 and 10 points at 10 points). A box at fractions `(x, y)` of the
//! displayed page has its top at `792 y` points from the page's top, and
//! its first baseline 8 points below that at 10 points.

use super::edit::{content, cyrillic_list};
use super::*;
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{ObjectId, Value};
use kernkraft_pdf::{
    add_text, block_style, blocks_by_distance, BlockStyle, Follow, NewText, PdfError, TextStyle,
};

fn text_of_document(doc: &Document) -> TextPage {
    doc.text_page(1, &NoProgress).unwrap()
}

const HELVETICA: &[u8] =
    b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>";

/// Hello in Courier at 10 points at (100, 700), and World in Helvetica at
/// 20 points at (100, 300): two blocks, 400 points apart.
fn two_fonts() -> Fixture {
    build(
        "BT /F1 10 Tf 100 700 Td (Hello) Tj ET BT /F2 20 Tf 100 300 Td (World) Tj ET",
        "/F2 61 0 R",
        "",
        None,
        &[(61, HELVETICA)],
    )
}

fn added(
    doc: &Document,
    follow: Follow<'_>,
    text: &str,
    at: (f64, f64),
    width: f64,
) -> kernkraft_pdf::TextAdded {
    add_text(doc, 1, follow, &NewText::new(text, at, width), &NoProgress).unwrap()
}

#[test]
fn new_text_follows_the_font_and_size_of_the_nearest_block() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    // Hello is at 92 points from the top (y 700 of 792), World at 492: a
    // box at 0.2 of the height, 158 points down, is nearest Hello.
    let near_hello = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.2), 0.5);
    assert_eq!(near_hello.followed, Some(0));
    let shown = content(&near_hello.document);
    assert!(shown.ends_with("(Hi) Tj ET Q"), "{shown}");
    assert!(shown.contains("/F1 10 Tf"), "{shown}");
    // 0.8 of the height, 634 points down, is nearest World.
    let near_world = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.8), 0.5);
    assert_eq!(near_world.followed, Some(1));
    let shown = content(&near_world.document);
    assert!(shown.contains("/F2 20 Tf"), "{shown}");
}

#[test]
fn the_first_baseline_is_the_ascent_below_the_top_of_the_box() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    // Nearest Hello, at 10 points: the top is 158.4 points down, the
    // baseline 8 below that, 166.4 down, which is y 625.6 in user space; x
    // is a quarter of 612.
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.2), 0.5);
    let shown = content(&made.document);
    assert!(shown.contains("1 0 0 1 153 625.6 Tm (Hi) Tj"), "{shown}");
    assert_eq!(made.lines, 1);
    // Nearest World, at 20 points: the top is 396 down and the ascent 16,
    // so the baseline is 412 down, y 380.
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.5), 0.5);
    let shown = content(&made.document);
    assert!(shown.contains("1 0 0 1 153 380 Tm (Hi) Tj"), "{shown}");
}

#[test]
fn the_text_is_wrapped_to_the_width_of_the_box() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    // 0.1 of 612 is 61.2 points, ten Courier glyphs at 10 points: "aaaaa "
    // is 36 and "bbbbb" after it ends at 66, past the box.
    let made = added(
        &doc,
        Follow::Nearest(&page),
        "aaaaa bbbbb ccccc",
        (0.25, 0.2),
        0.1,
    );
    assert_eq!(made.lines, 3);
    // A line is 10 points under the one before: the first baseline is 8
    // below the top, 158.4 down, so at y 792 - 166.4 = 625.6, then 615.6
    // and 605.6.
    let shown = content(&made.document);
    for y in ["625.6", "615.6", "605.6"] {
        assert!(
            shown.contains(&format!("1 0 0 1 153 {y} Tm")),
            "{y}: {shown}"
        );
    }
}

#[test]
fn the_style_overrides_what_the_block_gives() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    let style = TextStyle::default().with_size(14.0).with_colour(Rgb {
        r: 0.0,
        g: 0.0,
        b: 1.0,
    });
    let new = NewText::new("Hi", (0.25, 0.2), 0.5).with_style(style);
    let made = add_text(&doc, 1, Follow::Nearest(&page), &new, &NoProgress).unwrap();
    let shown = content(&made.document);
    assert!(shown.contains("/F1 14 Tf"), "{shown}");
    assert!(shown.contains("0 0 1 rg"), "{shown}");
}

#[test]
fn a_block_named_by_the_caller_is_followed_whatever_is_nearer() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Block(&page.blocks[1]), "Hi", (0.25, 0.2), 0.5);
    assert_eq!(made.followed, None);
    assert!(content(&made.document).contains("/F2 20 Tf"));
}

#[test]
fn the_colour_of_the_block_is_the_colour_of_the_new_text() {
    let fixture = one_page("1 0 0 rg BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.2), 0.5);
    assert!(content(&made.document).contains("1 0 0 rg"));
}

#[test]
fn the_size_is_the_one_the_reader_sees_when_the_page_scales_its_text() {
    // 20 points under a CTM of one half is 10 points on the page.
    let fixture = one_page("0.5 0 0 0.5 0 0 cm BT /F1 20 Tf 200 1400 Td (Hello) Tj ET");
    let doc = fixture.open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.2), 0.5);
    let shown = content(&made.document);
    assert!(shown.contains("/F1 10 Tf"), "{shown}");
    assert!(!shown.contains("/F1 20 Tf 0 Tc"), "{shown}");
}

#[test]
fn text_that_cannot_be_seen_is_not_followed() {
    // The invisible text is nearer; the visible text is 20 points.
    let fixture = one_page(
        "BT /F1 10 Tf 3 Tr 100 700 Td (OCR) Tj ET BT /F1 20 Tf 0 Tr 100 300 Td (Real) Tj ET",
    );
    let doc = fixture.open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.1), 0.5);
    assert!(content(&made.document).contains("/F1 20 Tf"));
    assert_eq!(made.followed, Some(1));
}

#[test]
fn the_new_text_is_a_stream_of_its_own_and_the_page_keeps_its_content() {
    let fixture = two_fonts();
    let doc = fixture.open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.2), 0.5);
    let page_object = made
        .document
        .required(ObjectId {
            number: PAGE,
            generation: 0,
        })
        .unwrap();
    let Some(Value::Array(streams)) = page_object
        .dictionary()
        .unwrap()
        .get(b"Contents")
        .map(|o| o.value().clone())
    else {
        panic!("the contents are not an array");
    };
    assert_eq!(streams.len(), 2);
    let Value::Reference(first) = streams[0].value() else {
        panic!("not a reference");
    };
    assert_eq!(first.number, CONTENT);
    let after = text_of_document(&made.document);
    assert_eq!(after.blocks.len(), 3);
    assert!(after.text().contains("Hi"));
}

#[test]
fn a_font_of_the_list_is_embedded_when_the_block_cannot_show_the_text() {
    let dir = tempfile::tempdir().unwrap();
    let list = cyrillic_list(&dir);
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    let new = NewText::new("\u{416}", (0.25, 0.2), 0.5).with_fonts(&list);
    let made = add_text(&doc, 1, Follow::Nearest(&page), &new, &NoProgress).unwrap();
    assert_eq!(made.family, "Cyrillic Sans");
    assert_eq!(made.substitutions.len(), 1);
    assert!(content(&made.document).contains("/KernFont1 10 Tf"));
    assert_eq!(text_of_document(&made.document).blocks.len(), 3);
}

#[test]
fn the_family_of_a_font_used_again_is_the_name_of_the_pages_font() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.2), 0.5);
    assert_eq!(made.family, "Courier");
    assert!(made.substitutions.is_empty());
}

#[test]
fn a_page_with_no_text_needs_a_size_and_a_font_list() {
    let fixture = one_page("0 0 10 10 re f");
    let doc = fixture.open();
    let page = text_of_document(&doc);
    assert!(page.blocks.is_empty());
    match add_text(
        &doc,
        1,
        Follow::Nearest(&page),
        &NewText::new("Hi", (0.25, 0.2), 0.5),
        &NoProgress,
    ) {
        Err(PdfError::NoSizeForNewText) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
    let sized =
        NewText::new("Hi", (0.25, 0.2), 0.5).with_style(TextStyle::default().with_size(12.0));
    match add_text(&doc, 1, Follow::Nearest(&page), &sized, &NoProgress) {
        Err(PdfError::FontListRequired) => {}
        other => panic!("{:?}", other.map(|_| ())),
    }
}

#[test]
fn a_page_with_no_text_takes_the_list_face_closest_to_helvetica_in_black() {
    let dir = tempfile::tempdir().unwrap();
    let list = super::edit::font_list(&dir);
    let fixture = one_page("0 0 10 10 re f");
    let doc = fixture.open();
    let page = text_of_document(&doc);
    let new = NewText::new("AB", (0.25, 0.2), 0.5)
        .with_style(TextStyle::default().with_size(12.0))
        .with_fonts(&list);
    let made = add_text(&doc, 1, Follow::Nearest(&page), &new, &NoProgress).unwrap();
    assert_eq!(made.family, "Test Sans");
    assert_eq!(made.followed, None);
    let shown = content(&made.document);
    assert!(shown.contains("/KernFont1 12 Tf"), "{shown}");
    assert!(shown.contains("0 0 0 rg"), "{shown}");
    assert_eq!(text_of_document(&made.document).text().trim(), "AB");
}

#[test]
fn text_on_a_turned_page_stands_upright() {
    let fixture = build(
        "BT /F1 10 Tf 100 700 Td (Hello) Tj ET",
        "",
        "/Rotate 90",
        None,
        &[],
    );
    let doc = fixture.open();
    let page = text_of_document(&doc);
    let made = added(&doc, Follow::Nearest(&page), "Hi", (0.25, 0.5), 0.5);
    // Displayed 792 wide and 612 high: the box's top is 306 down and its
    // left 198 across; the baseline 8 lower is at 314 down. The user space
    // point (x, y) is at (y, x), so text space's x runs up user y and its y
    // runs along user x, towards the left.
    let shown = content(&made.document);
    assert!(shown.contains("0 1 -1 0 314 198 Tm (Hi) Tj"), "{shown}");
}

#[test]
fn a_box_that_is_not_on_a_page_is_refused() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    for (at, width) in [
        ((f64::NAN, 0.2), 0.5),
        ((0.2, 0.2), 0.0),
        ((0.2, 0.2), -1.0),
    ] {
        let new = NewText::new("Hi", at, width);
        match add_text(&doc, 1, Follow::Nearest(&page), &new, &NoProgress) {
            Err(PdfError::Invalid(_)) => {}
            other => panic!("{at:?} {width}: {:?}", other.map(|_| ())),
        }
    }
}

#[test]
fn the_blocks_are_ordered_by_their_distance_from_the_point() {
    let doc = two_fonts().open();
    let page = text_of_document(&doc);
    // Hello is 92 points from the top and World 492; 0.2 of 792 is 158.4.
    assert_eq!(
        blocks_by_distance(&doc, 1, &page, (0.25, 0.2)).unwrap(),
        vec![0, 1]
    );
    assert_eq!(
        blocks_by_distance(&doc, 1, &page, (0.25, 0.8)).unwrap(),
        vec![1, 0]
    );
}

#[test]
fn a_block_gives_its_font_size_and_colour() {
    let fixture = one_page("0 0 1 rg BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let doc = fixture.open();
    let page = text_of_document(&doc);
    assert_eq!(
        block_style(&doc, 1, &page.blocks[0], &NoProgress).unwrap(),
        Some(BlockStyle {
            font: "Courier".to_owned(),
            embedded: false,
            size: 10.0,
            colour: Some(Rgb {
                r: 0.0,
                g: 0.0,
                b: 1.0
            }),
        })
    );
}

#[test]
fn invisible_text_gives_no_style() {
    let fixture = one_page("BT /F1 10 Tf 3 Tr 100 700 Td (OCR) Tj ET");
    let doc = fixture.open();
    let page = text_of_document(&doc);
    assert_eq!(
        block_style(&doc, 1, &page.blocks[0], &NoProgress).unwrap(),
        None
    );
}
