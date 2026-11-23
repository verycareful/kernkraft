// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `Document::annotations`: what a page's annotations are, as the file
//! says. ISO 32000-1, 12.5.2 (Tables 164 and 165), 12.5.6.10 (Table 179),
//! 12.5.6.13 (Table 184) and 8.6.4 (the device colour spaces `C` is given
//! in).

use super::*;
use kernkraft_pdf::{AnnotationKind, DiagnosticKind};

#[test]
fn a_highlight_lists_its_quads_text_colour_opacity_and_date() {
    // Two quads, one a line below the other, as `QuadPoints` of a selection
    // across two lines. `Contents` is UTF-16BE (7.9.2.2) for "é€": FE FF
    // 00 E9 20 AC.
    let fixture = annotated(
        "",
        &[50],
        &[(
            50,
            b"<< /Type /Annot /Subtype /Highlight /Rect [100 680 160 720] \
              /QuadPoints [100 720 160 720 100 700 160 700 100 700 130 700 100 680 130 680] \
              /C [0.5] /CA 0.25 /T (ann) /M (D:20260102030405+05'30') \
              /Contents <FEFF00E920AC> /F 4 >>"
                .as_slice(),
        )],
    );
    let listed = fixture.open().annotations(1).unwrap();
    assert_eq!(listed.len(), 1);
    let annotation = &listed[0];
    assert_eq!(annotation.id, Some(id(50)));
    assert_eq!(annotation.kind, AnnotationKind::Highlight);
    assert_rect(annotation.rect, 100.0, 72.0, 160.0, 112.0, "rect");
    assert_eq!(annotation.quads.len(), 2);
    assert_rect(annotation.quads[0], 100.0, 72.0, 160.0, 92.0, "first quad");
    assert_rect(
        annotation.quads[1],
        100.0,
        92.0,
        130.0,
        112.0,
        "second quad",
    );
    assert_eq!(annotation.contents.as_deref(), Some("é€"));
    assert_eq!(annotation.author.as_deref(), Some("ann"));
    // A gray of 0.5 is that grey in each of red, green and blue.
    let colour = annotation.colour.unwrap();
    assert_close(colour.r, 0.5, "red");
    assert_close(colour.g, 0.5, "green");
    assert_close(colour.b, 0.5, "blue");
    assert_close(annotation.opacity, 0.25, "opacity");
    assert_eq!(annotation.flags, 4);
    let modified = annotation.modified.unwrap();
    assert_eq!((modified.year, modified.month, modified.day), (2026, 1, 2));
    assert_eq!((modified.hour, modified.minute, modified.second), (3, 4, 5));
    assert_eq!(modified.offset_minutes, Some(5 * 60 + 30));
    assert!(annotation.line_width.is_none());
}

/// A four-number `C` is DeviceCMYK, converted through the CMYK profile of
/// the document: here the application's, which makes cyan neutral.
#[test]
fn a_cmyk_colour_follows_the_cmyk_profile_of_the_document() {
    let annotation = b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /C [1 0 0 0] >>".to_vec();
    let fixture = annotated("", &[50], &[(50, annotation.as_slice())]);
    let document = fixture.open();
    document
        .set_cmyk_profile(&crate::pdf_fixture::grey_by_black_profile())
        .unwrap();
    let colour = document.annotations(1).unwrap()[0].colour.unwrap();
    for level in [colour.r, colour.g, colour.b] {
        assert!((level - 1.0).abs() < 0.01, "{colour:?}");
    }
}

#[test]
fn colours_of_one_three_and_four_components_and_none() {
    let make = |c: &str| {
        format!("<< /Type /Annot /Subtype /Square /Rect [0 0 10 10] /C {c} >>").into_bytes()
    };
    let rgb = make("[0.25 0.5 0.75]");
    let cmyk_black = make("[0 0 0 1]");
    let cmyk_none = make("[0 0 0 0]");
    let empty = make("[]");
    let two = make("[0.1 0.2]");
    let fixture = annotated(
        "",
        &[50, 51, 52, 53, 54],
        &[
            (50, rgb.as_slice()),
            (51, cmyk_black.as_slice()),
            (52, cmyk_none.as_slice()),
            (53, empty.as_slice()),
            (54, two.as_slice()),
        ],
    );
    let listed = fixture.open().annotations(1).unwrap();
    let colour = |n: usize| listed[n].colour;
    let c = colour(0).unwrap();
    assert_close(c.r, 0.25, "red");
    assert_close(c.g, 0.5, "green");
    assert_close(c.b, 0.75, "blue");
    // DeviceCMYK goes through the CMYK profile: the full K is a dark
    // colour (a printed black is not a screen's 0), no ink is the paper.
    let black = colour(1).unwrap();
    assert!(
        black.r < 0.25 && black.g < 0.25 && black.b < 0.25,
        "{black:?}"
    );
    let white = colour(2).unwrap();
    assert!(
        white.r > 0.98 && white.g > 0.98 && white.b > 0.98,
        "{white:?}"
    );
    // An empty array is transparent (Table 164), and two numbers are no
    // colour.
    assert!(colour(3).is_none());
    assert!(colour(4).is_none());
}

#[test]
fn ink_lists_its_strokes_and_its_line_width() {
    let fixture = annotated(
        "",
        &[50, 51],
        &[
            (
                50,
                b"<< /Type /Annot /Subtype /Ink /Rect [90 390 210 410] \
                  /InkList [[100 400 200 400] [150 392 150 408]] /BS << /W 3 >> >>"
                    .as_slice(),
            ),
            (
                51,
                b"<< /Type /Annot /Subtype /Ink /Rect [0 0 10 10] /InkList [[1 1 2 2]] \
                  /Border [0 0 5] >>"
                    .as_slice(),
            ),
        ],
    );
    let listed = fixture.open().annotations(1).unwrap();
    assert_eq!(listed[0].kind, AnnotationKind::Ink);
    assert_eq!(listed[0].strokes.len(), 2);
    // User y 400 is 392 from the top.
    assert_close(listed[0].strokes[0][0].0, 100.0 / PAGE_WIDTH, "x");
    assert_close(listed[0].strokes[0][0].1, 392.0 / PAGE_HEIGHT, "y");
    assert_close(listed[0].strokes[1][1].1, 384.0 / PAGE_HEIGHT, "y");
    assert_eq!(listed[0].line_width, Some(3.0));
    // `BS` is the width where there is one, `Border`'s third number where
    // there is not (Table 166).
    assert_eq!(listed[1].line_width, Some(5.0));
}

#[test]
fn a_note_that_stays_upright_is_boxed_from_its_top_left_corner() {
    // On a page turned 90 degrees (displayed 792 wide) an upright icon
    // 20 by 20 whose Rect's upper-left corner is user (200, 300) hangs
    // from displayed (300, 200) right and down.
    let fixture = document(
        "",
        "/Rotate 90 /Annots [50 0 R]",
        "",
        &[(
            50,
            b"<< /Type /Annot /Subtype /Text /Rect [200 280 220 300] /F 16 >>".as_slice(),
        )],
    );
    let listed = fixture.open().annotations(1).unwrap();
    assert_eq!(listed[0].kind, AnnotationKind::Note);
    assert_close(listed[0].rect.left, 300.0 / 792.0, "left");
    assert_close(listed[0].rect.top, 200.0 / 612.0, "top");
    assert_close(listed[0].rect.right, 320.0 / 792.0, "right");
    assert_close(listed[0].rect.bottom, 220.0 / 612.0, "bottom");
}

#[test]
fn every_kind_is_listed_with_its_flags_in_page_order() {
    let fixture = annotated(
        "",
        &[50, 51, 52, 53],
        &[
            (
                50,
                b"<< /Type /Annot /Subtype /Link /Rect [0 0 10 10] >>".as_slice(),
            ),
            (
                51,
                b"<< /Type /Annot /Subtype /Popup /Rect [0 0 10 10] /F 2 >>".as_slice(),
            ),
            (
                52,
                b"<< /Type /Annot /Subtype /Underline /Rect [0 0 10 10] \
                  /QuadPoints [0 10 10 10 0 0 10 0] /F 2 >>"
                    .as_slice(),
            ),
            (
                53,
                b"<< /Type /Annot /Subtype /StrikeOut /Rect [0 0 10 10] \
                  /QuadPoints [0 10 10 10 0 0 10 0] >>"
                    .as_slice(),
            ),
        ],
    );
    let listed = fixture.open().annotations(1).unwrap();
    let kinds: Vec<_> = listed.iter().map(|a| a.kind.clone()).collect();
    assert_eq!(
        kinds,
        vec![
            AnnotationKind::Other("Link".to_string()),
            AnnotationKind::Other("Popup".to_string()),
            AnnotationKind::Underline,
            AnnotationKind::StrikeOut,
        ]
    );
    // A hidden annotation is listed, with the flag that says so (Table
    // 165: Hidden is bit 2).
    assert_eq!(listed[2].flags, 2);
    assert!(!AnnotationKind::Other("Link".to_string()).is_drawn_by_library());
    assert!(AnnotationKind::Ink.is_drawn_by_library());
}

#[test]
fn a_page_with_no_annots_lists_none() {
    assert!(blank().open().annotations(1).unwrap().is_empty());
}

#[test]
fn a_direct_annotation_is_listed_without_an_object() {
    let fixture = document(
        "",
        "/Annots [<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] >>]",
        "",
        &[],
    );
    let listed = fixture.open().annotations(1).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, None);
}

#[test]
fn an_annotation_with_no_rect_is_left_out_and_reported() {
    let fixture = annotated(
        "",
        &[50, 51],
        &[
            (
                50,
                b"<< /Type /Annot /Subtype /Text /Contents (no box) >>".as_slice(),
            ),
            (
                51,
                b"<< /Type /Annot /Subtype /Text /Rect [0 0 20] >>".as_slice(),
            ),
        ],
    );
    let document = fixture.open();
    assert!(document.annotations(1).unwrap().is_empty());
    let reported = document
        .diagnostics()
        .iter()
        .filter(|d| matches!(&d.kind, DiagnosticKind::IgnoredEntry { key } if key.as_bytes() == b"Rect"))
        .count();
    assert_eq!(reported, 2);
}

#[test]
fn damaged_quad_points_and_ink_lists_are_left_out_and_reported() {
    let fixture = annotated(
        "",
        &[50, 51],
        &[
            (
                50,
                // Seven numbers are no quadrilateral (Table 179).
                b"<< /Type /Annot /Subtype /Highlight /Rect [0 0 10 10] \
                  /QuadPoints [0 10 10 10 0 0 10] >>"
                    .as_slice(),
            ),
            (
                51,
                // A stroke of an odd count and one that is no array of
                // numbers (Table 184), beside a good one.
                b"<< /Type /Annot /Subtype /Ink /Rect [0 0 10 10] \
                  /InkList [[1 1 2] [3 (x)] [1 1 2 2]] >>"
                    .as_slice(),
            ),
        ],
    );
    let document = fixture.open();
    let listed = document.annotations(1).unwrap();
    assert!(listed[0].quads.is_empty());
    assert_eq!(listed[1].strokes.len(), 1);
    let ignored = |key: &[u8]| {
        document
            .diagnostics()
            .iter()
            .filter(|d| matches!(&d.kind, DiagnosticKind::IgnoredEntry { key: k } if k.as_bytes() == key))
            .count()
    };
    // The document's log holds a report once for an object, so the two bad
    // strokes of annotation 51 are one report.
    assert_eq!(ignored(b"QuadPoints"), 1);
    assert_eq!(ignored(b"InkList"), 1);
}

#[test]
fn an_annots_entry_that_is_no_annotation_is_skipped() {
    let fixture = document(
        "",
        "/Annots [50 0 R 99 0 R 7 (text) 11 0 R]",
        "",
        &[(
            50,
            b"<< /Type /Annot /Subtype /Text /Rect [0 0 20 20] >>".as_slice(),
        )],
    );
    // Object 99 does not exist (null, 7.3.10), a number and a string are no
    // dictionaries, and object 11 is the page's content stream.
    let listed = fixture.open().annotations(1).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, Some(id(50)));
}
