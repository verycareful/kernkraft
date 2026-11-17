// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The [`Device`] that paints: ISO 32000-1:2008, 8.5.3 (painting paths),
//! 8.5.4 (clipping), 9.3.6 and 9.4 (text rendering modes and showing text)
//! and 9.6.5 (Type 3 glyph descriptions).
//!
//! Colours reach the device as the stream wrote them; they are turned into
//! sRGB here, one paint at a time. What a later version of the renderer
//! draws (shadings, patterns, blend modes, soft masks) is reported
//! with [`DiagnosticKind::NotRendered`] once for each kind on a page, and
//! the mark is left out.

mod images;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kernkraft_fs::Progress;
use tiny_skia::{Path as Path2, PathBuilder, Transform};

use crate::content::ContentStream;
use crate::content::{
    Context, Device, FillRule, FormInfo, GraphicsState, ImageSource, InterpretOptions, Interpreter,
    Matrix, Paint, Path, PathPaint, PlacedGlyph, RenderMode, SoftMask, TextRun,
};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::error::Limit;
use crate::font::FontKind;
use crate::image::{DecodedImage, ImageOptions};
use crate::render::canvas::{Canvas, Colour, StrokeStyle};
use crate::render::colour::{Colours, Resolved};
use crate::render::glyphs::Glyphs;
use crate::render::shape;
use crate::syntax::{Dictionary, Name, Object, ObjectId};

/// The blend modes that paint as Normal does (11.3.5: `Compatible` is
/// `Normal`).
const NORMAL_BLEND_MODES: [&[u8]; 2] = [b"Normal", b"Compatible"];

/// How many Type 3 glyph descriptions may be open one inside another (a
/// glyph that shows text in a Type 3 font).
const MAX_GLYPH_NESTING: usize = 4;

/// What a mark is painted with.
#[derive(Clone, Copy)]
enum Ink {
    Fill,
    Stroke,
}

/// A device that draws on a [`Canvas`].
pub(super) struct Painter<'d, 'p> {
    pub canvas: Canvas,
    pub diagnostics: Diagnostics,
    doc: &'d Document,
    /// The page's resources, which a Type 3 font with none of its own uses.
    page_resources: Dictionary,
    colours: Colours<'d>,
    glyphs: Glyphs<'d>,
    /// The kinds of unrendered content already reported.
    noted: HashSet<&'static str>,
    /// The glyph outlines of the text object in progress, when a rendering
    /// mode adds them to the clip (9.3.6).
    text_clip: Option<PathBuilder>,
    /// The interpreter that runs Type 3 glyph descriptions; taken out while
    /// it runs, so a description that shows Type 3 text makes its own.
    glyph_interpreter: Option<Interpreter<'d>>,
    glyph_nesting: usize,
    image_options: ImageOptions,
    /// The images drawn so far, by object, while they fit the budget.
    images: HashMap<ObjectId, Arc<DecodedImage>>,
    image_bytes: usize,
    /// The content stream the last inline image was read from.
    inline_source: Option<ContentStream>,
    /// The first error a callback met; the callbacks cannot return one.
    pub error: Option<crate::PdfError>,
    progress: &'p dyn Progress,
}

impl<'d, 'p> Painter<'d, 'p> {
    pub fn new(
        doc: &'d Document,
        page_resources: Dictionary,
        canvas: Canvas,
        colours: Colours<'d>,
        glyphs: Glyphs<'d>,
        image_options: ImageOptions,
        progress: &'p dyn Progress,
    ) -> Self {
        Self {
            canvas,
            diagnostics: Diagnostics::new(),
            doc,
            page_resources,
            colours,
            glyphs,
            noted: HashSet::new(),
            text_clip: None,
            glyph_interpreter: None,
            glyph_nesting: 0,
            image_options,
            images: HashMap::new(),
            image_bytes: 0,
            inline_source: None,
            error: None,
            progress,
        }
    }

    fn note(&mut self, feature: &'static str) {
        if self.noted.insert(feature) {
            self.diagnostics
                .push(DiagnosticKind::NotRendered { feature }, 0);
        }
    }

    /// Reports what the state asks for that this renderer does not draw.
    fn note_transparency(&mut self, state: &GraphicsState) {
        if !NORMAL_BLEND_MODES.contains(&state.blend_mode.as_bytes()) {
            self.note("blend mode");
        }
        if !matches!(state.soft_mask, SoftMask::None) {
            self.note("soft mask");
        }
    }

    /// The colour a mark is painted in, or `None` when it leaves no mark.
    fn colour(&mut self, state: &GraphicsState, ink: Ink) -> Option<Colour> {
        self.note_transparency(state);
        let (paint, alpha): (&Paint, f64) = match ink {
            Ink::Fill => (&state.fill, state.fill_alpha),
            Ink::Stroke => (&state.stroke, state.stroke_alpha),
        };
        match self.colours.resolve(paint) {
            Ok(Resolved::Rgb(rgb)) => Some(Colour {
                rgb,
                alpha: alpha as f32,
            }),
            Ok(Resolved::Nothing) => None,
            Ok(Resolved::Pattern) => {
                self.note("pattern");
                None
            }
            Err(error) => {
                if matches!(error, crate::PdfError::Cancelled) {
                    self.error.get_or_insert(error);
                } else {
                    self.diagnostics.push(DiagnosticKind::ColourUnusable, 0);
                }
                None
            }
        }
    }

    fn stroke_style(state: &GraphicsState) -> StrokeStyle {
        // 8.4.3.2 draws a width of 0 as the thinnest line the device can
        // show, and leaves narrower ones to the device: a line under a
        // pixel wide is drawn one pixel wide, as readers draw them, so a
        // hairline rule or a fine grid does not fade away. `scale` is the
        // pixels to a user space unit.
        let scale = state.ctm.determinant().abs().sqrt();
        let thinnest = if scale.is_finite() && scale > 0.0 {
            1.0 / scale
        } else {
            0.0
        };
        StrokeStyle {
            width: state.line_width.max(thinnest) as f32,
            cap: state.line_cap,
            join: state.line_join,
            miter_limit: state.miter_limit as f32,
            dash: dash_of(&state.dash.array, state.dash.phase),
        }
    }

    fn stroke(&mut self, state: &GraphicsState, path: &tiny_skia::Path, ts: Transform) {
        if let Some(colour) = self.colour(state, Ink::Stroke) {
            self.canvas
                .stroke_path(path, &Self::stroke_style(state), ts, colour);
        }
    }

    /// Runs the description of a Type 3 glyph (9.6.5) onto the canvas.
    fn type3_glyph(&mut self, ctx: &Context<'_>, run: &TextRun, glyph: &PlacedGlyph) {
        if !ctx.visible || ctx.state.text.render_mode == RenderMode::Invisible {
            return;
        }
        let Some(type3) = run.font.type3() else {
            return;
        };
        let Some(procedure) = glyph
            .glyph_name
            .as_deref()
            .and_then(|name| type3.char_procs.get(name))
        else {
            return;
        };
        if self.glyph_nesting >= MAX_GLYPH_NESTING {
            self.error.get_or_insert(crate::PdfError::LimitExceeded {
                offset: ctx.span.range.start,
                limit: Limit::FormNesting,
            });
            return;
        }
        let resources = match type3.resources.as_ref().map(|r| self.doc.resolve(r)) {
            Some(Ok(resolved)) => resolved.dictionary().cloned(),
            _ => None,
        }
        .unwrap_or_else(|| self.page_resources.clone());
        let matrix = Matrix::from_array(type3.font_matrix).then(glyph.matrix);
        let mut interpreter = match self.glyph_interpreter.take() {
            Some(interpreter) => interpreter,
            None => match Interpreter::new(self.doc, InterpretOptions::default()) {
                Ok(interpreter) => interpreter,
                Err(error) => {
                    self.error.get_or_insert(error);
                    return;
                }
            },
        };
        let depth = self.canvas.depth();
        self.canvas.save();
        self.glyph_nesting += 1;
        let progress = self.progress;
        let result =
            interpreter.run_glyph(*procedure, &resources, matrix, ctx.state, self, progress);
        self.glyph_nesting -= 1;
        self.canvas.restore_to(depth);
        self.glyph_interpreter.get_or_insert(interpreter);
        if let Err(error) = result {
            self.error.get_or_insert(error);
        }
    }

    fn glyph(&mut self, ctx: &Context<'_>, run: &TextRun, glyph: &PlacedGlyph) {
        if run.font.kind() == FontKind::Type3 {
            self.type3_glyph(ctx, run, glyph);
            return;
        }
        let state = ctx.state;
        let mode = state.text.render_mode;
        if mode.clips() {
            // A glyph with no marks still adds its (empty) shape to the
            // clip: text of spaces in a clip mode hides what follows.
            self.text_clip.get_or_insert_with(PathBuilder::new);
        }
        let Some(outline) = self.glyphs.outline(run, glyph, &mut self.diagnostics) else {
            return;
        };
        let Some(glyph_ts) = shape::transform(&glyph.matrix) else {
            return;
        };
        if ctx.visible && mode.fills() {
            if let Some(colour) = self.colour(state, Ink::Fill) {
                self.canvas
                    .fill_path(&outline, FillRule::NonZeroWinding, glyph_ts, colour);
            }
        }
        if ctx.visible && mode.strokes() {
            // The pen is measured in user space: the outline goes to user
            // space and the CTM places the stroke.
            if let Some(inverse) = state.ctm.inverse() {
                let to_user = glyph.matrix.then(inverse);
                if let (Some(ts), Some(user)) =
                    (shape::transform(&to_user), shape::transform(&state.ctm))
                {
                    if let Some(user_path) = Path2::clone(&outline).transform(ts) {
                        self.stroke(state, &user_path, user);
                    }
                }
            }
        }
        if mode.clips() {
            if let (Some(device), Some(clip)) = (
                Path2::clone(&outline).transform(glyph_ts),
                self.text_clip.as_mut(),
            ) {
                clip.push_path(&device);
            }
        }
    }
}

/// The dash array and phase in the form the rasteriser takes (8.4.3.6): an
/// odd array is repeated to make an even one, and an array that is empty,
/// holds a negative number or adds up to nothing is a solid line.
fn dash_of(array: &[f64], phase: f64) -> Option<(Vec<f32>, f32)> {
    if array.is_empty() || array.iter().any(|n| !n.is_finite() || *n < 0.0) {
        return None;
    }
    if array.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let mut lengths: Vec<f32> = array.iter().map(|n| *n as f32).collect();
    if lengths.len() % 2 == 1 {
        lengths.extend_from_within(..);
    }
    let phase = if phase.is_finite() { phase as f32 } else { 0.0 };
    Some((lengths, phase))
}

impl Device for Painter<'_, '_> {
    fn save(&mut self, _ctx: &Context<'_>) {
        self.canvas.save();
    }

    fn restore(&mut self, _ctx: &Context<'_>) {
        self.canvas.restore();
    }

    fn path(
        &mut self,
        ctx: &Context<'_>,
        path: &Path,
        paint: PathPaint,
        _span: crate::content::OperatorSpan,
    ) {
        if !ctx.visible {
            return;
        }
        let state = ctx.state;
        let (Some(shape), Some(ts)) = (shape::path(path), shape::transform(&state.ctm)) else {
            return;
        };
        if let Some(rule) = paint.fill {
            if let Some(colour) = self.colour(state, Ink::Fill) {
                self.canvas.fill_path(&shape, rule, ts, colour);
            }
        }
        if paint.stroke {
            self.stroke(state, &shape, ts);
        }
    }

    fn clip(&mut self, ctx: &Context<'_>, path: &Path, rule: FillRule) {
        match (shape::path(path), shape::transform(&ctx.state.ctm)) {
            (Some(shape), Some(ts)) => self.canvas.clip_path(&shape, rule, ts),
            _ => self.canvas.clip_everything(),
        }
    }

    fn begin_text(&mut self, _ctx: &Context<'_>) {
        self.text_clip = None;
    }

    fn end_text(&mut self, _ctx: &Context<'_>) {
        let Some(builder) = self.text_clip.take() else {
            return;
        };
        match builder.finish() {
            Some(path) => {
                self.canvas
                    .clip_path(&path, FillRule::NonZeroWinding, Transform::identity())
            }
            None => self.canvas.clip_everything(),
        }
    }

    fn text(&mut self, ctx: &Context<'_>, run: &TextRun) {
        if self.progress.is_cancelled() {
            self.error.get_or_insert(crate::PdfError::Cancelled);
            return;
        }
        for glyph in &run.glyphs {
            self.glyph(ctx, run, glyph);
        }
    }

    fn image(&mut self, ctx: &Context<'_>, image: &ImageSource) {
        self.paint_image(ctx, image);
    }

    fn shading(&mut self, _ctx: &Context<'_>, _name: &Name, _object: &Object) {
        self.note("shading");
    }

    fn begin_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {
        // A transparency group composes as one object (11.4). Drawn
        // straight onto the page it differs only when the group itself is
        // translucent, blended or masked.
        let state = ctx.state;
        let composed = state.fill_alpha < 1.0
            || state.stroke_alpha < 1.0
            || !NORMAL_BLEND_MODES.contains(&state.blend_mode.as_bytes())
            || !matches!(state.soft_mask, SoftMask::None);
        if form.group.is_some() && composed {
            self.note("transparency group");
        }
    }
}
