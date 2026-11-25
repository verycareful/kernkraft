// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Drawing a page into a buffer the application allocated.
//!
//! The picture is straight RGBA8, row by row from the top, every alpha 255,
//! which an Android `Bitmap` of `ARGB_8888` takes as it is. Its width is
//! `width_px`; its height follows from the region's shape, and
//! `render_size` says what it is before the buffer is allocated.

use kernkraft_font::FontList;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::render::{raster_size, render_page, RenderOptions};
use kernkraft_pdf::{PdfError, Rect};
use serde::Deserialize;
use serde_json::json;

use crate::engine::Engine;
use crate::error::WireError;
use crate::fields::Fields;
use crate::jobs::JobId;
use crate::wire;

/// Bytes to a pixel.
const BYTES_PER_PIXEL: u64 = 4;
/// A picture is `width_px` wide, and the scale is chosen so that the width
/// in points times the scale rounds up to exactly that: half a pixel under,
/// well clear of the rounding of the product.
const HALF_PIXEL: f64 = 0.5;

/// A page, or part of it, drawn to a given width.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderRequest {
    pub handle: i64,
    pub page: u32,
    /// The part of the displayed page to draw, as fractions of it:
    /// `[left, top, right, bottom]`. The whole page when left out.
    #[serde(default)]
    pub region: Option<[f64; 4]>,
    /// The picture's width in pixels.
    pub width_px: u32,
    /// The job that can cancel `render` (see [`crate::jobs`]); `render_size`
    /// has no long work and takes none.
    #[serde(default)]
    pub job: Option<JobId>,
}

/// What a request asks of the page, checked against the document.
struct Target {
    document: Document,
    scale: f64,
    region: Option<Rect>,
    width: u32,
    height: u32,
    bytes: u64,
}

impl Target {
    fn options<'f>(&self, fonts: &'f FontList) -> RenderOptions<'f> {
        RenderOptions {
            scale: self.scale,
            region: self.region,
            fonts: Some(fonts),
            ..RenderOptions::default()
        }
    }
}

impl Engine {
    /// The size of the picture `render` makes: `width`, `height` and
    /// `bytes`.
    pub fn render_size(&self, request: RenderRequest) -> Result<Fields, WireError> {
        let target = self.target(&request)?;
        Ok(size_fields(target.width, target.height, target.bytes))
    }

    /// Draws the page into `buffer`, which must hold the whole picture.
    pub fn render(&self, request: RenderRequest, buffer: &mut [u8]) -> Result<Fields, WireError> {
        let running = self.jobs.begin(request.job, Some(request.handle))?;
        let target = self.target(&request)?;
        let given = u64::try_from(buffer.len()).unwrap_or(u64::MAX);
        if target.bytes > given {
            return Err(WireError::buffer_too_small(target.bytes, given));
        }
        let fonts = self.fonts();
        let options = target.options(&fonts);
        let raster = render_page(&target.document, request.page, &options, &running)?;
        if (raster.width, raster.height) != (target.width, target.height) {
            return Err(PdfError::Invalid(format!(
                "the picture is {} by {} pixels, not the {} by {} that was asked",
                raster.width, raster.height, target.width, target.height
            ))
            .into());
        }
        let length = usize::try_from(target.bytes)
            .map_err(|_| WireError::buffer_too_small(target.bytes, given))?;
        buffer[..length].copy_from_slice(&raster.rgba);
        Ok(size_fields(target.width, target.height, target.bytes))
    }

    fn target(&self, request: &RenderRequest) -> Result<Target, WireError> {
        if request.width_px == 0 {
            return Err(WireError::bad_request("width_px is 0"));
        }
        let (document, _) = self.snapshot(request.handle)?;
        let geometry = document.page_geometry(request.page)?;
        let region = request.region.map(wire::rect_from);
        let whole = Rect {
            left: 0.0,
            top: 0.0,
            right: 1.0,
            bottom: 1.0,
        };
        let width_pt = region.unwrap_or(whole).width() * geometry.width;
        let scale = (f64::from(request.width_px) - HALF_PIXEL) / width_pt;
        let fonts = self.fonts();
        let mut target = Target {
            document,
            scale,
            region,
            width: 0,
            height: 0,
            bytes: 0,
        };
        let (width, height) = raster_size(&target.document, request.page, &target.options(&fonts))?;
        if width != request.width_px {
            return Err(WireError::bad_request(format!(
                "a picture {} pixels wide of this region is {width} pixels wide",
                request.width_px
            )));
        }
        target.width = width;
        target.height = height;
        target.bytes = u64::from(width) * u64::from(height) * BYTES_PER_PIXEL;
        Ok(target)
    }
}

fn size_fields(width: u32, height: u32, bytes: u64) -> Fields {
    let mut fields = Fields::new();
    fields.insert("width".to_owned(), json!(width));
    fields.insert("height".to_owned(), json!(height));
    fields.insert("bytes".to_owned(), json!(bytes));
    fields
}
