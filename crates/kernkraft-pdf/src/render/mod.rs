// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page rendering: a raster [`Device`](crate::content::Device) for the
//! content interpreter. ISO 32000-1:2008, 8.5 (paths), 8.5.4 (clipping),
//! 8.7 (patterns and shadings), 9 (text), 10.2 (the painting model's device
//! space), 11 (transparency) and 12.5.5 (annotation appearances).
//!
//! [`render_page`] draws a page, or a region of it, at a scale, onto an
//! opaque white background, and gives straight RGBA8 pixels row by row from
//! the top. It draws paths (fills with both rules, strokes with width, caps,
//! joins, miter limit and dashes), clipping paths, and text in every
//! rendering mode with the glyphs of embedded Type 1, CFF and TrueType
//! programs, Type 3 glyph descriptions, and a face from the application's
//! font list for a font with no program. Colours of every space are
//! converted to sRGB. Images are drawn at their place in the unit square of
//! user space, with their masks, smoothed when they are shrunk or when
//! `Interpolate` asks for it.
//!
//! Tiling patterns (coloured and uncoloured) and shading patterns paint
//! paths, strokes, text and stencil masks; the `sh` operator paints the
//! clip with a shading of any of the seven types (function-based, axial,
//! radial, free-form and lattice triangle meshes, Coons and tensor-product
//! patches). The 16 blend modes, the constant alpha and its `AIS` form, soft
//! masks (luminosity and alpha, with `BC` and `TR`) and transparency groups
//! (isolated or not, knockout or not) are drawn as 11 says. The page's
//! annotations are drawn over its content from their normal appearance
//! streams, honouring `Hidden`, `NoView`, `NoRotate`, `NoZoom` and optional
//! content, unless [`RenderOptions::annotations`] is off.
//!
//! What is not drawn is reported, not drawn: see
//! [`DiagnosticKind`](crate::DiagnosticKind) (`NotRendered`, and the
//! `Unusable` kinds for a shading, pattern, soft mask or appearance that is
//! damaged).
//!
//! Geometry follows the displayed page: the CropBox clipped to the MediaBox
//! with `Rotate` applied, `y` down, in the fractions [`Rect`] uses.

mod blend;
mod canvas;
mod colour;
mod device;
pub(crate) mod glyphs;
mod shading;
mod shape;

use kernkraft_font::FontList;
use kernkraft_fs::Progress;

use crate::content::{InterpretOptions, Interpreter, Matrix};
use crate::diagnostic::Diagnostics;
use crate::display::PageView;
use crate::doc::Document;
use crate::error::Limit;
use crate::image::ImageOptions;
use crate::syntax::Dictionary;
use crate::{PdfError, Rect};

use canvas::Canvas;
use colour::Colours;
use device::{PageSetup, Painter};
use glyphs::Glyphs;

/// The most pixels one render may produce when the caller does not say: the
/// 2D image side limit common GPUs report (16384) squared, which is also a
/// gibibyte of RGBA.
pub const DEFAULT_MAX_PIXELS: u64 = 16384 * 16384;

/// What the caller sets for a render.
#[derive(Debug, Clone)]
pub struct RenderOptions<'f> {
    /// Pixels to a point: 1 is 72 dpi.
    pub scale: f64,
    /// The part of the displayed page to draw, as fractions of it; `None`
    /// for the whole page. The picture is as large as the region at the
    /// scale, rounded up to whole pixels.
    pub region: Option<Rect>,
    /// The application's fonts, for text in a font with no program of its
    /// own. With none, such text is not drawn and is reported.
    pub fonts: Option<&'f FontList>,
    /// The most pixels the picture may have ([`Limit::RenderPixels`]).
    pub max_pixels: u64,
    /// The most pixels an image of the page may have; one with more is
    /// left out and reported ([`Limit::ImagePixels`]).
    pub max_image_pixels: u64,
    /// Whether the appearances of the page's annotations are drawn over its
    /// content (12.5.5). An annotation the file marks hidden or not
    /// viewable, or that optional content hides, is never drawn.
    pub annotations: bool,
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            scale: 1.0,
            region: None,
            fonts: None,
            max_pixels: DEFAULT_MAX_PIXELS,
            max_image_pixels: crate::image::DEFAULT_MAX_IMAGE_PIXELS,
            annotations: true,
        }
    }
}

/// A rendered page: `width * height` pixels of straight RGBA8, row by row
/// from the top, every alpha 255.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// The origin of the region on the displayed page, in points, and the size
/// in pixels of the picture [`render_page`] makes of it: the region at the
/// scale, rounded up to whole pixels.
fn target(view: &PageView, options: &RenderOptions<'_>) -> Result<(f64, f64, u32, u32), PdfError> {
    let region = options.region.unwrap_or(Rect {
        left: 0.0,
        top: 0.0,
        right: 1.0,
        bottom: 1.0,
    });
    let (origin_x, origin_y) = (region.left * view.width, region.top * view.height);
    let (width_pt, height_pt) = (region.width() * view.width, region.height() * view.height);
    let sizes = [options.scale, width_pt, height_pt];
    if sizes.iter().any(|n| !n.is_finite() || *n <= 0.0) {
        return Err(PdfError::Invalid(format!(
            "render scale {} over a region of {width_pt} by {height_pt} points",
            options.scale
        )));
    }
    let (width, height) = (
        (width_pt * options.scale).ceil(),
        (height_pt * options.scale).ceil(),
    );
    if width * height > options.max_pixels as f64
        || width > f64::from(u32::MAX)
        || height > f64::from(u32::MAX)
    {
        return Err(PdfError::LimitExceeded {
            offset: 0,
            limit: Limit::RenderPixels {
                max: options.max_pixels,
            },
        });
    }
    // Both are whole numbers within `u32`, checked above.
    Ok((origin_x, origin_y, width as u32, height as u32))
}

/// The width and height in pixels of the picture [`render_page`] makes of
/// page `number` with `options`, with the errors it gives for a region,
/// scale or size it refuses. A caller that must hold the pixels before
/// they are made (a buffer it allocates) asks this first.
pub fn raster_size(
    doc: &Document,
    number: u32,
    options: &RenderOptions<'_>,
) -> Result<(u32, u32), PdfError> {
    let page = doc.page(number)?;
    let mut diagnostics = Diagnostics::new();
    let view = PageView::of(&page, &mut diagnostics);
    doc.record_diagnostics(diagnostics);
    let (_, _, width, height) = target(&view, options)?;
    Ok((width, height))
}

/// Draws page `number` (counted from 1) of `doc`.
///
/// A region or scale that gives no pixels, or more than
/// [`RenderOptions::max_pixels`], is [`PdfError::LimitExceeded`] (or
/// [`PdfError::Invalid`] for a scale or region that is not a positive,
/// finite size). A page whose content is damaged draws as far as it can,
/// and what was left out is in the document's diagnostics.
pub fn render_page(
    doc: &Document,
    number: u32,
    options: &RenderOptions<'_>,
    progress: &dyn Progress,
) -> Result<Raster, PdfError> {
    let page = doc.page(number)?;
    let mut diagnostics = Diagnostics::new();
    let view = PageView::of(&page, &mut diagnostics);
    doc.record_diagnostics(diagnostics);

    let (origin_x, origin_y, width, height) = target(&view, options)?;
    let canvas = Canvas::new(width, height)
        .ok_or_else(|| PdfError::Invalid(format!("a render of {width} by {height} pixels")))?;

    let base_matrix = view
        .matrix
        .then(Matrix::scaling(options.scale, options.scale))
        .then(Matrix::translation(
            -origin_x * options.scale,
            -origin_y * options.scale,
        ));
    let resources = match &page.resources {
        Some(object) => doc
            .resolve(object)?
            .dictionary()
            .cloned()
            .unwrap_or_default(),
        None => Dictionary::new(),
    };
    let colours = Colours::new(doc, &resources)?;
    let image_options = ImageOptions {
        max_pixels: options.max_image_pixels,
        defaults: colours.defaults().clone(),
    };
    let mut painter = Painter::new(
        doc,
        PageSetup {
            resources: resources.clone(),
            base: base_matrix,
        },
        canvas,
        colours,
        Glyphs::new(doc, options.fonts),
        image_options,
        progress,
    );
    let mut interpreter = Interpreter::new(
        doc,
        InterpretOptions {
            base_matrix,
            ..InterpretOptions::default()
        },
    )?;
    let run = interpreter.run_page(&page, &mut painter, progress);
    if run.is_ok() && options.annotations && painter.error.is_none() {
        painter.paint_annotations(&page, base_matrix, options.scale);
    }
    doc.record_diagnostics(std::mem::take(&mut painter.diagnostics));
    run?;
    if let Some(error) = painter.error.take() {
        return Err(error);
    }
    let (width, height) = (painter.canvas.width(), painter.canvas.height());
    Ok(Raster {
        width,
        height,
        rgba: painter.canvas.into_rgba(),
    })
}
