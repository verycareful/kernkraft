// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tiling patterns: ISO 32000-1:2008, 8.7.3. The page is 20 by 20 points at
//! a pixel to the point. A cell of 5 points whose content is a red band of
//! the bottom 2 points puts red on the user space rows 0 to 2, 5 to 7 and so
//! on; user space row `y` (counted from the bottom) is pixel row `19 - y`.

use kernkraft_pdf::{DiagnosticKind, PaintFault};

use crate::{
    assert_box, assert_near, diagnostics_of, px, render, square_page, stream, BLUE, RED, WHITE,
};

const PATTERN: u32 = 20;
const FORM: u32 = 21;
const IMAGE: u32 = 22;

/// A tiling pattern object with `entries` and cell content `data`.
fn tiling(entries: &str, data: &str) -> Vec<u8> {
    stream(
        &format!(" /Type /Pattern /PatternType 1 /TilingType 1 /Resources << >> {entries}"),
        data.as_bytes(),
    )
}

/// A cell of `step` points whose bottom `band` points are red.
fn red_band(step: u32, band: u32) -> Vec<u8> {
    tiling(
        &format!("/PaintType 1 /BBox [0 0 {step} {step}] /XStep {step} /YStep {step}"),
        &format!("1 0 0 rg 0 0 {step} {band} re f"),
    )
}

fn page(content: &str, pattern: &[u8]) -> kernkraft_pdf::doc::Document {
    square_page(content, "/Pattern << /P 20 0 R >>", &[(PATTERN, pattern)])
}

const FILL: &str = "/Pattern cs /P scn 0 0 20 20 re f";

/// Whether pixel row `row` is in the red band of a cell of `step` points
/// with a band of `band`.
fn in_band(row: u32, step: u32, band: u32) -> bool {
    let y = 20.0 - f64::from(row) - 0.5;
    y.rem_euclid(f64::from(step)) < f64::from(band)
}

#[test]
fn a_coloured_pattern_repeats_its_cell_over_the_path() {
    let raster = render(&page(FILL, &red_band(5, 2)));
    for row in 0..20 {
        let expected = if in_band(row, 5, 2) { RED } else { WHITE };
        assert_box(&raster, 0, row, 20, row + 1, expected);
    }
}

#[test]
fn a_pattern_only_paints_inside_the_path() {
    let raster = render(&page("/Pattern cs /P scn 0 0 10 20 re f", &red_band(5, 2)));
    assert_box(&raster, 0, 18, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn the_pattern_matrix_scales_the_cell() {
    let pattern = tiling(
        "/PaintType 1 /BBox [0 0 5 5] /XStep 5 /YStep 5 /Matrix [2 0 0 2 0 0]",
        "1 0 0 rg 0 0 5 2 re f",
    );
    let raster = render(&page(FILL, &pattern));
    // The cell is 10 points, its band 4.
    for row in 0..20 {
        let expected = if in_band(row, 10, 4) { RED } else { WHITE };
        assert_box(&raster, 0, row, 20, row + 1, expected);
    }
}

#[test]
fn the_pattern_matrix_is_relative_to_the_page_and_not_to_the_ctm() {
    let raster = render(&page(
        "q 2 0 0 2 0 0 cm /Pattern cs /P scn 0 0 10 10 re f Q",
        &red_band(5, 2),
    ));
    for row in 0..20 {
        let expected = if in_band(row, 5, 2) { RED } else { WHITE };
        assert_box(&raster, 0, row, 20, row + 1, expected);
    }
}

#[test]
fn an_uncoloured_pattern_takes_the_colour_it_is_painted_in() {
    // The cell sets red; for an uncoloured pattern that is ignored.
    let pattern = tiling(
        "/PaintType 2 /BBox [0 0 5 5] /XStep 5 /YStep 5",
        "1 0 0 rg 0 0 5 2 re f",
    );
    let doc = square_page(
        "/Cs1 cs 0 0 1 /P scn 0 0 20 20 re f",
        "/Pattern << /P 20 0 R >> /ColorSpace << /Cs1 [/Pattern /DeviceRGB] >>",
        &[(PATTERN, &pattern)],
    );
    let raster = render(&doc);
    for row in 0..20 {
        let expected = if in_band(row, 5, 2) { BLUE } else { WHITE };
        assert_box(&raster, 0, row, 20, row + 1, expected);
    }
}

#[test]
fn an_uncoloured_pattern_with_no_colour_is_reported() {
    let pattern = tiling(
        "/PaintType 2 /BBox [0 0 5 5] /XStep 5 /YStep 5",
        "0 0 5 2 re f",
    );
    let raster = render(&page(FILL, &pattern));
    assert_box(&raster, 0, 0, 20, 20, WHITE);
}

#[test]
fn a_cell_wider_than_its_step_overlaps_the_next_cells() {
    // The red band spans 10 points over a step of 5, and a blue bar sits at
    // 8 to 10: it reaches into the next cell, so it is at x = 3 to 5 of each
    // tile as well. The cell to the right is painted after the one to the
    // left, so its band is over the bar in the band's row.
    let pattern = tiling(
        "/PaintType 1 /BBox [0 0 10 5] /XStep 5 /YStep 5",
        "1 0 0 rg 0 0 10 1 re f 0 0 1 rg 8 0 2 5 re f",
    );
    let raster = render(&page(FILL, &pattern));
    for x in 0..20 {
        for row in 0..20 {
            let y = 20.0 - f64::from(row) - 0.5;
            let (cx, cy) = (x % 5, y.rem_euclid(5.0));
            let expected = if cy < 1.0 {
                RED
            } else if cx >= 3 {
                BLUE
            } else {
                WHITE
            };
            assert_eq!(px(&raster, x, row), expected, "pixel ({x}, {row})");
        }
    }
}

#[test]
fn a_pattern_strokes_a_line() {
    let raster = render(&page(
        "/Pattern CS /P SCN 4 w 0 10 m 20 10 l S",
        &red_band(5, 2),
    ));
    // The line covers user rows 8 to 12: row 8 and 9 (cell rows 3, 4) are
    // blank, 10 and 11 (cell rows 0, 1) are the band.
    assert_box(&raster, 0, 8, 20, 10, RED);
    assert_box(&raster, 0, 10, 20, 12, WHITE);
    assert_box(
        &raster,
        0,
        0,
        20,
        8,
        if in_band(0, 5, 2) { RED } else { WHITE },
    );
}

#[test]
fn a_pattern_in_a_form_is_relative_to_the_form_space() {
    // Vertical bars at x = 0 to 2 of each 5; the form is moved 1 to the
    // right, and the pattern with it.
    let pattern = tiling(
        "/PaintType 1 /BBox [0 0 5 5] /XStep 5 /YStep 5",
        "1 0 0 rg 0 0 2 5 re f",
    );
    let form = stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 20 20] /Matrix [1 0 0 1 1 0] \
         /Resources << /Pattern << /P 20 0 R >> >>",
        FILL.as_bytes(),
    );
    let doc = square_page(
        "/F Do",
        "/XObject << /F 21 0 R >>",
        &[(PATTERN, &pattern), (FORM, &form)],
    );
    let raster = render(&doc);
    for x in 1..20 {
        let expected = if (x - 1) % 5 < 2 { RED } else { WHITE };
        assert_eq!(px(&raster, x, 10), expected, "column {x}");
    }
    assert_eq!(px(&raster, 0, 10), WHITE);
}

#[test]
fn a_pattern_takes_the_constant_alpha() {
    let doc = square_page(
        "/G gs /Pattern cs /P scn 0 0 20 20 re f",
        "/Pattern << /P 20 0 R >> /ExtGState << /G << /ca 0.5 >> >>",
        &[(PATTERN, &red_band(5, 2))],
    );
    let raster = render(&doc);
    assert_near(px(&raster, 3, 19), [255, 128, 128, 255], 1);
    assert_eq!(px(&raster, 3, 15), WHITE);
}

#[test]
fn a_stencil_mask_painted_in_a_pattern_keeps_the_pattern_where_it_paints() {
    // A 2 by 1 stencil: its first sample (0) paints, its second does not.
    let stencil = stream(
        " /Type /XObject /Subtype /Image /ImageMask true /Width 2 /Height 1 \
         /BitsPerComponent 1",
        &[0b0100_0000],
    );
    let doc = square_page(
        "q 20 0 0 20 0 0 cm /Pattern cs /P scn /Im Do Q",
        "/Pattern << /P 20 0 R >> /XObject << /Im 22 0 R >>",
        &[(PATTERN, &red_band(5, 2)), (IMAGE, &stencil)],
    );
    let raster = render(&doc);
    for row in 0..20 {
        let expected = if in_band(row, 5, 2) { RED } else { WHITE };
        assert_box(&raster, 0, row, 10, row + 1, expected);
    }
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn a_pattern_with_no_step_paints_nothing_and_is_reported() {
    let pattern = tiling(
        "/PaintType 1 /BBox [0 0 5 5] /XStep 0 /YStep 5",
        "1 0 0 rg 0 0 5 2 re f",
    );
    let doc = page(FILL, &pattern);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 20, WHITE);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(
            k,
            DiagnosticKind::PatternUnusable {
                fault: PaintFault::Size
            }
        ))
        .len(),
        1
    );
}

#[test]
fn a_pattern_of_an_unknown_type_is_reported() {
    let doc = square_page(
        FILL,
        "/Pattern << /P 20 0 R >>",
        &[(PATTERN, b"<< /PatternType 7 >>")],
    );
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 20, WHITE);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(
            k,
            DiagnosticKind::PatternUnusable {
                fault: PaintFault::Type
            }
        ))
        .len(),
        1
    );
}

#[test]
fn a_pattern_inside_a_pattern_cell_is_laid_on_the_cell() {
    // The outer cell is 10 points and holds a quarter-size inner pattern
    // fill: the bottom left 5 by 5, filled with the red band of an inner
    // 5 point cell.
    let inner = red_band(5, 2);
    let outer = stream(
        " /Type /Pattern /PatternType 1 /PaintType 1 /TilingType 1 /BBox [0 0 10 10] \
         /XStep 10 /YStep 10 /Resources << /Pattern << /Q 23 0 R >> >>",
        b"/Pattern cs /Q scn 0 0 5 10 re f",
    );
    let doc = square_page(
        FILL,
        "/Pattern << /P 20 0 R >>",
        &[(PATTERN, &outer), (23, &inner)],
    );
    let raster = render(&doc);
    // The left half of each 10 point cell has the inner pattern, whose
    // bands are at the page's rows, the right half is empty.
    for row in 0..20 {
        let expected = if in_band(row, 5, 2) { RED } else { WHITE };
        assert_box(&raster, 0, row, 5, row + 1, expected);
        assert_box(&raster, 5, row, 10, row + 1, WHITE);
    }
}
