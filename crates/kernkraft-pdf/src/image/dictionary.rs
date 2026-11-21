// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The entries of an image dictionary: ISO 32000-1:2008, 8.9.5, Table 89
//! (image XObjects) and 8.9.7, Tables 92 and 93 (inline images, whose keys
//! the content interpreter has already written out in full).

use crate::diagnostic::ImageFault;
use crate::doc::Document;
use crate::syntax::{Dictionary, Object, Value};
use crate::PdfError;

/// What went wrong with an image: either a fault of the image, which is
/// reported and leaves the image out, or an error that ends the work.
#[derive(Debug)]
pub(super) enum Fail {
    Fault(ImageFault),
    Hard(PdfError),
    /// The image paints nothing (a Separation or DeviceN space of
    /// `None`): no mark and nothing to report.
    Nothing,
}

impl From<PdfError> for Fail {
    fn from(error: PdfError) -> Self {
        Fail::Hard(error)
    }
}

impl From<ImageFault> for Fail {
    fn from(fault: ImageFault) -> Self {
        Fail::Fault(fault)
    }
}

/// Table 89: the values `BitsPerComponent` may take.
const BITS_PER_COMPONENT: [u32; 5] = [1, 2, 4, 8, 16];

/// The entries of Table 89 the decoding of the samples needs.
#[derive(Debug)]
pub(super) struct ImageDict {
    pub width: u32,
    pub height: u32,
    /// `BitsPerComponent`, when the dictionary gives it.
    pub bits: Option<u32>,
    /// `ImageMask`.
    pub stencil: bool,
    /// `ColorSpace` as written.
    pub colour_space: Option<Object>,
    /// `Decode`: the numbers, in pairs.
    pub decode: Option<Vec<f64>>,
    pub interpolate: bool,
    /// `SMaskInData` of a JPEG 2000 image (Table 89): 0, 1 or 2.
    pub smask_in_data: i64,
}

impl ImageDict {
    /// Reads `dictionary`. `Width` and `Height` are required, and
    /// `BitsPerComponent` when the image is not a mask and has no codec
    /// that states it; that is for the caller to check.
    pub(super) fn read(doc: &Document, dictionary: &Dictionary) -> Result<Self, Fail> {
        let dimension = |key: &[u8]| -> Result<u32, Fail> {
            match number(doc, dictionary, key)? {
                Some(value)
                    if value >= 1.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX) =>
                {
                    Ok(value as u32)
                }
                _ => Err(ImageFault::Dimensions.into()),
            }
        };
        let bits = match number(doc, dictionary, b"BitsPerComponent")? {
            None => None,
            Some(value) => match BITS_PER_COMPONENT
                .iter()
                .find(|bits| f64::from(**bits) == value)
            {
                Some(bits) => Some(*bits),
                None => return Err(ImageFault::Dimensions.into()),
            },
        };
        let stencil = flag(doc, dictionary, b"ImageMask")?;
        let colour_space = match dictionary.get(b"ColorSpace") {
            Some(object) if !matches!(object.value(), Value::Null) => Some(object.clone()),
            _ => None,
        };
        let decode = match doc.get(dictionary, b"Decode")?.value() {
            Value::Array(items) => {
                let mut numbers = Vec::with_capacity(items.len());
                for item in items {
                    match number_of(doc, item)? {
                        Some(value) => numbers.push(value),
                        None => return Err(ImageFault::Data.into()),
                    }
                }
                Some(numbers)
            }
            _ => None,
        };
        Ok(Self {
            width: dimension(b"Width")?,
            height: dimension(b"Height")?,
            bits,
            stencil,
            colour_space,
            decode,
            interpolate: flag(doc, dictionary, b"Interpolate")?,
            smask_in_data: number(doc, dictionary, b"SMaskInData")?.map_or(0, |value| value as i64),
        })
    }
}

/// The finite number `object` is, or refers to.
pub(super) fn number_of(doc: &Document, object: &Object) -> Result<Option<f64>, PdfError> {
    Ok(match doc.resolve(object)?.value() {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    })
}

pub(super) fn number(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<f64>, PdfError> {
    Ok(match doc.get(dictionary, key)?.value() {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    })
}

fn flag(doc: &Document, dictionary: &Dictionary, key: &[u8]) -> Result<bool, PdfError> {
    Ok(matches!(
        doc.get(dictionary, key)?.value(),
        Value::Boolean(true)
    ))
}
