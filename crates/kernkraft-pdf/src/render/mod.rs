// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page rendering: a raster [`Device`](crate::content::Device) for the
//! content interpreter. ISO 32000-1:2008, 8.5 (paths), 8.5.4 (clipping),
//! 9 (text) and 10.2 (the painting model's device space).
//!
//! [`render_page`] draws a page, or a region of it, at a scale, onto an
//! opaque white background, and gives straight RGBA8 pixels row by row from
//! the top. It draws paths (fills with both rules, strokes with width, caps,
//! joins, miter limit and dashes), clipping paths, and text in every
//! rendering mode with the glyphs of embedded Type 1, CFF and TrueType
//! programs, Type 3 glyph descriptions, and a face from the application's
//! font list for a font with no program. Colours of every space are
//! converted to sRGB; constant alpha applies.
//!
//! What a later step draws is reported, not drawn: see
//! [`DiagnosticKind::NotRendered`](crate::DiagnosticKind::NotRendered).
//!
//! Geometry follows the displayed page: the CropBox clipped to the MediaBox
//! with `Rotate` applied, `y` down, in the fractions [`Rect`] uses.

mod canvas;
mod colour;
mod device;
mod glyphs;
mod shape;

use kernkraft_font::FontList;
use kernkraft_fs::Progress;

use crate::content::{InterpretOptions, Interpreter, Matrix};
use crate::diagnostic::Diagnostics;
use crate::display::PageView;
use crate::doc::Document;
use crate::error::Limit;
use crate::syntax::Dictionary;
use crate::{PdfError, Rect};

use canvas::Canvas;
use colour::Colours;
use device::Painter;
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
}

impl Default for RenderOptions<'_> {
    fn default() -> Self {
        Self {
            scale: 1.0,
            region: None,
            fonts: None,
            max_pixels: DEFAULT_MAX_PIXELS,
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
    let canvas = Canvas::new(width as u32, height as u32)
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
    let mut painter = Painter::new(
        doc,
        resources.clone(),
        canvas,
        Colours::new(doc, &resources)?,
        Glyphs::new(doc, options.fonts),
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
