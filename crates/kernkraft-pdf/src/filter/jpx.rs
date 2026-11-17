// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! JPXDecode: ISO 32000-1:2008, 7.4.9, over ISO/IEC 15444-1 (JPEG 2000
//! part 1: the JP2 file format and the codestream), which the
//! `hayro-jpeg2000` crate decodes.
//!
//! The stream is a whole JP2 file or a bare codestream. The crate resolves
//! the file's palette and channel definitions and converts its enumerated
//! colour space to RGB, grey or CMYK samples; each sample is scaled to
//! eight bits. The channel an opacity box marks is split from the colour
//! channels.

use hayro_jpeg2000::{ColorSpace, DecodeSettings, DecoderContext, Image};

use super::output::Output;
use super::Stop;
use crate::PdfError;

/// The colour space the file describes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JpxColour {
    Gray,
    Rgb,
    Cmyk,
    /// An ICC profile (ISO/IEC 15444-1, Annex I.5.3.3) with this many
    /// channels.
    Icc {
        profile: Vec<u8>,
        channels: u8,
    },
    /// A colour space the file does not name, with this many channels.
    Unknown {
        channels: u8,
    },
}

impl JpxColour {
    /// The colour channels, not counting opacity.
    pub fn channels(&self) -> u8 {
        match self {
            JpxColour::Gray => 1,
            JpxColour::Rgb => 3,
            JpxColour::Cmyk => 4,
            JpxColour::Icc { channels, .. } | JpxColour::Unknown { channels } => *channels,
        }
    }
}

/// What a JPX stream says about its image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct JpxInfo {
    pub width: u32,
    pub height: u32,
    pub colour: JpxColour,
    /// The opacity channel, one byte a pixel, when the file has one.
    pub opacity: Option<Vec<u8>>,
}

fn settings() -> DecodeSettings {
    DecodeSettings {
        resolve_palette_indices: true,
        strict: true,
        target_resolution: None,
    }
}

fn colour_of(space: &ColorSpace) -> JpxColour {
    match space {
        ColorSpace::Gray => JpxColour::Gray,
        ColorSpace::RGB => JpxColour::Rgb,
        ColorSpace::CMYK => JpxColour::Cmyk,
        ColorSpace::Icc {
            profile,
            num_channels,
        } => JpxColour::Icc {
            profile: profile.clone(),
            channels: *num_channels,
        },
        ColorSpace::Unknown { num_channels } => JpxColour::Unknown {
            channels: *num_channels,
        },
    }
}

/// Decodes `input` into `out`: the colour channels of each pixel together.
pub(super) fn decode(input: &[u8], out: &mut Output) -> Result<(Stop, JpxInfo), PdfError> {
    let Ok(image) = Image::new(input, &settings()) else {
        return Ok((
            Stop::Damaged,
            JpxInfo {
                width: 0,
                height: 0,
                colour: JpxColour::Unknown { channels: 0 },
                opacity: None,
            },
        ));
    };
    let colour = colour_of(image.color_space());
    let channels = usize::from(colour.channels());
    let total = channels + usize::from(image.has_alpha());
    let (width, height) = (image.width(), image.height());
    let pixels = (width as usize).saturating_mul(height as usize);
    out.check_room(pixels.saturating_mul(channels))?;

    let mut context = DecoderContext::default();
    let decoded = match image.decode(&mut context) {
        Ok(decoded) => decoded,
        Err(_) => {
            return Ok((
                Stop::Damaged,
                JpxInfo {
                    width,
                    height,
                    colour,
                    opacity: None,
                },
            ))
        }
    };
    let interleaved = decoded.data_u8();
    let mut opacity = None;
    if total == channels {
        out.extend(&interleaved)?;
    } else {
        let mut colours = Vec::with_capacity(pixels * channels);
        let mut alpha = Vec::with_capacity(pixels);
        for pixel in interleaved.chunks_exact(total) {
            colours.extend_from_slice(&pixel[..channels]);
            alpha.push(pixel[channels]);
        }
        out.extend(&colours)?;
        opacity = Some(alpha);
    }
    Ok((
        Stop::EndOfData,
        JpxInfo {
            width,
            height,
            colour,
            opacity,
        },
    ))
}
