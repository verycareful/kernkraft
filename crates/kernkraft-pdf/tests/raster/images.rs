// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Images on the page: ISO 32000-1:2008, 8.9. The page is 20 by 20 points
//! at a pixel to the point, so an image drawn on whole numbers covers whole
//! pixels and a sample covers a block of them; the expected pixels follow
//! from the samples and the placement.

use kernkraft_pdf::render::RenderOptions;
use kernkraft_pdf::{DiagnosticKind, ImageFault};

use crate::{
    assert_box, assert_near, diagnostics_of, document, px, render, render_with, square_page,
    stream, BLACK, BLUE, RED, WHITE,
};

const IMAGE: u32 = 20;
const SECOND: u32 = 21;

/// `entries` and `data` as an image XObject.
fn image(entries: &str, data: &[u8]) -> Vec<u8> {
    stream(&format!(" /Type /XObject /Subtype /Image {entries}"), data)
}

/// Draws the image object 20 on the whole page.
const FULL: &str = "q 20 0 0 20 0 0 cm /Im Do Q";
const RESOURCES: &str = "/XObject << /Im 20 0 R >>";

fn page(content: &str, extra: &[(u32, &[u8])]) -> kernkraft_pdf::doc::Document {
    square_page(content, RESOURCES, extra)
}

fn gray(value: u8) -> [u8; 4] {
    [value, value, value, 255]
}

#[test]
fn a_gray_image_fills_the_unit_square_with_its_first_row_on_top() {
    // Rows [0 255] and [128 64], each sample a 10 by 10 block.
    let data = image(
        "/Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[0, 255, 128, 64],
    );
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 10, gray(0));
    assert_box(&raster, 10, 0, 20, 10, gray(255));
    assert_box(&raster, 0, 10, 10, 20, gray(128));
    assert_box(&raster, 10, 10, 20, 20, gray(64));
}

#[test]
fn the_ctm_places_and_scales_the_image() {
    let data = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0, 0, 0, 255],
    );
    // The 10 by 10 square at (5, 5): red on the left half, blue on the right.
    let raster = render(&page("q 10 0 0 10 5 5 cm /Im Do Q", &[(IMAGE, &data)]));
    assert_box(&raster, 5, 5, 10, 15, RED);
    assert_box(&raster, 10, 5, 15, 15, BLUE);
    assert_box(&raster, 0, 0, 20, 5, WHITE);
    assert_box(&raster, 0, 15, 20, 20, WHITE);
    assert_box(&raster, 0, 0, 5, 20, WHITE);
    assert_box(&raster, 15, 0, 20, 20, WHITE);
}

#[test]
fn a_negative_scale_mirrors_the_image() {
    let data = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0, 0, 0, 255],
    );
    let raster = render(&page("q -20 0 0 20 20 0 cm /Im Do Q", &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 20, BLUE);
    assert_box(&raster, 10, 0, 20, 20, RED);
}

#[test]
fn a_negative_vertical_scale_turns_the_image_over() {
    let data = image(
        "/Width 1 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0, 0, 0, 255],
    );
    let upright = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&upright, 0, 0, 20, 10, RED);
    assert_box(&upright, 0, 10, 20, 20, BLUE);
    let flipped = render(&page("q 20 0 0 -20 0 20 cm /Im Do Q", &[(IMAGE, &data)]));
    assert_box(&flipped, 0, 0, 20, 10, BLUE);
    assert_box(&flipped, 0, 10, 20, 20, RED);
}

#[test]
fn a_rotated_image_turns_a_quarter() {
    // [0 20 -20 0 20 0] maps (x, y) to (20 - 20y, 20x): the image's right
    // is up the page, so its left (red) is at the bottom and its right
    // (blue) at the top.
    let data = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0, 0, 0, 255],
    );
    let raster = render(&page("q 0 20 -20 0 20 0 cm /Im Do Q", &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 20, 10, BLUE);
    assert_box(&raster, 0, 10, 20, 20, RED);
}

#[test]
fn one_bit_samples_are_packed_and_rows_padded() {
    // 3 by 2, rows 010 and 101, each padded to a byte.
    let data = image(
        "/Width 3 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 1",
        &[0b0100_0000, 0b1010_0000],
    );
    let raster = render(&page("q 30 0 0 20 0 0 cm /Im Do Q", &[(IMAGE, &data)]));
    // The page is 20 wide: the 30-point image is cut at the edge, each
    // sample 10 points wide.
    assert_box(&raster, 0, 0, 10, 10, BLACK);
    assert_box(&raster, 10, 0, 20, 10, WHITE);
    assert_box(&raster, 0, 10, 10, 20, WHITE);
    assert_box(&raster, 10, 10, 20, 20, BLACK);
}

#[test]
fn the_decode_array_inverts_the_samples() {
    let data = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Decode [1 0]",
        &[0, 255],
    );
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 20, WHITE);
    assert_box(&raster, 10, 0, 20, 20, BLACK);
}

#[test]
fn an_indexed_image_looks_its_samples_up() {
    let data = image(
        "/Width 2 /Height 1 /BitsPerComponent 1 \
         /ColorSpace [/Indexed /DeviceRGB 1 <FF0000 0000FF>]",
        &[0b0100_0000],
    );
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn a_filtered_image_is_decoded_first() {
    let data = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /ASCIIHexDecode",
        b"FF0000 0000FF>",
    );
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn the_fill_alpha_applies_to_an_image() {
    let data = image(
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[0],
    );
    let doc = square_page(
        "/G gs q 20 0 0 20 0 0 cm /Im Do Q",
        "/XObject << /Im 20 0 R >> /ExtGState << /G << /ca 0.5 >> >>",
        &[(IMAGE, &data)],
    );
    let raster = render(&doc);
    assert_near(px(&raster, 5, 5), [127, 127, 127, 255], 1);
}

#[test]
fn an_image_mask_is_painted_in_the_fill_colour() {
    // Rows 01 and 10: the sample 0 paints (Decode [0 1]), so the top left
    // and bottom right blocks are painted.
    let mask = image(
        "/Width 2 /Height 2 /ImageMask true /BitsPerComponent 1",
        &[0b0100_0000, 0b1000_0000],
    );
    let raster = render(&page(
        "1 0 0 rg q 20 0 0 20 0 0 cm /Im Do Q",
        &[(IMAGE, &mask)],
    ));
    assert_box(&raster, 0, 0, 10, 10, RED);
    assert_box(&raster, 10, 0, 20, 10, WHITE);
    assert_box(&raster, 0, 10, 10, 20, WHITE);
    assert_box(&raster, 10, 10, 20, 20, RED);
}

#[test]
fn an_image_mask_with_decode_1_0_paints_the_ones() {
    let mask = image(
        "/Width 2 /Height 1 /ImageMask true /Decode [1 0]",
        &[0b0100_0000],
    );
    let raster = render(&page(
        "0 0 1 rg q 20 0 0 20 0 0 cm /Im Do Q",
        &[(IMAGE, &mask)],
    ));
    assert_box(&raster, 0, 0, 10, 20, WHITE);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn a_soft_mask_sets_the_alpha() {
    let mask = image(
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[128],
    );
    let colour = format!(
        "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {SECOND} 0 R"
    );
    let colour = image(&colour, &[255, 0, 0]);
    let raster = render(&page(FULL, &[(IMAGE, &colour), (SECOND, &mask)]));
    // Red at 128/255 over white: green and blue are 255 * (1 - 128/255).
    assert_near(px(&raster, 10, 10), [255, 127, 127, 255], 1);
}

#[test]
fn a_soft_mask_of_another_size_is_taken_at_the_nearest_sample() {
    // One red pixel, its mask two samples wide: opaque, then clear.
    let mask = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[255, 0],
    );
    let colour = format!(
        "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {SECOND} 0 R"
    );
    let colour = image(&colour, &[255, 0, 0]);
    let raster = render(&page(FULL, &[(IMAGE, &colour), (SECOND, &mask)]));
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn a_matte_undoes_the_blending_of_a_soft_masked_image() {
    // The image was blended with a black matte at alpha 128/255: a white
    // source pixel is stored as 128. Un-blended, c = m + (c' - m) / alpha
    // = 0 + 128 / (128 / 255) = 255, white at half opacity: 255 over the
    // page's white stays white; over a black fill it is half grey.
    let mask = "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8 /Matte [0 0 0]";
    let mask = image(mask, &[128]);
    let colour = format!(
        "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask {SECOND} 0 R"
    );
    let colour = image(&colour, &[128, 128, 128]);
    let raster = render(&page(
        &format!("0 g 0 0 20 20 re f {FULL}"),
        &[(IMAGE, &colour), (SECOND, &mask)],
    ));
    assert_near(px(&raster, 10, 10), [128, 128, 128, 255], 2);
}

#[test]
fn an_explicit_mask_hides_where_its_sample_is_one() {
    // Mask samples 1 0: the left pixel is masked out, the right painted.
    let mask = image(
        "/Width 2 /Height 1 /ImageMask true /BitsPerComponent 1",
        &[0b1000_0000],
    );
    let colour =
        format!("/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Mask {SECOND} 0 R");
    let colour = image(&colour, &[255, 0, 0, 0, 0, 255]);
    let raster = render(&page(FULL, &[(IMAGE, &colour), (SECOND, &mask)]));
    assert_box(&raster, 0, 0, 10, 20, WHITE);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn colour_key_masking_makes_matching_pixels_transparent() {
    let colour = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8 \
         /Mask [250 255 0 5 0 5]",
        &[255, 0, 0, 0, 0, 255],
    );
    let raster = render(&page(FULL, &[(IMAGE, &colour)]));
    assert_box(&raster, 0, 0, 10, 20, WHITE);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn an_inline_image_is_painted() {
    let content = "q 20 0 0 20 0 0 cm \
                   BI /W 2 /H 1 /CS /RGB /BPC 8 /F /AHx ID FF0000 0000FF> EI Q";
    let raster = render(&square_page(content, "", &[]));
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn an_inline_image_may_name_a_colour_space_resource() {
    let content = "q 20 0 0 20 0 0 cm BI /W 2 /H 1 /CS /Cs1 /BPC 1 /F /AHx ID 40> EI Q";
    let resources = "/ColorSpace << /Cs1 [/Indexed /DeviceRGB 1 <FF0000 0000FF>] >>";
    let raster = render(&square_page(content, resources, &[]));
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, BLUE);
}

#[test]
fn an_inline_image_mask_takes_the_fill_colour() {
    let content = "0 0 1 rg q 20 0 0 20 0 0 cm BI /W 2 /H 1 /IM true /F /AHx ID 40> EI Q";
    let raster = render(&square_page(content, "", &[]));
    assert_box(&raster, 0, 0, 10, 20, BLUE);
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn the_clip_cuts_an_image() {
    let data = image(
        "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0],
    );
    let raster = render(&page(
        &format!("0 0 10 20 re W n {FULL}"),
        &[(IMAGE, &data)],
    ));
    assert_box(&raster, 0, 0, 10, 20, RED);
    assert_box(&raster, 10, 0, 20, 20, WHITE);
}

#[test]
fn an_image_used_twice_is_drawn_twice() {
    let data = image(
        "/Width 1 /Height 1 /ColorSpace /DeviceRGB /BitsPerComponent 8",
        &[255, 0, 0],
    );
    let content = "q 5 0 0 5 0 0 cm /Im Do Q q 5 0 0 5 10 10 cm /Im Do Q";
    let raster = render(&page(content, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 15, 5, 20, RED);
    assert_box(&raster, 10, 5, 15, 10, RED);
    assert_box(&raster, 5, 5, 10, 15, WHITE);
}

#[test]
fn shrinking_an_image_averages_it() {
    // A one-pixel checkerboard of black and white, 8 by 8, drawn 2 by 2:
    // every output pixel is the mean of a 4 by 4 block, mid grey.
    let samples: Vec<u8> = (0..64)
        .map(|i| if (i / 8 + i % 8) % 2 == 0 { 0 } else { 255 })
        .collect();
    let data = image(
        "/Width 8 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &samples,
    );
    // The square at user (0, 18) is the two top rows of pixels.
    let raster = render(&page("q 2 0 0 2 0 18 cm /Im Do Q", &[(IMAGE, &data)]));
    for (x, y) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
        assert_near(px(&raster, x, y), [128, 128, 128, 255], 1);
    }
}

#[test]
fn an_image_over_the_limit_is_left_out_and_said_so() {
    let data = image(
        "/Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[0, 0, 0, 0],
    );
    let doc = page(FULL, &[(IMAGE, &data)]);
    let options = RenderOptions {
        max_image_pixels: 3,
        ..Default::default()
    };
    let raster = render_with(&doc, &options).unwrap();
    assert_box(&raster, 0, 0, 20, 20, WHITE);
    let found = diagnostics_of(&doc, |k| {
        matches!(
            k,
            DiagnosticKind::ImageUnusable {
                fault: ImageFault::TooLarge
            }
        )
    });
    assert_eq!(found.len(), 1);
}

#[test]
fn an_image_with_no_dimensions_is_left_out_and_said_so() {
    let data = image("/ColorSpace /DeviceGray /BitsPerComponent 8", &[0, 0, 0, 0]);
    let doc = page(FULL, &[(IMAGE, &data)]);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 20, WHITE);
    let found = diagnostics_of(&doc, |k| {
        matches!(
            k,
            DiagnosticKind::ImageUnusable {
                fault: ImageFault::Dimensions
            }
        )
    });
    assert_eq!(found.len(), 1);
}

#[test]
fn short_data_reads_as_zero_and_is_said_so() {
    // Two rows of two pixels, one row of data: the second row is black.
    let data = image(
        "/Width 2 /Height 2 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[255, 255],
    );
    let doc = page(FULL, &[(IMAGE, &data)]);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 10, WHITE);
    assert_box(&raster, 0, 10, 20, 20, BLACK);
    assert_eq!(
        diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::ImageDataShort)).len(),
        1
    );
}

#[test]
fn a_separation_of_none_paints_nothing_and_says_nothing() {
    let tint = "<< /FunctionType 2 /Domain [0 1] /N 1 /C0 [0] /C1 [1] >>";
    let data = image(
        &format!(
            "/Width 1 /Height 1 /BitsPerComponent 8 \
             /ColorSpace [/Separation /None /DeviceGray {tint}]"
        ),
        &[255],
    );
    let doc = page(FULL, &[(IMAGE, &data)]);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 20, WHITE);
    assert!(diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::ImageUnusable { .. })).is_empty());
}

#[test]
fn an_image_is_no_longer_reported_as_not_rendered() {
    let data = image(
        "/Width 1 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[0],
    );
    let doc = page(FULL, &[(IMAGE, &data)]);
    render(&doc);
    assert!(diagnostics_of(&doc, |k| matches!(
        k,
        DiagnosticKind::NotRendered { feature: "image" }
    ))
    .is_empty());
}

const GRAY_JPEG: &[u8] = include_bytes!("../../../../testdata/image/gray-60-200.jpg");
const CMYK_JPEG: &[u8] = include_bytes!("../../../../testdata/image/cmyk-10-20-30-40.jpg");
const RGB_JPX: &[u8] = include_bytes!("../../../../testdata/image/rgb-red-blue.jp2");
const RGBA_JPX: &[u8] = include_bytes!("../../../../testdata/image/rgba-red-blue-alpha.jp2");
const BARS_G4: &[u8] = include_bytes!("../../../../testdata/image/bars-g4.bin");

#[test]
fn a_jpeg_image() {
    let data = image(
        "/Width 16 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
        GRAY_JPEG,
    );
    let doc = document(
        "/MediaBox [0 0 16 8]",
        "q 16 0 0 8 0 0 cm /Im Do Q",
        RESOURCES,
        &[(IMAGE, &data)],
    );
    let raster = render(&doc);
    assert_near(px(&raster, 2, 3), gray(60), 3);
    assert_near(px(&raster, 13, 3), gray(200), 3);
}

/// The CMYK file stores (10, 20, 30, 40) inverted, as Adobe does, and the
/// image says so with `Decode [1 0 ...]`. The colour it comes to is
/// (10, 20, 30, 40) of 255 through the CMYK profile, converted here by
/// `moxcms` from the bundled profile's bytes.
#[test]
fn an_adobe_cmyk_jpeg_is_turned_right_by_its_decode_array() {
    let data = image(
        "/Width 8 /Height 8 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Filter /DCTDecode \
         /Decode [1 0 1 0 1 0 1 0]",
        CMYK_JPEG,
    );
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    let expected = through_bundled_profile([10.0, 20.0, 30.0, 40.0].map(|level| level / 255.0));
    assert_near(
        px(&raster, 10, 10),
        [expected[0], expected[1], expected[2], 255],
        2,
    );
}

/// `colour` (four components, 0 to 1) in sRGB levels, by `moxcms` alone:
/// the bundled profile to sRGB, relative colorimetric (8.6.5.8).
fn through_bundled_profile(colour: [f64; 4]) -> [u8; 3] {
    use moxcms::{ColorProfile, Layout, RenderingIntent, TransformOptions};
    let source = ColorProfile::new_from_slice(kernkraft_pdf::colour::BUNDLED_PROFILE).unwrap();
    let options = TransformOptions {
        rendering_intent: RenderingIntent::RelativeColorimetric,
        ..TransformOptions::default()
    };
    let transform = source
        .create_transform_f32(
            Layout::Rgba,
            &ColorProfile::new_srgb(),
            Layout::Rgb,
            options,
        )
        .unwrap();
    let mut out = [0f32; 3];
    transform
        .transform(&colour.map(|level| level as f32), &mut out)
        .unwrap();
    out.map(|level| (f64::from(level).clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// The same file in a document whose CMYK profile is the application's,
/// a profile whose lightness is that of the black ink alone: the colour is
/// a grey, `L* = 100 (1 - 40 / 255)`, encoded by sRGB.
#[test]
fn a_cmyk_image_follows_the_cmyk_profile_of_the_document() {
    let data = image(
        "/Width 8 /Height 8 /ColorSpace /DeviceCMYK /BitsPerComponent 8 /Filter /DCTDecode \
         /Decode [1 0 1 0 1 0 1 0]",
        CMYK_JPEG,
    );
    let doc = page(FULL, &[(IMAGE, &data)]);
    doc.set_cmyk_profile(&crate::pdf_fixture::grey_by_black_profile())
        .unwrap();
    let lightness = 100.0 * (1.0 - 40.0 / 255.0);
    let linear = ((lightness + 16.0) / 116.0_f64).powi(3);
    let encoded = 1.055 * linear.powf(1.0 / 2.4) - 0.055;
    let grey = (encoded * 255.0).round() as u8;
    assert_near(px(&render(&doc), 10, 10), [grey, grey, grey, 255], 3);
}

#[test]
fn a_jpeg_whose_components_do_not_fit_its_colour_space_is_left_out() {
    let data = image(
        "/Width 8 /Height 8 /ColorSpace /DeviceRGB /BitsPerComponent 8 /Filter /DCTDecode",
        CMYK_JPEG,
    );
    let doc = page(FULL, &[(IMAGE, &data)]);
    let raster = render(&doc);
    assert_box(&raster, 0, 0, 20, 20, WHITE);
    let found = diagnostics_of(&doc, |k| {
        matches!(
            k,
            DiagnosticKind::ImageUnusable {
                fault: ImageFault::ColourSpace
            }
        )
    });
    assert_eq!(found.len(), 1);
}

#[test]
fn a_jpeg_2000_image_names_its_own_colour_space() {
    let data = image("/Width 8 /Height 8 /Filter /JPXDecode", RGB_JPX);
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 20, [200, 50, 50, 255]);
    assert_box(&raster, 10, 0, 20, 20, [50, 50, 200, 255]);
}

#[test]
fn a_jpeg_2000_opacity_channel_is_used_when_smask_in_data_asks() {
    // Opacity 255 on the left half and 100 on the right.
    let data = image(
        "/Width 8 /Height 8 /Filter /JPXDecode /SMaskInData 1",
        RGBA_JPX,
    );
    let raster = render(&page(FULL, &[(IMAGE, &data)]));
    assert_box(&raster, 0, 0, 10, 20, [200, 50, 50, 255]);
    // (50, 50, 200) at 100/255 over white.
    let blend = |c: f64| (c * 100.0 / 255.0 + 255.0 * 155.0 / 255.0).round() as u8;
    assert_near(
        px(&raster, 15, 10),
        [blend(50.0), blend(50.0), blend(200.0), 255],
        1,
    );
    // Without SMaskInData the opacity is not used.
    let plain = image("/Width 8 /Height 8 /Filter /JPXDecode", RGBA_JPX);
    let raster = render(&page(FULL, &[(IMAGE, &plain)]));
    assert_box(&raster, 10, 0, 20, 20, [50, 50, 200, 255]);
}

/// The strip of `testdata/image/bars-g4.bin`: a 0 of the 40 by 6 image it
/// was written from is a CCITT white run, and the filter gives 1 for white.
#[test]
fn a_ccitt_image() {
    let data = image(
        "/Width 40 /Height 6 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode \
         /DecodeParms << /K -1 /Columns 40 /Rows 6 >>",
        BARS_G4,
    );
    let doc = document(
        "/MediaBox [0 0 40 6]",
        "q 40 0 0 6 0 0 cm /Im Do Q",
        RESOURCES,
        &[(IMAGE, &data)],
    );
    let raster = render(&doc);
    for y in 0..6 {
        for x in 0..40 {
            let zero = match y {
                2 => false,
                4 => true,
                _ => (3..10).contains(&x) || (20..22).contains(&x),
            };
            assert_eq!(
                px(&raster, x, y),
                if zero { WHITE } else { BLACK },
                "({x}, {y})"
            );
        }
    }
}

/// Without its globals the page of 7.4.7 cannot be decoded: the image is
/// left out and said so, not drawn as a blank or black page.
#[test]
fn a_jbig2_image_missing_its_globals_is_left_out_and_said_so() {
    let data = image(
        "/Width 52 /Height 66 /ColorSpace /DeviceGray /BitsPerComponent 1 \
         /Filter [/ASCIIHexDecode /JBIG2Decode]",
        PAGE_HEX.as_bytes(),
    );
    let doc = document(
        "/MediaBox [0 0 52 66]",
        "q 52 0 0 66 0 0 cm /Im Do Q",
        RESOURCES,
        &[(IMAGE, &data)],
    );
    let raster = render(&doc);
    assert!(raster.rgba.iter().all(|byte| *byte == 255));
    let found = diagnostics_of(&doc, |k| {
        matches!(
            k,
            DiagnosticKind::ImageUnusable {
                fault: ImageFault::Data
            }
        )
    });
    assert_eq!(found.len(), 1);
}

const GLOBALS_HEX: &str = "00 00 00 00 00 01 00 00 00 00 32 00 00 03 FF FD FF 02 FE FE FE 00 00 \
                           00 01 00 00 00 01 2A E2 25 AE A9 A5 A5 38 B4 D9 99 9C 5C 8E 56 EF 0F \
                           87 27 F2 B5 3D 4E 37 EF 79 5C C5 50 6D FF AC>";
const PAGE_HEX: &str = "00 00 00 01 30 00 01 00 00 00 13 00 00 00 34 00 00 00 42 00 00 00 00 \
                        00 00 00 00 40 00 00 00 00 00 02 06 20 00 01 00 00 00 1E 00 00 00 34 \
                        00 00 00 42 00 00 00 00 00 00 00 00 02 00 10 00 00 00 02 31 DB 51 CE \
                        51 FF AC>";

/// The image object of 7.4.7, Example 1, with its globals stream.
#[test]
fn a_jbig2_image_with_globals() {
    let globals = stream("/Filter /ASCIIHexDecode", GLOBALS_HEX.as_bytes());
    let data = image(
        "/Width 52 /Height 66 /ColorSpace /DeviceGray /BitsPerComponent 1 \
         /Filter [/ASCIIHexDecode /JBIG2Decode] /DecodeParms [null << /JBIG2Globals 21 0 R >>]",
        PAGE_HEX.as_bytes(),
    );
    let doc = document(
        "/MediaBox [0 0 52 66]",
        "q 52 0 0 66 0 0 cm /Im Do Q",
        RESOURCES,
        &[(IMAGE, &data), (SECOND, &globals)],
    );
    let raster = render(&doc);
    let dark = raster
        .rgba
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[0] < 128)
        .count();
    assert!(
        dark > 0 && dark < raster.rgba.len() / 8,
        "{dark} dark pixels"
    );
    assert!(diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::ImageUnusable { .. })).is_empty());
}

/// Every image codec on one page each, their files then cut and damaged.
fn codec_files() -> Vec<crate::Fixture> {
    let dct = image(
        "/Width 16 /Height 8 /ColorSpace /DeviceGray /BitsPerComponent 8 /Filter /DCTDecode",
        GRAY_JPEG,
    );
    let jpx = image(
        "/Width 8 /Height 8 /Filter /JPXDecode /SMaskInData 1",
        RGBA_JPX,
    );
    let ccitt = image(
        "/Width 40 /Height 6 /ColorSpace /DeviceGray /BitsPerComponent 1 /Filter /CCITTFaxDecode \
         /DecodeParms << /K -1 /Columns 40 /Rows 6 >>",
        BARS_G4,
    );
    let globals = stream("/Filter /ASCIIHexDecode", GLOBALS_HEX.as_bytes());
    let jbig2 = image(
        "/Width 52 /Height 66 /ColorSpace /DeviceGray /BitsPerComponent 1 \
         /Filter [/ASCIIHexDecode /JBIG2Decode] /DecodeParms [null << /JBIG2Globals 21 0 R >>]",
        PAGE_HEX.as_bytes(),
    );
    let mask = image(
        "/Width 2 /Height 1 /ColorSpace /DeviceGray /BitsPerComponent 8",
        &[255, 80],
    );
    let soft = image(
        "/Width 2 /Height 2 /ColorSpace /DeviceRGB /BitsPerComponent 8 /SMask 21 0 R \
         /Mask [0 10 0 10 0 10]",
        &[255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 9, 9],
    );
    let page =
        |extra: &[(u32, &[u8])]| crate::fixture("/MediaBox [0 0 20 20]", FULL, RESOURCES, extra);
    vec![
        page(&[(IMAGE, &dct)]),
        page(&[(IMAGE, &jpx)]),
        page(&[(IMAGE, &ccitt)]),
        page(&[(IMAGE, &jbig2), (SECOND, &globals)]),
        page(&[(IMAGE, &soft), (SECOND, &mask)]),
    ]
}

#[test]
fn every_codec_file_renders_before_it_is_damaged() {
    for file in codec_files() {
        let doc = file.open();
        render(&doc);
        assert!(
            diagnostics_of(&doc, |k| matches!(k, DiagnosticKind::ImageUnusable { .. })).is_empty()
        );
    }
}

#[test]
fn damaged_image_data_is_drawn_or_refused_never_a_panic() {
    for file in codec_files() {
        let bytes = file.bytes;
        for length in (0..bytes.len()).step_by((bytes.len() / 60).max(1)) {
            let _ = crate::robustness_render(bytes[..length].to_vec());
        }
        for at in (0..bytes.len()).step_by(3) {
            for value in [0x00, 0xFF, 0x7F] {
                let mut damaged = bytes.clone();
                damaged[at] = value;
                let _ = crate::robustness_render(damaged);
            }
        }
    }
}
