// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The samples of an image stream: its filters and codec run, the size and
//! components they report checked against the image dictionary (8.9.5,
//! Table 89; 7.4.6 to 7.4.9 for what each codec states itself).

use crate::diagnostic::{DiagnosticKind, ImageFault};
use crate::filter::{CodecInfo, Filter, Pipeline};
use crate::PdfError;

use super::dictionary::{Fail, ImageDict};
use super::samples::Packed;
use super::{Loader, Origin, Raw, STENCIL_BITS};

/// The bits to a sample of what DCTDecode and JPXDecode deliver.
const CODEC_BITS: u32 = 8;
/// The most colour channels a codec delivers (DCT and JPX: four).
const MOST_CHANNELS: usize = 4;

/// An error from the filters or the file, as the failure of the image or
/// as one that ends the work.
pub(super) fn soften(error: PdfError) -> Fail {
    match error {
        PdfError::DecodedTooLarge { .. } => ImageFault::TooLarge.into(),
        PdfError::Fs(_)
        | PdfError::Cancelled
        | PdfError::XrefMismatch { .. }
        | PdfError::Encrypted
        | PdfError::WrongPassword
        | PdfError::UnsupportedSecurityHandler { .. }
        | PdfError::UnsupportedEncryption { .. }
        | PdfError::UnsupportedCryptFilter { .. } => Fail::Hard(error),
        _ => ImageFault::Data.into(),
    }
}

impl Loader<'_> {
    /// Decodes the stream of `origin`. `components` is the number of
    /// components the colour space has, when it is known before the data
    /// is decoded; it sizes the cap on what the filters may produce.
    pub(super) fn read_samples(
        &mut self,
        origin: &Origin<'_>,
        dict: &ImageDict,
        components: Option<usize>,
    ) -> Result<Raw, Fail> {
        let pipeline = match origin {
            Origin::Object(_, stream) => self.doc.stream_pipeline(&stream.dictionary, true),
            Origin::Inline(dictionary, _) => Pipeline::from_dictionary(dictionary),
        }
        .map_err(soften)?;
        let codec = pipeline.filters().last();
        let (bits, expected_components, columns) = match codec {
            // The codec states its own components, which may be more than
            // the colour space has: the cap allows the most there are, and
            // the mismatch is found once the codec has said.
            Some(Filter::Dct(_) | Filter::Jpx) => (CODEC_BITS, MOST_CHANNELS, dict.width),
            Some(Filter::Ccitt(params)) => (STENCIL_BITS, 1, params.columns().max(dict.width)),
            Some(Filter::Jbig2 { .. }) => (STENCIL_BITS, 1, dict.width),
            _ if dict.stencil => (STENCIL_BITS, 1, dict.width),
            _ => (
                dict.bits.ok_or(ImageFault::Dimensions)?,
                components.unwrap_or(1),
                dict.width,
            ),
        };
        let cap = Packed::row_bytes(columns, expected_components, bits)
            .checked_mul(dict.height as usize)
            .ok_or(ImageFault::TooLarge)?;

        let (decoded, info) = match origin {
            Origin::Object(id, stream) => self
                .doc
                .decode_stream_codec(*id, stream, cap)
                .map_err(soften)?,
            Origin::Inline(_, data) => {
                let (decoded, info) = pipeline
                    .decode_image(data, cap, 0, &mut self.diagnostics)
                    .map_err(soften)?;
                (
                    crate::filter::Decoded {
                        data: std::borrow::Cow::Owned(decoded.data.into_owned()),
                        complete: decoded.complete,
                    },
                    info,
                )
            }
        };
        // Damage before the first sample leaves nothing to show: padding it
        // with zeros would draw a black picture.
        if !decoded.complete && decoded.data.is_empty() {
            return Err(ImageFault::Data.into());
        }
        let (mut width, mut height, mut channels, mut opacity, mut jpx_colour) =
            (dict.width, dict.height, expected_components, None, None);
        match info {
            Some(CodecInfo::Dct(info)) => {
                if info.components == 0 {
                    return Err(ImageFault::Data.into());
                }
                (width, height, channels) = (info.width, info.height, usize::from(info.components));
            }
            Some(CodecInfo::Jpx(info)) => {
                if info.width == 0 {
                    return Err(ImageFault::Data.into());
                }
                (width, height) = (info.width, info.height);
                channels = usize::from(info.colour.channels());
                opacity = info.opacity;
                jpx_colour = Some(info.colour);
            }
            Some(CodecInfo::Jbig2 {
                width: w,
                height: h,
            }) => {
                if w == 0 {
                    return Err(ImageFault::Data.into());
                }
                (width, height) = (w, h);
            }
            Some(CodecInfo::Ccitt { columns, rows }) => {
                let rows_known = if rows == 0 { dict.height } else { rows };
                (width, height) = (columns, rows_known.max(dict.height));
            }
            None => {}
        }
        if (width, height) != (dict.width, dict.height) {
            self.diagnostics.push(DiagnosticKind::ImageSizeDiffers, 0);
            self.check_pixels(width, height)?;
        }
        if let Some(components) = components {
            if channels != components && !matches!(codec, Some(Filter::Jpx)) {
                return Err(ImageFault::ColourSpace.into());
            }
        }
        let (packed, short) = Packed::new(width, height, channels, bits, decoded.data.into_owned());
        if short || !decoded.complete {
            self.diagnostics.push(DiagnosticKind::ImageDataShort, 0);
        }
        Ok(Raw {
            packed,
            opacity,
            jpx_colour,
        })
    }
}
