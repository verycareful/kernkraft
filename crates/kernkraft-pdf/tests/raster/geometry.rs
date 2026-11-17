// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Where the page lands in the picture: the scale, a region, `Rotate`, the
//! CropBox, and the limits on the size.

use kernkraft_fs::Progress;
use kernkraft_pdf::{Limit, Rect};

use super::*;

const RED_SQUARE: &str = "1 0 0 rg 0 0 10 10 re f";

#[test]
fn the_scale_sets_the_size_in_pixels() {
    let doc = square_page(RED_SQUARE, "", &[]);
    let raster = render_with(
        &doc,
        &RenderOptions {
            scale: 2.0,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!((raster.width, raster.height), (40, 40));
    assert_box(&raster, 0, 20, 20, 40, RED);
    assert_box(&raster, 20, 0, 40, 40, WHITE);
}

#[test]
fn a_region_draws_that_part_of_the_page() {
    // The right half of the page: fractions 0.5 to 1 across.
    let doc = square_page("1 0 0 rg 10 0 10 20 re f 0 0 1 rg 0 0 10 20 re f", "", &[]);
    let options = RenderOptions {
        region: Some(Rect {
            left: 0.5,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        }),
        ..Default::default()
    };
    let raster = render_with(&doc, &options).unwrap();
    assert_eq!((raster.width, raster.height), (10, 20));
    assert_box(&raster, 0, 0, 10, 20, RED);
}

#[test]
fn a_region_down_the_page_starts_below_its_top() {
    // The bottom quarter in fractions (top 0.75): user y 0 to 5.
    let doc = square_page("1 0 0 rg 0 0 20 5 re f", "", &[]);
    let options = RenderOptions {
        region: Some(Rect {
            left: 0.0,
            top: 0.75,
            right: 1.0,
            bottom: 1.0,
        }),
        ..Default::default()
    };
    let raster = render_with(&doc, &options).unwrap();
    assert_eq!((raster.width, raster.height), (20, 5));
    assert_box(&raster, 0, 0, 20, 5, RED);
}

#[test]
fn a_page_turned_ninety_degrees_is_drawn_turned() {
    // Rotate 90 is clockwise: the lower-left corner of user space goes to
    // the upper left of the picture.
    let doc = document("/MediaBox [0 0 20 20] /Rotate 90", RED_SQUARE, "", &[]);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 10, 10, RED);
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn a_page_turned_a_half_turn_is_drawn_upside_down() {
    let doc = document("/MediaBox [0 0 20 20] /Rotate 180", RED_SQUARE, "", &[]);
    let raster = render(&doc);
    assert_box(&raster, 10, 0, 20, 10, RED);
}

#[test]
fn a_page_turned_270_degrees_puts_the_lower_left_at_the_lower_right() {
    let doc = document("/MediaBox [0 0 20 20] /Rotate 270", RED_SQUARE, "", &[]);
    let raster = render(&doc);
    assert_box(&raster, 10, 10, 20, 20, RED);
}

#[test]
fn a_crop_box_is_the_picture() {
    let doc = document(
        "/MediaBox [0 0 40 40] /CropBox [10 10 30 30]",
        "1 0 0 rg 10 10 10 10 re f",
        "",
        &[],
    );
    let raster = render(&doc);
    assert_eq!((raster.width, raster.height), (20, 20));
    assert_box(&raster, 0, 10, 10, 20, RED);
}

#[test]
fn a_fractional_size_rounds_up_to_whole_pixels() {
    let doc = document("/MediaBox [0 0 10.5 10.5]", "", "", &[]);
    let raster = render(&doc);
    assert_eq!((raster.width, raster.height), (11, 11));
}

#[test]
fn too_many_pixels_are_refused() {
    let doc = square_page("", "", &[]);
    let options = RenderOptions {
        max_pixels: 399,
        ..Default::default()
    };
    assert!(matches!(
        render_with(&doc, &options),
        Err(PdfError::LimitExceeded {
            limit: Limit::RenderPixels { max: 399 },
            ..
        })
    ));
    let at_the_limit = RenderOptions {
        max_pixels: 400,
        ..Default::default()
    };
    assert!(render_with(&doc, &at_the_limit).is_ok());
}

#[test]
fn a_scale_that_is_no_size_is_refused() {
    let doc = square_page("", "", &[]);
    for scale in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let options = RenderOptions {
            scale,
            ..Default::default()
        };
        assert!(
            matches!(render_with(&doc, &options), Err(PdfError::Invalid(_))),
            "{scale}"
        );
    }
}

#[test]
fn an_empty_region_is_refused() {
    let doc = square_page("", "", &[]);
    let options = RenderOptions {
        region: Some(Rect {
            left: 0.5,
            top: 0.0,
            right: 0.5,
            bottom: 1.0,
        }),
        ..Default::default()
    };
    assert!(matches!(
        render_with(&doc, &options),
        Err(PdfError::Invalid(_))
    ));
}

#[test]
fn a_cancelled_render_stops() {
    struct Cancelled;
    impl Progress for Cancelled {
        fn report(&self, _: u64, _: u64, _: &str) {}
        fn is_cancelled(&self) -> bool {
            true
        }
    }
    let doc = square_page(&RED_SQUARE.repeat(2000), "", &[]);
    let result = render_page(&doc, 1, &RenderOptions::default(), &Cancelled);
    assert!(matches!(result, Err(PdfError::Cancelled)));
}

#[test]
fn a_page_past_the_end_is_an_error() {
    let doc = square_page("", "", &[]);
    assert!(render_page(&doc, 2, &RenderOptions::default(), &NoProgress).is_err());
}

#[test]
fn the_same_page_renders_to_the_same_bytes() {
    let doc = square_page("0 0 1 rg 3 3 9.5 9.5 re f 1 w 0 0 m 20 20 l S", "", &[]);
    assert_eq!(render(&doc), render(&doc));
}
