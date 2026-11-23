// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! ICCBased spaces (8.6.5.5). The profiles are written by `moxcms`: an
//! sRGB profile, which converts to sRGB as itself, and a gray profile
//! of gamma 2.2, whose conversion follows from its definition.

use super::*;
use moxcms::ColorProfile;

/// What an 8-bit pipeline can tell apart: the profile transform is done
/// in 14-bit and fixed-point arithmetic.
const TOLERANCE: f64 = 0.01;

fn srgb() -> Vec<u8> {
    ColorProfile::new_srgb().encode().unwrap()
}

fn gray_gamma_22() -> Vec<u8> {
    ColorProfile::new_gray_with_gamma(2.2).encode().unwrap()
}

fn icc_space(entries: &str, profile: &[u8]) -> (Document, Result<ColourSpace, PdfError>) {
    space_of(&[(60, b"[/ICCBased 61 0 R]"), (61, &stream(entries, profile))])
}

#[test]
fn an_srgb_profile_converts_to_itself() {
    let (document, space) = icc_space(" /N 3", &srgb());
    let space = space.unwrap();
    assert_eq!(space.components(), 3);
    let ColourSpace::IccBased(icc) = &space else {
        panic!("not an ICCBased space")
    };
    assert!(icc.uses_profile());
    for colour in [
        [0.2, 0.5, 0.8],
        [1.0, 0.0, 0.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0],
    ] {
        assert_rgb(rgb(&space, &colour), colour, TOLERANCE);
    }
    assert!(of_kind(&document, &DiagnosticKind::IccProfileUnusable).is_empty());
    assert_eq!(space.initial_colour(), [0.0; 3]);
    assert_eq!(space.default_decode(8), [[0.0, 1.0]; 3]);
}

fn rgb(space: &ColourSpace, colour: &[f64]) -> Rgb {
    space.to_rgb(colour).unwrap()
}

/// A gray profile of gamma 2.2 turns the value `v` into the light `v^2.2`,
/// which sRGB encodes.
#[test]
fn a_gray_profile_follows_its_gamma() {
    let (_, space) = icc_space(" /N 1", &gray_gamma_22());
    let space = space.unwrap();
    assert_eq!(space.components(), 1);
    for v in [0.0, 0.25, 0.5, 0.75, 1.0] {
        let expected = srgb_encode(f64::powf(v, 2.2));
        assert_rgb(rgb(&space, &[v]), [expected; 3], TOLERANCE);
    }
}

/// Table 66: a colour is given in the `Range` and the profile sees it as
/// a fraction of it.
#[test]
fn the_range_normalises_the_components() {
    let (_, space) = icc_space(" /N 3 /Range [0 2 0 2 0 2]", &srgb());
    let space = space.unwrap();
    assert_rgb(rgb(&space, &[1.0, 1.0, 1.0]), [0.5; 3], TOLERANCE);
    assert_rgb(rgb(&space, &[2.0, 0.0, 1.0]), [1.0, 0.0, 0.5], TOLERANCE);
    assert_eq!(space.default_decode(8), [[0.0, 2.0]; 3]);
    // Zero is in the range, so it is the initial colour; where it is not,
    // the nearest value is.
    assert_eq!(space.initial_colour(), [0.0; 3]);
    let (_, shifted) = icc_space(" /N 3 /Range [0.25 1 0 1 0 1]", &srgb());
    assert_eq!(shifted.unwrap().initial_colour(), [0.25, 0.0, 0.0]);
}

/// A profile whose data colour space has other components than `N`, and
/// a profile that is not one: the alternate stands in, and it is reported.
#[test]
fn an_unusable_profile_is_replaced_by_the_alternate_and_reported() {
    // An RGB profile for a stream that says N 1.
    let (document, space) = icc_space(" /N 1", &srgb());
    let space = space.unwrap();
    let ColourSpace::IccBased(icc) = &space else {
        panic!("not an ICCBased space")
    };
    assert!(!icc.uses_profile());
    assert_rgb(rgb(&space, &[0.4]), [0.4; 3], 0.0);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::IccProfileUnusable).len(),
        1
    );
    // Garbage with an Alternate.
    let (document, space) = icc_space(" /N 3 /Alternate /DeviceRGB", b"not an ICC profile");
    let space = space.unwrap();
    assert_rgb(rgb(&space, &[0.1, 0.2, 0.3]), [0.1, 0.2, 0.3], 0.0);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::IccProfileUnusable).len(),
        1
    );
    // The Alternate of a four-component space is used for its colours.
    let (document, space) = icc_space(" /N 4 /Alternate /DeviceCMYK", b"");
    let cmyk = document.device_cmyk().unwrap();
    let black = cmyk.to_rgb(&[0.0, 0.0, 0.0, 1.0]).unwrap();
    assert_rgb(
        rgb(&space.unwrap(), &[0.0, 0.0, 0.0, 1.0]),
        [black.r, black.g, black.b],
        0.0,
    );
}

/// Without an `Alternate` the device space of `N` components stands in.
#[test]
fn the_default_alternate_is_the_device_space_of_n_components() {
    let (document, space) = icc_space(" /N 4", b"x");
    let cyan = document
        .device_cmyk()
        .unwrap()
        .to_rgb(&[1.0, 0.0, 0.0, 0.0])
        .unwrap();
    assert_rgb(
        rgb(&space.unwrap(), &[1.0, 0.0, 0.0, 0.0]),
        [cyan.r, cyan.g, cyan.b],
        0.0,
    );
}

/// An `Alternate` with another number of components, or one that is not
/// a colour space, is left out and reported.
#[test]
fn an_alternate_that_does_not_fit_is_ignored_and_reported() {
    let (document, space) = icc_space(" /N 3 /Alternate /DeviceGray", b"x");
    assert!(space.is_ok());
    assert_eq!(
        of_kind(
            &document,
            &DiagnosticKind::IgnoredEntry {
                key: Name::new(b"Alternate".to_vec())
            }
        )
        .len(),
        1
    );
}

#[test]
fn an_icc_stream_with_a_bad_n_is_refused() {
    for n in ["", " /N 2", " /N 0", " /N 5", " /N 3.0"] {
        match icc_space(n, &srgb()).1 {
            Err(PdfError::InvalidEntry { key, .. }) => assert_eq!(key, "N", "{n:?}"),
            other => panic!("{n:?}: {other:?}"),
        }
    }
    // A space that names no stream.
    assert!(space_of(&[(60, b"[/ICCBased]")]).1.is_err());
    assert!(space_of(&[(60, b"[/ICCBased << /N 3 >>]")]).1.is_err());
}

/// An image's colours go through the profile in one pass and give what
/// one at a time does.
#[test]
fn many_colours_through_a_profile_match_one_at_a_time() {
    let (_, space) = icc_space(" /N 3", &srgb());
    let space = space.unwrap();
    let colours = [0.1, 0.2, 0.3, 0.9, 0.5, 0.0, 0.0, 1.0, 0.5];
    let mut out = Vec::new();
    space.to_rgb_many(&colours, &mut out).unwrap();
    assert_eq!(out.len(), 3);
    for (i, colour) in colours.chunks(3).enumerate() {
        assert_eq!(out[i], rgb(&space, colour));
    }
}
