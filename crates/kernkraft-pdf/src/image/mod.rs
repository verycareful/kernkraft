// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Images: ISO 32000-1:2008, 8.9 (image XObjects and inline images), 8.9.5
//! (image dictionaries, Table 89), 8.9.6 (masked images) and 11.6.5.3 (soft
//! masks and `Matte`).
//!
//! [`Document::image_xobject`] and [`Document::inline_image`] decode an
//! image to pixels that no longer depend on the document: straight RGBA8
//! for a picture, a coverage plane for an image mask. The codecs
//! ([`filter`](crate::filter)) give the samples, [`ColourSpace`] turns them
//! into sRGB, and the masks of 8.9.6 set the alpha:
//!
//! * a stencil mask (`ImageMask`) is a [`Stencil`]: 255 where the sample is
//!   the one the `Decode` array paints (0 by default), 0 elsewhere. The
//!   caller paints it in the current fill colour;
//! * colour key masking (`Mask` array) makes pixels whose samples all lie in
//!   the ranges transparent;
//! * explicit masking (`Mask` stream, an image mask) and soft masking
//!   (`SMask`) set the alpha from their own samples, at the larger of the
//!   two sizes, each plane taken at the nearest sample. `Matte` undoes the
//!   pre-blending of a soft-masked image;
//! * the opacity channel of a JPEG 2000 file is used when `SMaskInData`
//!   asks for it and no `SMask` overrides it.
//!
//! An image that cannot be used is not an error: it is
//! [`DiagnosticKind::ImageUnusable`](crate::DiagnosticKind::ImageUnusable)
//! on the document and `None` here, so that a page with one bad image still
//! draws. A mask that cannot be used leaves the picture without it, with
//! [`DiagnosticKind::ImageMaskUnusable`](crate::DiagnosticKind::ImageMaskUnusable).

mod dictionary;
mod read;
mod resample;
mod samples;

use std::sync::Arc;

use crate::colour::{ColourSpace, DefaultSpaces, IccBased};
use crate::diagnostic::{DiagnosticKind, Diagnostics, ImageFault};
use crate::doc::{Content, DocObject, DocStream, Document};
use crate::filter::JpxColour;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;
use dictionary::{number_of, Fail, ImageDict};
use samples::{decode_pairs, sample, to_rgba, ColourKey, Packed};

pub(crate) use resample::box_down;

/// The most pixels one image may have when the caller does not say: the 2D
/// image side limit common GPUs report (16384) squared, which is also a
/// gibibyte of RGBA.
pub const DEFAULT_MAX_IMAGE_PIXELS: u64 = 16384 * 16384;

const OPAQUE: u8 = u8::MAX;
/// An image mask has one bit to a sample (8.9.6.2).
const STENCIL_BITS: u32 = 1;

/// What the caller sets for the decoding of an image.
#[derive(Debug, Clone)]
pub struct ImageOptions {
    /// The most pixels an image, or one of its masks, may have
    /// ([`Limit::ImagePixels`]).
    pub max_pixels: u64,
    /// The default colour spaces (8.6.5.6) of the page: they stand in for a
    /// device space an image names.
    pub defaults: DefaultSpaces,
}

impl Default for ImageOptions {
    fn default() -> Self {
        Self {
            max_pixels: DEFAULT_MAX_IMAGE_PIXELS,
            defaults: DefaultSpaces::default(),
        }
    }
}

/// A picture: straight (not premultiplied) RGBA8, row by row from the top.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bitmap {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
    /// `Interpolate` (8.9.5.3): the reader should smooth the image when it
    /// is enlarged.
    pub interpolate: bool,
}

/// An image mask (8.9.6.2): where it paints, 255, and where it leaves the
/// page as it is, 0.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stencil {
    pub width: u32,
    pub height: u32,
    pub coverage: Vec<u8>,
    pub interpolate: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodedImage {
    Colour(Bitmap),
    Stencil(Stencil),
}

impl DecodedImage {
    pub fn width(&self) -> u32 {
        match self {
            DecodedImage::Colour(bitmap) => bitmap.width,
            DecodedImage::Stencil(stencil) => stencil.width,
        }
    }

    pub fn height(&self) -> u32 {
        match self {
            DecodedImage::Colour(bitmap) => bitmap.height,
            DecodedImage::Stencil(stencil) => stencil.height,
        }
    }

    /// The bytes the pixels take.
    pub fn bytes(&self) -> usize {
        match self {
            DecodedImage::Colour(bitmap) => bitmap.rgba.len(),
            DecodedImage::Stencil(stencil) => stencil.coverage.len(),
        }
    }
}

impl Document {
    /// Decodes the image XObject `id`. `None` when it is not an image, or
    /// cannot be used (the document's diagnostics say why).
    pub fn image_xobject(
        &self,
        id: ObjectId,
        options: &ImageOptions,
    ) -> Result<Option<DecodedImage>, PdfError> {
        let Some(loaded) = self.object(id)? else {
            return Ok(None);
        };
        let Content::Stream(stream) = &loaded.content else {
            return Ok(None);
        };
        let mut loader = Loader::new(self, options);
        loader.diagnostics.set_object(Some(id));
        let result = loader.load(&Origin::Object(id, stream));
        loader.finish();
        result
    }

    /// Decodes an inline image (8.9.7): its dictionary, with the keys and
    /// the colour space and filter names written out in full, and the data
    /// as the filters encoded it.
    pub fn inline_image(
        &self,
        dictionary: &Dictionary,
        data: &[u8],
        options: &ImageOptions,
    ) -> Result<Option<DecodedImage>, PdfError> {
        let mut loader = Loader::new(self, options);
        let result = loader.load(&Origin::Inline(dictionary, data));
        loader.finish();
        result
    }
}

/// Where an image's dictionary and encoded data are.
enum Origin<'a> {
    Object(ObjectId, &'a DocStream),
    Inline(&'a Dictionary, &'a [u8]),
}

impl Origin<'_> {
    fn dictionary(&self) -> &Dictionary {
        match self {
            Origin::Object(_, stream) => &stream.dictionary,
            Origin::Inline(dictionary, _) => dictionary,
        }
    }
}

/// The samples of one image as the codec and filters left them.
struct Raw {
    packed: Packed,
    /// The opacity channel of a JPEG 2000 file, one byte a pixel.
    opacity: Option<Vec<u8>>,
    /// The colour space a JPEG 2000 file names.
    jpx_colour: Option<JpxColour>,
}

/// A plane of one byte a pixel and the size it covers.
struct Plane {
    width: u32,
    height: u32,
    data: Vec<u8>,
    /// `Matte` of a soft mask, as sRGB.
    matte: Option<[u8; 3]>,
}

struct Loader<'d> {
    doc: &'d Document,
    options: &'d ImageOptions,
    diagnostics: Diagnostics,
}

impl<'d> Loader<'d> {
    fn new(doc: &'d Document, options: &'d ImageOptions) -> Self {
        Self {
            doc,
            options,
            diagnostics: Diagnostics::new(),
        }
    }

    fn finish(self) {
        self.doc.record_diagnostics(self.diagnostics);
    }

    fn fault(&mut self, fault: ImageFault) {
        self.diagnostics
            .push(DiagnosticKind::ImageUnusable { fault }, 0);
    }

    fn load(&mut self, origin: &Origin<'_>) -> Result<Option<DecodedImage>, PdfError> {
        match self.picture(origin) {
            Ok(image) => Ok(Some(image)),
            Err(Fail::Fault(fault)) => {
                self.fault(fault);
                Ok(None)
            }
            Err(Fail::Nothing) => Ok(None),
            Err(Fail::Hard(error)) => Err(error),
        }
    }

    fn check_pixels(&self, width: u32, height: u32) -> Result<(), Fail> {
        if u64::from(width) * u64::from(height) > self.options.max_pixels {
            return Err(ImageFault::TooLarge.into());
        }
        Ok(())
    }

    fn picture(&mut self, origin: &Origin<'_>) -> Result<DecodedImage, Fail> {
        let dict = ImageDict::read(self.doc, origin.dictionary())?;
        self.check_pixels(dict.width, dict.height)?;
        if dict.stencil {
            let plane = self.stencil_plane(origin, &dict)?;
            return Ok(DecodedImage::Stencil(Stencil {
                width: plane.width,
                height: plane.height,
                coverage: plane.data,
                interpolate: dict.interpolate,
            }));
        }
        let declared = match &dict.colour_space {
            Some(object) => Some(self.read_space(object)?),
            None => None,
        };
        let raw = self.read_samples(
            origin,
            &dict,
            declared.as_ref().map(ColourSpace::components),
        )?;
        let space = match declared {
            Some(space) => space,
            None => self.space_of_codec(&raw)?,
        };
        if let ColourSpace::Separation(tint) | ColourSpace::DeviceN(tint) = &space {
            if tint.paints_nothing() {
                return Err(Fail::Nothing);
            }
        }
        if space.components() != raw.packed.components
            || matches!(space, ColourSpace::Pattern(_))
            || (matches!(space, ColourSpace::Indexed(_)) && raw.packed.bits > 8)
        {
            return Err(ImageFault::ColourSpace.into());
        }
        let (pairs, ignored) = decode_pairs(dict.decode.as_deref(), &space, raw.packed.bits);
        if ignored {
            self.ignored(b"Decode");
        }
        let key = self.colour_key(origin.dictionary(), &space)?;
        let mut rgba = to_rgba(&raw.packed, &space, &pairs, key.as_ref())
            .map_err(|_| Fail::Fault(ImageFault::ColourSpace))?;
        let (mut width, mut height) = (raw.packed.width, raw.packed.height);

        let mask = self.mask_plane(origin.dictionary(), &space)?;
        let mask = match (mask, &raw.opacity, dict.smask_in_data) {
            (Some(plane), _, _) => Some(plane),
            (None, Some(opacity), 1 | 2) => {
                if dict.smask_in_data == 2 {
                    unpremultiply(&mut rgba, opacity);
                }
                Some(Plane {
                    width,
                    height,
                    data: opacity.clone(),
                    matte: None,
                })
            }
            (None, _, _) => None,
        };
        if let Some(plane) = mask {
            (rgba, width, height) = self.apply_plane(rgba, width, height, plane)?;
        }
        Ok(DecodedImage::Colour(Bitmap {
            width,
            height,
            rgba,
            interpolate: dict.interpolate,
        }))
    }

    fn ignored(&mut self, key: &[u8]) {
        self.diagnostics.push(
            DiagnosticKind::IgnoredEntry {
                key: Name::new(key.to_vec()),
            },
            0,
        );
    }

    /// The colour space `object` names or is, with the page's default
    /// spaces applied to a device space.
    fn read_space(&mut self, object: &Object) -> Result<ColourSpace, Fail> {
        match self.doc.colour_space(object) {
            Ok(space) => Ok(self.options.defaults.apply(&space).clone()),
            Err(PdfError::InvalidEntry { .. } | PdfError::Invalid(_)) => {
                Err(ImageFault::ColourSpace.into())
            }
            Err(error) => Err(error.into()),
        }
    }

    /// The space a JPEG 2000 file describes, for an image that names none.
    fn space_of_codec(&mut self, raw: &Raw) -> Result<ColourSpace, Fail> {
        let space = match &raw.jpx_colour {
            Some(JpxColour::Gray) => ColourSpace::DeviceGray,
            Some(JpxColour::Rgb) => ColourSpace::DeviceRgb,
            Some(JpxColour::Cmyk) => self.doc.device_cmyk()?,
            Some(JpxColour::Icc { profile, channels }) => {
                IccBased::from_profile(profile, usize::from(*channels), &self.doc.cmyk_profile()?)
                    .ok_or(ImageFault::ColourSpace)?
            }
            Some(JpxColour::Unknown { channels: 1 }) => ColourSpace::DeviceGray,
            Some(JpxColour::Unknown { channels: 3 }) => ColourSpace::DeviceRgb,
            Some(JpxColour::Unknown { channels: 4 }) => self.doc.device_cmyk()?,
            _ => return Err(ImageFault::ColourSpace.into()),
        };
        Ok(self.options.defaults.apply(&space).clone())
    }

    /// The `Mask` array (8.9.6.4) as ranges of raw samples, when the image
    /// has one.
    fn colour_key(
        &mut self,
        dictionary: &Dictionary,
        space: &ColourSpace,
    ) -> Result<Option<ColourKey>, Fail> {
        // `Mask` is an array (colour key masking) or a stream (explicit
        // masking): a reference to one is not followed as a value.
        let array = match dictionary.get(b"Mask").map(Object::value) {
            Some(Value::Array(_)) => dictionary.get(b"Mask").cloned(),
            Some(Value::Reference(id)) => match self.doc.object(*id)? {
                Some(loaded) => loaded.object().cloned(),
                None => None,
            },
            _ => None,
        };
        let Some(Value::Array(items)) = array.as_ref().map(Object::value) else {
            return Ok(None);
        };
        let mut numbers = Vec::with_capacity(items.len());
        for item in items {
            match number_of(self.doc, item)? {
                Some(value)
                    if value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX) =>
                {
                    numbers.push(value as u32);
                }
                _ => {
                    self.mask_fault(ImageFault::Data);
                    return Ok(None);
                }
            }
        }
        if numbers.len() != space.components() * 2 {
            self.mask_fault(ImageFault::Data);
            return Ok(None);
        }
        Ok(Some(numbers.as_chunks::<2>().0.to_vec()))
    }

    fn mask_fault(&mut self, fault: ImageFault) {
        self.diagnostics
            .push(DiagnosticKind::ImageMaskUnusable { fault }, 0);
    }

    /// The stream an entry refers to.
    fn stream_entry(
        &self,
        dictionary: &Dictionary,
        key: &[u8],
    ) -> Result<Option<(ObjectId, Arc<DocObject>)>, PdfError> {
        let Some(Value::Reference(id)) = dictionary.get(key).map(Object::value) else {
            return Ok(None);
        };
        Ok(self.doc.object(*id)?.map(|loaded| (*id, loaded)))
    }

    /// The plane of the soft mask or explicit mask the dictionary names, if
    /// it names one; `SMask` is used before `Mask`.
    fn mask_plane(
        &mut self,
        dictionary: &Dictionary,
        parent: &ColourSpace,
    ) -> Result<Option<Plane>, Fail> {
        for (key, soft) in [(&b"SMask"[..], true), (&b"Mask"[..], false)] {
            let Some((id, loaded)) = self.stream_entry(dictionary, key)? else {
                continue;
            };
            let Content::Stream(stream) = &loaded.content else {
                self.mask_fault(ImageFault::NotAnImage);
                return Ok(None);
            };
            let origin = Origin::Object(id, stream);
            let result = if soft {
                self.soft_plane(&origin, parent)
            } else {
                ImageDict::read(self.doc, origin.dictionary())
                    .and_then(|dict| self.check_pixels(dict.width, dict.height).map(|()| dict))
                    .and_then(|dict| {
                        if dict.stencil {
                            self.stencil_plane(&origin, &dict)
                        } else {
                            Err(ImageFault::NotAnImage.into())
                        }
                    })
            };
            return match result {
                Ok(plane) => Ok(Some(plane)),
                Err(Fail::Fault(fault)) => {
                    self.mask_fault(fault);
                    Ok(None)
                }
                Err(Fail::Nothing) => Ok(None),
                Err(hard) => Err(hard),
            };
        }
        Ok(None)
    }

    /// The coverage of an image mask: 255 where the sample is the one the
    /// `Decode` array paints.
    fn stencil_plane(&mut self, origin: &Origin<'_>, dict: &ImageDict) -> Result<Plane, Fail> {
        let raw = self.read_samples(origin, dict, Some(1))?;
        if raw.packed.components != 1 || raw.packed.bits != STENCIL_BITS {
            return Err(ImageFault::NotAnImage.into());
        }
        // Decode [0 1] paints where the sample is 0 and [1 0] where it is 1
        // (8.9.6.2).
        let paints = match dict.decode.as_deref() {
            None => 0,
            Some([a, b]) if (*a, *b) == (0.0, 1.0) => 0,
            Some([a, b]) if (*a, *b) == (1.0, 0.0) => 1,
            Some(_) => {
                self.ignored(b"Decode");
                0
            }
        };
        let mut data = Vec::with_capacity(raw.packed.width as usize * raw.packed.height as usize);
        for row in raw.packed.rows() {
            for x in 0..raw.packed.width as usize {
                data.push(if sample(row, x, STENCIL_BITS) == paints {
                    OPAQUE
                } else {
                    0
                });
            }
        }
        Ok(Plane {
            width: raw.packed.width,
            height: raw.packed.height,
            data,
            matte: None,
        })
    }

    /// The alpha of a soft mask (11.6.5.2): its samples, through its
    /// `Decode` array, as opacity.
    fn soft_plane(&mut self, origin: &Origin<'_>, parent: &ColourSpace) -> Result<Plane, Fail> {
        let dict = ImageDict::read(self.doc, origin.dictionary())?;
        self.check_pixels(dict.width, dict.height)?;
        if dict.stencil {
            return Err(ImageFault::NotAnImage.into());
        }
        let raw = self.read_samples(origin, &dict, Some(1))?;
        if raw.packed.components != 1 {
            return Err(ImageFault::ColourSpace.into());
        }
        let (pairs, ignored) = decode_pairs(
            dict.decode.as_deref(),
            &ColourSpace::DeviceGray,
            raw.packed.bits,
        );
        if ignored {
            self.ignored(b"Decode");
        }
        let max = f64::from(raw.packed.max_sample());
        let mut data = Vec::with_capacity(raw.packed.width as usize * raw.packed.height as usize);
        for row in raw.packed.rows() {
            for x in 0..raw.packed.width as usize {
                let value = pairs[0][0]
                    + f64::from(sample(row, x, raw.packed.bits)) * (pairs[0][1] - pairs[0][0])
                        / max;
                data.push((value.clamp(0.0, 1.0) * f64::from(OPAQUE)).round() as u8);
            }
        }
        let matte = self.matte(origin.dictionary(), parent)?;
        Ok(Plane {
            width: raw.packed.width,
            height: raw.packed.height,
            data,
            matte,
        })
    }

    /// `Matte` (Table 144): the colour the image was blended with, in the
    /// image's colour space, as sRGB.
    fn matte(
        &mut self,
        dictionary: &Dictionary,
        parent: &ColourSpace,
    ) -> Result<Option<[u8; 3]>, Fail> {
        let resolved = self.doc.get(dictionary, b"Matte")?;
        let Value::Array(items) = resolved.value() else {
            return Ok(None);
        };
        let mut colour = Vec::with_capacity(items.len());
        for item in items {
            match number_of(self.doc, item)? {
                Some(value) => colour.push(value),
                None => {
                    self.mask_fault(ImageFault::Data);
                    return Ok(None);
                }
            }
        }
        match parent.to_rgb(&colour) {
            Ok(rgb) => Ok(Some(rgb.to_bytes())),
            Err(_) => {
                self.mask_fault(ImageFault::ColourSpace);
                Ok(None)
            }
        }
    }

    /// Sets the alpha of `rgba` from `plane`, both brought to the larger of
    /// their sizes.
    fn apply_plane(
        &mut self,
        rgba: Vec<u8>,
        width: u32,
        height: u32,
        plane: Plane,
    ) -> Result<(Vec<u8>, u32, u32), Fail> {
        let (target_width, target_height) = (width.max(plane.width), height.max(plane.height));
        self.check_pixels(target_width, target_height)?;
        let mut rgba = resample::nearest(&rgba, width, height, 4, target_width, target_height);
        let alpha = resample::nearest(
            &plane.data,
            plane.width,
            plane.height,
            1,
            target_width,
            target_height,
        );
        for (pixel, alpha) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(&alpha) {
            pixel[3] = *alpha;
            if let (Some(matte), true) = (plane.matte, *alpha > 0) {
                for (channel, m) in pixel[..3].iter_mut().zip(matte) {
                    // 11.6.5.3: c = m + (c' - m) / alpha.
                    let m = f64::from(m);
                    let value =
                        m + (f64::from(*channel) - m) * f64::from(OPAQUE) / f64::from(*alpha);
                    *channel = value.round().clamp(0.0, f64::from(OPAQUE)) as u8;
                }
            }
        }
        Ok((rgba, target_width, target_height))
    }
}

/// A JPEG 2000 opacity channel that was multiplied into the colour
/// (`SMaskInData` 2): each channel is divided by it again.
fn unpremultiply(rgba: &mut [u8], opacity: &[u8]) {
    for (pixel, alpha) in rgba.as_chunks_mut::<4>().0.iter_mut().zip(opacity) {
        if *alpha > 0 && *alpha < OPAQUE {
            for channel in &mut pixel[..3] {
                let value = u32::from(*channel) * u32::from(OPAQUE) / u32::from(*alpha);
                *channel = value.min(u32::from(OPAQUE)) as u8;
            }
        }
    }
}
