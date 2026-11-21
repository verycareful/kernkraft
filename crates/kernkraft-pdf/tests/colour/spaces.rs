// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Colour spaces (8.6) and the conversions among device spaces (10.3).

use super::*;
use kernkraft_pdf::content;
use kernkraft_pdf::Limit;

fn parse(object: &[u8]) -> ColourSpace {
    let (_, space) = space_of(&[(60, object)]);
    space.unwrap()
}

fn rgb(space: &ColourSpace, colour: &[f64]) -> Rgb {
    space.to_rgb(colour).unwrap()
}

const D65: &str = "[0.9505 1 1.089]";
/// The columns of the sRGB matrix (IEC 61966-2-1): the XYZ of the red,
/// green and blue primaries.
const SRGB_MATRIX: &str = "[0.4124 0.2126 0.0193 0.3576 0.7152 0.1192 0.1805 0.0722 0.9505]";

// --- device spaces (10.3) ---

#[test]
fn device_names_read_as_the_device_spaces() {
    let document = document(&[]);
    for (text, count) in [("DeviceGray", 1), ("DeviceRGB", 3), ("DeviceCMYK", 4)] {
        let space = document.colour_space(&name(text)).unwrap();
        assert_eq!(space.components(), count, "{text}");
    }
    // The short names of Table 94.
    for (text, count) in [("G", 1), ("RGB", 3), ("CMYK", 4)] {
        assert_eq!(
            document.colour_space(&name(text)).unwrap().components(),
            count
        );
    }
    assert!(document.colour_space(&name("Bogus")).is_err());
    assert!(document
        .colour_space(&Object::new(Value::Integer(1)))
        .is_err());
}

/// 10.3.2: a gray level is an RGB colour with the three components equal.
#[test]
fn gray_gives_equal_components() {
    let space = ColourSpace::DeviceGray;
    assert_rgb(rgb(&space, &[0.25]), [0.25; 3], 0.0);
    assert_rgb(rgb(&space, &[1.5]), [1.0; 3], 0.0);
    assert_rgb(rgb(&space, &[-1.0]), [0.0; 3], 0.0);
}

/// 10.3.5: `red = 1 - min(1, cyan + black)`, and so for green and blue.
#[test]
fn cmyk_follows_the_conversion_of_the_format() {
    let space = ColourSpace::DeviceCmyk;
    assert_rgb(rgb(&space, &[0.0, 0.0, 0.0, 0.0]), [1.0, 1.0, 1.0], 0.0);
    assert_rgb(rgb(&space, &[1.0, 0.0, 0.0, 0.0]), [0.0, 1.0, 1.0], 0.0);
    assert_rgb(rgb(&space, &[0.0, 1.0, 0.0, 0.0]), [1.0, 0.0, 1.0], 0.0);
    assert_rgb(rgb(&space, &[0.0, 0.0, 1.0, 0.0]), [1.0, 1.0, 0.0], 0.0);
    assert_rgb(rgb(&space, &[0.0, 0.0, 0.0, 1.0]), [0.0, 0.0, 0.0], 0.0);
    // 1 - min(1, 0.25 + 0.5), 1 - min(1, 0.5 + 0.5), 1 - min(1, 0 + 0.5).
    assert_rgb(rgb(&space, &[0.25, 0.5, 0.0, 0.5]), [0.25, 0.0, 0.5], 1e-12);
    // The sum is capped at 1: 0.8 + 0.8 leaves no red; green is
    // 1 - min(1, 0 + 0.8) and blue the same.
    assert_rgb(rgb(&space, &[0.8, 0.0, 0.0, 0.8]), [0.0, 0.2, 0.2], 1e-12);
}

#[test]
fn device_rgb_is_clipped_and_nan_is_zero() {
    let space = ColourSpace::DeviceRgb;
    assert_rgb(rgb(&space, &[2.0, -1.0, 0.5]), [1.0, 0.0, 0.5], 0.0);
    assert_rgb(rgb(&space, &[f64::NAN, 0.0, 0.0]), [0.0; 3], 0.0);
    assert_eq!(rgb(&space, &[1.0, 0.5, 0.0]).to_bytes(), [255, 128, 0]);
}

#[test]
fn a_colour_of_the_wrong_length_is_refused() {
    match ColourSpace::DeviceRgb.to_rgb(&[0.5]) {
        Err(PdfError::ColourComponents {
            expected: 3,
            found: 1,
        }) => {}
        other => panic!("{other:?}"),
    }
}

// --- CIE-based spaces (8.6.5) ---

/// 8.6.5.2: with a gamma of 1 the gray level is the Y of the white point
/// scaled: white is white, and the level is linear light.
#[test]
fn cal_gray_scales_the_white_point() {
    let space = parse(format!("[/CalGray << /WhitePoint {D65} >>]").as_bytes());
    assert_eq!(space.components(), 1);
    assert_rgb(rgb(&space, &[1.0]), [1.0; 3], 2e-3);
    assert_rgb(rgb(&space, &[0.0]), [0.0; 3], 2e-3);
    let half = srgb_encode(0.5);
    assert_rgb(rgb(&space, &[0.5]), [half; 3], 2e-3);
    // With a gamma, A is raised to it.
    let space = parse(format!("[/CalGray << /WhitePoint {D65} /Gamma 2.2 >>]").as_bytes());
    let value = srgb_encode(0.5f64.powf(2.2));
    assert_rgb(rgb(&space, &[0.5]), [value; 3], 2e-3);
}

/// 8.6.5.3: the primaries of the matrix are the sRGB ones, so a colour
/// at one of them is that sRGB primary.
#[test]
fn cal_rgb_with_the_srgb_primaries_is_srgb() {
    let space =
        parse(format!("[/CalRGB << /WhitePoint {D65} /Matrix {SRGB_MATRIX} >>]").as_bytes());
    assert_eq!(space.components(), 3);
    assert_rgb(rgb(&space, &[1.0, 0.0, 0.0]), [1.0, 0.0, 0.0], 2e-3);
    assert_rgb(rgb(&space, &[0.0, 1.0, 0.0]), [0.0, 1.0, 0.0], 2e-3);
    assert_rgb(rgb(&space, &[0.0, 0.0, 1.0]), [0.0, 0.0, 1.0], 2e-3);
    assert_rgb(rgb(&space, &[1.0, 1.0, 1.0]), [1.0; 3], 2e-3);
    let half = srgb_encode(0.5);
    assert_rgb(rgb(&space, &[0.5, 0.5, 0.5]), [half; 3], 2e-3);
    // With gammas, each channel is raised to its own.
    let space = parse(
        format!("[/CalRGB << /WhitePoint {D65} /Matrix {SRGB_MATRIX} /Gamma [2 1 1] >>]")
            .as_bytes(),
    );
    assert_rgb(
        rgb(&space, &[0.5, 0.0, 0.0]),
        [srgb_encode(0.25), 0.0, 0.0],
        2e-3,
    );
}

/// A white point other than D65 is adapted to the display white: the
/// white of the space is white.
#[test]
fn the_white_of_a_cie_space_is_white() {
    let d50 = "[0.9642 1 0.8249]";
    let space = parse(
        format!("[/CalRGB << /WhitePoint {d50} /Matrix [0.4361 0.2225 0.0139 0.3851 0.7169 0.0971 0.1431 0.0606 0.7141] >>]")
            .as_bytes(),
    );
    assert_rgb(rgb(&space, &[1.0, 1.0, 1.0]), [1.0; 3], 5e-3);
}

/// 8.6.5.4: `L* = 100, a* = b* = 0` is the white point; `L* = 0` is black;
/// on the neutral axis the light is `((L + 16) / 116)^3` of the white.
#[test]
fn lab_follows_the_formulas_of_the_format() {
    let space = parse(format!("[/Lab << /WhitePoint {D65} >>]").as_bytes());
    assert_eq!(space.components(), 3);
    assert_rgb(rgb(&space, &[100.0, 0.0, 0.0]), [1.0; 3], 3e-3);
    assert_rgb(rgb(&space, &[0.0, 0.0, 0.0]), [0.0; 3], 3e-3);
    let y = ((50.0 + 16.0) / 116.0f64).powi(3);
    assert_rgb(rgb(&space, &[50.0, 0.0, 0.0]), [srgb_encode(y); 3], 3e-3);
    // Below the knee the function g is linear: L* = 4 is at m = 20/116 < 6/29.
    let m = 20.0 / 116.0;
    let y = 108.0 / 841.0 * (m - 4.0 / 29.0);
    assert_rgb(rgb(&space, &[4.0, 0.0, 0.0]), [srgb_encode(y); 3], 3e-3);
    // a* and b* beyond the Range are clipped to it.
    let narrow = parse(format!("[/Lab << /WhitePoint {D65} /Range [-10 10 -10 10] >>]").as_bytes());
    assert_rgb(
        rgb(&narrow, &[60.0, 90.0, -90.0]),
        {
            let c = rgb(&narrow, &[60.0, 10.0, -10.0]);
            [c.r, c.g, c.b]
        },
        0.0,
    );
}

/// Table 74: the colour a space starts with.
#[test]
fn initial_colours_follow_table_74() {
    assert_eq!(ColourSpace::DeviceGray.initial_colour(), [0.0]);
    assert_eq!(ColourSpace::DeviceRgb.initial_colour(), [0.0; 3]);
    assert_eq!(
        ColourSpace::DeviceCmyk.initial_colour(),
        [0.0, 0.0, 0.0, 1.0]
    );
    // 8.6.5.4: zero is outside a Range that does not include it, so the
    // nearest value of the range is the initial one.
    let lab = parse(format!("[/Lab << /WhitePoint {D65} /Range [5 20 -30 -10] >>]").as_bytes());
    assert_eq!(lab.initial_colour(), [0.0, 5.0, -10.0]);
}

#[test]
fn a_cie_space_with_a_bad_entry_is_refused() {
    for (body, key) in [
        ("[/CalGray << >>]", "WhitePoint"),
        ("[/CalGray << /WhitePoint [0 1 1] >>]", "WhitePoint"),
        ("[/CalGray << /WhitePoint [1 1] >>]", "WhitePoint"),
        ("[/CalGray << /WhitePoint [1 1 1] /Gamma 0 >>]", "Gamma"),
        (
            "[/CalGray << /WhitePoint [1 1 1] /BlackPoint [0 0] >>]",
            "BlackPoint",
        ),
        ("[/CalRGB << /WhitePoint [1 1 1] /Gamma [1 1] >>]", "Gamma"),
        (
            "[/CalRGB << /WhitePoint [1 1 1] /Matrix [1 0 0] >>]",
            "Matrix",
        ),
        (
            "[/Lab << /WhitePoint [1 1 1] /Range [10 -10 0 0] >>]",
            "Range",
        ),
        ("[/Lab << /WhitePoint [1 1 1] /Range [0 1 0] >>]", "Range"),
        ("[/Lab]", "ColorSpace"),
    ] {
        match space_of(&[(60, body.as_bytes())]).1 {
            Err(PdfError::InvalidEntry { key: found, .. }) => assert_eq!(found, key, "{body}"),
            other => panic!("{body}: {other:?}"),
        }
    }
}

// --- Indexed (8.6.6.3) ---

/// A table of two RGB entries: index 0 is red, 1 is blue.
#[test]
fn indexed_looks_the_colour_up_in_its_table() {
    let space = parse(b"[/Indexed /DeviceRGB 1 <FF0000 0000FF>]");
    assert_eq!(space.components(), 1);
    assert_rgb(rgb(&space, &[0.0]), [1.0, 0.0, 0.0], 0.0);
    assert_rgb(rgb(&space, &[1.0]), [0.0, 0.0, 1.0], 0.0);
    // The index is rounded, and clipped to 0 to hival.
    assert_rgb(rgb(&space, &[0.4]), [1.0, 0.0, 0.0], 0.0);
    assert_rgb(rgb(&space, &[0.6]), [0.0, 0.0, 1.0], 0.0);
    assert_rgb(rgb(&space, &[9.0]), [0.0, 0.0, 1.0], 0.0);
    assert_rgb(rgb(&space, &[-3.0]), [1.0, 0.0, 0.0], 0.0);
    assert_eq!(space.initial_colour(), [0.0]);
    // 8.9.5.2: the default decode of an image is 0 to 2^n - 1.
    assert_eq!(space.default_decode(4), [[0.0, 15.0]]);
    assert_eq!(space.default_decode(8), [[0.0, 255.0]]);
}

#[test]
fn an_indexed_table_may_be_a_stream_and_a_short_one_is_padded() {
    let table = stream("", &[0xFF, 0x00, 0x00, 0x00, 0xFF, 0x00]);
    let (_, space) = space_of(&[(60, b"[/I /DeviceRGB 1 61 0 R]"), (61, &table)]);
    let space = space.unwrap();
    assert_rgb(rgb(&space, &[1.0]), [0.0, 1.0, 0.0], 0.0);
    let (document, space) = space_of(&[(60, b"[/Indexed /DeviceRGB 1 <FF0000>]")]);
    let space = space.unwrap();
    assert_rgb(rgb(&space, &[0.0]), [1.0, 0.0, 0.0], 0.0);
    assert_rgb(rgb(&space, &[1.0]), [0.0, 0.0, 0.0], 0.0);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::IndexedTableShort).len(),
        1
    );
}

/// 8.6.6.3: a byte is scaled to the base space's range as an 8-bit image
/// sample is; for Lab that is 0 to 100 and the `Range`.
#[test]
fn indexed_over_lab_decodes_its_bytes_through_the_lab_ranges() {
    let space = parse(
        format!(
            "[/Indexed [/Lab << /WhitePoint {D65} /Range [-128 127 -128 127] >>] 0 <FF 80 80>]"
        )
        .as_bytes(),
    );
    // L = 100, a = -128 + 128/255 * 255 = 0, b = 0: the white point.
    assert_rgb(rgb(&space, &[0.0]), [1.0; 3], 3e-3);
}

#[test]
fn an_indexed_space_with_a_bad_entry_is_refused() {
    for body in [
        "[/Indexed /DeviceRGB 256 <00>]",
        "[/Indexed /DeviceRGB -1 <00>]",
        "[/Indexed /Pattern 0 <00>]",
        "[/Indexed [/Indexed /DeviceGray 0 <00>] 0 <00>]",
        "[/Indexed /DeviceRGB 0]",
        "[/Indexed /DeviceRGB 0 12]",
        "[/Indexed /Bogus 0 <00>]",
    ] {
        assert!(space_of(&[(60, body.as_bytes())]).1.is_err(), "{body}");
    }
}

// --- Separation and DeviceN (8.6.6.4, 8.6.6.5) ---

const TINT_TO_CMYK: &[u8] = b"<< /FunctionType 2 /Domain [0 1] /C0 [0 0 0 0] /C1 [0 1 1 0] /N 1 >>";

#[test]
fn separation_runs_its_tint_transform_into_the_alternate_space() {
    let (_, space) = space_of(&[
        (60, b"[/Separation /Spot /DeviceCMYK 61 0 R]"),
        (61, TINT_TO_CMYK),
    ]);
    let space = space.unwrap();
    assert_eq!(space.components(), 1);
    assert_eq!(space.initial_colour(), [1.0]);
    // Tint 1: cmyk (0 1 1 0) is red; tint 0: white; tint 0.5: (1 .5 .5).
    assert_rgb(rgb(&space, &[1.0]), [1.0, 0.0, 0.0], 1e-12);
    assert_rgb(rgb(&space, &[0.0]), [1.0, 1.0, 1.0], 1e-12);
    assert_rgb(rgb(&space, &[0.5]), [1.0, 0.5, 0.5], 1e-12);
    assert_eq!(space.default_decode(8), [[0.0, 1.0]]);
    let ColourSpace::Separation(tint) = &space else {
        panic!("not a Separation space")
    };
    assert_eq!(tint.names[0].as_bytes(), b"Spot");
    assert!(!tint.paints_nothing());
}

#[test]
fn the_colourant_none_paints_nothing() {
    let (_, space) = space_of(&[
        (60, b"[/Separation /None /DeviceCMYK 61 0 R]"),
        (61, TINT_TO_CMYK),
    ]);
    let ColourSpace::Separation(tint) = space.unwrap() else {
        panic!("not a Separation space")
    };
    assert!(tint.paints_nothing());
}

/// A calculator function of two inputs and three outputs: `{ 0 }` leaves
/// the inputs and a zero, which are red, green and blue.
#[test]
fn devicen_runs_its_tint_transform() {
    let function = stream(
        " /FunctionType 4 /Domain [0 1 0 1] /Range [0 1 0 1 0 1]",
        b"{ 0 }",
    );
    let (_, space) = space_of(&[
        (60, b"[/DeviceN [/A /B] /DeviceRGB 61 0 R]"),
        (61, &function),
    ]);
    let space = space.unwrap();
    assert_eq!(space.components(), 2);
    assert_eq!(space.initial_colour(), [1.0, 1.0]);
    assert_rgb(rgb(&space, &[0.2, 0.4]), [0.2, 0.4, 0.0], 1e-12);
    // The optional attributes dictionary does not matter to the colour.
    let (_, with_attributes) = space_of(&[
        (
            60,
            b"[/DeviceN [/A /B] /DeviceRGB 61 0 R << /Subtype /NChannel >>]",
        ),
        (61, &function),
    ]);
    assert_rgb(
        rgb(&with_attributes.unwrap(), &[0.2, 0.4]),
        [0.2, 0.4, 0.0],
        1e-12,
    );
}

#[test]
fn a_tint_transform_that_does_not_fit_its_space_is_refused() {
    let one_to_three = stream(
        " /FunctionType 4 /Domain [0 1] /Range [0 1 0 1 0 1]",
        b"{ dup dup }",
    );
    for (body, function) in [
        // Four outputs are needed for CMYK, the function gives three.
        (
            &b"[/Separation /S /DeviceCMYK 61 0 R]"[..],
            one_to_three.clone(),
        ),
        // Two colourants, a function of one input.
        (
            &b"[/DeviceN [/A /B] /DeviceRGB 61 0 R]"[..],
            one_to_three.clone(),
        ),
        // A Pattern space cannot be the alternate.
        (
            &b"[/Separation /S /Pattern 61 0 R]"[..],
            TINT_TO_CMYK.to_vec(),
        ),
    ] {
        assert!(
            space_of(&[(60, body), (61, &function)]).1.is_err(),
            "{}",
            String::from_utf8_lossy(body)
        );
    }
    // More colourants than DeviceN allows (Annex C, Table C.1).
    let names: String = (0..33).map(|i| format!("/N{i} ")).collect();
    let body = format!("[/DeviceN [{names}] /DeviceGray 61 0 R]");
    assert!(space_of(&[(60, body.as_bytes()), (61, &one_to_three)])
        .1
        .is_err());
}

/// A tint transform that fails at run time is reported with the colour.
#[test]
fn a_failing_tint_transform_is_an_error_not_a_guess() {
    let failing = stream(
        " /FunctionType 4 /Domain [0 1] /Range [0 1]",
        b"{ pop 1 0 div }",
    );
    let (_, space) = space_of(&[(60, b"[/Separation /S /DeviceGray 61 0 R]"), (61, &failing)]);
    match space.unwrap().to_rgb(&[0.5]) {
        Err(PdfError::Function(_)) => {}
        other => panic!("{other:?}"),
    }
}

// --- Pattern (8.6.6.2) ---

#[test]
fn the_pattern_space_may_name_an_underlying_space() {
    let (_, plain) = space_of(&[(60, b"[/Pattern]")]);
    let plain = plain.unwrap();
    assert_eq!(plain.components(), 0);
    assert!(plain.to_rgb(&[]).is_err());
    let (_, uncoloured) = space_of(&[(60, b"[/Pattern /DeviceRGB]")]);
    let uncoloured = uncoloured.unwrap();
    assert_eq!(uncoloured.components(), 3);
    assert_rgb(rgb(&uncoloured, &[1.0, 0.0, 0.0]), [1.0, 0.0, 0.0], 0.0);
    assert!(space_of(&[(60, b"[/Pattern [/Pattern]]")]).1.is_err());
    assert!(matches!(
        document(&[]).colour_space(&name("Pattern")).unwrap(),
        ColourSpace::Pattern(None)
    ));
}

// --- nesting, defaults, the interpreter's spaces ---

/// A space that contains itself is refused at the nesting limit.
#[test]
fn a_space_that_contains_itself_is_refused() {
    let (_, space) = space_of(&[(60, b"[/Indexed 60 0 R 1 <00>]")]);
    match space {
        Err(PdfError::LimitExceeded {
            limit: Limit::ColourSpaceNesting,
            ..
        }) => {}
        other => panic!("{other:?}"),
    }
}

/// 8.6.5.6: a resource dictionary's `DefaultRGB` stands for DeviceRGB. A
/// default with the wrong number of components is ignored and reported.
#[test]
fn default_colour_spaces_replace_the_device_spaces() {
    let document = document(&[
        (
            60,
            format!(
                "<< /ColorSpace << /DefaultRGB [/CalRGB << /WhitePoint {D65} \
                 /Matrix {SRGB_MATRIX} /Gamma [2 2 2] >>] /DefaultGray [/CalRGB << /WhitePoint {D65} >>] >> >>"
            )
            .as_bytes(),
        ),
    ]);
    let resources = document
        .required(id(60))
        .unwrap()
        .dictionary()
        .unwrap()
        .clone();
    let defaults = document.default_colour_spaces(&resources).unwrap();
    let rgb_space = ColourSpace::DeviceRgb;
    let replaced = defaults.apply(&rgb_space);
    assert!(matches!(replaced, ColourSpace::CalRgb(_)));
    // 0.5 with a gamma of 2 is 0.25 linear light.
    assert_rgb(
        rgb(replaced, &[0.5, 0.5, 0.5]),
        [srgb_encode(0.25); 3],
        3e-3,
    );
    // The gray default has three components: it is ignored.
    assert!(matches!(
        defaults.apply(&ColourSpace::DeviceGray),
        ColourSpace::DeviceGray
    ));
    assert!(matches!(
        defaults.apply(&ColourSpace::DeviceCmyk),
        ColourSpace::DeviceCmyk
    ));
    assert_eq!(
        of_kind(
            &document,
            &DiagnosticKind::IgnoredEntry {
                key: Name::new(b"DefaultGray".to_vec())
            }
        )
        .len(),
        1
    );
    // Only device spaces are replaced.
    let lab = ColourSpace::Pattern(None);
    assert!(matches!(defaults.apply(&lab), ColourSpace::Pattern(None)));
}

/// The colour a content stream's operators left current becomes a space
/// the renderer converts in.
#[test]
fn the_spaces_of_the_interpreter_become_colour_spaces() {
    let document = document(&[(60, b"[/Indexed /DeviceRGB 0 <FF0000>]")]);
    let resource = content::ColorSpace::Resource {
        name: Name::new(b"Cs1".to_vec()),
        object: reference(60),
    };
    assert_eq!(
        document.paint_colour_space(&resource).unwrap().components(),
        1
    );
    for (space, count) in [
        (content::ColorSpace::DeviceGray, 1),
        (content::ColorSpace::DeviceRGB, 3),
        (content::ColorSpace::DeviceCMYK, 4),
        (content::ColorSpace::Pattern, 0),
    ] {
        assert_eq!(
            document.paint_colour_space(&space).unwrap().components(),
            count
        );
    }
}

#[test]
fn many_colours_convert_in_one_call() {
    let space = ColourSpace::DeviceRgb;
    let mut out = Vec::new();
    space
        .to_rgb_many(&[1.0, 0.0, 0.0, 0.0, 0.0, 1.0], &mut out)
        .unwrap();
    assert_eq!(out.len(), 2);
    assert_rgb(out[1], [0.0, 0.0, 1.0], 0.0);
    assert!(space.to_rgb_many(&[1.0, 0.0], &mut out).is_err());
    assert!(ColourSpace::Pattern(None)
        .to_rgb_many(&[], &mut out)
        .is_err());
}
