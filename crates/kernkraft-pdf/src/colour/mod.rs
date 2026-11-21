// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Colour: ISO 32000-1:2008, 8.6 (colour spaces), 7.10 (functions) and
//! 10.3 (conversions among device colour spaces).
//!
//! [`Document::colour_space`] reads a colour space object (a name or an
//! array, as a `ColorSpace` resource, an image dictionary or an array
//! inside another space gives it) into a [`ColourSpace`]: the device
//! spaces, CalGray, CalRGB, Lab, ICCBased, Indexed, Separation, DeviceN
//! and Pattern. [`ColourSpace::to_rgb`] then turns a colour of the space
//! into the sRGB of the display.
//!
//! The conversions are these:
//!
//! - DeviceGray gives equal red, green and blue (10.3.2) and DeviceRGB is
//!   taken as sRGB.
//! - DeviceCMYK follows 10.3.5: `red = 1 - min(1, cyan + black)`, and so
//!   for green and blue. It is the format's own approximation; a CMYK
//!   colour in an ICCBased space goes through its profile instead.
//! - CalGray, CalRGB and Lab give CIE XYZ, which is adapted to the sRGB
//!   white (see `cie`).
//! - ICCBased goes through the profile with the relative colorimetric
//!   intent, 8.6.5.8's default (see `icc`). A profile that cannot be used
//!   is reported ([`DiagnosticKind::IccProfileUnusable`](crate::DiagnosticKind::IccProfileUnusable))
//!   and the space's
//!   `Alternate`, or the device space of its component count, stands in.
//! - Indexed looks the colour up in its table and converts it in its base
//!   space.
//! - Separation and DeviceN run their tint transform and convert the
//!   result in the alternate space.
//!
//! Colours are given as numbers in the ranges the space defines (8.6.4):
//! 0 to 1 for the device and CalRGB and CalGray spaces, `L*` 0 to 100 and
//! `a*` and `b*` in the `Range` for Lab, the profile's `Range` for
//! ICCBased, the table index for Indexed and the tint for Separation and
//! DeviceN.

mod cie;
mod function;
mod icc;
mod parse;

use std::sync::Arc;

pub use cie::{CalGray, CalRgb, Lab};
pub use function::{Function, FunctionFault, PostScript, MAX_FUNCTION_BYTES, MAX_SAMPLED_INPUTS};
pub use icc::IccTransform;

use crate::content;
use crate::doc::Document;
use crate::syntax::Name;
use crate::PdfError;

/// The most colourants a DeviceN space may have (Annex C, Table C.1).
pub const MAX_DEVICEN_COMPONENTS: usize = 32;
/// The most colour spaces one may have open at once, one inside another
/// (an Indexed base, an alternate space, an ICCBased alternate). Spaces
/// nest two or three deep in practice; this bounds a cycle of references.
pub const MAX_COLOUR_SPACE_DEPTH: usize = 8;

/// The most a colour of an indexed space can be indexed by: `hival`
/// is at most 255 (8.6.6.3).
const INDEXED_MAX_HIVAL: usize = 255;
/// A byte of an Indexed table as a fraction of its base space's range
/// (8.6.6.3: the byte is scaled to the range as an 8-bit image sample is).
const BYTE_MAX: f64 = 255.0;

/// A colour in the sRGB of the display, each component 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rgb {
    pub r: f64,
    pub g: f64,
    pub b: f64,
}

impl Rgb {
    /// The colour as 8-bit components, rounded to the nearest.
    pub fn to_bytes(self) -> [u8; 3] {
        [self.r, self.g, self.b].map(|c| (c.clamp(0.0, 1.0) * BYTE_MAX).round() as u8)
    }
}

/// An ICCBased space (8.6.5.5).
#[derive(Debug, Clone)]
pub struct IccBased {
    /// The `N` of the stream: 1, 3 or 4.
    pub components: usize,
    /// The `Range` (Table 66): `[min, max]` of each component.
    pub range: Vec<[f64; 2]>,
    /// The `Alternate`, or the device space of `components` components.
    pub alternate: Box<ColourSpace>,
    /// The profile's transform, absent when the profile is unusable.
    pub(crate) transform: Option<IccTransform>,
}

impl IccBased {
    /// The space of an ICC profile that is not in an ICCBased stream (a
    /// JPEG 2000 file's), of `components` channels with the default range.
    /// A profile that cannot be used leaves the device space of that many
    /// components in its place; [`Self::uses_profile`] says which.
    pub fn from_profile(profile: &[u8], components: usize) -> Option<ColourSpace> {
        let alternate = match components {
            1 => ColourSpace::DeviceGray,
            3 => ColourSpace::DeviceRgb,
            4 => ColourSpace::DeviceCmyk,
            _ => return None,
        };
        let range = vec![[0.0, 1.0]; components];
        Some(ColourSpace::IccBased(IccBased {
            components,
            transform: IccTransform::new(profile, components, range.clone()),
            range,
            alternate: Box::new(alternate),
        }))
    }

    /// Whether the profile is in use: false when [`Self::alternate`] stands
    /// in for it.
    pub fn uses_profile(&self) -> bool {
        self.transform.is_some()
    }
}

/// An Indexed space (8.6.6.3).
#[derive(Debug, Clone)]
pub struct Indexed {
    pub base: Box<ColourSpace>,
    /// The largest index.
    pub hival: usize,
    /// The table: `hival + 1` entries of the base space's component count
    /// in bytes, padded with zeros if the file's table was short.
    pub table: Arc<[u8]>,
}

/// A Separation or DeviceN space (8.6.6.4, 8.6.6.5): the colourants, the
/// space the tint transform gives colours in, and the transform.
#[derive(Debug, Clone)]
pub struct Tint {
    /// The colourant names: one for a Separation space.
    pub names: Vec<Name>,
    pub alternate: Box<ColourSpace>,
    pub transform: Arc<Function>,
}

impl Tint {
    /// True when every colourant is `None`: painting in the space leaves no
    /// marks (8.6.6.4).
    pub fn paints_nothing(&self) -> bool {
        self.names.iter().all(|name| name.as_bytes() == b"None")
    }
}

/// A colour space (8.6).
#[derive(Debug, Clone)]
pub enum ColourSpace {
    DeviceGray,
    DeviceRgb,
    DeviceCmyk,
    CalGray(CalGray),
    CalRgb(CalRgb),
    Lab(Lab),
    IccBased(IccBased),
    Indexed(Indexed),
    Separation(Tint),
    DeviceN(Tint),
    /// The Pattern space, with the space of an uncoloured pattern's
    /// colour when the space has one (8.6.6.2).
    Pattern(Option<Box<ColourSpace>>),
}

impl ColourSpace {
    /// How many components a colour of the space has: 0 for a Pattern
    /// space with no underlying space.
    pub fn components(&self) -> usize {
        match self {
            Self::DeviceGray | Self::CalGray(_) | Self::Indexed(_) => 1,
            Self::DeviceRgb | Self::CalRgb(_) | Self::Lab(_) => 3,
            Self::DeviceCmyk => 4,
            Self::IccBased(icc) => icc.components,
            Self::Separation(_) => 1,
            Self::DeviceN(tint) => tint.names.len(),
            Self::Pattern(under) => under.as_ref().map_or(0, |space| space.components()),
        }
    }

    /// The colour a space starts with when it is made current (8.6.5.1 to
    /// 8.6.6.5, Table 74): zero in each component (the nearest value in the
    /// range where zero is outside it), black in DeviceCMYK, and the full
    /// tint of a Separation or DeviceN space.
    pub fn initial_colour(&self) -> Vec<f64> {
        match self {
            Self::DeviceCmyk => vec![0.0, 0.0, 0.0, 1.0],
            Self::Lab(lab) => vec![
                0.0,
                0.0f64.clamp(lab.range[0], lab.range[1]),
                0.0f64.clamp(lab.range[2], lab.range[3]),
            ],
            Self::IccBased(icc) => icc
                .range
                .iter()
                .map(|[low, high]| 0.0f64.clamp(*low, *high))
                .collect(),
            Self::Separation(_) => vec![1.0],
            Self::DeviceN(tint) => vec![1.0; tint.names.len()],
            Self::Pattern(under) => under
                .as_ref()
                .map(|space| space.initial_colour())
                .unwrap_or_default(),
            other => vec![0.0; other.components()],
        }
    }

    /// The `Decode` array an image in this space has by default (8.9.5.2,
    /// Table 90) for samples of `bits_per_component` bits.
    pub fn default_decode(&self, bits_per_component: u32) -> Vec<[f64; 2]> {
        match self {
            Self::Lab(lab) => vec![
                [0.0, 100.0],
                [lab.range[0], lab.range[1]],
                [lab.range[2], lab.range[3]],
            ],
            Self::IccBased(icc) => icc.range.clone(),
            Self::Indexed(_) => {
                let largest = (1u64 << bits_per_component.min(32)) - 1;
                vec![[0.0, largest as f64]]
            }
            other => vec![[0.0, 1.0]; other.components()],
        }
    }

    /// The sRGB of `colour`, which has [`Self::components`] components.
    /// A colour in a Pattern space with no underlying space has none to
    /// give: [`PdfError::ColourComponents`]. A tint transform that fails
    /// is [`PdfError::Function`].
    pub fn to_rgb(&self, colour: &[f64]) -> Result<Rgb, PdfError> {
        self.check(colour.len())?;
        Ok(match self {
            Self::DeviceGray => gray(colour[0]),
            Self::DeviceRgb => rgb(colour[0], colour[1], colour[2]),
            Self::DeviceCmyk => cmyk(colour[0], colour[1], colour[2], colour[3]),
            Self::CalGray(space) => space.to_rgb(colour[0]),
            Self::CalRgb(space) => space.to_rgb([colour[0], colour[1], colour[2]]),
            Self::Lab(space) => space.to_rgb([colour[0], colour[1], colour[2]]),
            Self::IccBased(icc) => match &icc.transform {
                Some(transform) => {
                    let mut out = Vec::with_capacity(1);
                    match transform.convert(colour, &mut out) {
                        Some(()) if !out.is_empty() => Rgb {
                            r: out[0][0],
                            g: out[0][1],
                            b: out[0][2],
                        },
                        _ => icc.alternate.to_rgb(colour)?,
                    }
                }
                None => icc.alternate.to_rgb(colour)?,
            },
            Self::Indexed(space) => space.to_rgb(colour[0])?,
            Self::Separation(tint) | Self::DeviceN(tint) => {
                let mapped = tint.transform.call(colour)?;
                tint.alternate.to_rgb(&mapped)?
            }
            Self::Pattern(Some(under)) => under.to_rgb(colour)?,
            Self::Pattern(None) => {
                return Err(PdfError::Invalid(
                    "a Pattern space with no underlying space has no colour to convert".into(),
                ))
            }
        })
    }

    /// The sRGB of each colour of `colours`, which holds
    /// [`Self::components`] numbers for each, appended to `out`. For the
    /// colours of an image: an ICCBased profile converts them in one pass.
    pub fn to_rgb_many(&self, colours: &[f64], out: &mut Vec<Rgb>) -> Result<(), PdfError> {
        let components = self.components();
        if components == 0 || !colours.len().is_multiple_of(components) {
            return Err(PdfError::ColourComponents {
                expected: components,
                found: colours.len(),
            });
        }
        if let Self::IccBased(IccBased {
            transform: Some(transform),
            ..
        }) = self
        {
            let mut converted = Vec::with_capacity(colours.len() / components);
            if transform.convert(colours, &mut converted).is_some() {
                out.extend(converted.into_iter().map(|[r, g, b]| Rgb { r, g, b }));
                return Ok(());
            }
        }
        for colour in colours.chunks_exact(components) {
            out.push(self.to_rgb(colour)?);
        }
        Ok(())
    }

    fn check(&self, found: usize) -> Result<(), PdfError> {
        let expected = self.components();
        if found == expected {
            Ok(())
        } else {
            Err(PdfError::ColourComponents { expected, found })
        }
    }
}

impl Indexed {
    /// The sRGB of table entry `index`, rounded and clipped to 0 to
    /// `hival` (8.6.6.3).
    fn to_rgb(&self, index: f64) -> Result<Rgb, PdfError> {
        let index = if index.is_finite() {
            (index.round().max(0.0) as usize).min(self.hival)
        } else {
            0
        };
        let width = self.base.components();
        let entry = &self.table[index * width..(index + 1) * width];
        let decode = self.base.default_decode(8);
        let colour: Vec<f64> = entry
            .iter()
            .zip(&decode)
            .map(|(byte, [low, high])| low + f64::from(*byte) / BYTE_MAX * (high - low))
            .collect();
        self.base.to_rgb(&colour)
    }
}

fn unit(value: f64) -> f64 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn gray(value: f64) -> Rgb {
    let g = unit(value);
    Rgb { r: g, g, b: g }
}

fn rgb(r: f64, g: f64, b: f64) -> Rgb {
    Rgb {
        r: unit(r),
        g: unit(g),
        b: unit(b),
    }
}

/// 10.3.5.
fn cmyk(c: f64, m: f64, y: f64, k: f64) -> Rgb {
    let k = unit(k);
    Rgb {
        r: 1.0 - (unit(c) + k).min(1.0),
        g: 1.0 - (unit(m) + k).min(1.0),
        b: 1.0 - (unit(y) + k).min(1.0),
    }
}

/// The colour spaces a resource dictionary substitutes for the device
/// spaces (8.6.5.6): the `DefaultGray`, `DefaultRGB` and `DefaultCMYK`
/// entries of its `ColorSpace` dictionary.
#[derive(Debug, Clone, Default)]
pub struct DefaultSpaces {
    gray: Option<ColourSpace>,
    rgb: Option<ColourSpace>,
    cmyk: Option<ColourSpace>,
}

impl DefaultSpaces {
    /// The space to use in place of `space`: its default when it is a
    /// device space that has one, `space` itself otherwise.
    pub fn apply<'a>(&'a self, space: &'a ColourSpace) -> &'a ColourSpace {
        let replacement = match space {
            ColourSpace::DeviceGray => &self.gray,
            ColourSpace::DeviceRgb => &self.rgb,
            ColourSpace::DeviceCmyk => &self.cmyk,
            _ => return space,
        };
        replacement.as_ref().unwrap_or(space)
    }
}

impl Document {
    /// The colour space that a content stream's current colour is in
    /// (8.6.8): a device space, the Pattern space, or the `ColorSpace`
    /// resource it names.
    pub fn paint_colour_space(&self, space: &content::ColorSpace) -> Result<ColourSpace, PdfError> {
        match space {
            content::ColorSpace::DeviceGray => Ok(ColourSpace::DeviceGray),
            content::ColorSpace::DeviceRGB => Ok(ColourSpace::DeviceRgb),
            content::ColorSpace::DeviceCMYK => Ok(ColourSpace::DeviceCmyk),
            content::ColorSpace::Pattern => Ok(ColourSpace::Pattern(None)),
            content::ColorSpace::Resource { object, .. } => self.colour_space(object),
        }
    }
}
