// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Annotation appearances: ISO 32000-1:2008, 12.5. The page is 20 by 20
//! points at a pixel to the point; an annotation whose `Rect` is the box 5
//! to 15 covers the pixels 5 to 15 of both columns and rows.

use kernkraft_pdf::render::RenderOptions;
use kernkraft_pdf::{DiagnosticKind, PaintFault};

use crate::{
    assert_box, assert_near, diagnostics_of, document, hidden_group_page, render, render_with,
    stream, RED, WHITE,
};

const ANNOTATION: u32 = 30;
const NORMAL: u32 = 31;
const OTHER: u32 = 32;

/// An appearance stream whose unit square is red.
fn red_square() -> Vec<u8> {
    stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 1 1]",
        b"1 0 0 rg 0 0 1 1 re f",
    )
}

fn annotation(entries: &str) -> Vec<u8> {
    format!("<< /Type /Annot /Subtype /Square /Rect [5 5 15 15] {entries} >>").into_bytes()
}

fn page(
    content: &str,
    annotation_object: &[u8],
    more: &[(u32, &[u8])],
) -> kernkraft_pdf::doc::Document {
    let normal = red_square();
    let mut objects: Vec<(u32, &[u8])> = vec![(ANNOTATION, annotation_object), (NORMAL, &normal)];
    objects.extend_from_slice(more);
    document(
        "/MediaBox [0 0 20 20] /Annots [30 0 R]",
        content,
        "",
        &objects,
    )
}

#[test]
fn the_normal_appearance_fills_the_rectangle() {
    let doc = page("", &annotation("/AP << /N 31 0 R >>"), &[]);
    let raster = render(&doc);
    assert_box(&raster, 5, 5, 15, 15, RED);
    assert_box(&raster, 0, 0, 20, 5, WHITE);
    assert_box(&raster, 0, 15, 20, 20, WHITE);
    assert_box(&raster, 0, 5, 5, 15, WHITE);
}

#[test]
fn an_appearance_is_over_the_page_content() {
    let doc = page(
        "0 0 1 rg 0 0 20 20 re f",
        &annotation("/AP << /N 31 0 R >>"),
        &[],
    );
    let raster = render(&doc);
    assert_box(&raster, 5, 5, 15, 15, RED);
    assert_eq!(crate::px(&raster, 2, 2), crate::BLUE);
}

#[test]
fn the_state_the_annotation_names_picks_the_appearance() {
    let blue = stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 1 1]",
        b"0 0 1 rg 0 0 1 1 re f",
    );
    let doc = page(
        "",
        &annotation("/AS /Off /AP << /N << /On 31 0 R /Off 32 0 R >> >>"),
        &[(OTHER, &blue)],
    );
    let raster = render(&doc);
    assert_box(&raster, 5, 5, 15, 15, crate::BLUE);
    let doc = page(
        "",
        &annotation("/AS /On /AP << /N << /On 31 0 R /Off 32 0 R >> >>"),
        &[(OTHER, &blue)],
    );
    assert_box(&render(&doc), 5, 5, 15, 15, RED);
}

#[test]
fn the_form_matrix_and_box_are_fitted_to_the_rectangle() {
    // The box is 2 by 4 and turned a quarter: its box after the matrix is 4
    // wide and 2 high, which the fit stretches to the 10 by 10 rectangle. The
    // left half of the form (x 0 to 1) is red.
    let form = stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 2 4] /Matrix [0 1 -1 0 0 0]",
        b"1 0 0 rg 0 0 1 4 re f",
    );
    let doc = page("", &annotation("/AP << /N 32 0 R >>"), &[(OTHER, &form)]);
    let raster = render(&doc);
    // The turn takes the form's x to the page's y: red is the lower half
    // of the rectangle, the user rows 5 to 10, pixel rows 10 to 15.
    assert_box(&raster, 5, 10, 15, 15, RED);
    assert_box(&raster, 5, 5, 15, 10, WHITE);
}

#[test]
fn a_hidden_annotation_is_not_drawn() {
    for flags in [2, 32, 2 | 4] {
        let doc = page(
            "",
            &annotation(&format!("/F {flags} /AP << /N 31 0 R >>")),
            &[],
        );
        assert_box(&render(&doc), 0, 0, 20, 20, WHITE);
    }
}

#[test]
fn flags_that_do_not_hide_leave_the_annotation_drawn() {
    // Print (4) and Locked (128).
    let doc = page("", &annotation("/F 132 /AP << /N 31 0 R >>"), &[]);
    assert_box(&render(&doc), 5, 5, 15, 15, RED);
}

#[test]
fn annotations_can_be_left_out_by_the_caller() {
    let doc = page("", &annotation("/AP << /N 31 0 R >>"), &[]);
    let options = RenderOptions {
        annotations: false,
        ..Default::default()
    };
    let raster = render_with(&doc, &options).expect("renders");
    assert_box(&raster, 0, 0, 20, 20, WHITE);
}

#[test]
fn an_annotation_in_hidden_optional_content_is_not_drawn() {
    let normal = red_square();
    let annotation = annotation("/OC 20 0 R /AP << /N 31 0 R >>");
    let doc = hidden_group_page(
        "/Annots [30 0 R]",
        "",
        "",
        &[(ANNOTATION, &annotation), (NORMAL, &normal)],
    );
    assert_box(&render(&doc), 0, 0, 20, 20, WHITE);
}

#[test]
fn the_appearances_own_blend_mode_applies_over_the_page() {
    let multiply = stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 1 1] \
         /Resources << /ExtGState << /M << /BM /Multiply >> >> >>",
        b"/M gs 0.6 0.2 0.4 rg 0 0 1 1 re f",
    );
    let doc = page(
        "0.8 0.4 0.2 rg 0 0 20 20 re f",
        &annotation("/AP << /N 32 0 R >>"),
        &[(OTHER, &multiply)],
    );
    let raster = render(&doc);
    assert_near(crate::px(&raster, 10, 10), [122, 20, 20, 255], 1);
    assert_eq!(crate::px(&raster, 2, 2), [204, 102, 51, 255]);
}

#[test]
fn an_annotation_follows_the_pages_rotation() {
    // Turned a quarter clockwise, the page's left half is its top half.
    let normal = red_square();
    let annotation = b"<< /Type /Annot /Subtype /Square /Rect [0 0 10 20] \
                       /AP << /N 31 0 R >> >>"
        .to_vec();
    let doc = document(
        "/MediaBox [0 0 20 20] /Rotate 90 /Annots [30 0 R]",
        "",
        "",
        &[(ANNOTATION, &annotation), (NORMAL, &normal)],
    );
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 10, RED);
    assert_box(&raster, 0, 10, 20, 20, WHITE);
}

#[test]
fn popups_and_links_draw_nothing_and_need_no_report() {
    let popup = b"<< /Type /Annot /Subtype /Popup /Rect [5 5 15 15] >>".to_vec();
    let doc = document(
        "/MediaBox [0 0 20 20] /Annots [30 0 R 32 0 R]",
        "",
        "",
        &[
            (ANNOTATION, &popup),
            (
                OTHER,
                b"<< /Type /Annot /Subtype /Link /Rect [0 0 20 20] /Border [0 0 1] >>",
            ),
        ],
    );
    assert_box(&render(&doc), 0, 0, 20, 20, WHITE);
    assert!(diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::NotRendered { .. })).is_empty());
}

#[test]
fn an_annotation_with_no_appearance_is_reported_once() {
    let bare = b"<< /Type /Annot /Subtype /Square /Rect [5 5 15 15] >>".to_vec();
    let doc = document(
        "/MediaBox [0 0 20 20] /Annots [30 0 R 32 0 R]",
        "",
        "",
        &[(ANNOTATION, &bare), (OTHER, &bare)],
    );
    assert_box(&render(&doc), 0, 0, 20, 20, WHITE);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(
            k,
            DiagnosticKind::NotRendered {
                feature: "annotation without an appearance"
            }
        ))
        .len(),
        1
    );
}

#[test]
fn an_appearance_with_no_area_is_reported() {
    let flat = stream(" /Type /XObject /Subtype /Form /BBox [0 0 0 5]", b"");
    let doc = page("", &annotation("/AP << /N 32 0 R >>"), &[(OTHER, &flat)]);
    assert_box(&render(&doc), 0, 0, 20, 20, WHITE);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(
            k,
            DiagnosticKind::AppearanceUnusable {
                fault: PaintFault::Size
            }
        ))
        .len(),
        1
    );
}

/// A page with the annotation of `flags` in the box 2 to 8 by 2 to 4, with
/// `page_entries` added.
fn flagged(flags: u32, page_entries: &str) -> kernkraft_pdf::doc::Document {
    let normal = red_square();
    let annotation = b"<< /Type /Annot /Subtype /Square /Rect [2 2 8 4] \
                       /F FLAGS /AP << /N 31 0 R >> >>"
        .to_vec();
    let annotation = String::from_utf8(annotation)
        .unwrap()
        .replace("FLAGS", &flags.to_string())
        .into_bytes();
    document(
        &format!("/MediaBox [0 0 20 20] /Annots [30 0 R] {page_entries}"),
        "",
        "",
        &[(ANNOTATION, &annotation), (NORMAL, &normal)],
    )
}

fn red_box(raster: &kernkraft_pdf::render::Raster) -> (u32, u32, u32, u32) {
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for y in 0..raster.height {
        for x in 0..raster.width {
            if crate::px(raster, x, y) == RED {
                x0 = x0.min(x);
                y0 = y0.min(y);
                x1 = x1.max(x + 1);
                y1 = y1.max(y + 1);
            }
        }
    }
    (x0, y0, x1, y1)
}

#[test]
fn a_turned_page_turns_the_annotation_with_it() {
    // Turned a quarter, the box of 6 by 2 is 2 wide and 6 tall: user (x, y)
    // is at pixel (y, x).
    let raster = render(&flagged(0, "/Rotate 90"));
    assert_eq!(red_box(&raster), (2, 2, 4, 8));
}

#[test]
fn no_rotate_keeps_the_annotation_upright_from_its_upper_left_corner() {
    // The upper-left corner of the rectangle, user (2, 4), is at pixel
    // (4, 2) on the turned page; the box runs from there to the right and
    // down, 6 by 2.
    let raster = render(&flagged(16, "/Rotate 90"));
    assert_eq!(red_box(&raster), (4, 2, 10, 4));
}

#[test]
fn a_scaled_render_scales_the_annotation_with_the_page() {
    let options = RenderOptions {
        scale: 2.0,
        ..Default::default()
    };
    let raster = render_with(&flagged(0, ""), &options).unwrap();
    // User (2..8, 2..4) at two pixels to the point; the page is 40 tall.
    assert_eq!(red_box(&raster), (4, 32, 16, 36));
}

#[test]
fn no_zoom_keeps_the_annotation_at_its_size_at_one_pixel_to_the_point() {
    let options = RenderOptions {
        scale: 2.0,
        ..Default::default()
    };
    let raster = render_with(&flagged(8, ""), &options).unwrap();
    // The corner stays at pixel (4, 32); the box is 6 by 2 pixels.
    assert_eq!(red_box(&raster), (4, 32, 10, 34));
}

#[test]
fn no_zoom_and_no_rotate_together() {
    let options = RenderOptions {
        scale: 2.0,
        ..Default::default()
    };
    let raster = render_with(&flagged(8 | 16, "/Rotate 90"), &options).unwrap();
    // The corner (2, 4) is at pixel (8, 4) on the turned page at this
    // scale; the box is 6 by 2 pixels from there.
    assert_eq!(red_box(&raster), (8, 4, 14, 6));
}
