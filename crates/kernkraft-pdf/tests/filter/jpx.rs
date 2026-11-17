// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! JPXDecode, 7.4.9. The files in `testdata/image` are 8 by 8 images
//! written losslessly (the reversible 5-3 transform), so the decoded
//! samples are exactly the pixels stated beside each test.

use crate::{assert_too_large, decode, decode_capped};

const JPX: &str = "<</Filter /JPXDecode>>";

const GRAY: &[u8] = include_bytes!("../../../../testdata/image/gray-60-200.jp2");
const RGB: &[u8] = include_bytes!("../../../../testdata/image/rgb-red-blue.jp2");
const RGBA: &[u8] = include_bytes!("../../../../testdata/image/rgba-red-blue-alpha.jp2");

/// 8 by 8 pixels, the left 4 columns `left` and the right 4 `right`.
fn halves(left: &[u8], right: &[u8]) -> Vec<u8> {
    let mut samples = Vec::new();
    for _ in 0..8 {
        for _ in 0..4 {
            samples.extend_from_slice(left);
        }
        for _ in 0..4 {
            samples.extend_from_slice(right);
        }
    }
    samples
}

/// Grey, left 60 and right 200.
#[test]
fn one_channel() {
    let run = decode(JPX, GRAY);
    assert!(run.complete && run.kinds.is_empty());
    assert_eq!(run.data, halves(&[60], &[200]));
}

/// RGB (200, 50, 50) left and (50, 50, 200) right.
#[test]
fn three_channels() {
    let run = decode(JPX, RGB);
    assert!(run.complete && run.kinds.is_empty());
    assert_eq!(run.data, halves(&[200, 50, 50], &[50, 50, 200]));
}

/// The same pixels with opacity 255 left and 100 right: the filter gives
/// the colour channels, and the opacity channel is the image layer's.
#[test]
fn an_opacity_channel_is_not_part_of_the_samples() {
    let run = decode(JPX, RGBA);
    assert!(run.complete);
    assert_eq!(run.data, halves(&[200, 50, 50], &[50, 50, 200]));
}

#[test]
fn a_cap_below_the_samples_refuses_the_file() {
    assert_too_large(decode_capped(JPX, RGB, 191), 191);
    assert_eq!(decode_capped(JPX, RGB, 192).unwrap().data.len(), 192);
}

#[test]
fn damaged_data_is_an_error_or_incomplete_never_a_panic() {
    for cut in [10, GRAY.len() / 2, GRAY.len() - 4] {
        let _ = decode_capped(JPX, &GRAY[..cut], 1 << 20);
    }
    let run = decode_capped(JPX, b"not a jpeg 2000 file", 1 << 20).unwrap();
    assert!(!run.complete);
    assert!(run.data.is_empty());
}
