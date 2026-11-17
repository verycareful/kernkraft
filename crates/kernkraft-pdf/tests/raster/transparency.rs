// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Transparency: ISO 32000-1:2008, 11. Blend modes (11.3.5), constant alpha
//! (11.6.4.4), transparency groups (11.4, 11.6.6) and soft masks (11.6.5).
//! The page is 20 by 20 points at a pixel to the point. The backdrop of the
//! blend tests is `(0.8, 0.4, 0.2)` and the source `(0.6, 0.2, 0.4)`, whose
//! bytes (204, 102, 51 and 153, 51, 102) are exact; each expectation is the
//! blend function of Table 136 worked out on those numbers.

use kernkraft_pdf::{DiagnosticKind, PaintFault};

use crate::{
    assert_box, assert_near, diagnostics_of, px, render, square_page, stream, BLUE, RED, WHITE,
};

const BACKDROP: [u8; 4] = [204, 102, 51, 255];
const GROUP: u32 = 20;
const MASK: u32 = 21;

/// The page: the backdrop everywhere, then the source in the box 5 to 15
/// with the graphics state `G` of `entries` in force.
fn blend_page(entries: &str) -> kernkraft_pdf::doc::Document {
    square_page(
        "0.8 0.4 0.2 rg 0 0 20 20 re f /G gs 0.6 0.2 0.4 rg 5 5 10 10 re f",
        &format!("/ExtGState << /G << {entries} >> >>"),
        &[],
    )
}

fn blended(mode: &str) -> [u8; 4] {
    let raster = render(&blend_page(&format!("/BM /{mode}")));
    assert_eq!(px(&raster, 2, 2), BACKDROP, "outside the box is untouched");
    px(&raster, 10, 10)
}

fn bytes(r: f64, g: f64, b: f64) -> [u8; 4] {
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        255,
    ]
}

#[test]
fn normal_and_compatible_paint_the_source() {
    for mode in ["Normal", "Compatible"] {
        assert_eq!(blended(mode), [153, 51, 102, 255], "{mode}");
    }
}

#[test]
fn multiply_is_the_product_of_the_two() {
    assert_near(blended("Multiply"), bytes(0.48, 0.08, 0.08), 1);
}

#[test]
fn screen_is_the_complement_of_the_product_of_complements() {
    assert_near(blended("Screen"), bytes(0.92, 0.52, 0.52), 1);
}

#[test]
fn darken_and_lighten_take_the_smaller_and_the_larger() {
    assert_near(blended("Darken"), bytes(0.6, 0.2, 0.2), 1);
    assert_near(blended("Lighten"), bytes(0.8, 0.4, 0.4), 1);
}

#[test]
fn difference_and_exclusion() {
    assert_near(blended("Difference"), bytes(0.2, 0.2, 0.2), 1);
    assert_near(blended("Exclusion"), bytes(0.44, 0.44, 0.44), 1);
}

#[test]
fn overlay_and_hard_light_multiply_or_screen_by_one_of_the_two() {
    // Overlay multiplies where the backdrop is dark (below 0.5) and
    // screens where it is light; HardLight does so by the source. Here the
    // two agree: backdrop 0.8 and source 0.6, 0.4 and 0.2 and 0.4.
    let expected = bytes(0.84, 0.16, 0.16);
    assert_near(blended("Overlay"), expected, 1);
    assert_near(blended("HardLight"), expected, 1);
}

#[test]
fn colour_dodge_and_burn_divide() {
    // Dodge: backdrop / (1 - source), held to 1: 2, 0.5, 1/3.
    assert_near(blended("ColorDodge"), bytes(1.0, 0.5, 1.0 / 3.0), 1);
    // Burn: 1 - (1 - backdrop) / source, held to 0: 2/3, 0, 0.
    assert_near(blended("ColorBurn"), bytes(2.0 / 3.0, 0.0, 0.0), 1);
}

#[test]
fn soft_light_follows_its_two_branches() {
    // Source 0.6 is above one half: B = cb + (2cs - 1)(sqrt(cb) - cb).
    let first = 0.8 + 0.2 * (0.8f64.sqrt() - 0.8);
    // Sources 0.2 and 0.4 are below: B = cb - (1 - 2cs) cb (1 - cb).
    let second = 0.4 - 0.6 * 0.4 * 0.6;
    let third = 0.2 - 0.2 * 0.2 * 0.8;
    assert_near(blended("SoftLight"), bytes(first, second, third), 1);
}

#[test]
fn the_non_separable_modes_mix_hue_saturation_and_luminosity() {
    // Lum = 0.30 r + 0.59 g + 0.11 b: backdrop 0.498, source 0.342.
    // Luminosity: the backdrop's colour with the source's luminosity.
    assert_near(blended("Luminosity"), bytes(0.644, 0.244, 0.044), 2);
    // Color: the source's hue and saturation with the backdrop's luminosity.
    assert_near(blended("Color"), bytes(0.756, 0.356, 0.556), 2);
    // Hue: the source's hue with the backdrop's saturation (0.6) and
    // luminosity. The source's red is its largest, green its smallest and
    // blue between, so the largest becomes 0.6, the smallest 0 and blue
    // (0.4 - 0.2) * 0.6 / (0.6 - 0.2) = 0.3: (0.6, 0, 0.3), of luminosity
    // 0.213, moved up by 0.498 - 0.213.
    assert_near(blended("Hue"), bytes(0.885, 0.285, 0.585), 2);
    // Saturation: the backdrop with the source's saturation (0.4): (0.4,
    // 2/15, 0) moved up by 0.498 - 0.19867.
    assert_near(
        blended("Saturation"),
        bytes(0.4 + 0.29933, 2.0 / 15.0 + 0.29933, 0.29933),
        2,
    );
}

#[test]
fn an_array_of_modes_takes_the_first_that_is_known() {
    let raster = render(&blend_page("/BM [/NoSuchMode /Multiply]"));
    assert_near(px(&raster, 10, 10), bytes(0.48, 0.08, 0.08), 1);
}

#[test]
fn an_unknown_mode_paints_normally() {
    assert_eq!(blended("NoSuchMode"), [153, 51, 102, 255]);
}

#[test]
fn constant_alpha_mixes_the_blended_colour_with_the_backdrop() {
    // (1 - 0.5) cb + 0.5 * cb * cs.
    let raster = render(&blend_page("/BM /Multiply /ca 0.5"));
    assert_near(
        px(&raster, 10, 10),
        bytes(
            0.5 * 0.8 + 0.5 * 0.48,
            0.5 * 0.4 + 0.5 * 0.08,
            0.5 * 0.2 + 0.5 * 0.08,
        ),
        1,
    );
}

#[test]
fn a_blend_mode_applies_to_text_images_and_strokes_alike() {
    let raster = render(&square_page(
        "0.8 0.4 0.2 rg 0 0 20 20 re f /G gs 0.6 0.2 0.4 RG 6 w 0 10 m 20 10 l S",
        "/ExtGState << /G << /BM /Multiply >> >>",
        &[],
    ));
    assert_near(px(&raster, 10, 10), bytes(0.48, 0.08, 0.08), 1);
}

/// A form XObject, with `entries` and `content`.
fn form(entries: &str, content: &str) -> Vec<u8> {
    stream(
        &format!(" /Type /XObject /Subtype /Form /BBox [0 0 20 20] {entries}"),
        content.as_bytes(),
    )
}

const OVERLAP: &str = "1 0 0 rg 0 0 12 20 re f 0 0 1 rg 8 0 12 20 re f";

fn alpha_page(group: bool) -> kernkraft_pdf::doc::Document {
    let entries = if group {
        "/Group << /S /Transparency >>"
    } else {
        ""
    };
    square_page(
        "/G gs /F Do",
        "/ExtGState << /G << /ca 0.5 >> >> /XObject << /F 20 0 R >>",
        &[(GROUP, &form(entries, OVERLAP))],
    )
}

#[test]
fn a_form_without_a_group_composes_each_mark_on_its_own() {
    let raster = render(&alpha_page(false));
    assert_near(px(&raster, 2, 10), [255, 128, 128, 255], 1);
    // The blue half over the red half, each at one half.
    assert_near(px(&raster, 10, 10), [128, 64, 191, 255], 1);
    assert_near(px(&raster, 16, 10), [128, 128, 255, 255], 1);
}

#[test]
fn a_transparency_group_composes_as_one_object() {
    let raster = render(&alpha_page(true));
    assert_near(px(&raster, 2, 10), [255, 128, 128, 255], 1);
    // Inside the group blue covers red; the group is then at one half.
    assert_near(px(&raster, 10, 10), [128, 128, 255, 255], 1);
    assert_near(px(&raster, 16, 10), [128, 128, 255, 255], 1);
}

fn blend_group_page(group: &str, state: &str) -> kernkraft_pdf::doc::Document {
    square_page(
        "0.8 0.4 0.2 rg 0 0 20 20 re f /G gs /F Do",
        &format!("/ExtGState << /G << {state} >> >> /XObject << /F 20 0 R >>"),
        &[(
            GROUP,
            &form(
                &format!("/Group << /S /Transparency {group} >>"),
                "1 0 0 rg 5 5 10 10 re f",
            ),
        )],
    )
}

#[test]
fn a_group_is_composed_with_the_blend_mode_it_was_invoked_with() {
    let raster = render(&blend_group_page("", "/BM /Multiply"));
    assert_near(px(&raster, 10, 10), bytes(0.8, 0.0, 0.0), 1);
    assert_eq!(px(&raster, 2, 2), BACKDROP);
}

#[test]
fn what_a_group_paints_does_not_start_with_the_blend_mode_or_alpha_outside() {
    // The group is composed once with Multiply; inside it the state is
    // Normal at full opacity, so the red is red, not red multiplied twice
    // or at one half.
    let raster = render(&blend_group_page("", "/BM /Multiply /ca 1"));
    assert_near(px(&raster, 10, 10), bytes(0.8, 0.0, 0.0), 1);
}

/// The page whose group paints cyan with Multiply over the red page.
fn isolation_page(group: &str) -> kernkraft_pdf::doc::Document {
    let inside = "/M gs 0 1 1 rg 0 0 20 20 re f";
    square_page(
        "1 0 0 rg 0 0 20 20 re f /F Do",
        "/XObject << /F 20 0 R >>",
        &[(
            GROUP,
            &stream(
                &format!(
                    " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
                     /Group << /S /Transparency {group} >> \
                     /Resources << /ExtGState << /M << /BM /Multiply >> >> >>"
                ),
                inside.as_bytes(),
            ),
        )],
    )
}

#[test]
fn a_non_isolated_group_blends_with_the_page_under_it() {
    // Cyan multiplied by red is black.
    let raster = render(&isolation_page("/I false"));
    assert_box(&raster, 0, 0, 20, 20, [0, 0, 0, 255]);
}

#[test]
fn an_isolated_group_blends_only_within_itself() {
    // Over nothing, Multiply leaves the cyan; the group then covers the
    // red page as it is.
    let raster = render(&isolation_page("/I true"));
    assert_box(&raster, 0, 0, 20, 20, [0, 255, 255, 255]);
}

/// A soft-mask group of `content`, which `mask_state` sets as the `SMask`.
fn masked(
    mask_state: &str,
    content: &str,
    group: &str,
    resources: &str,
) -> kernkraft_pdf::doc::Document {
    let mask = stream(
        &format!(
            " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
             /Group << /S /Transparency /CS /DeviceGray >> {group}"
        ),
        b"1 g 0 0 10 20 re f 0.5 g 10 0 5 20 re f",
    );
    square_page(
        content,
        &format!("/ExtGState << /GS << /SMask << /Type /Mask {mask_state} /G 21 0 R >> >> >> {resources}"),
        &[(MASK, &mask)],
    )
}

const RED_PAGE: &str = "/GS gs 1 0 0 rg 0 0 20 20 re f";

#[test]
fn a_luminosity_mask_scales_the_paint_by_the_group_gray() {
    let raster = render(&masked("/S /Luminosity", RED_PAGE, "", ""));
    assert_box(&raster, 0, 0, 10, 20, RED);
    // Mid gray is a coverage of 128 / 255 over white.
    let covered: f64 = 255.0 * (1.0 - 128.0 / 255.0);
    assert_near(
        px(&raster, 12, 10),
        [255, covered.round() as u8, covered.round() as u8, 255],
        1,
    );
    assert_box(&raster, 15, 0, 20, 20, WHITE);
}

#[test]
fn an_alpha_mask_is_the_alpha_of_the_group() {
    // The group paints opaque on the left 15 points whatever its colours.
    let raster = render(&masked("/S /Alpha", RED_PAGE, "", ""));
    assert_box(&raster, 0, 0, 15, 20, RED);
    assert_box(&raster, 15, 0, 20, 20, WHITE);
}

#[test]
fn a_backdrop_colour_gives_the_value_where_the_group_paints_nothing() {
    // BC is white (1): outside what the group painted the mask is 1.
    let raster = render(&masked("/S /Luminosity /BC [1]", RED_PAGE, "", ""));
    assert_box(&raster, 0, 0, 10, 20, RED);
    // Over the mid gray band the mask is the gray, over the rest it is the
    // backdrop's white.
    assert_box(&raster, 15, 0, 20, 20, RED);
}

#[test]
fn a_transfer_function_maps_the_mask_values() {
    // 1 - v: the white band is off, the black region is on.
    let raster = render(&masked(
        "/S /Luminosity /TR << /FunctionType 2 /Domain [0 1] /C0 [1] /C1 [0] /N 1 >>",
        RED_PAGE,
        "",
        "",
    ));
    assert_box(&raster, 0, 0, 10, 20, WHITE);
    assert_box(&raster, 15, 0, 20, 20, RED);
}

#[test]
fn the_identity_transfer_is_no_transfer() {
    let raster = render(&masked("/S /Luminosity /TR /Identity", RED_PAGE, "", ""));
    assert_box(&raster, 0, 0, 10, 20, RED);
}

#[test]
fn the_mask_none_removes_the_soft_mask() {
    let mask = stream(
        " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
         /Group << /S /Transparency /CS /DeviceGray >>",
        b"1 g 0 0 10 20 re f",
    );
    let doc = square_page(
        "/GS gs 0 0 1 rg 0 0 20 5 re f /N gs 1 0 0 rg 0 15 20 5 re f",
        "/ExtGState << /GS << /SMask << /Type /Mask /S /Luminosity /G 21 0 R >> >> \
         /N << /SMask /None >> >>",
        &[(MASK, &mask)],
    );
    let raster = render(&doc);
    // Blue under the mask, on the left only; red once it is removed.
    assert_box(&raster, 0, 15, 10, 20, BLUE);
    assert_box(&raster, 10, 15, 20, 20, WHITE);
    assert_box(&raster, 0, 0, 20, 5, RED);
}

#[test]
fn the_mask_lies_where_the_ctm_put_it_when_it_was_set() {
    // Set under a scale of one half, so its white half is the left 5
    // points; the fill then undoes the scale, so it spans the page.
    let raster = render(&masked(
        "/S /Luminosity",
        "q 0.5 0 0 0.5 0 0 cm /GS gs 2 0 0 2 0 0 cm 1 0 0 rg 0 0 20 20 re f Q",
        "",
        "",
    ));
    // The group's 20 points are 10 on the page, from the bottom up: its
    // white left half is the 5 columns of the lower 10 rows, and its mid
    // gray band (10 to 15 in the group) the columns 5 and 6 and half of 7.
    assert_box(&raster, 0, 10, 5, 20, RED);
    assert_near(px(&raster, 5, 12), [255, 127, 127, 255], 1);
    assert_near(px(&raster, 6, 12), [255, 127, 127, 255], 1);
    assert_box(&raster, 8, 10, 20, 20, WHITE);
    assert_box(&raster, 0, 0, 20, 10, WHITE);
}

#[test]
fn a_restored_state_has_the_mask_it_had() {
    let raster = render(&masked(
        "/S /Luminosity",
        "q /GS gs Q 1 0 0 rg 0 0 20 20 re f",
        "",
        "",
    ));
    assert_box(&raster, 0, 0, 20, 20, RED);
}

#[test]
fn a_soft_mask_cuts_a_group_as_a_whole() {
    let doc = square_page(
        "/GS gs /F Do",
        "/ExtGState << /GS << /SMask << /Type /Mask /S /Luminosity /G 21 0 R >> >> >> \
         /XObject << /F 20 0 R >>",
        &[
            (GROUP, &form("/Group << /S /Transparency >>", OVERLAP)),
            (
                MASK,
                &stream(
                    " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
                     /Group << /S /Transparency /CS /DeviceGray >>",
                    b"0.5 g 0 0 20 20 re f",
                ),
            ),
        ],
    );
    let raster = render(&doc);
    // Blue covers red in the group; the half mask then keeps half of it.
    assert_near(px(&raster, 10, 10), [127, 127, 255, 255], 1);
    assert_near(px(&raster, 2, 10), [255, 127, 127, 255], 1);
}

#[test]
fn a_soft_mask_with_no_group_is_reported_and_leaves_no_mask() {
    let doc = square_page(
        "/GS gs 1 0 0 rg 0 0 20 20 re f",
        "/ExtGState << /GS << /SMask << /Type /Mask /S /Luminosity >> >> >>",
        &[],
    );
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 20, RED);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(
            k,
            DiagnosticKind::SoftMaskUnusable {
                fault: PaintFault::Entry
            }
        ))
        .len(),
        1
    );
}

/// A page that composes the group object 20 as `/F Do`, whose content
/// paints a red and a blue box overlapping in the columns 8 to 12, each at
/// the alpha `H` sets.
fn knockout_page(group: &str, state: &str) -> kernkraft_pdf::doc::Document {
    let content = format!("/H gs {OVERLAP}");
    square_page(
        "/F Do",
        "/XObject << /F 20 0 R >>",
        &[(
            GROUP,
            &stream(
                &format!(
                    " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
                     /Group << /S /Transparency {group} >> \
                     /Resources << /ExtGState << /H << {state} >> >> >>"
                ),
                content.as_bytes(),
            ),
        )],
    )
}

#[test]
fn in_a_knockout_group_a_mark_replaces_the_marks_before_it() {
    for isolated in ["/I false", "/I true"] {
        let raster = render(&knockout_page(&format!("/K true {isolated}"), "/ca 0.5"));
        assert_near(px(&raster, 2, 10), [255, 128, 128, 255], 1);
        // The blue is composed with the empty backdrop, not with the red.
        assert_near(px(&raster, 10, 10), [128, 128, 255, 255], 1);
        assert_near(px(&raster, 16, 10), [128, 128, 255, 255], 1);
    }
}

#[test]
fn in_a_group_that_is_not_a_knockout_translucent_marks_accumulate() {
    let raster = render(&knockout_page("/K false /I false", "/ca 0.5"));
    assert_near(px(&raster, 10, 10), [128, 64, 191, 255], 1);
}

#[test]
fn an_alpha_that_is_a_shape_knocks_out_in_proportion() {
    // Shape one half, opacity 1: where the blue covers the red it leaves
    // half of it, so the box shows a half of each.
    let raster = render(&knockout_page("/K true /I true", "/ca 0.5 /AIS true"));
    assert_near(px(&raster, 2, 10), [255, 128, 128, 255], 1);
    assert_near(px(&raster, 10, 10), [128, 64, 191, 255], 1);
    assert_near(px(&raster, 16, 10), [128, 128, 255, 255], 1);
}

#[test]
fn a_composed_group_that_is_not_isolated_blends_its_marks_with_the_backdrop() {
    // The page is red, the group is composed at one half, and inside it the
    // cyan is multiplied with what is under the group: black. Half black
    // over red is (0.5, 0, 0).
    let doc = square_page(
        "1 0 0 rg 0 0 20 20 re f /H gs /F Do",
        "/XObject << /F 20 0 R >> /ExtGState << /H << /ca 0.5 >> >>",
        &[(
            GROUP,
            &stream(
                " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
                 /Group << /S /Transparency /I false >> \
                 /Resources << /ExtGState << /M << /BM /Multiply >> >> >>",
                b"/M gs 0 1 1 rg 0 0 20 20 re f",
            ),
        )],
    );
    assert_box_near(&render(&doc), [128, 0, 0, 255]);
}

#[test]
fn a_composed_isolated_group_does_not() {
    // Over nothing the multiply leaves the cyan; half of it over red is
    // the mean of the two.
    let doc = square_page(
        "1 0 0 rg 0 0 20 20 re f /H gs /F Do",
        "/XObject << /F 20 0 R >> /ExtGState << /H << /ca 0.5 >> >>",
        &[(
            GROUP,
            &stream(
                " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
                 /Group << /S /Transparency /I true >> \
                 /Resources << /ExtGState << /M << /BM /Multiply >> >> >>",
                b"/M gs 0 1 1 rg 0 0 20 20 re f",
            ),
        )],
    );
    assert_box_near(&render(&doc), [128, 128, 128, 255]);
}

#[test]
fn a_non_isolated_group_takes_out_the_backdrop_it_started_with() {
    // The group paints green at one half over the red page, so inside it
    // the layer is (0.5, 0.5, 0); with the backdrop taken out the group is
    // green at alpha one half, and composed at one half again it covers
    // the page by a quarter: (0.75, 0.25, 0).
    let doc = square_page(
        "1 0 0 rg 0 0 20 20 re f /H gs /F Do",
        "/XObject << /F 20 0 R >> /ExtGState << /H << /ca 0.5 >> >>",
        &[(
            GROUP,
            &stream(
                " /Type /XObject /Subtype /Form /BBox [0 0 20 20] \
                 /Group << /S /Transparency /I false >> \
                 /Resources << /ExtGState << /H << /ca 0.5 >> >> >>",
                b"/H gs 0 1 0 rg 0 0 20 20 re f",
            ),
        )],
    );
    assert_box_near(&render(&doc), [191, 64, 0, 255]);
}

fn assert_box_near(raster: &kernkraft_pdf::render::Raster, colour: [u8; 4]) {
    for (x, y) in [(0, 0), (10, 10), (19, 19), (3, 15)] {
        assert_near(px(raster, x, y), colour, 1);
    }
}
