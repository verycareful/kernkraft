// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Fills, strokes and clips: ISO 32000-1, 8.5.3 and 8.5.4.

use super::*;

#[test]
fn an_empty_page_is_white() {
    let raster = draw("");
    assert_eq!((raster.width, raster.height), (SIDE, SIDE));
    assert_box(&raster, 0, 0, SIDE, SIDE, WHITE);
}

#[test]
fn a_filled_rectangle_covers_its_pixels_and_no_others() {
    // User y 4 to 14 is rows 6 to 16 of the picture (y runs down).
    let raster = draw("1 0 0 rg 4 4 10 10 re f");
    assert_box(&raster, 4, 6, 14, 16, RED);
    assert_box(&raster, 0, 0, SIDE, 6, WHITE);
    assert_box(&raster, 0, 16, SIDE, SIDE, WHITE);
    assert_box(&raster, 0, 6, 4, 16, WHITE);
    assert_box(&raster, 14, 6, SIDE, 16, WHITE);
}

#[test]
fn the_nonzero_rule_fills_a_nested_rectangle_drawn_the_same_way() {
    // Two rectangles wound the same way: the inner one has winding 2.
    let raster = draw("0 0 1 rg 2 2 16 16 re 6 6 8 8 re f");
    assert_box(&raster, 6, 6, 14, 14, BLUE);
}

#[test]
fn the_even_odd_rule_leaves_a_hole() {
    let raster = draw("0 0 1 rg 2 2 16 16 re 6 6 8 8 re f*");
    assert_box(&raster, 6, 6, 14, 14, WHITE);
    assert_box(&raster, 2, 2, 18, 6, BLUE);
}

#[test]
fn a_stroke_is_centred_on_the_path() {
    // Width 4 about y = 10: user y 8 to 12, rows 8 to 12.
    let raster = draw("4 w 0 10 m 20 10 l S");
    assert_box(&raster, 0, 8, SIDE, 12, BLACK);
    assert_box(&raster, 0, 0, SIDE, 8, WHITE);
    assert_box(&raster, 0, 12, SIDE, SIDE, WHITE);
}

#[test]
fn the_stroke_colour_is_not_the_fill_colour() {
    let raster = draw("1 0 0 RG 0 0 1 rg 4 w 0 10 m 20 10 l S");
    assert_box(&raster, 0, 8, SIDE, 12, RED);
}

#[test]
fn a_dash_pattern_alternates_marks_and_gaps() {
    let raster = draw("4 w [4 4] 0 d 0 10 m 20 10 l S");
    assert_box(&raster, 0, 8, 4, 12, BLACK);
    assert_box(&raster, 4, 8, 8, 12, WHITE);
    assert_box(&raster, 8, 8, 12, 12, BLACK);
    assert_box(&raster, 12, 8, 16, 12, WHITE);
}

#[test]
fn a_dash_phase_starts_part_way_into_the_pattern() {
    // Phase 4 starts in the gap: marks at 4 to 8 and 12 to 16.
    let raster = draw("4 w [4 4] 4 d 0 10 m 20 10 l S");
    assert_box(&raster, 0, 8, 4, 12, WHITE);
    assert_box(&raster, 4, 8, 8, 12, BLACK);
}

#[test]
fn an_odd_dash_array_is_repeated() {
    // [4] is 4 on, 4 off (8.4.3.6).
    let raster = draw("4 w [4] 0 d 0 10 m 20 10 l S");
    assert_box(&raster, 0, 8, 4, 12, BLACK);
    assert_box(&raster, 4, 8, 8, 12, WHITE);
}

#[test]
fn a_butt_cap_stops_at_the_end_and_a_square_cap_goes_past_it() {
    let butt = draw("6 w 0 J 8 10 m 12 10 l S");
    assert_eq!(px(&butt, 6, 10), WHITE);
    assert_eq!(px(&butt, 10, 10), BLACK);
    let square = draw("6 w 2 J 8 10 m 12 10 l S");
    assert_eq!(px(&square, 6, 10), BLACK);
    assert_eq!(px(&square, 14, 10), BLACK);
    assert_eq!(px(&square, 4, 10), WHITE);
}

#[test]
fn a_miter_join_reaches_the_corner_and_a_bevel_does_not() {
    // A right angle at (14, 14), pen width 4: the miter fills the outer
    // corner out to (16, 16), the bevel cuts it along x + y = 30.
    let content = |join: u8| format!("4 w {join} j 4 14 m 14 14 l 14 4 l S");
    let miter = draw(&content(0));
    // The pixel at user x 15 to 16, y 15 to 16 is column 15, row 4.
    assert_eq!(px(&miter, 15, 4), BLACK);
    let bevel = draw(&content(2));
    assert_eq!(px(&bevel, 15, 4), WHITE);
}

#[test]
fn a_clip_cuts_later_painting() {
    let raster = draw("0 0 10 20 re W n 0 0 1 rg 0 0 20 20 re f");
    assert_box(&raster, 0, 0, 10, SIDE, BLUE);
    assert_box(&raster, 10, 0, SIDE, SIDE, WHITE);
}

#[test]
fn clips_intersect() {
    let raster = draw("0 0 10 20 re W n 0 10 20 10 re W n 0 0 1 rg 0 0 20 20 re f");
    assert_box(&raster, 0, 0, 10, 10, BLUE);
    assert_box(&raster, 0, 10, SIDE, SIDE, WHITE);
    assert_box(&raster, 10, 0, SIDE, 10, WHITE);
}

#[test]
fn q_and_big_q_keep_and_restore_the_clip() {
    let raster = draw("q 0 0 10 20 re W n 1 0 0 rg 0 0 20 10 re f Q 0 0 1 rg 0 10 20 10 re f");
    // Red only in the clipped lower-left quarter; blue across the top
    // half, which the restored clip no longer limits.
    assert_box(&raster, 0, 10, 10, SIDE, RED);
    assert_box(&raster, 10, 10, SIDE, SIDE, WHITE);
    assert_box(&raster, 0, 0, SIDE, 10, BLUE);
}

#[test]
fn an_empty_clip_hides_everything_after_it() {
    let raster = draw("0 0 m W n 0 0 1 rg 0 0 20 20 re f");
    assert_box(&raster, 0, 0, SIDE, SIDE, WHITE);
}

#[test]
fn the_ctm_places_a_path() {
    let raster = draw("1 0 0 rg 1 0 0 1 10 10 cm 0 0 5 5 re f");
    assert_box(&raster, 10, 5, 15, 10, RED);
}

#[test]
fn a_non_uniform_scale_squeezes_the_pen_as_well() {
    // Width 2 under x scaled by 3 and y by 1: 6 wide for a vertical line.
    let raster = draw("3 0 0 1 0 0 cm 2 w 3 0 m 3 20 l S");
    assert_box(&raster, 6, 0, 12, SIDE, BLACK);
    assert_box(&raster, 0, 0, 6, SIDE, WHITE);
}

#[test]
fn a_zero_width_line_is_one_pixel_wide() {
    let raster = draw("0 w 0 10 m 20 10 l S");
    let dark: Vec<u32> = (0..SIDE).filter(|y| px(&raster, 5, *y) != WHITE).collect();
    assert!(!dark.is_empty() && dark.len() <= 2, "{dark:?}");
}

#[test]
fn constant_alpha_blends_with_the_page() {
    let doc = square_page(
        "/G gs 1 0 0 rg 0 0 20 20 re f",
        "/ExtGState << /G << /ca 0.5 >> >>",
        &[],
    );
    assert_near(px(&render(&doc), 10, 10), [255, 128, 128, 255], 1);
}

#[test]
fn stroking_alpha_is_the_other_constant() {
    let doc = square_page(
        "/G gs 4 w 0 10 m 20 10 l S",
        "/ExtGState << /G << /CA 0.5 /ca 1 >> >>",
        &[],
    );
    assert_near(px(&render(&doc), 10, 10), [128, 128, 128, 255], 1);
}

#[test]
fn colours_of_every_device_space_are_converted() {
    // DeviceCMYK black is 0 0 0 1, a dark colour through the CMYK
    // profile; gray 0.5 is 128.
    let raster = draw("0 0 0 1 k 0 0 10 20 re f 0.5 g 10 0 10 20 re f");
    let black = px(&raster, 5, 10);
    assert!(black[..3].iter().all(|level| *level <= 64), "{black:?}");
    assert_eq!(black[3], 255);
    assert_near(px(&raster, 15, 10), [128, 128, 128, 255], 1);
}

#[test]
fn a_hidden_optional_content_group_paints_nothing() {
    let doc = {
        let catalog = b"<< /Type /Catalog /Pages 2 0 R /OCProperties << /OCGs [20 0 R] /D << /OFF [20 0 R] >> >> >>";
        let mut objects: Vec<(u32, Vec<u8>)> = vec![
            (1, catalog.to_vec()),
            (2, b"<< /Type /Pages /Kids [10 0 R] /Count 1 >>".to_vec()),
            (
                10,
                b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R /MediaBox [0 0 20 20] \
                  /Resources << /Properties << /L 20 0 R >> >> >>"
                    .to_vec(),
            ),
            (
                11,
                stream(
                    "",
                    b"/OC /L BDC 1 0 0 rg 0 0 20 20 re f EMC 0 0 1 rg 0 0 5 5 re f",
                ),
            ),
            (20, b"<< /Type /OCG /Name (Layer) >>".to_vec()),
        ];
        objects.sort_by_key(|(n, _)| *n);
        table_file(&borrowed(&objects), "/Root 1 0 R").open()
    };
    let raster = render(&doc);
    assert_eq!(px(&raster, 15, 5), WHITE);
    assert_eq!(px(&raster, 2, 17), BLUE);
}

#[test]
fn transfer_and_colour_removal_functions_are_not_read() {
    // These entries are functions, which are streams; a page that sets such
    // a state still draws (8.4.5 lets a renderer ignore them).
    let function = stream(
        "/FunctionType 0 /Domain [0 1] /Range [0 1] /Size [2] /BitsPerSample 8",
        &[0, 255],
    );
    let doc = square_page(
        "/G gs 1 0 0 rg 0 0 20 20 re f",
        "/ExtGState << /G << /BG 30 0 R /UCR 30 0 R /TR 30 0 R /TR2 30 0 R >> >>",
        &[(30, &function)],
    );
    assert_box(&render(&doc), 0, 0, SIDE, SIDE, RED);
}

#[test]
fn a_line_thinner_than_a_pixel_is_drawn_one_pixel_wide() {
    // User y 10 to 11 is row 9; a line of 0.1 about y = 10.5 fills it.
    let raster = draw("0.1 w 0 10.5 m 20 10.5 l S");
    assert_box(&raster, 0, 9, SIDE, 10, BLACK);
    assert_box(&raster, 0, 8, SIDE, 9, WHITE);
    assert_box(&raster, 0, 10, SIDE, 11, WHITE);
}

#[test]
fn the_thinnest_line_follows_the_scale_of_the_page() {
    // At 2 pixels to the point a line of 0.1 is 0.2 pixel: drawn as 1.
    let doc = square_page("0.1 w 0 10.25 m 20 10.25 l S", "", &[]);
    let options = RenderOptions {
        scale: 2.0,
        ..Default::default()
    };
    let raster = render_with(&doc, &options).unwrap();
    // User y 10 to 10.5 is row 19 of 40.
    assert_box(&raster, 0, 19, 40, 20, BLACK);
}
