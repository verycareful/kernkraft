// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The pixels and the clipping path: ISO 32000-1:2008, 8.5.3 (path-painting
//! operators, the fill rules of 8.5.3.3), 8.5.3.2 (stroke, Table 52's line
//! parameters) and 8.5.4 (clipping path operators).
//!
//! A [`Canvas`] owns an RGBA pixmap and the clip the next mark is cut by.
//! The clip is a coverage mask, so a clip with a curved edge is
//! anti-aliased like a fill is. It is part of the graphics state: `q` keeps
//! it and `Q` brings it back, which [`Canvas::save`] and [`Canvas::restore`]
//! do. A mask is shared between the states that hold it (it is never
//! changed in place), so a `q` costs a pointer, not a page of bytes.

use std::sync::Arc;

use tiny_skia::{
    FillRule as SkiaFillRule, LineCap as SkiaCap, LineJoin as SkiaJoin, Mask, Paint, Path, Pixmap,
    Stroke, StrokeDash, Transform,
};

use crate::content::{FillRule, LineCap, LineJoin};

/// A colour with its alpha: sRGB components 0 to 255, alpha 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Colour {
    pub rgb: [u8; 3],
    pub alpha: f32,
}

/// The line parameters of Table 52 that a stroke needs, with the dash
/// already in the form the rasteriser takes.
#[derive(Debug, Clone)]
pub(super) struct StrokeStyle {
    pub width: f32,
    pub cap: LineCap,
    pub join: LineJoin,
    pub miter_limit: f32,
    /// The dash lengths and phase, `None` for a solid line.
    pub dash: Option<(Vec<f32>, f32)>,
}

/// The page being painted.
pub(super) struct Canvas {
    pixmap: Pixmap,
    clip: Option<Arc<Mask>>,
    saved: Vec<Option<Arc<Mask>>>,
}

fn rule(rule: FillRule) -> SkiaFillRule {
    match rule {
        FillRule::NonZeroWinding => SkiaFillRule::Winding,
        FillRule::EvenOdd => SkiaFillRule::EvenOdd,
    }
}

impl Canvas {
    /// A white, opaque canvas. `None` when a side is 0 or past what the
    /// rasteriser numbers.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let mut pixmap = Pixmap::new(width, height)?;
        pixmap.fill(tiny_skia::Color::WHITE);
        Some(Self {
            pixmap,
            clip: None,
            saved: Vec::new(),
        })
    }

    pub fn width(&self) -> u32 {
        self.pixmap.width()
    }

    pub fn height(&self) -> u32 {
        self.pixmap.height()
    }

    /// `q`.
    pub fn save(&mut self) {
        self.saved.push(self.clip.clone());
    }

    /// `Q`. A `Q` with nothing saved leaves the clip as it is.
    pub fn restore(&mut self) {
        if let Some(clip) = self.saved.pop() {
            self.clip = clip;
        }
    }

    /// How many states are saved.
    pub fn depth(&self) -> usize {
        self.saved.len()
    }

    /// Restores to `depth` saved states, for the end of a form or glyph
    /// whose `q` and `Q` did not balance.
    pub fn restore_to(&mut self, depth: usize) {
        while self.saved.len() > depth {
            self.restore();
        }
    }

    /// Cuts the clip down to the inside of `path` (given in user space,
    /// moved to pixels by `transform`).
    pub fn clip_path(&mut self, path: &Path, fill: FillRule, transform: Transform) {
        let mask = match &self.clip {
            None => {
                let Some(mut mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) else {
                    return;
                };
                mask.fill_path(path, rule(fill), true, transform);
                mask
            }
            Some(current) => {
                let mut mask = Mask::clone(current);
                mask.intersect_path(path, rule(fill), true, transform);
                mask
            }
        };
        self.clip = Some(Arc::new(mask));
    }

    /// Cuts the clip down to nothing: everything after it is clipped away.
    pub fn clip_everything(&mut self) {
        if let Some(mask) = Mask::new(self.pixmap.width(), self.pixmap.height()) {
            self.clip = Some(Arc::new(mask));
        }
    }

    pub fn fill_path(&mut self, path: &Path, fill: FillRule, transform: Transform, colour: Colour) {
        let paint = paint_of(colour);
        self.pixmap
            .fill_path(path, &paint, rule(fill), transform, self.clip.as_deref());
    }

    pub fn stroke_path(
        &mut self,
        path: &Path,
        style: &StrokeStyle,
        transform: Transform,
        colour: Colour,
    ) {
        let paint = paint_of(colour);
        let mut stroke = Stroke {
            width: style.width,
            miter_limit: style.miter_limit,
            line_cap: match style.cap {
                LineCap::Butt => SkiaCap::Butt,
                LineCap::Round => SkiaCap::Round,
                LineCap::Square => SkiaCap::Square,
            },
            line_join: match style.join {
                LineJoin::Miter => SkiaJoin::Miter,
                LineJoin::Round => SkiaJoin::Round,
                LineJoin::Bevel => SkiaJoin::Bevel,
            },
            dash: None,
        };
        if let Some((array, phase)) = &style.dash {
            stroke.dash = StrokeDash::new(array.clone(), *phase);
        }
        self.pixmap
            .stroke_path(path, &paint, &stroke, transform, self.clip.as_deref());
    }

    /// The pixels as straight (not premultiplied) RGBA8, row by row from
    /// the top. The canvas starts opaque white and is only ever painted
    /// onto, so every alpha is 255 and premultiplied equals straight.
    pub fn into_rgba(self) -> Vec<u8> {
        self.pixmap.take()
    }
}

fn paint_of(colour: Colour) -> Paint<'static> {
    let mut paint = Paint::default();
    let [r, g, b] = colour.rgb;
    let channel = |value: u8| f32::from(value) / 255.0;
    if let Some(colour) = tiny_skia::Color::from_rgba(
        channel(r),
        channel(g),
        channel(b),
        colour.alpha.clamp(0.0, 1.0),
    ) {
        paint.set_color(colour);
    }
    paint.anti_alias = true;
    paint
}
