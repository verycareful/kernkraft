// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! DCTDecode, 7.4.8. The files in `testdata/image` are written from the
//! images stated beside each test, at the highest quality, so the decoded
//! samples are those pixels within the rounding of the transform.

use crate::{assert_too_large, decode, decode_capped, pipeline, Run};

const DCT: &str = "<</Filter /DCTDecode>>";
const NO_TRANSFORM: &str = "<</Filter /DCTDecode /DecodeParms <</ColorTransform 0>>>>";

const GRAY: &[u8] = include_bytes!("../../../../testdata/image/gray-60-200.jpg");
const RGB: &[u8] = include_bytes!("../../../../testdata/image/rgb-red-blue.jpg");
const CMYK: &[u8] = include_bytes!("../../../../testdata/image/cmyk-10-20-30-40.jpg");

/// The most a sample of the decoded file may differ from the value the
/// image was written with.
const TOLERANCE: i32 = 3;

fn assert_near(found: &[u8], expected: &[u8]) {
    assert_eq!(found.len(), expected.len());
    for (i, (found, expected)) in found.iter().zip(expected).enumerate() {
        assert!(
            (i32::from(*found) - i32::from(*expected)).abs() <= TOLERANCE,
            "sample {i}: {found} for {expected}"
        );
    }
}

/// 16 by 8 pixels, the left 8 columns `left` and the right 8 `right`, each
/// pixel's components together.
fn halves(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut samples = Vec::new();
    for _ in 0..8 {
        for _ in 0..8 {
            samples.extend_from_slice(left);
        }
        for _ in 0..8 {
            samples.extend_from_slice(right);
        }
    }
    samples
}

fn samples(run: Run) -> Vec<u8> {
    assert!(run.complete && run.kinds.is_empty());
    run.data
}

/// Grey, left 60 and right 200.
#[test]
fn one_component() {
    assert_near(&samples(decode(DCT, GRAY)), &halves(&[60], &[200]));
}

/// RGB (200, 50, 50) left and (50, 50, 200) right, stored as YCbCr as JFIF
/// files are; with three components the transform is the default.
#[test]
fn three_components_are_transformed_to_rgb_by_default() {
    assert_near(
        &samples(decode(DCT, RGB)),
        &halves(&[200, 50, 50], &[50, 50, 200]),
    );
}

/// `ColorTransform 0` leaves the stored YCbCr as it is. The stored values
/// are the forward equations of JFIF 1.02: for (200, 50, 50), Y is
/// 0.299 * 200 + 0.587 * 50 + 0.114 * 50 = 94.85, Cb is 128 - 0.168736 *
/// 200 - 0.331264 * 50 + 0.5 * 50 = 102.7 and Cr is 128 + 0.5 * 200 -
/// 0.418688 * 50 - 0.081312 * 50 = 203.0; for (50, 50, 200) they are
/// 0.299 * 50 + 0.587 * 50 + 0.114 * 200 = 67.1, 128 - 0.168736 * 50 -
/// 0.331264 * 50 + 0.5 * 200 = 203.0 and 128 + 0.5 * 50 - 0.418688 * 50 -
/// 0.081312 * 200 = 115.8.
#[test]
fn no_transform_gives_the_stored_components() {
    assert_near(
        &samples(decode(NO_TRANSFORM, RGB)),
        &halves(&[95, 103, 203], &[67, 203, 116]),
    );
}

/// CMYK (10, 20, 30, 40), written with an Adobe marker whose transform is
/// 0. Adobe stores CMYK inverted (TN #5116), and the filter does not undo
/// it: a PDF says so with a `Decode` array.
#[test]
fn four_components_are_stored_inverted_and_left_so() {
    let found = samples(decode(DCT, CMYK));
    assert_eq!(found.len(), 8 * 8 * 4);
    assert_near(&found[..4], &[245, 235, 225, 215]);
    assert_near(&found[found.len() - 4..], &[245, 235, 225, 215]);
}

/// The Adobe marker decides over the parameter: this file's marker says
/// transform 0, so `ColorTransform 1` changes nothing.
#[test]
fn the_adobe_marker_wins_over_the_parameter() {
    let dictionary = "<</Filter /DCTDecode /DecodeParms <</ColorTransform 1>>>>";
    assert_eq!(decode(dictionary, CMYK), decode(DCT, CMYK));
}

#[test]
fn a_cap_below_the_samples_refuses_the_file() {
    assert_too_large(decode_capped(DCT, GRAY, 127), 127);
    assert_eq!(decode_capped(DCT, GRAY, 128).unwrap().data.len(), 128);
}

#[test]
fn damaged_data_is_incomplete_not_a_panic() {
    let run = decode(DCT, &RGB[..20]);
    assert!(!run.complete);
    assert!(run.data.is_empty());
    let run = decode(DCT, b"not a jpeg");
    assert!(!run.complete);
    assert!(run.data.is_empty());
}

#[test]
fn a_truncated_scan_does_not_panic() {
    for cut in [RGB.len() / 2, RGB.len() - 30, RGB.len() - 3] {
        let _ = decode(DCT, &RGB[..cut]);
    }
}

#[test]
fn colour_transform_must_be_an_integer() {
    crate::assert_parameters(
        pipeline("<</Filter /DCTDecode /DecodeParms <</ColorTransform true>>>>"),
        "DCTDecode",
        "ColorTransform",
    );
}
