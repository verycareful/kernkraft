// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The device that gathers the glyphs of a page: ISO 32000-1:2008, 9.4.4
//! (the text rendering matrix), 9.8 (Table 122: `Ascent` and `Descent`)
//! and 14.7.4.4 (the `MCID` of a marked-content sequence).
//!
//! Every glyph is kept with the box it covers on the displayed page, the
//! direction the text it is in flows, its Unicode text and its place in
//! the structure tree. Nothing is laid out here: [`super::layout`] turns
//! the glyphs into lines and blocks.

use crate::content::{
    Context, Device, MarkedContent, OperatorSpan, PlacedGlyph, StreamId, TextRun,
};
use crate::display::PageView;
use crate::font::Font;
use crate::text::structure::{Slot, StructureIndex};

/// Glyph space units in one em (9.2.4: 1000 for a font whose
/// `FontMatrix` is the default, which a descriptor's `Ascent` and
/// `Descent` are given in).
const UNITS_PER_EM: f64 = 1000.0;

/// The ascent and descent of a font that has no descriptor or whose
/// descriptor gives none, in thousandths of an em. They cover the
/// ascenders and descenders of the Core 14 fonts (the AFM files give
/// Helvetica 718 and -207, Times-Roman 683 and -217, Courier 629 and
/// -157) so that a box drawn from them holds the glyphs it stands for.
const FALLBACK_ASCENT: f64 = 800.0;
const FALLBACK_DESCENT: f64 = -200.0;

/// The width of the box of a glyph of a vertical font, in ems: the em
/// square of a full-width character (9.7.4.3 centres the glyph on the
/// position vector).
const VERTICAL_GLYPH_WIDTH: f64 = 1.0;

/// One glyph of the page.
#[derive(Debug, Clone)]
pub(super) struct RawGlyph {
    pub text: String,
    /// The glyph's origin, on the displayed page in points.
    pub origin: (f64, f64),
    /// A unit vector along the direction the glyph's text flows.
    pub dir: (f64, f64),
    /// The font size on the displayed page, in points.
    pub size: f64,
    /// How far the glyph's box reaches along the flow from its origin, in
    /// points.
    pub extent: f64,
    /// The corners of the glyph's box: bottom left, bottom right, top
    /// right and top left of the glyph as it stands, on the displayed page
    /// in points.
    pub corners: [(f64, f64); 4],
    pub slot: Option<Slot>,
}

/// What the page's glyphs came to.
pub(super) struct Collected {
    pub glyphs: Vec<RawGlyph>,
    /// The first operation that showed a glyph with no Unicode text.
    pub unmapped: Option<OperatorSpan>,
    /// Whether the page showed more glyphs than the caller allows.
    pub overflow: Option<OperatorSpan>,
}

pub(super) struct Collector<'a> {
    view: &'a PageView,
    structure: &'a StructureIndex,
    max_glyphs: usize,
    /// The stream each open marked-content sequence began in, outermost
    /// first. A sequence a stream leaves open is dropped by the
    /// interpreter without an event, so only the entries below the depth
    /// the context reports are current.
    streams: Vec<StreamId>,
    out: Collected,
}

impl<'a> Collector<'a> {
    pub(super) fn new(
        view: &'a PageView,
        structure: &'a StructureIndex,
        max_glyphs: usize,
    ) -> Self {
        Self {
            view,
            structure,
            max_glyphs,
            streams: Vec::new(),
            out: Collected {
                glyphs: Vec::new(),
                unmapped: None,
                overflow: None,
            },
        }
    }

    pub(super) fn finish(self) -> Collected {
        self.out
    }

    /// The structure slot of the innermost marked-content sequence that
    /// carries an `MCID`.
    fn slot(&self, ctx: &Context<'_>) -> Option<Slot> {
        ctx.marked
            .iter()
            .enumerate()
            .rev()
            .find_map(|(depth, marked): (usize, &MarkedContent)| {
                let mcid = marked.mcid()?;
                let stream = self.streams.get(depth).copied().unwrap_or(ctx.span.stream);
                // The innermost sequence with an MCID decides, whether or
                // not the tree lists it.
                Some(self.structure.slot(stream, mcid))
            })
            .flatten()
    }

    fn glyph(
        &self,
        run: &TextRun,
        glyph: &PlacedGlyph,
        text: String,
        slot: Option<Slot>,
    ) -> Option<RawGlyph> {
        let matrix = glyph.matrix.then(self.view.matrix);
        let (ex, ey) = ((matrix.a, matrix.b), (matrix.c, matrix.d));
        let size = ey.0.hypot(ey.1);
        let em_x = ex.0.hypot(ex.1);
        let vertical = run.font.is_vertical();
        let flow_length = if vertical { size } else { em_x };
        if !(size.is_finite() && flow_length.is_finite()) || size == 0.0 || flow_length == 0.0 {
            return None;
        }
        let dir = if vertical {
            (-ey.0 / size, -ey.1 / size)
        } else {
            (ex.0 / em_x, ex.1 / em_x)
        };
        let (left, right, bottom, top, extent) = if vertical {
            let reach = if run.font_size == 0.0 {
                1.0
            } else {
                (glyph.advance.1 / run.font_size).abs()
            };
            (
                -VERTICAL_GLYPH_WIDTH / 2.0,
                VERTICAL_GLYPH_WIDTH / 2.0,
                -reach,
                0.0,
                reach * size,
            )
        } else {
            let (ascent, descent) = vertical_extent(&run.font);
            let width = glyph.width / UNITS_PER_EM;
            (0.0, width, descent, ascent, width * em_x)
        };
        let corners = [
            matrix.apply(left, bottom),
            matrix.apply(right, bottom),
            matrix.apply(right, top),
            matrix.apply(left, top),
        ];
        let reaches = corners.iter().all(|(x, y)| x.is_finite() && y.is_finite());
        if !reaches || self.off_page(&corners) {
            return None;
        }
        Some(RawGlyph {
            text,
            origin: matrix.apply(0.0, 0.0),
            dir,
            size,
            extent,
            corners,
            slot,
        })
    }

    /// Whether the box lies wholly outside the displayed page.
    fn off_page(&self, corners: &[(f64, f64); 4]) -> bool {
        let xs = corners.iter().map(|corner| corner.0);
        let ys = corners.iter().map(|corner| corner.1);
        let (left, right) = bounds(xs);
        let (top, bottom) = bounds(ys);
        right < 0.0 || bottom < 0.0 || left > self.view.width || top > self.view.height
    }
}

/// The least and greatest of `values`.
fn bounds(values: impl Iterator<Item = f64>) -> (f64, f64) {
    values.fold(
        (f64::INFINITY, f64::NEG_INFINITY),
        |(least, greatest), value| (least.min(value), greatest.max(value)),
    )
}

/// The ascent and descent of the glyphs of `font`, in ems.
fn vertical_extent(font: &Font) -> (f64, f64) {
    let descriptor = font.descriptor();
    let ascent = descriptor
        .map(|descriptor| descriptor.ascent.abs())
        .filter(|ascent| *ascent > 0.0)
        .unwrap_or(FALLBACK_ASCENT);
    // A descent is negative (Table 122); a positive one is a writer's
    // slip for the same distance below the baseline.
    let descent = descriptor
        .map(|descriptor| -descriptor.descent.abs())
        .filter(|descent| *descent < 0.0)
        .unwrap_or(FALLBACK_DESCENT);
    (ascent / UNITS_PER_EM, descent / UNITS_PER_EM)
}

impl Device for Collector<'_> {
    fn begin_marked(&mut self, ctx: &Context<'_>) {
        self.streams.truncate(ctx.marked.len().saturating_sub(1));
        self.streams.push(ctx.span.stream);
    }

    fn text(&mut self, ctx: &Context<'_>, run: &TextRun) {
        // Content optional content hides is not text of the page. Text in
        // the invisible render mode is, since that is how a scan carries
        // its recognised text.
        if !ctx.visible || self.out.overflow.is_some() {
            return;
        }
        let slot = self.slot(ctx);
        for glyph in &run.glyphs {
            let text = match glyph.unicode.as_deref() {
                // A character the font maps to no text at all.
                Some("") => continue,
                Some(text) => text.to_owned(),
                None => {
                    self.out.unmapped.get_or_insert(ctx.span);
                    char::REPLACEMENT_CHARACTER.to_string()
                }
            };
            if let Some(raw) = self.glyph(run, glyph, text, slot) {
                if self.out.glyphs.len() >= self.max_glyphs {
                    self.out.overflow = Some(ctx.span);
                    return;
                }
                self.out.glyphs.push(raw);
            }
        }
    }
}
