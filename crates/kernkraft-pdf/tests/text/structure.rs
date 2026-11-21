// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The reading order of a tagged file: `text/structure.rs`, ISO 32000-1,
//! 14.7 and 14.8.

use super::*;
use kernkraft_pdf::DiagnosticKind;

/// Catalog entries naming the structure tree root, object 70.
const TAGGED: &str = "/StructTreeRoot 70 0 R /MarkInfo << /Marked true >>";

/// Two tagged lines far apart: "Upper" (MCID 0) at y 700 and "Lower"
/// (MCID 1) at y 600.
const TWO_SEQUENCES: &str = "/P << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Upper) Tj ET EMC \
                             /P << /MCID 1 >> BDC BT /F1 10 Tf 100 600 Td (Lower) Tj ET EMC";

fn tree(root: &str, elements: &[(u32, &str)]) -> Vec<(u32, Vec<u8>)> {
    let mut objects = vec![(70, root.as_bytes().to_vec())];
    for (number, body) in elements {
        objects.push((*number, body.as_bytes().to_vec()));
    }
    objects
}

fn tagged(content: &str, root: &str, elements: &[(u32, &str)]) -> Fixture {
    let objects = tree(root, elements);
    document(
        &[PageSpec::of(content)],
        TAGGED,
        "",
        &objects
            .iter()
            .map(|(n, b)| (*n, b.as_slice()))
            .collect::<Vec<_>>(),
    )
}

#[test]
fn an_untagged_page_is_read_by_its_geometry() {
    let page = page(TWO_SEQUENCES);
    assert_eq!(page.text(), "Upper\n\nLower");
    assert!(!page.tagged);
}

#[test]
fn the_structure_tree_gives_the_order_of_the_blocks() {
    let fixture = tagged(
        TWO_SEQUENCES,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[
            (
                71,
                "<< /Type /StructElem /S /Document /P 70 0 R /K [72 0 R 73 0 R] >>",
            ),
            (
                72,
                "<< /Type /StructElem /S /P /P 71 0 R /Pg 10 0 R /K 1 >>",
            ),
            (
                73,
                "<< /Type /StructElem /S /P /P 71 0 R /Pg 10 0 R /K 0 >>",
            ),
        ],
    );
    let page = text_of(&fixture);
    assert!(page.tagged);
    assert_eq!(page.text(), "Lower\n\nUpper");
}

#[test]
fn a_marked_content_reference_dictionary_names_its_page_and_sequence() {
    let fixture = tagged(
        TWO_SEQUENCES,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[(
            71,
            "<< /Type /StructElem /S /Document /K [ \
                 << /Type /StructElem /S /P /K << /Type /MCR /Pg 10 0 R /MCID 1 >> >> \
                 << /Type /StructElem /S /P /K << /Type /MCR /Pg 10 0 R /MCID 0 >> >> ] >>",
        )],
    );
    assert_eq!(text_of(&fixture).text(), "Lower\n\nUpper");
}

#[test]
fn spans_of_one_paragraph_are_one_line() {
    // "Hello" (MCID 0) and "World" (MCID 1), 4 points apart on one
    // baseline, are two spans of one paragraph: inline-level elements do
    // not start a block.
    let content = "/Span << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Hello) Tj ET EMC \
                   /Span << /MCID 1 >> BDC BT /F1 10 Tf 134 700 Td (World) Tj ET EMC";
    let fixture = tagged(
        content,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[
            (
                71,
                "<< /Type /StructElem /S /P /Pg 10 0 R /K [72 0 R 73 0 R] >>",
            ),
            (72, "<< /Type /StructElem /S /Span /P 71 0 R /K 0 >>"),
            (73, "<< /Type /StructElem /S /Span /P 71 0 R /K 1 >>"),
        ],
    );
    let page = text_of(&fixture);
    assert!(page.tagged);
    assert_eq!(page.blocks.len(), 1);
    assert_eq!(page.text(), "Hello World");
}

#[test]
fn a_custom_type_the_role_map_makes_inline_is_inline() {
    let content = "/Word << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Hello) Tj ET EMC \
                   /Word << /MCID 1 >> BDC BT /F1 10 Tf 134 700 Td (World) Tj ET EMC";
    let fixture = tagged(
        content,
        "<< /Type /StructTreeRoot /K 71 0 R /RoleMap << /Word /Quote /Quote /Span >> >>",
        &[
            (
                71,
                "<< /Type /StructElem /S /P /Pg 10 0 R /K [72 0 R 73 0 R] >>",
            ),
            (72, "<< /Type /StructElem /S /Word /P 71 0 R /K 0 >>"),
            (73, "<< /Type /StructElem /S /Word /P 71 0 R /K 1 >>"),
        ],
    );
    let page = text_of(&fixture);
    assert!(page.tagged);
    assert_eq!(page.blocks.len(), 1);
}

#[test]
fn table_cells_on_one_baseline_are_not_one_line() {
    // Two cells 4 points apart would be one line by geometry; as elements
    // of two cells they are two blocks, in the order of the tree: the
    // right cell first.
    let content = "/TD << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Left) Tj ET EMC \
                   /TD << /MCID 1 >> BDC BT /F1 10 Tf 128 700 Td (Right) Tj ET EMC";
    let fixture = tagged(
        content,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[
            (
                71,
                "<< /Type /StructElem /S /TR /Pg 10 0 R /K [72 0 R 73 0 R] >>",
            ),
            (72, "<< /Type /StructElem /S /TD /P 71 0 R /K 1 >>"),
            (73, "<< /Type /StructElem /S /TD /P 71 0 R /K 0 >>"),
        ],
    );
    assert_eq!(text_of(&fixture).text(), "Right\n\nLeft");
}

#[test]
fn the_same_mcid_in_two_streams_is_two_sequences() {
    // MCID 0 of the page content and MCID 0 of a form: the tree lists the
    // form's first, by `Stm`.
    let content = "/Fm0 Do /P << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Page) Tj ET EMC";
    let objects = tree(
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[(
            71,
            "<< /Type /StructElem /S /Document /K [ \
                 << /Type /StructElem /S /P /K << /Type /MCR /Pg 10 0 R /Stm 80 0 R /MCID 0 >> >> \
                 << /Type /StructElem /S /P /Pg 10 0 R /K 0 >> ] >>",
        )],
    );
    let form = stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 612 792] /Resources << /Font << /F1 60 0 R >> >>",
        b"/P << /MCID 0 >> BDC BT /F1 10 Tf 100 600 Td (Form) Tj ET EMC",
    );
    let mut extra: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    extra.push((80, &form));
    let fixture = document(
        &[PageSpec::of(content)],
        TAGGED,
        "/XObject << /Fm0 80 0 R >>",
        &extra,
    );
    assert_eq!(text_of(&fixture).text(), "Form\n\nPage");
}

#[test]
fn text_outside_the_tree_comes_after_the_tagged_text() {
    // A page number marked as an artifact, at the top of the page, is
    // read after the tagged text.
    let content = "/Artifact BMC BT /F1 10 Tf 100 750 Td (7) Tj ET EMC \
                   /P << /MCID 0 >> BDC BT /F1 10 Tf 100 700 Td (Body) Tj ET EMC";
    let fixture = tagged(
        content,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[(71, "<< /Type /StructElem /S /P /Pg 10 0 R /K 0 >>")],
    );
    assert_eq!(text_of(&fixture).text(), "Body\n\n7");
}

#[test]
fn a_tree_that_reaches_an_element_twice_is_read_once_and_reported() {
    // Element 71 is a child of itself.
    let fixture = tagged(
        TWO_SEQUENCES,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[(
            71,
            "<< /Type /StructElem /S /Document /Pg 10 0 R /K [71 0 R 0 1] >>",
        )],
    );
    let document = open(&fixture);
    let page = document.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.text(), "Upper\n\nLower");
    assert!(document.diagnostics().iter().any(
        |d| matches!(&d.kind, DiagnosticKind::IgnoredEntry { key } if key.as_bytes() == b"K")
    ));
}

#[test]
fn a_sequence_with_no_page_is_reported_and_left_out_of_the_order() {
    // No `Pg` on the element or above it: the tree cannot say which page
    // MCID 1 is of, so only MCID 0 has a place and the rest follows.
    let fixture = tagged(
        TWO_SEQUENCES,
        "<< /Type /StructTreeRoot /K 71 0 R >>",
        &[(
            71,
            "<< /Type /StructElem /S /Document /K [ \
             << /Type /StructElem /S /P /Pg 10 0 R /K 1 >> \
             << /Type /StructElem /S /P /K 0 >> ] >>",
        )],
    );
    let document = open(&fixture);
    let page = document.text_page(1, &NoProgress).unwrap();
    assert_eq!(page.text(), "Lower\n\nUpper");
    assert!(document.diagnostics().iter().any(
        |d| matches!(&d.kind, DiagnosticKind::IgnoredEntry { key } if key.as_bytes() == b"Pg")
    ));
}

#[test]
fn a_tree_nested_past_the_limit_is_refused() {
    use kernkraft_pdf::{Limit, PdfError};
    // 300 elements, each the only child of the one before: objects 100 to
    // 399.
    let mut elements: Vec<(u32, String)> = (0..299)
        .map(|i| {
            (
                100 + i,
                format!("<< /Type /StructElem /S /Div /K {} 0 R >>", 101 + i),
            )
        })
        .collect();
    elements.push((
        399,
        "<< /Type /StructElem /S /P /Pg 10 0 R /K 0 >>".to_owned(),
    ));
    let borrowed: Vec<(u32, &str)> = elements.iter().map(|(n, b)| (*n, b.as_str())).collect();
    let fixture = tagged(
        TWO_SEQUENCES,
        "<< /Type /StructTreeRoot /K 100 0 R >>",
        &borrowed,
    );
    let error = open(&fixture).text_page(1, &NoProgress).unwrap_err();
    assert!(matches!(
        error,
        PdfError::LimitExceeded {
            limit: Limit::NestingDepth,
            ..
        }
    ));
}
