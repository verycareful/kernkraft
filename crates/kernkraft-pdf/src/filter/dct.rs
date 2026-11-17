// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! DCTDecode: ISO 32000-1:2008, 7.4.8 (Table 13) and Adobe Technical Note
//! #5116, over ITU-T T.81 (baseline and progressive JPEG), whose entropy
//! decoding, inverse transform and upsampling the `zune-jpeg` crate does.
//!
//! The crate is asked for the components as the file stores them. The
//! colour transform of Table 13 is applied here, so that it follows the PDF
//! rule rather than the crate's own: the Adobe marker if the data has one,
//! else `ColorTransform`, else 1 for three components and 0 for the rest.
//!
//! * Three components with a transform: YCbCr to RGB, JFIF 1.02 "Conversion
//!   to and from RGB" (the CCIR 601-1 equations).
//! * Four components with a transform: YCCK to CMYK, the colour part as for
//!   three components followed by C = 255 - R, M = 255 - G, Y = 255 - B,
//!   with K as it is.
//!
//! Adobe writes CMYK inverted, and a PDF says so with a `Decode` array of
//! `[1 0 1 0 1 0 1 0]`; the filter does not undo it.

use zune_jpeg::zune_core::bytestream::ZCursor;
use zune_jpeg::zune_core::options::DecoderOptions;
use zune_jpeg::JpegDecoder;

use super::output::Output;
use super::Stop;
use crate::syntax::Dictionary;
use crate::PdfError;

const FILTER: &str = "DCTDecode";
const COLOR_TRANSFORM_KEY: &str = "ColorTransform";

/// T.81, B.2.2: a frame's width and height are 16-bit numbers.
const MAX_SIDE: usize = u16::MAX as usize;

/// Table 13: with three components the default `ColorTransform` is 1, with
/// any other number it is 0.
const RGB_COMPONENTS: u8 = 3;
const CMYK_COMPONENTS: u8 = 4;

/// JPEG markers (T.81, Table B.1) and the Adobe segment (TN #5116).
const MARKER_PREFIX: u8 = 0xFF;
const SOI: u8 = 0xD8;
const EOI: u8 = 0xD9;
const SOS: u8 = 0xDA;
const TEM: u8 = 0x01;
const RST_FIRST: u8 = 0xD0;
const RST_LAST: u8 = 0xD7;
const APP14: u8 = 0xEE;
const ADOBE_ID: &[u8] = b"Adobe";
/// The Adobe segment's data: the identifier, `DCTEncodeVersion`,
/// `APP14Flags0` and `APP14Flags1` (2 bytes each), then `ColorTransform`.
const ADOBE_TRANSFORM_OFFSET: usize = 11;
const LENGTH_BYTES: usize = 2;

/// What the stream says about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct DctInfo {
    pub width: u32,
    pub height: u32,
    pub components: u8,
}

/// The `ColorTransform` of the parameters, if there is one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct DctParams {
    color_transform: Option<bool>,
}

impl DctParams {
    pub(super) fn read(parms: Option<&Dictionary>) -> Result<Self, PdfError> {
        let value = match parms {
            Some(parms) => super::predictor::integer(parms, COLOR_TRANSFORM_KEY, FILTER)?,
            None => None,
        };
        Ok(Self {
            color_transform: value.map(|value| value != 0),
        })
    }
}

/// The `ColorTransform` byte of the Adobe marker segment, when the data has
/// one before its first scan.
fn adobe_transform(data: &[u8]) -> Option<u8> {
    if data.get(..2)? != [MARKER_PREFIX, SOI] {
        return None;
    }
    let mut at = 2;
    loop {
        // T.81, B.1.1.2: any marker may be preceded by fill bytes.
        while data.get(at..at + 2)? == [MARKER_PREFIX, MARKER_PREFIX] {
            at += 1;
        }
        if *data.get(at)? != MARKER_PREFIX {
            return None;
        }
        let marker = *data.get(at + 1)?;
        at += 2;
        if marker == SOI || marker == TEM || (RST_FIRST..=RST_LAST).contains(&marker) {
            continue;
        }
        if marker == SOS || marker == EOI {
            return None;
        }
        let length = usize::from(u16::from_be_bytes([*data.get(at)?, *data.get(at + 1)?]));
        if length < LENGTH_BYTES {
            return None;
        }
        if marker == APP14 {
            let body = data.get(at + LENGTH_BYTES..at + length)?;
            if body.starts_with(ADOBE_ID) {
                return body.get(ADOBE_TRANSFORM_OFFSET).copied();
            }
        }
        at += length;
    }
}

/// Whether the components are transformed, by the rule above.
fn transforms(adobe: Option<u8>, params: &DctParams, components: u8) -> bool {
    match (adobe, params.color_transform) {
        (Some(value), _) => value != 0,
        (None, Some(value)) => value,
        (None, None) => components == RGB_COMPONENTS,
    }
}

/// JFIF 1.02: the RGB of one YCbCr sample.
fn ycc_to_rgb(y: u8, cb: u8, cr: u8) -> [u8; 3] {
    let (y, cb, cr) = (f32::from(y), f32::from(cb) - 128.0, f32::from(cr) - 128.0);
    let channel = |value: f32| value.round().clamp(0.0, 255.0) as u8;
    [
        channel(y + 1.402 * cr),
        channel(y - 0.344_136 * cb - 0.714_136 * cr),
        channel(y + 1.772 * cb),
    ]
}

fn transform(samples: &mut [u8], components: u8) {
    let step = usize::from(components);
    for sample in samples.chunks_exact_mut(step) {
        let [r, g, b] = ycc_to_rgb(sample[0], sample[1], sample[2]);
        if components == CMYK_COMPONENTS {
            sample[0] = u8::MAX - r;
            sample[1] = u8::MAX - g;
            sample[2] = u8::MAX - b;
        } else {
            sample[0] = r;
            sample[1] = g;
            sample[2] = b;
        }
    }
}

fn options() -> DecoderOptions {
    DecoderOptions::default()
        .set_max_width(MAX_SIDE)
        .set_max_height(MAX_SIDE)
        .set_strict_mode(false)
}

/// Decodes `input` into `out`: `width * height * components` bytes, the
/// samples of each pixel together.
pub(super) fn decode(
    input: &[u8],
    params: &DctParams,
    out: &mut Output,
) -> Result<(Stop, DctInfo), PdfError> {
    let mut decoder = JpegDecoder::new_with_options(ZCursor::new(input), options());
    let unreadable = (Stop::Damaged, DctInfo::default());
    if decoder.decode_headers().is_err() {
        return Ok(unreadable);
    }
    let Some(frame) = decoder.info() else {
        return Ok(unreadable);
    };
    let info = DctInfo {
        width: u32::from(frame.width),
        height: u32::from(frame.height),
        components: frame.components,
    };
    // Ask for the stored components: the crate copies them as they are
    // when the colour space asked for is the one in the file.
    let Some(stored) = decoder.input_colorspace() else {
        return Ok(unreadable);
    };
    decoder.set_options(options().jpeg_set_out_colorspace(stored));
    let Some(size) = decoder.output_buffer_size() else {
        return Ok(unreadable);
    };
    out.check_room(size)?;
    let mut samples = vec![0_u8; size];
    let stop = match decoder.decode_into(&mut samples) {
        Ok(()) => Stop::EndOfData,
        Err(_) => Stop::Damaged,
    };
    if matches!(stop, Stop::EndOfData)
        && matches!(info.components, RGB_COMPONENTS | CMYK_COMPONENTS)
        && transforms(adobe_transform(input), params, info.components)
    {
        transform(&mut samples, info.components);
    }
    if matches!(stop, Stop::EndOfData) {
        out.extend(&samples)?;
    }
    Ok((stop, info))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn segment(marker: u8, body: &[u8]) -> Vec<u8> {
        let length = u16::try_from(body.len() + LENGTH_BYTES).unwrap();
        let mut bytes = vec![MARKER_PREFIX, marker];
        bytes.extend(length.to_be_bytes());
        bytes.extend(body);
        bytes
    }

    #[test]
    fn finds_the_adobe_transform_after_other_segments() {
        let mut data = vec![MARKER_PREFIX, SOI];
        data.extend(segment(0xE0, b"JFIF\0\x01\x02\0\0\x01\0\x01\0\0"));
        // A fill byte before the marker.
        data.push(MARKER_PREFIX);
        data.extend(segment(APP14, b"Adobe\0\x64\0\0\0\0\x02"));
        data.extend(segment(SOS, &[]));
        assert_eq!(adobe_transform(&data), Some(2));
    }

    #[test]
    fn no_adobe_segment_before_the_scan_is_none() {
        let mut data = vec![MARKER_PREFIX, SOI];
        data.extend(segment(0xE0, b"JFIF\0"));
        data.extend(segment(SOS, &[]));
        data.extend(segment(APP14, b"Adobe\0\x64\0\0\0\0\x01"));
        assert_eq!(adobe_transform(&data), None);
        assert_eq!(adobe_transform(b"not a jpeg"), None);
        assert_eq!(adobe_transform(&[MARKER_PREFIX, SOI, MARKER_PREFIX]), None);
    }

    #[test]
    fn the_rule_of_table_13() {
        let none = DctParams::default();
        let off = DctParams {
            color_transform: Some(false),
        };
        let on = DctParams {
            color_transform: Some(true),
        };
        // The Adobe marker wins over the parameter.
        assert!(!transforms(Some(0), &on, 3));
        assert!(transforms(Some(2), &off, 4));
        // The parameter wins over the default.
        assert!(!transforms(None, &off, 3));
        assert!(transforms(None, &on, 4));
        // The defaults.
        assert!(transforms(None, &none, 3));
        assert!(!transforms(None, &none, 4));
    }

    #[test]
    fn ycc_to_rgb_keeps_grey_and_moves_the_primaries() {
        assert_eq!(ycc_to_rgb(100, 128, 128), [100, 100, 100]);
        // Full Cr raises red and lowers green: 1.402 * 127 = 178.05.
        assert_eq!(ycc_to_rgb(100, 128, 255), [255, 9, 100]);
        // Full Cb raises blue: 1.772 * 127 = 225.04.
        assert_eq!(ycc_to_rgb(0, 255, 128), [0, 0, 225]);
    }

    #[test]
    fn a_four_component_transform_inverts_to_cmy_and_keeps_k() {
        let mut samples = [100, 128, 128, 77];
        transform(&mut samples, 4);
        assert_eq!(samples, [155, 155, 155, 77]);
    }
}
