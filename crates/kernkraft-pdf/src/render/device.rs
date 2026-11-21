// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The [`Device`] that paints: ISO 32000-1:2008, 8.5.3 (painting paths),
//! 8.5.4 (clipping), 9.3.6 and 9.4 (text rendering modes and showing text)
//! and 9.6.5 (Type 3 glyph descriptions).
//!
//! Colours reach the device as the stream wrote them; they are turned into
//! sRGB here, one paint at a time, and a pattern paint becomes pixels laid
//! over the mark. Transparency (11): the blend mode and constant alpha of
//! the state are the mark's, a form XObject with a transparency group is
//! painted on a layer of its own when it has to be composed as one object,
//! and a soft mask in the state cuts every mark.
//!
//! What the renderer does not draw is reported with
//! [`DiagnosticKind::NotRendered`] once for each kind on a page, and the
//! mark is left out.

mod annotations;
mod groups;
mod images;
mod patterns;
mod shadings;
mod softmask;

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kernkraft_fs::Progress;
use tiny_skia::{Path as Path2, PathBuilder, Pixmap, Transform};

use crate::content::ContentStream;
use crate::content::{
    Context, Device, ExtGStateInfo, FillRule, FormInfo, GraphicsState, ImageSource,
    InterpretOptions, Interpreter, Matrix, Path, PathPaint, PlacedGlyph, RenderMode, TextRun,
};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::error::Limit;
use crate::font::FontKind;
use crate::image::{DecodedImage, ImageOptions};
use crate::render::blend::blend_mode;
use crate::render::canvas::{Area, Brush, Canvas, Colour, Source, StrokeStyle};
use crate::render::colour::{Colours, Resolved};
use crate::render::glyphs::Glyphs;
use crate::render::shading::Shading;
use crate::render::shape;
use crate::syntax::{Dictionary, Name, Object, ObjectId};

/// How many streams may be run one inside another by this device: Type 3
/// glyph descriptions that show Type 3 text, pattern cells that use
/// patterns, soft masks whose group sets a soft mask.
const MAX_NESTING: usize = 6;

/// The most pixels the cell of a tiling pattern may be rendered at.
const MAX_TILE_PIXELS: u64 = 1 << 24;

/// Which of the two colours of the state a mark is painted with.
#[derive(Clone, Copy)]
enum Ink {
    Fill,
    Stroke,
}

/// What a mark is painted with, ready for the canvas.
enum Ready {
    Solid(Colour),
    Pixels {
        pixmap: Arc<Pixmap>,
        transform: Transform,
        repeat: bool,
        smooth: bool,
        alpha: f32,
    },
}

impl Ready {
    fn brush(&self, blend: tiny_skia::BlendMode, alpha_is_shape: bool) -> Brush<'_> {
        match self {
            Ready::Solid(colour) => Brush::solid(*colour, blend, alpha_is_shape),
            Ready::Pixels {
                pixmap,
                transform,
                repeat,
                smooth,
                alpha,
            } => Brush {
                source: Source::Pixels {
                    pixmap,
                    transform: *transform,
                    repeat: *repeat,
                    smooth: *smooth,
                    alpha: *alpha,
                },
                blend,
                alpha_is_shape,
            },
        }
    }
}

/// The shape of a mark, which a pattern paint needs to know to be rendered
/// only where it is seen.
#[derive(Clone, Copy)]
struct Marked<'a> {
    path: &'a Path2,
    transform: Transform,
    /// How far past the path the mark reaches, in device pixels.
    reach: f32,
}

/// What the painter needs to know of the page it draws.
pub(super) struct PageSetup {
    /// The page's resources, which a Type 3 font with none of its own uses.
    pub resources: Dictionary,
    /// The default user space to the pixels (8.7.3.1).
    pub base: Matrix,
}

/// The parts of the state of a run through a content stream: what a stream
/// run for a pattern, a soft mask or an annotation has of its own.
struct Run {
    /// The glyph outlines of the text object in progress, when a rendering
    /// mode adds them to the clip (9.3.6).
    text_clip: Option<PathBuilder>,
    /// For each form open, whether it is painted on a layer of its own.
    forms: Vec<bool>,
    /// The matrix of the default coordinate system of each content stream
    /// open, which a pattern matrix is relative to (8.7.3.1).
    bases: Vec<Matrix>,
}

impl Run {
    fn new(base: Matrix) -> Self {
        Self {
            text_clip: None,
            forms: Vec::new(),
            bases: vec![base],
        }
    }
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
    run: Run,
    /// The interpreter that runs Type 3 glyph descriptions, pattern cells,
    /// soft masks and annotation appearances; taken out while it runs, so
    /// a stream that needs one in turn makes its own.
    interpreter: Option<Interpreter<'d>>,
    nesting: usize,
    /// The shadings read so far, by object.
    shadings: HashMap<ObjectId, Option<Arc<Shading>>>,
    /// The cells of tiling patterns rendered so far.
    tiles: HashMap<patterns::TileKey, Arc<Pixmap>>,
    tile_bytes: usize,
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
        page: PageSetup,
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
            page_resources: page.resources,
            colours,
            glyphs,
            noted: HashSet::new(),
            run: Run::new(page.base),
            interpreter: None,
            nesting: 0,
            shadings: HashMap::new(),
            tiles: HashMap::new(),
            tile_bytes: 0,
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

    /// What the mark `mark` is painted with, or `None` when it leaves no
    /// mark.
    fn ink(&mut self, state: &GraphicsState, ink: Ink, mark: Option<Marked<'_>>) -> Option<Ready> {
        let (paint, alpha) = match ink {
            Ink::Fill => (&state.fill, state.fill_alpha as f32),
            Ink::Stroke => (&state.stroke, state.stroke_alpha as f32),
        };
        match self.colours.resolve(paint) {
            Ok(Resolved::Rgb(rgb)) => Some(Ready::Solid(Colour { rgb, alpha })),
            Ok(Resolved::Nothing) => None,
            Ok(Resolved::Pattern { object, under }) => {
                self.pattern_ink(&object, under, alpha, mark)
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

    /// The area of the page's pixels that `mark` can touch, within the
    /// layer being painted.
    fn mark_area(&self, mark: &Marked<'_>) -> Option<Area> {
        Area::of_bounds(mark.path.bounds(), mark.transform, mark.reach)?
            .intersect(self.canvas.bounds())
    }

    /// Runs `f` with an interpreter for a stream of this device's own to
    /// run: the interpreter is taken out of the device while it runs, so
    /// that a stream which needs one in turn makes its own.
    fn sub_run<R>(
        &mut self,
        f: impl FnOnce(&mut Self, &mut Interpreter<'d>) -> Result<R, crate::PdfError>,
    ) -> Result<R, crate::PdfError> {
        if self.nesting >= MAX_NESTING {
            return Err(crate::PdfError::LimitExceeded {
                offset: 0,
                limit: Limit::FormNesting,
            });
        }
        let mut interpreter = match self.interpreter.take() {
            Some(interpreter) => interpreter,
            None => Interpreter::new(self.doc, InterpretOptions::default())?,
        };
        self.nesting += 1;
        let result = f(self, &mut interpreter);
        self.nesting -= 1;
        self.interpreter.get_or_insert(interpreter);
        result
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

    fn fill(&mut self, state: &GraphicsState, path: &Path2, rule: FillRule, ts: Transform) {
        let mark = Marked {
            path,
            transform: ts,
            reach: 0.0,
        };
        if let Some(ready) = self.ink(state, Ink::Fill, Some(mark)) {
            let brush = ready.brush(blend_mode(&state.blend_mode), state.alpha_is_shape);
            self.canvas.fill_path(path, rule, ts, &brush);
        }
    }

    fn stroke(&mut self, state: &GraphicsState, path: &Path2, ts: Transform) {
        let style = Self::stroke_style(state);
        // A stroke reaches half its width from the path, further at a miter
        // join, in pixels.
        let scale = state.ctm.determinant().abs().sqrt() as f32;
        let mark = Marked {
            path,
            transform: ts,
            reach: style.width * scale * style.miter_limit.max(1.0),
        };
        if let Some(ready) = self.ink(state, Ink::Stroke, Some(mark)) {
            let brush = ready.brush(blend_mode(&state.blend_mode), state.alpha_is_shape);
            self.canvas.stroke_path(path, &style, ts, &brush);
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
        let resources = match type3.resources.as_ref().map(|r| self.doc.resolve(r)) {
            Some(Ok(resolved)) => resolved.dictionary().cloned(),
            _ => None,
        }
        .unwrap_or_else(|| self.page_resources.clone());
        let matrix = Matrix::from_array(type3.font_matrix).then(glyph.matrix);
        let depth = self.canvas.depth();
        self.canvas.save();
        self.run.bases.push(matrix);
        let result = self.sub_run(|painter, interpreter| {
            let progress = painter.progress;
            interpreter.run_glyph(*procedure, &resources, matrix, ctx.state, painter, progress)
        });
        self.run.bases.pop();
        self.canvas.restore_to(depth);
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
            self.run.text_clip.get_or_insert_with(PathBuilder::new);
        }
        let Some(outline) = self.glyphs.outline(run, glyph, &mut self.diagnostics) else {
            return;
        };
        let Some(glyph_ts) = shape::transform(&glyph.matrix) else {
            return;
        };
        if ctx.visible && mode.fills() {
            self.fill(state, &outline, FillRule::NonZeroWinding, glyph_ts);
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
                self.run.text_clip.as_mut(),
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
            self.fill(state, &shape, rule, ts);
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
        self.run.text_clip = None;
    }

    fn end_text(&mut self, _ctx: &Context<'_>) {
        let Some(builder) = self.run.text_clip.take() else {
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

    fn shading(&mut self, ctx: &Context<'_>, _name: &Name, object: &Object) {
        self.paint_shading(ctx, object);
    }

    fn begin_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {
        self.open_form(ctx, form);
    }

    fn end_form(&mut self, _ctx: &Context<'_>, _form: &FormInfo) {
        self.close_form();
    }

    fn ext_gstate(&mut self, ctx: &Context<'_>, info: &ExtGStateInfo) {
        self.apply_soft_mask(ctx, info);
    }
}
