// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `Document::text_block_editability`: which blocks can be edited and why
//! the others cannot.

use super::*;
use kernkraft_pdf::text::{TextBlock, TextChar, TextLine};
use kernkraft_pdf::{EditBlocker, Editability, Rect};

const HELLO: &str = "BT /F1 10 Tf 100 700 Td (Hello) Tj ET";

fn editability(fixture: &Fixture) -> Editability {
    let doc = fixture.open();
    let page = doc.text_page(1, &NoProgress).unwrap();
    doc.text_block_editability(1, &page.blocks[0], &NoProgress)
        .unwrap()
}

#[test]
fn a_block_of_plain_text_is_editable() {
    assert_eq!(editability(&one_page(HELLO)), Editability::Editable);
}

#[test]
fn invisible_text_is_editable() {
    // Render mode 3 (9.3.6) paints nothing, yet its text is text of the page
    // (the recognised text of a scan), so it can be edited.
    let fixture = one_page("BT /F1 10 Tf 3 Tr 100 700 Td (Hello) Tj ET");
    assert_eq!(editability(&fixture), Editability::Editable);
}

#[test]
fn a_clipping_render_mode_blocks_the_edit() {
    // Mode 7 fills nothing and adds the glyphs to the clipping path.
    let fixture = one_page("BT /F1 10 Tf 7 Tr 100 700 Td (Hello) Tj ET");
    assert_eq!(
        editability(&fixture),
        Editability::Blocked(EditBlocker::ClippingRenderMode)
    );
}

#[test]
fn a_type_3_font_blocks_the_edit() {
    let fixture = build(
        "BT /T3 10 Tf 100 700 Td (a) Tj ET",
        "/T3 70 0 R",
        "",
        None,
        &[
            (
                70,
                b"<< /Type /Font /Subtype /Type3 /FontBBox [0 0 1000 1000] \
                  /FontMatrix [0.001 0 0 0.001 0 0] /CharProcs << /a 71 0 R >> \
                  /Encoding << /Type /Encoding /Differences [97 /a] >> \
                  /FirstChar 97 /LastChar 97 /Widths [1000] >>"
                    .as_slice(),
            ),
            (
                71,
                &stream("", b"1000 0 0 0 1000 1000 d1 0 0 1000 1000 re f"),
            ),
        ],
    );
    assert_eq!(
        editability(&fixture),
        Editability::Blocked(EditBlocker::Type3Font)
    );
}

#[test]
fn text_in_a_form_is_editable() {
    let fixture = build(
        "/Fm1 Do",
        "",
        "/Resources << /XObject << /Fm1 70 0 R >> /Font << /F1 60 0 R >> >>",
        None,
        &[(
            70,
            &stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 60 0 R >> >>",
                b"BT /F1 10 Tf 100 700 Td (Hello) Tj ET",
            ),
        )],
    );
    assert_eq!(editability(&fixture), Editability::Editable);
}

#[test]
fn a_block_shown_by_two_streams_blocks_the_edit() {
    // The form's word ends where the page's word begins, on one baseline,
    // so the layout makes one block of the two.
    let fixture = build(
        "BT /F1 10 Tf 130 700 Td (World) Tj ET /Fm1 Do",
        "",
        "/Resources << /XObject << /Fm1 70 0 R >> /Font << /F1 60 0 R >> >>",
        None,
        &[(
            70,
            &stream(
                "/Type /XObject /Subtype /Form /BBox [0 0 612 792] \
                 /Resources << /Font << /F1 60 0 R >> >>",
                b"BT /F1 10 Tf 100 700 Td (Hello) Tj ET",
            ),
        )],
    );
    assert_eq!(
        editability(&fixture),
        Editability::Blocked(EditBlocker::SplitAcrossStreams)
    );
}

#[test]
fn an_operation_across_two_content_streams_blocks_the_edit() {
    // 7.8.2 allows the break between the string and its operator.
    let fixture = build(
        "BT /F1 10 Tf 100 700 Td (Hello)",
        "",
        "",
        Some("[11 0 R 12 0 R]"),
        &[(12, &stream("", b"Tj ET"))],
    );
    assert_eq!(
        editability(&fixture),
        Editability::Blocked(EditBlocker::StraddlesStreams)
    );
}

#[test]
fn a_block_of_another_snapshot_is_not_on_the_page() {
    let before = one_page(HELLO);
    let page = text_of(&before);
    let after = one_page("BT /F1 10 Tf ET");
    let doc = after.open();
    assert_eq!(
        doc.text_block_editability(1, &page.blocks[0], &NoProgress)
            .unwrap(),
        Editability::Blocked(EditBlocker::NotOnPage)
    );
}

#[test]
fn a_block_of_only_synthetic_spaces_has_no_glyphs() {
    let space = TextChar {
        text: " ".to_owned(),
        rect: Rect::default(),
        synthetic: true,
        source: None,
    };
    let block = TextBlock {
        rect: Rect::default(),
        lines: vec![TextLine {
            rect: Rect::default(),
            chars: vec![space],
        }],
    };
    let doc = one_page(HELLO).open();
    assert_eq!(
        doc.text_block_editability(1, &block, &NoProgress).unwrap(),
        Editability::Blocked(EditBlocker::NoGlyphs)
    );
}
