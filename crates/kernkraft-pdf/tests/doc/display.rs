// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The displayed page (`display.rs`): ISO 32000-1:2008, 7.7.3.3.

use super::*;
use kernkraft_pdf::doc::Rectangle;
use kernkraft_pdf::{DiagnosticKind, PageGeometry};

/// A one-page document whose page has `entries` beside a MediaBox of
/// 200 by 300 points.
fn geometry(entries: &str) -> (kernkraft_pdf::doc::Document, PageGeometry) {
    let page = format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 300] {entries} >>");
    let document = table_file(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, page.as_bytes()),
        ],
        "/Root 1 0 R",
    )
    .open();
    let geometry = document.page_geometry(1).unwrap();
    (document, geometry)
}

fn size(geometry: &PageGeometry) -> (f64, f64) {
    (geometry.width, geometry.height)
}

#[test]
fn an_upright_page_is_its_media_box() {
    let (_, geometry) = geometry("");

    assert_eq!(size(&geometry), (200.0, 300.0));
    assert_eq!(geometry.rotate, 0);
    assert_eq!(
        geometry.bounds,
        Rectangle {
            llx: 0.0,
            lly: 0.0,
            urx: 200.0,
            ury: 300.0
        }
    );
}

#[test]
fn a_quarter_turn_swaps_width_and_height_and_a_half_turn_does_not() {
    assert_eq!(size(&geometry("/Rotate 90").1), (300.0, 200.0));
    assert_eq!(size(&geometry("/Rotate 270").1), (300.0, 200.0));
    assert_eq!(size(&geometry("/Rotate 180").1), (200.0, 300.0));
    assert_eq!(geometry("/Rotate 90").1.rotate, 90);
}

#[test]
fn the_crop_box_is_the_displayed_page() {
    let (_, geometry) = geometry("/CropBox [10 20 110 120]");

    assert_eq!(size(&geometry), (100.0, 100.0));
    assert_eq!(
        geometry.bounds,
        Rectangle {
            llx: 10.0,
            lly: 20.0,
            urx: 110.0,
            ury: 120.0
        }
    );
}

#[test]
fn a_crop_box_past_the_media_box_is_clipped_to_it() {
    // [100 100 400 500] meets [0 0 200 300] in [100 100 200 300].
    let (_, upright) = geometry("/CropBox [100 100 400 500]");
    let (_, turned) = geometry("/CropBox [100 100 400 500] /Rotate 90");

    assert_eq!(size(&upright), (100.0, 200.0));
    assert_eq!(
        upright.bounds,
        Rectangle {
            llx: 100.0,
            lly: 100.0,
            urx: 200.0,
            ury: 300.0
        }
    );
    assert_eq!(size(&turned), (200.0, 100.0));
}

#[test]
fn a_crop_box_outside_the_media_box_is_ignored_and_reported() {
    let (document, geometry) = geometry("/CropBox [500 500 600 600]");

    assert_eq!(size(&geometry), (200.0, 300.0));
    assert!(document
        .diagnostics()
        .iter()
        .any(|d| matches!(d.kind, DiagnosticKind::IgnoredEntry { .. })));
}

#[test]
fn a_page_with_no_media_box_is_letter() {
    let document = table_file(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, b"<< /Type /Page /Parent 2 0 R >>"),
        ],
        "/Root 1 0 R",
    )
    .open();

    assert_eq!(size(&document.page_geometry(1).unwrap()), (612.0, 792.0));
}

#[test]
fn a_page_past_the_end_is_out_of_range() {
    let (document, _) = geometry("");

    assert!(matches!(
        document.page_geometry(2),
        Err(kernkraft_pdf::PdfError::PageOutOfRange { page: 2, count: 1 })
    ));
}
