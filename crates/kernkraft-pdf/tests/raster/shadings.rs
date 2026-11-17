// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Shadings: ISO 32000-1:2008, 8.7.4. The page is 20 by 20 points at a
//! pixel to the point, so the pixel in column `x` and row `y` has its centre
//! at user space `(x + 0.5, 20 - y - 0.5)`; every expected colour is the
//! formula of the shading type evaluated there.

use kernkraft_pdf::{DiagnosticKind, PaintFault};

use crate::{
    assert_box, assert_near, diagnostics_of, hidden_group_page, px, render, square_page, stream,
    BLUE, RED, WHITE,
};

/// A red to blue function of one variable.
const RED_TO_BLUE: &str = "<< /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >>";

const MESH: u32 = 20;

fn shading_page(shading: &str, content: &str) -> kernkraft_pdf::doc::Document {
    square_page(content, &format!("/Shading << /S {shading} >>"), &[])
}

/// A pixel the red to blue gradient gives at parameter `s`.
fn mixed(s: f64) -> [u8; 4] {
    [
        ((1.0 - s) * 255.0).round() as u8,
        0,
        (s * 255.0).round() as u8,
        255,
    ]
}

/// The centre of pixel `(x, y)` in user space.
fn centre(x: u32, y: u32) -> (f64, f64) {
    (f64::from(x) + 0.5, 20.0 - f64::from(y) - 0.5)
}

fn axial(coords: &str, extra: &str) -> String {
    format!(
        "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [{coords}] \
         /Function {RED_TO_BLUE} {extra} >>"
    )
}

#[test]
fn an_axial_shading_colours_each_pixel_by_its_place_along_the_axis() {
    let doc = shading_page(&axial("0 0 20 0", ""), "/S sh");
    let raster = render(&doc);
    for x in 0..20 {
        let expected = mixed((f64::from(x) + 0.5) / 20.0);
        for y in [0, 9, 19] {
            assert_near(px(&raster, x, y), expected, 1);
        }
    }
}

#[test]
fn an_axial_shading_that_is_not_extended_paints_nothing_past_its_ends() {
    let doc = shading_page(&axial("5 0 15 0", ""), "/S sh");
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 5, 20, WHITE);
    assert_box(&raster, 15, 0, 20, 20, WHITE);
    assert_near(px(&raster, 10, 10), mixed(0.55), 1);
}

#[test]
fn extend_holds_the_end_colours() {
    let doc = shading_page(&axial("5 0 15 0", "/Extend [true false]"), "/S sh");
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 5, 20, RED);
    assert_box(&raster, 15, 0, 20, 20, WHITE);
    let doc = shading_page(&axial("5 0 15 0", "/Extend [false true]"), "/S sh");
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 5, 20, WHITE);
    assert_box(&raster, 15, 0, 20, 20, BLUE);
}

#[test]
fn the_domain_picks_the_part_of_the_function_used() {
    // Parameters 0.5 to 1 over the axis: the left end is the middle colour.
    let doc = shading_page(&axial("0 0 20 0", "/Domain [0.5 1]"), "/S sh");
    let raster = render(&doc);
    assert_near(px(&raster, 0, 5), mixed(0.5 + 0.5 * 0.025), 1);
    assert_near(px(&raster, 19, 5), mixed(0.5 + 0.5 * 0.975), 1);
}

#[test]
fn a_shading_is_cut_by_the_clip_and_follows_the_ctm() {
    let doc = shading_page(
        &axial("0 0 1 0", "/Extend [true true]"),
        "5 5 10 10 re W n 10 0 0 10 5 0 cm /S sh",
    );
    let raster = render(&doc);
    // Inside the clip, the axis runs from x = 5 to x = 15: parameter
    // (x - 5) / 10 at the pixel's centre.
    assert_near(px(&raster, 5, 10), mixed(0.05), 1);
    assert_near(px(&raster, 14, 10), mixed(0.95), 1);
    assert_box(&raster, 0, 0, 20, 5, WHITE);
    assert_box(&raster, 0, 15, 20, 20, WHITE);
    assert_box(&raster, 0, 5, 5, 15, WHITE);
    assert_box(&raster, 15, 5, 20, 15, WHITE);
}

#[test]
fn a_radial_shading_follows_the_circles() {
    let doc = shading_page(
        &format!(
            "<< /ShadingType 3 /ColorSpace /DeviceRGB /Coords [10 10 0 10 10 10] \
             /Function {RED_TO_BLUE} >>"
        ),
        "/S sh",
    );
    let raster = render(&doc);
    for (x, y) in [(10, 9), (19, 10), (3, 4), (14, 14)] {
        let (ux, uy) = centre(x, y);
        let distance = (ux - 10.0).hypot(uy - 10.0);
        assert_near(px(&raster, x, y), mixed(distance / 10.0), 1);
    }
    // Past the outer circle, which is not extended.
    assert_eq!(px(&raster, 0, 0), WHITE);
    assert_eq!(px(&raster, 19, 19), WHITE);
}

#[test]
fn a_radial_shading_with_a_larger_start_circle_extends_inward() {
    // Circles from radius 2 to 6 about the centre, extended at the start:
    // inside radius 2 the colour is the first one.
    let doc = shading_page(
        &format!(
            "<< /ShadingType 3 /ColorSpace /DeviceRGB /Coords [10 10 2 10 10 6] \
             /Function {RED_TO_BLUE} /Extend [true false] >>"
        ),
        "/S sh",
    );
    let raster = render(&doc);
    assert_eq!(px(&raster, 10, 10), RED);
    let (ux, uy) = centre(15, 10);
    let s = ((ux - 10.0).hypot(uy - 10.0) - 2.0) / 4.0;
    assert_near(px(&raster, 15, 10), mixed(s), 1);
    assert_eq!(px(&raster, 18, 10), WHITE);
}

#[test]
fn a_function_based_shading_calls_its_function_at_each_point() {
    // x y -> x/20 y/20 0, in the domain [0 20] by [0 20].
    let function = stream(
        "/FunctionType 4 /Domain [0 20 0 20] /Range [0 1 0 1 0 1]",
        b"{ exch 20 div exch 20 div 0 }",
    );
    let doc = square_page(
        "/S sh",
        "/Shading << /S << /ShadingType 1 /ColorSpace /DeviceRGB /Domain [0 20 0 20] \
         /Function 20 0 R >> >>",
        &[(20, &function)],
    );
    let raster = render(&doc);
    for (x, y) in [(0, 19), (19, 0), (7, 3), (12, 15)] {
        let (ux, uy) = centre(x, y);
        let expected = [
            (ux / 20.0 * 255.0).round() as u8,
            (uy / 20.0 * 255.0).round() as u8,
            0,
            255,
        ];
        assert_near(px(&raster, x, y), expected, 1);
    }
}

#[test]
fn a_function_based_shading_paints_only_inside_its_domain() {
    // The matrix moves the domain [0 1] by [0 1] to the square 5 to 15.
    let function = stream(
        "/FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]",
        b"{ pop pop 0 1 0 }",
    );
    let doc = square_page(
        "/S sh",
        "/Shading << /S << /ShadingType 1 /ColorSpace /DeviceRGB /Matrix [10 0 0 10 5 5] \
         /Function 20 0 R >> >>",
        &[(20, &function)],
    );
    let raster = render(&doc);
    assert_box(&raster, 5, 5, 15, 15, [0, 255, 0, 255]);
    assert_box(&raster, 0, 0, 20, 5, WHITE);
    assert_box(&raster, 15, 0, 20, 20, WHITE);
}

/// A mesh shading object `type_` of `entries`, its stream `data`.
fn mesh(type_: u32, entries: &str, data: &[u8]) -> Vec<u8> {
    stream(
        &format!(
            " /ShadingType {type_} /ColorSpace /DeviceRGB /BitsPerCoordinate 8 \
             /BitsPerComponent 8 {entries} /Decode [0 20 0 20 0 1 0 1 0 1]"
        ),
        data,
    )
}

fn mesh_page(object: &[u8]) -> kernkraft_pdf::doc::Document {
    square_page("/S sh", "/Shading << /S 20 0 R >>", &[(MESH, object)])
}

#[test]
fn a_free_form_triangle_mesh_interpolates_the_corner_colours() {
    // Corners (0, 0) red, (20, 0) green, (0, 20) blue; each vertex is the
    // flag, x, y and the three colour bytes.
    let data = [
        0, 0, 0, 255, 0, 0, //
        0, 255, 0, 0, 255, 0, //
        0, 0, 255, 0, 0, 255,
    ];
    let raster = render(&mesh_page(&mesh(4, "/BitsPerFlag 8", &data)));
    // Barycentric weights at (x, y) are (1 - x/20 - y/20, x/20, y/20).
    let (ux, uy) = centre(4, 14);
    let (g, b) = (ux / 20.0, uy / 20.0);
    let expected = [
        ((1.0 - g - b) * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        255,
    ];
    assert_near(px(&raster, 4, 14), expected, 2);
    // Past the hypotenuse x + y = 20.
    assert_eq!(px(&raster, 15, 2), WHITE);
}

#[test]
fn an_edge_flag_continues_the_mesh_from_the_last_triangle() {
    // The first triangle is (0,0) (20,0) (0,20); flag 1 adds (20,20) to the
    // last two vertices, making the square, all one gray.
    let gray = [128, 128, 128];
    let vertex = |flag: u8, x: u8, y: u8| [&[flag, x, y][..], &gray].concat();
    let data = [
        vertex(0, 0, 0),
        vertex(0, 255, 0),
        vertex(0, 0, 255),
        vertex(1, 255, 255),
    ]
    .concat();
    let raster = render(&mesh_page(&mesh(4, "/BitsPerFlag 8", &data)));
    assert_box(&raster, 0, 0, 20, 20, [128, 128, 128, 255]);
}

#[test]
fn a_lattice_mesh_joins_each_row_to_the_one_before() {
    // Two rows of two: red along y = 0, blue along y = 20; the colour
    // depends on y only, so it is exact in both triangles.
    let data = [
        0, 0, 255, 0, 0, //
        255, 0, 255, 0, 0, //
        0, 255, 0, 0, 255, //
        255, 255, 0, 0, 255,
    ];
    let raster = render(&mesh_page(&mesh(5, "/VerticesPerRow 2", &data)));
    for (x, y) in [(0, 19), (10, 10), (19, 0), (3, 7)] {
        let (_, uy) = centre(x, y);
        assert_near(px(&raster, x, y), mixed(uy / 20.0), 2);
    }
}

/// The twelve boundary points of the square 0 to 20, thirds along each
/// side, in the order of Table 85 (up the left, along the top, down the
/// right, back along the bottom), as raw bytes of the 0 to 255 range.
const THIRDS: [u8; 4] = [0, 85, 170, 255];
fn boundary() -> Vec<u8> {
    let t = THIRDS;
    [
        (t[0], t[0]),
        (t[0], t[1]),
        (t[0], t[2]),
        (t[0], t[3]),
        (t[1], t[3]),
        (t[2], t[3]),
        (t[3], t[3]),
        (t[3], t[2]),
        (t[3], t[1]),
        (t[3], t[0]),
        (t[2], t[0]),
        (t[1], t[0]),
    ]
    .iter()
    .flat_map(|(x, y)| [*x, *y])
    .collect()
}

/// Corner colours at (0,0) red, (0,20) green, (20,20) blue, (20,0) white.
const CORNERS: [u8; 12] = [255, 0, 0, 0, 255, 0, 0, 0, 255, 255, 255, 255];

fn bilinear_expected(x: u32, y: u32) -> [u8; 4] {
    let (ux, uy) = centre(x, y);
    let (u, v) = (ux / 20.0, uy / 20.0);
    let weights = [(1.0 - u) * (1.0 - v), (1.0 - u) * v, u * v, u * (1.0 - v)];
    let mut out = [0.0; 3];
    for (w, corner) in weights.iter().zip(CORNERS.chunks(3)) {
        for (slot, c) in out.iter_mut().zip(corner) {
            *slot += w * f64::from(*c);
        }
    }
    [
        out[0].round() as u8,
        out[1].round() as u8,
        out[2].round() as u8,
        255,
    ]
}

#[test]
fn a_coons_patch_blends_its_corner_colours_across_the_surface() {
    let data = [&[0u8][..], &boundary(), &CORNERS].concat();
    let raster = render(&mesh_page(&mesh(6, "/BitsPerFlag 8", &data)));
    for (x, y) in [(4, 14), (15, 3), (10, 10), (1, 1)] {
        assert_near(px(&raster, x, y), bilinear_expected(x, y), 3);
    }
}

#[test]
fn a_tensor_patch_with_its_interior_points_is_the_same_square() {
    let t = THIRDS;
    let interior: Vec<u8> = [(t[1], t[1]), (t[1], t[2]), (t[2], t[2]), (t[2], t[1])]
        .iter()
        .flat_map(|(x, y)| [*x, *y])
        .collect();
    let data = [&[0u8][..], &boundary(), &interior, &CORNERS].concat();
    let raster = render(&mesh_page(&mesh(7, "/BitsPerFlag 8", &data)));
    for (x, y) in [(4, 14), (15, 3), (10, 10), (1, 1)] {
        assert_near(px(&raster, x, y), bilinear_expected(x, y), 3);
    }
}

#[test]
fn a_mesh_that_ends_in_a_vertex_paints_what_came_before_and_is_reported() {
    let gray = [128u8, 128, 128];
    let vertex = |x: u8, y: u8| [&[0, x, y][..], &gray].concat();
    let mut data = [vertex(0, 0), vertex(255, 0), vertex(0, 255)].concat();
    data.extend_from_slice(&[0, 255, 255]);
    let doc = mesh_page(&mesh(4, "/BitsPerFlag 8", &data));
    let raster = render(&doc);
    assert_eq!(px(&raster, 2, 17), [128, 128, 128, 255]);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::ShadingDataShort)).len(),
        1
    );
}

#[test]
fn a_mesh_coloured_by_a_function_interpolates_the_parameter() {
    // One parameter per vertex, 0 at x = 0 and 1 at x = 20 (the third
    // vertex is the middle, at 0.5), coloured red to blue.
    let function = RED_TO_BLUE;
    let data = [
        0, 0, 0, 0, //
        0, 255, 0, 255, //
        0, 0, 255, 0,
    ];
    let object = stream(
        " /ShadingType 4 /ColorSpace /DeviceRGB /BitsPerCoordinate 8 /BitsPerComponent 8 \
         /BitsPerFlag 8 /Decode [0 20 0 20 0 1] /Function 21 0 R",
        &data,
    );
    let doc = square_page(
        "/S sh",
        "/Shading << /S 20 0 R >>",
        &[(MESH, &object), (21, function.as_bytes())],
    );
    let raster = render(&doc);
    let (ux, _) = centre(6, 17);
    assert_near(px(&raster, 6, 17), mixed(ux / 20.0), 2);
}

#[test]
fn the_bbox_of_a_shading_clips_it() {
    let doc = shading_page(&axial("0 0 20 0", "/BBox [5 5 15 15]"), "/S sh");
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 5, WHITE);
    assert_box(&raster, 0, 15, 20, 20, WHITE);
    assert_box(&raster, 0, 5, 5, 15, WHITE);
    assert_near(px(&raster, 10, 10), mixed(0.525), 1);
}

#[test]
fn an_unknown_shading_type_paints_nothing_and_is_reported() {
    let doc = shading_page(
        "<< /ShadingType 99 /ColorSpace /DeviceRGB >>",
        "/S sh 1 0 0 rg 0 0 5 5 re f",
    );
    let raster = render(&doc);
    assert_eq!(px(&raster, 10, 10), WHITE);
    assert_eq!(px(&raster, 2, 17), RED);
    let found = diagnostics_of(&doc, |k| {
        matches!(
            k,
            DiagnosticKind::ShadingUnusable {
                fault: PaintFault::Type
            }
        )
    });
    assert_eq!(found.len(), 1);
}

#[test]
fn a_function_that_does_not_fit_the_colour_space_is_reported() {
    // A function with one output for a three component space.
    let doc = shading_page(
        "<< /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 20 0] \
         /Function << /FunctionType 2 /Domain [0 1] /C0 [0] /C1 [1] /N 1 >> >>",
        "/S sh",
    );
    let raster = render(&doc);
    assert_eq!(px(&raster, 10, 10), WHITE);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(
            k,
            DiagnosticKind::ShadingUnusable {
                fault: PaintFault::Function
            }
        ))
        .len(),
        1
    );
}

#[test]
fn a_shading_in_hidden_optional_content_is_not_painted() {
    let hidden = hidden_group_page(
        "",
        "/OC /L BDC /S sh EMC 1 0 0 rg 0 0 5 5 re f",
        "/Shading << /S << /ShadingType 2 /ColorSpace /DeviceRGB /Coords [0 0 20 0] \
         /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [0 0 1] /N 1 >> \
         /Extend [true true] >> >>",
        &[],
    );
    let raster = render(&hidden);
    assert_eq!(px(&raster, 10, 10), WHITE);
    assert_eq!(px(&raster, 2, 17), RED);
}

fn pattern_page(shading: &str, content: &str) -> kernkraft_pdf::doc::Document {
    let pattern = format!("<< /PatternType 2 /Shading {shading} >>");
    square_page(
        content,
        "/Pattern << /P 20 0 R >>",
        &[(20, pattern.as_bytes())],
    )
}

#[test]
fn a_shading_pattern_fills_a_path_with_the_shading() {
    let doc = pattern_page(&axial("0 0 20 0", ""), "/Pattern cs /P scn 0 0 10 20 re f");
    let raster = render(&doc);
    for x in [0, 4, 9] {
        assert_near(px(&raster, x, 8), mixed((f64::from(x) + 0.5) / 20.0), 1);
    }
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn a_shading_pattern_strokes_too() {
    let doc = pattern_page(
        &axial("0 0 20 0", ""),
        "/Pattern CS /P SCN 4 w 0 10 m 20 10 l S",
    );
    let raster = render(&doc);
    assert_near(px(&raster, 3, 9), mixed(3.5 / 20.0), 1);
    assert_eq!(px(&raster, 3, 5), WHITE);
}

#[test]
fn the_pattern_matrix_is_relative_to_the_page_not_to_the_ctm() {
    // The CTM doubles user space, but the pattern is laid on the page's
    // default space: the gradient still runs 0 to 20 across the page.
    let doc = pattern_page(
        &axial("0 0 20 0", ""),
        "q 2 0 0 2 0 0 cm /Pattern cs /P scn 0 0 5 10 re f Q",
    );
    let raster = render(&doc);
    assert_near(px(&raster, 0, 15), mixed(0.5 / 20.0), 1);
    assert_near(px(&raster, 9, 15), mixed(9.5 / 20.0), 1);
}

#[test]
fn the_pattern_dictionarys_matrix_moves_the_shading() {
    let pattern = format!(
        "<< /PatternType 2 /Matrix [1 0 0 1 10 0] /Shading {} >>",
        axial("0 0 10 0", "/Extend [true true]")
    );
    let doc = square_page(
        "/Pattern cs /P scn 0 0 20 20 re f",
        "/Pattern << /P 20 0 R >>",
        &[(20, pattern.as_bytes())],
    );
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_near(px(&raster, 15, 5), mixed(0.55), 1);
    assert_box(
        &raster,
        19,
        0,
        20,
        20,
        [mixed(0.95)[0], 0, mixed(0.95)[2], 255],
    );
}

#[test]
fn a_shading_pattern_fills_what_the_shading_does_not_reach_with_its_background() {
    let doc = pattern_page(
        &axial("5 0 15 0", "/Background [0 1 0]"),
        "/Pattern cs /P scn 0 0 20 20 re f",
    );
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 5, 20, [0, 255, 0, 255]);
    assert_box(&raster, 15, 0, 20, 20, [0, 255, 0, 255]);
    assert_near(px(&raster, 10, 10), mixed(0.55), 1);
}

#[test]
fn the_sh_operator_does_not_use_the_background() {
    let doc = shading_page(&axial("5 0 15 0", "/Background [0 1 0]"), "/S sh");
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 5, 20, WHITE);
}

#[test]
fn a_shading_takes_the_constant_alpha_of_the_state() {
    let doc = square_page(
        "/G gs /S sh",
        "/ExtGState << /G << /ca 0.5 >> >> /Shading << /S << /ShadingType 2 \
         /ColorSpace /DeviceRGB /Coords [0 0 20 0] /Extend [true true] \
         /Function << /FunctionType 2 /Domain [0 1] /C0 [1 0 0] /C1 [1 0 0] /N 1 >> >> >>",
        &[],
    );
    let raster = render(&doc);
    assert_near(px(&raster, 10, 10), [255, 128, 128, 255], 1);
}
