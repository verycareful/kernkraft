// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Painting an image: ISO 32000-1:2008, 8.9.4 (the image coordinate system:
//! the image fills the unit square of user space, its first row on top),
//! 8.9.5.3 (interpolation) and 8.9.6.2 (an image mask painted in the
//! current fill colour).
//!
//! An image much larger than the pixels it covers is first brought down by
//! whole blocks (the mean of each), so that the sampling that follows reads
//! every source pixel's share and a photograph reduced to a thumbnail is
//! not a scatter of aliased points.

use std::sync::Arc;

use tiny_skia::Transform;

use crate::content::{Context, GraphicsState, ImageSource, DEFAULT_MAX_CONTENT_BYTES};
use crate::image::{box_down, DecodedImage};
use crate::PdfError;

use super::{Ink, Painter};

/// The bytes of decoded images one page keeps for reuse. A page that
/// repeats a logo hundreds of times decodes it once, and a page of many
/// large photographs does not hold them all at once.
const IMAGE_CACHE_BYTES: usize = 256 << 20;
const OPAQUE: u32 = 255;

impl Painter<'_, '_> {
    pub(super) fn paint_image(&mut self, ctx: &Context<'_>, image: &ImageSource) {
        if !ctx.visible {
            return;
        }
        if self.progress.is_cancelled() {
            self.error.get_or_insert(PdfError::Cancelled);
            return;
        }
        let state = ctx.state;
        let Some(decoded) = self.decoded_image(ctx, image) else {
            return;
        };
        match &*decoded {
            DecodedImage::Colour(bitmap) => {
                self.note_transparency(state);
                self.draw_straight(
                    &bitmap.rgba,
                    bitmap.width,
                    bitmap.height,
                    bitmap.interpolate,
                    state,
                    state.fill_alpha as f32,
                );
            }
            DecodedImage::Stencil(stencil) => {
                let Some(colour) = self.colour(state, Ink::Fill) else {
                    return;
                };
                let rgba: Vec<u8> = stencil
                    .coverage
                    .iter()
                    .flat_map(|coverage| [colour.rgb[0], colour.rgb[1], colour.rgb[2], *coverage])
                    .collect();
                self.draw_straight(
                    &rgba,
                    stencil.width,
                    stencil.height,
                    stencil.interpolate,
                    state,
                    colour.alpha,
                );
            }
        }
    }

    /// The decoded image, from the page's cache when it was drawn before.
    fn decoded_image(
        &mut self,
        ctx: &Context<'_>,
        image: &ImageSource,
    ) -> Option<Arc<DecodedImage>> {
        let result = match image {
            ImageSource::XObject { id, .. } => {
                if let Some(hit) = self.images.get(id) {
                    return Some(hit.clone());
                }
                self.doc
                    .image_xobject(*id, &self.image_options)
                    .map(|decoded| decoded.map(|decoded| (Some(*id), decoded)))
            }
            ImageSource::Inline(inline) => self.inline_data(ctx, inline.data).and_then(|data| {
                self.doc
                    .inline_image(&inline.dictionary, &data, &self.image_options)
                    .map(|decoded| decoded.map(|decoded| (None, decoded)))
            }),
        };
        match result {
            Ok(Some((id, decoded))) => {
                let decoded = Arc::new(decoded);
                if let Some(id) = id {
                    if self.image_bytes + decoded.bytes() > IMAGE_CACHE_BYTES {
                        self.images.clear();
                        self.image_bytes = 0;
                    }
                    self.image_bytes += decoded.bytes();
                    self.images.insert(id, decoded.clone());
                }
                Some(decoded)
            }
            Ok(None) => None,
            Err(error) => {
                self.error.get_or_insert(error);
                None
            }
        }
    }

    /// The bytes of an inline image's data, in the content stream the
    /// operation is in.
    fn inline_data(
        &mut self,
        ctx: &Context<'_>,
        span: crate::syntax::Span,
    ) -> Result<Vec<u8>, PdfError> {
        let id = ctx.span.stream;
        if self.inline_source.as_ref().map(|source| source.id) != Some(id) {
            self.inline_source = Some(self.doc.content_stream(id, DEFAULT_MAX_CONTENT_BYTES)?);
        }
        let data = self
            .inline_source
            .as_ref()
            .map_or(&[][..], |source| &source.data[..]);
        let (Ok(start), Ok(end)) = (usize::try_from(span.start), usize::try_from(span.end)) else {
            return Ok(Vec::new());
        };
        Ok(data.get(start..end).unwrap_or_default().to_vec())
    }

    /// Draws straight RGBA8 in the unit square of the current user space.
    fn draw_straight(
        &mut self,
        rgba: &[u8],
        width: u32,
        height: u32,
        interpolate: bool,
        state: &GraphicsState,
        opacity: f32,
    ) {
        let Some(placement) = super::shape::transform(&state.ctm) else {
            return;
        };
        // The unit square's sides in device pixels.
        let device_width = f64::from(placement.sx).hypot(f64::from(placement.ky));
        let device_height = f64::from(placement.kx).hypot(f64::from(placement.sy));
        if !(device_width.is_finite() && device_height.is_finite())
            || device_width <= 0.0
            || device_height <= 0.0
        {
            return;
        }
        let factor =
            |source: u32, device: f64| (f64::from(source) / device).floor().max(1.0) as u32;
        let (factor_x, factor_y) = (factor(width, device_width), factor(height, device_height));
        let reduced;
        let (rgba, width, height) = if factor_x > 1 || factor_y > 1 {
            let (pixels, w, h) = box_down(rgba, width, height, factor_x, factor_y);
            reduced = pixels;
            (&reduced[..], w, h)
        } else {
            (rgba, width, height)
        };
        // Smoothing when the image is shrunk or enlarged by less than a
        // whole pixel; an enlarged image shows its pixels unless it asks
        // to be interpolated.
        let shrunk = f64::from(width) >= device_width || f64::from(height) >= device_height;
        let pixel_to_unit =
            Transform::from_row(1.0 / width as f32, 0.0, 0.0, -1.0 / height as f32, 0.0, 1.0);
        let premultiplied = premultiply(rgba);
        self.canvas.draw_pixels(
            &premultiplied,
            width,
            height,
            placement.pre_concat(pixel_to_unit),
            opacity,
            shrunk || interpolate,
        );
    }
}

/// Straight RGBA8 as premultiplied, each colour channel rounded.
fn premultiply(rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rgba.len());
    for pixel in rgba.as_chunks::<4>().0 {
        let alpha = u32::from(pixel[3]);
        for channel in &pixel[..3] {
            out.push(((u32::from(*channel) * alpha + OPAQUE / 2) / OPAQUE) as u8);
        }
        out.push(pixel[3]);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn premultiplying_scales_the_colour_by_the_alpha() {
        assert_eq!(premultiply(&[200, 100, 50, 255]), [200, 100, 50, 255]);
        assert_eq!(premultiply(&[200, 100, 50, 0]), [0, 0, 0, 0]);
        // 255 * 128 / 255 = 128; 100 * 128 / 255 = 50.2.
        assert_eq!(premultiply(&[255, 100, 0, 128]), [128, 50, 0, 128]);
    }
}
