// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The pixels, the clip and the transparency groups: ISO 32000-1:2008,
//! 8.5.3 (path-painting operators, the fill rules of 8.5.3.3), 8.5.3.2
//! (stroke, Table 52's line parameters), 8.5.4 (clipping path operators),
//! 11.3 (blend modes), 11.4 (transparency groups) and 11.6.5 (soft masks).
//!
//! A [`Canvas`] is a stack of layers. The bottom one is the page, an opaque
//! white pixmap; a transparency group that has to be composed as one object
//! is painted on a layer of its own, only as large as the group's bounding
//! box, and [`Canvas::pop_group`] composes it on the layer below with the
//! group's alpha and blend mode (11.4.5).
//!
//! The three kinds of group differ in what is under a mark (11.4.4, 11.4.8):
//!
//! - An isolated group starts transparent: its marks blend with each other
//!   and not with what is under the group.
//! - A group that is not isolated starts as a copy of what is under it, so
//!   that its marks blend with the backdrop; the alpha of the group's own
//!   marks is kept apart (the layer's `shadow`), and when the group ends the
//!   backdrop's share is taken out of the colour (11.4.8, the equation for
//!   the colour of a non-isolated group) before the group is composed.
//! - In a knockout group each mark is composed with the group's initial
//!   backdrop and replaces what the marks before it left where it covers,
//!   in proportion to its shape (11.4.6.2).
//!
//! Every method takes and gives device pixels of the page: the layer's own
//! position is added here.
//!
//! The clip is a coverage mask the size of the layer, so a clip with a
//! curved edge is anti-aliased like a fill is. The soft mask of the state
//! is another, and a mark is cut by both (11.6.5.1). They are part of the
//! graphics state: `q` keeps them and `Q` brings them back, which
//! [`Canvas::save`] and [`Canvas::restore`] do. A mask is shared between
//! the states that hold it (it is never changed in place), so a `q` costs a
//! pointer, not a page of bytes.

use std::sync::Arc;

use tiny_skia::{
    BlendMode, FillRule as SkiaFillRule, FilterQuality, LineCap as SkiaCap, LineJoin as SkiaJoin,
    Mask, Paint, Path, Pattern, Pixmap, PixmapPaint, PixmapRef, Rect, SpreadMode, Stroke,
    StrokeDash, Transform,
};

use crate::content::{FillRule, LineCap, LineJoin};

/// A rectangle of whole pixels: the page's device pixels, or a layer's.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Area {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl Area {
    /// The part of both, `None` when they share no pixel.
    pub fn intersect(self, other: Area) -> Option<Area> {
        let left = self.x.max(other.x);
        let top = self.y.max(other.y);
        let right = self.right().min(other.right());
        let bottom = self.bottom().min(other.bottom());
        if right <= left || bottom <= top {
            return None;
        }
        Some(Area {
            x: left,
            y: top,
            width: (i64::from(right) - i64::from(left)) as u32,
            height: (i64::from(bottom) - i64::from(top)) as u32,
        })
    }

    /// The first column past the area, saturating.
    pub fn right(self) -> i32 {
        (i64::from(self.x) + i64::from(self.width)).min(i64::from(i32::MAX)) as i32
    }

    /// The first row past the area, saturating.
    pub fn bottom(self) -> i32 {
        (i64::from(self.y) + i64::from(self.height)).min(i64::from(i32::MAX)) as i32
    }

    /// The area of the device pixels that `bounds` (a rectangle of the
    /// space `transform` leaves) reaches, `reach` pixels further on every
    /// side. `None` when a number is not finite.
    pub fn of_bounds(bounds: Rect, transform: Transform, reach: f32) -> Option<Area> {
        let (mut low, mut high) = ([f32::MAX; 2], [f32::MIN; 2]);
        for (x, y) in [
            (bounds.left(), bounds.top()),
            (bounds.right(), bounds.top()),
            (bounds.right(), bounds.bottom()),
            (bounds.left(), bounds.bottom()),
        ] {
            let device = [
                transform.sx * x + transform.kx * y + transform.tx,
                transform.ky * x + transform.sy * y + transform.ty,
            ];
            for axis in 0..2 {
                low[axis] = low[axis].min(device[axis]);
                high[axis] = high[axis].max(device[axis]);
            }
        }
        if !low.iter().chain(&high).all(|n| n.is_finite()) {
            return None;
        }
        let reach = reach.max(0.0) + 1.0;
        let clamp = |n: f32| (n as i64).clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        let (x0, y0) = (
            clamp((low[0] - reach).floor()),
            clamp((low[1] - reach).floor()),
        );
        let (x1, y1) = (
            clamp((high[0] + reach).ceil()),
            clamp((high[1] + reach).ceil()),
        );
        Some(Area {
            x: x0 as i32,
            y: y0 as i32,
            width: (x1 - x0).clamp(0, i64::from(u32::MAX)) as u32,
            height: (y1 - y0).clamp(0, i64::from(u32::MAX)) as u32,
        })
    }
}

/// A colour with its alpha: sRGB components 0 to 255, alpha 0 to 1.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Colour {
    pub rgb: [u8; 3],
    pub alpha: f32,
}

/// Where and how [`Canvas::draw_pixels`] puts an image.
#[derive(Debug, Clone, Copy)]
pub(super) struct Placement {
    /// Moves the pixmap to the page's device pixels.
    pub transform: Transform,
    /// The constant alpha of 11.6.4.4.
    pub opacity: f32,
    /// Whether the samples are filtered; without it each pixel of the
    /// canvas takes the nearest sample.
    pub smooth: bool,
    pub blend: BlendMode,
    /// Whether the alpha is a shape and not an opacity (`AIS`, 11.6.4.4).
    pub alpha_is_shape: bool,
}

/// What a mark is painted with.
pub(super) enum Source<'a> {
    Solid(Colour),
    /// A pixmap of premultiplied pixels, placed in the page's device space
    /// by `transform` (which maps a pixel of the pixmap to a pixel of the
    /// page), repeated in both directions when `repeat` is set and held at
    /// its edge pixels when not.
    Pixels {
        pixmap: &'a Pixmap,
        transform: Transform,
        repeat: bool,
        smooth: bool,
        alpha: f32,
    },
}

/// A source and how it combines with what is under it.
pub(super) struct Brush<'a> {
    pub source: Source<'a>,
    pub blend: BlendMode,
    /// Whether the alpha is a shape and not an opacity (`AIS`, 11.6.4.4).
    pub alpha_is_shape: bool,
}

impl Brush<'_> {
    /// The share of the brush's alpha that is a shape: the alpha when it is
    /// one, 1 when it is an opacity.
    fn shape_alpha(&self) -> f32 {
        if !self.alpha_is_shape {
            return 1.0;
        }
        match &self.source {
            Source::Solid(colour) => colour.alpha,
            Source::Pixels { alpha, .. } => *alpha,
        }
    }

    pub fn solid(colour: Colour, blend: BlendMode, alpha_is_shape: bool) -> Self {
        Self {
            source: Source::Solid(colour),
            blend,
            alpha_is_shape,
        }
    }
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

/// What a group is, and how it joins the layer below.
#[derive(Debug, Clone, Copy)]
pub(super) struct GroupParams {
    /// The constant alpha the group is composed with.
    pub alpha: f32,
    pub blend: BlendMode,
    /// `I`: the group does not see what is under it.
    pub isolated: bool,
    /// `K`: each mark replaces the marks before it where it covers.
    pub knockout: bool,
}

/// The part of the graphics state the canvas keeps.
#[derive(Clone)]
struct Masks {
    clip: Option<Arc<Mask>>,
    soft: Option<Arc<Mask>>,
}

/// One layer: the page, or a group in progress.
struct Layer {
    pixmap: Pixmap,
    /// The layer's top-left pixel, in the page's device pixels.
    origin: (i32, i32),
    /// What composes the layer on the one below, `None` for the page.
    group: Option<Group>,
    /// For a group that is not isolated: the alpha of the group's own marks
    /// alone, in the alpha channel of a pixmap the size of the layer.
    shadow: Option<Pixmap>,
}

/// How a finished group joins the layer below it, and the state of that
/// layer to come back to.
struct Group {
    params: GroupParams,
    below: Masks,
    /// How many states were saved when the group began.
    saved: usize,
    /// For a group that is not isolated: the layer below as the group saw
    /// it when it began.
    backdrop: Option<Pixmap>,
}

/// What a mark is drawn as, on one of the pixmaps it goes to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// The mark itself.
    Colour,
    /// Its alpha, composed with Normal.
    Shadow,
    /// Its shape: opaque where it covers.
    Shape,
}

/// How one mark is drawn on a pixmap: the pixmap, where its top-left pixel
/// is on the page, the pass, the mask that cuts the mark, and whether the
/// pixmap is for a knockout group.
type Drawer<'a> = dyn FnMut(&mut Pixmap, (i32, i32), Pass, Option<&Mask>, bool) + 'a;

/// The page being painted.
pub(super) struct Canvas {
    layers: Vec<Layer>,
    masks: Masks,
    /// The clip and the soft mask as one, for the marks.
    cut: Option<Arc<Mask>>,
    saved: Vec<Masks>,
}

fn rule(rule: FillRule) -> SkiaFillRule {
    match rule {
        FillRule::NonZeroWinding => SkiaFillRule::Winding,
        FillRule::EvenOdd => SkiaFillRule::EvenOdd,
    }
}

/// The product of two coverage masks of one size.
fn product(a: &Mask, b: &Mask) -> Mask {
    let mut mask = Mask::clone(a);
    for (x, y) in mask.data_mut().iter_mut().zip(b.data()) {
        *x = ((u32::from(*x) * u32::from(*y) + 127) / 255) as u8;
    }
    mask
}

/// A copy of the part of `source` at `(x, y)` of its own pixels, `width` by
/// `height`: the pixels past its edge are transparent.
fn crop_pixmap(source: &Pixmap, x: i32, y: i32, width: u32, height: u32) -> Option<Pixmap> {
    let mut out = Pixmap::new(width, height)?;
    let (source_width, source_height) = (source.width() as i64, source.height() as i64);
    for row in 0..i64::from(height) {
        let from_row = i64::from(y) + row;
        if from_row < 0 || from_row >= source_height {
            continue;
        }
        let first = i64::from(x).max(0);
        let last = (i64::from(x) + i64::from(width)).min(source_width);
        if last <= first {
            continue;
        }
        let count = (last - first) as usize * 4;
        let from = (from_row * source_width + first) as usize * 4;
        let to = (row * i64::from(width) + (first - i64::from(x))) as usize * 4;
        out.data_mut()[to..to + count].copy_from_slice(&source.data()[from..from + count]);
    }
    Some(out)
}

/// As [`crop_pixmap`], for a mask: what is past the edge is 0.
fn crop_mask(source: &Mask, x: i32, y: i32, width: u32, height: u32) -> Option<Mask> {
    let mut out = Mask::new(width, height)?;
    let (source_width, source_height) = (i64::from(source.width()), i64::from(source.height()));
    for row in 0..i64::from(height) {
        let from_row = i64::from(y) + row;
        if from_row < 0 || from_row >= source_height {
            continue;
        }
        let first = i64::from(x).max(0);
        let last = (i64::from(x) + i64::from(width)).min(source_width);
        if last <= first {
            continue;
        }
        let count = (last - first) as usize;
        let from = (from_row * source_width + first) as usize;
        let to = (row * i64::from(width) + (first - i64::from(x))) as usize;
        out.data_mut()[to..to + count].copy_from_slice(&source.data()[from..from + count]);
    }
    Some(out)
}

/// The paint for a brush on a pixmap whose top-left pixel is `origin` of the
/// page, for a mark drawn with `within`, the transform the path is given.
fn paint_of<'a>(
    brush: &'a Brush<'a>,
    within: Transform,
    origin: (i32, i32),
    pass: Pass,
    knockout: bool,
) -> Option<Paint<'a>> {
    // In a knockout group an alpha that is a shape knocks out in
    // proportion and paints opaquely; one that is an opacity does not
    // knock out in proportion (11.6.4.4).
    let alpha_of = |alpha: f32| {
        match pass {
            Pass::Shape if brush.alpha_is_shape && knockout => alpha,
            Pass::Shape => 1.0,
            _ if brush.alpha_is_shape && knockout => 1.0,
            _ => alpha,
        }
        .clamp(0.0, 1.0)
    };
    let mut paint = Paint {
        blend_mode: if pass == Pass::Colour {
            brush.blend
        } else {
            BlendMode::SourceOver
        },
        anti_alias: true,
        ..Paint::default()
    };
    match &brush.source {
        Source::Solid(colour) => {
            let [r, g, b] = colour.rgb;
            let channel = |value: u8| f32::from(value) / 255.0;
            paint.set_color(tiny_skia::Color::from_rgba(
                channel(r),
                channel(g),
                channel(b),
                alpha_of(colour.alpha),
            )?);
        }
        Source::Pixels {
            pixmap,
            transform,
            repeat,
            smooth,
            alpha,
        } => {
            // The shader is moved by the transform the mark is drawn
            // with, so it is given the one that undoes it.
            let device = transform.post_translate(-(origin.0 as f32), -(origin.1 as f32));
            let shader = within.invert()?.pre_concat(device);
            paint.shader = Pattern::new(
                pixmap.as_ref(),
                if *repeat {
                    SpreadMode::Repeat
                } else {
                    SpreadMode::Pad
                },
                if *smooth {
                    FilterQuality::Bilinear
                } else {
                    FilterQuality::Nearest
                },
                alpha_of(*alpha),
                shader,
            );
        }
    }
    Some(paint)
}

impl Canvas {
    /// A white, opaque canvas. `None` when a side is 0 or past what the
    /// rasteriser numbers.
    pub fn new(width: u32, height: u32) -> Option<Self> {
        let mut pixmap = Pixmap::new(width, height)?;
        pixmap.fill(tiny_skia::Color::WHITE);
        Some(Self::with_page(pixmap))
    }

    /// A canvas that starts as `backdrop` (transparent when `None`), for a
    /// picture that is not the page: a pattern cell or a soft mask.
    pub fn offscreen(area: Area, backdrop: Option<tiny_skia::Color>) -> Option<Self> {
        let mut pixmap = Pixmap::new(area.width, area.height)?;
        if let Some(colour) = backdrop {
            pixmap.fill(colour);
        }
        let mut canvas = Self::with_page(pixmap);
        canvas.layers[0].origin = (area.x, area.y);
        Some(canvas)
    }

    fn with_page(pixmap: Pixmap) -> Self {
        Self {
            layers: vec![Layer {
                pixmap,
                origin: (0, 0),
                group: None,
                shadow: None,
            }],
            masks: Masks {
                clip: None,
                soft: None,
            },
            cut: None,
            saved: Vec::new(),
        }
    }

    fn top(&self) -> &Layer {
        // The page is never popped.
        &self.layers[self.layers.len() - 1]
    }

    /// The page's width in pixels.
    pub fn width(&self) -> u32 {
        self.layers[0].pixmap.width()
    }

    /// The page's height in pixels.
    pub fn height(&self) -> u32 {
        self.layers[0].pixmap.height()
    }

    /// The layer being painted: its top-left pixel and its size, in the
    /// page's device pixels.
    pub fn bounds(&self) -> Area {
        let layer = self.top();
        Area {
            x: layer.origin.0,
            y: layer.origin.1,
            width: layer.pixmap.width(),
            height: layer.pixmap.height(),
        }
    }

    /// The smallest area of the layer that the clip and the soft mask leave
    /// open, in the page's device pixels; `None` when they leave nothing.
    pub fn cut_bounds(&self) -> Option<Area> {
        let layer = self.top();
        let (width, height) = (
            layer.pixmap.width() as usize,
            layer.pixmap.height() as usize,
        );
        let Some(cut) = &self.cut else {
            return Some(self.bounds());
        };
        let (mut left, mut top, mut right, mut bottom) = (width, height, 0, 0);
        for (y, row) in cut.data().chunks_exact(width).enumerate() {
            let Some(first) = row.iter().position(|v| *v != 0) else {
                continue;
            };
            let last = row.iter().rposition(|v| *v != 0).unwrap_or(first);
            left = left.min(first);
            right = right.max(last + 1);
            top = top.min(y);
            bottom = y + 1;
        }
        if right <= left || bottom <= top {
            return None;
        }
        Some(Area {
            x: layer.origin.0 + left as i32,
            y: layer.origin.1 + top as i32,
            width: (right - left) as u32,
            height: (bottom - top) as u32,
        })
    }

    fn refresh(&mut self) {
        self.cut = match (&self.masks.clip, &self.masks.soft) {
            (clip, None) => clip.clone(),
            (None, Some(soft)) => Some(soft.clone()),
            (Some(clip), Some(soft)) => Some(Arc::new(product(clip, soft))),
        };
    }

    /// `q`.
    pub fn save(&mut self) {
        self.saved.push(self.masks.clone());
    }

    /// `Q`. A `Q` with nothing saved leaves the masks as they are.
    pub fn restore(&mut self) {
        if let Some(masks) = self.saved.pop() {
            self.masks = masks;
            self.refresh();
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
        let (x, y) = self.top().origin;
        let transform = transform.post_translate(-(x as f32), -(y as f32));
        let mask = match &self.masks.clip {
            None => {
                let layer = self.top();
                let Some(mut mask) = Mask::new(layer.pixmap.width(), layer.pixmap.height()) else {
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
        self.masks.clip = Some(Arc::new(mask));
        self.refresh();
    }

    /// Cuts the clip down to nothing: everything after it is clipped away.
    pub fn clip_everything(&mut self) {
        let layer = self.top();
        if let Some(mask) = Mask::new(layer.pixmap.width(), layer.pixmap.height()) {
            self.masks.clip = Some(Arc::new(mask));
            self.refresh();
        }
    }

    /// Sets the soft mask of the state: coverage over the current layer, or
    /// none (11.6.5.1).
    pub fn set_soft_mask(&mut self, mask: Option<Mask>) {
        self.masks.soft = mask.map(Arc::new);
        self.refresh();
    }

    /// Puts one mark on the layer being painted. `draw` paints the mark on
    /// the pixmap it is given, whose top-left pixel is `origin` of the page,
    /// in the `pass` it is told, cut by `mask` (when there is one, of the
    /// pixmap's size). `area` is where the mark reaches, and `shape_alpha`
    /// the share of the mark's alpha that is a shape (`AIS`: the alpha, 1
    /// otherwise).
    ///
    /// On a plain layer that is one call. A layer that keeps the alpha of
    /// its group's marks gets a second call for it. A knockout layer gets
    /// the mark composed with the group's initial backdrop on a pixmap of its
    /// own and, by its shape, replaces what the layer had.
    fn mark(&mut self, area: Option<Area>, shape_alpha: f32, draw: &mut Drawer<'_>) {
        let cut = self.cut.clone();
        let clip = self.masks.clip.clone();
        let last = self.layers.len() - 1;
        let knockout = self.layers[last]
            .group
            .as_ref()
            .is_some_and(|group| group.params.knockout);
        if !knockout {
            let layer = &mut self.layers[last];
            let origin = layer.origin;
            draw(
                &mut layer.pixmap,
                origin,
                Pass::Colour,
                cut.as_deref(),
                false,
            );
            if let Some(shadow) = &mut layer.shadow {
                draw(shadow, origin, Pass::Shadow, cut.as_deref(), false);
            }
            return;
        }
        let here = self.bounds();
        let Some(area) = area.and_then(|area| area.intersect(here)) else {
            return;
        };
        let layer = &mut self.layers[last];
        let (left, top) = (area.x - layer.origin.0, area.y - layer.origin.1);
        let (width, height) = (area.width, area.height);
        let crop = |mask: &Option<Arc<Mask>>| {
            mask.as_ref()
                .and_then(|mask| crop_mask(mask, left, top, width, height))
        };
        let (cut_here, clip_here) = (crop(&cut), crop(&clip));
        let origin = (area.x, area.y);

        // The mark on the group's initial backdrop.
        let backdrop = layer
            .group
            .as_ref()
            .and_then(|group| group.backdrop.as_ref())
            .and_then(|backdrop| crop_pixmap(backdrop, left, top, width, height));
        let non_isolated = backdrop.is_some();
        let Some(mut over) = backdrop.or_else(|| Pixmap::new(width, height)) else {
            return;
        };
        draw(&mut over, origin, Pass::Colour, cut_here.as_ref(), true);
        // Its shape.
        let Some(mut shape) = Pixmap::new(width, height) else {
            return;
        };
        draw(&mut shape, origin, Pass::Shape, clip_here.as_ref(), true);
        // Its alpha alone.
        let mut alone = Pixmap::new(width, height);
        if let Some(alone) = &mut alone {
            draw(alone, origin, Pass::Shadow, cut_here.as_ref(), true);
        }

        // What the mark brings to the layer, in proportion to its shape
        // where it was composed with a backdrop (which it replaces), and in
        // proportion to its shape alpha where it was not.
        let blend = |target: &mut Pixmap, with: &Pixmap, per_pixel: bool| {
            let target_width = target.width() as usize;
            for row in 0..height as usize {
                for column in 0..width as usize {
                    let at = (row * width as usize + column) * 4;
                    let s = f32::from(shape.data()[at + 3]) / 255.0;
                    let to = ((row + top as usize) * target_width + column + left as usize) * 4;
                    for channel in 0..4 {
                        let old = f32::from(target.data()[to + channel]);
                        let new = f32::from(with.data()[at + channel]);
                        let value = old * (1.0 - s) + new * if per_pixel { s } else { shape_alpha };
                        target.data_mut()[to + channel] = value.round().clamp(0.0, 255.0) as u8;
                    }
                }
            }
        };
        blend(&mut layer.pixmap, &over, non_isolated);
        if let (Some(shadow), Some(alone)) = (&mut layer.shadow, &alone) {
            blend(shadow, alone, false);
        }
    }

    pub fn fill_path(&mut self, path: &Path, fill: FillRule, transform: Transform, brush: &Brush) {
        let area = Area::of_bounds(path.bounds(), transform, 0.0);
        self.mark(
            area,
            brush.shape_alpha(),
            &mut |pixmap, origin, pass, mask, knockout| {
                let ts = transform.post_translate(-(origin.0 as f32), -(origin.1 as f32));
                if let Some(paint) = paint_of(brush, ts, origin, pass, knockout) {
                    pixmap.fill_path(path, &paint, rule(fill), ts, mask);
                }
            },
        );
    }

    pub fn stroke_path(
        &mut self,
        path: &Path,
        style: &StrokeStyle,
        transform: Transform,
        brush: &Brush,
    ) {
        let scale = (transform.sx * transform.sy - transform.kx * transform.ky)
            .abs()
            .sqrt();
        let reach = style.width * scale * style.miter_limit.max(1.0);
        let area = Area::of_bounds(path.bounds(), transform, reach);
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
        self.mark(
            area,
            brush.shape_alpha(),
            &mut |pixmap, origin, pass, mask, knockout| {
                let ts = transform.post_translate(-(origin.0 as f32), -(origin.1 as f32));
                if let Some(paint) = paint_of(brush, ts, origin, pass, knockout) {
                    pixmap.stroke_path(path, &paint, &stroke, ts, mask);
                }
            },
        );
    }

    /// Draws `pixels` (premultiplied RGBA8, `width` by `height`) as `how`
    /// places them.
    pub fn draw_pixels(&mut self, pixels: &[u8], width: u32, height: u32, how: &Placement) {
        let (Some(source), Some(unit)) = (
            PixmapRef::from_bytes(pixels, width, height),
            Rect::from_xywh(0.0, 0.0, width as f32, height as f32),
        ) else {
            return;
        };
        let area = Area::of_bounds(unit, how.transform, 0.0);
        let shape_alpha = if how.alpha_is_shape { how.opacity } else { 1.0 };
        self.mark(
            area,
            shape_alpha,
            &mut |pixmap, origin, pass, mask, knockout| {
                let alpha = match pass {
                    Pass::Shape if how.alpha_is_shape && knockout => how.opacity,
                    Pass::Shape => 1.0,
                    _ if how.alpha_is_shape && knockout => 1.0,
                    _ => how.opacity,
                };
                let paint = Paint {
                    shader: Pattern::new(
                        source,
                        SpreadMode::Pad,
                        if how.smooth {
                            FilterQuality::Bilinear
                        } else {
                            FilterQuality::Nearest
                        },
                        alpha.clamp(0.0, 1.0),
                        Transform::identity(),
                    ),
                    blend_mode: if pass == Pass::Colour {
                        how.blend
                    } else {
                        BlendMode::SourceOver
                    },
                    anti_alias: true,
                    ..Paint::default()
                };
                let ts = how
                    .transform
                    .post_translate(-(origin.0 as f32), -(origin.1 as f32));
                if let Some(rect) = Rect::from_xywh(0.0, 0.0, width as f32, height as f32) {
                    pixmap.fill_rect(rect, &paint, ts, mask);
                }
            },
        );
    }

    /// Draws `pixmap` with its top-left pixel at `(x, y)` of the page's
    /// device pixels, cut by the clip and the soft mask.
    pub fn draw_pixmap(&mut self, pixmap: &Pixmap, x: i32, y: i32, opacity: f32, blend: BlendMode) {
        let area = Some(Area {
            x,
            y,
            width: pixmap.width(),
            height: pixmap.height(),
        });
        self.mark(area, 1.0, &mut |target, origin, pass, mask, _| {
            let paint = PixmapPaint {
                opacity: if pass == Pass::Shape {
                    1.0
                } else {
                    opacity.clamp(0.0, 1.0)
                },
                blend_mode: if pass == Pass::Colour {
                    blend
                } else {
                    BlendMode::SourceOver
                },
                quality: FilterQuality::Nearest,
            };
            target.draw_pixmap(
                x - origin.0,
                y - origin.1,
                pixmap.as_ref(),
                &paint,
                Transform::identity(),
                mask,
            );
        });
    }

    /// Begins a transparency group (11.4): what is painted from here to the
    /// matching [`Self::pop_group`] goes on a layer covering `area` (the part
    /// of it on the current layer), and is then composed on the layer below
    /// as `how` says. The clip and the soft mask start out as none. False
    /// when there is no room for the layer, in which case nothing was begun
    /// and nothing is to be ended.
    pub fn push_group(&mut self, area: Area, how: GroupParams) -> bool {
        let here = self.bounds();
        // A group that lies outside the layer paints nothing: its layer is
        // one pixel, cut away entirely.
        let (area, empty) = match area.intersect(here) {
            Some(area) => (area, false),
            None => (
                Area {
                    x: here.x,
                    y: here.y,
                    width: 1,
                    height: 1,
                },
                true,
            ),
        };
        let backdrop = if how.isolated {
            None
        } else {
            let top = self.top();
            crop_pixmap(
                &top.pixmap,
                area.x - top.origin.0,
                area.y - top.origin.1,
                area.width,
                area.height,
            )
        };
        let (pixmap, shadow) = if how.isolated {
            (Pixmap::new(area.width, area.height), None)
        } else {
            (backdrop.clone(), Pixmap::new(area.width, area.height))
        };
        let Some(pixmap) = pixmap else {
            return false;
        };
        if !how.isolated && (backdrop.is_none() || shadow.is_none()) {
            return false;
        }
        let below = std::mem::replace(
            &mut self.masks,
            Masks {
                clip: None,
                soft: None,
            },
        );
        self.layers.push(Layer {
            pixmap,
            origin: (area.x, area.y),
            group: Some(Group {
                params: how,
                below,
                saved: self.saved.len(),
                backdrop,
            }),
            shadow,
        });
        self.cut = None;
        if empty {
            self.clip_everything();
        }
        true
    }

    /// Ends the group begun last: composes its layer on the one below, cut
    /// by the clip and the soft mask that layer had.
    pub fn pop_group(&mut self) {
        if self.layers.len() < 2 {
            return;
        }
        let Some(mut layer) = self.layers.pop() else {
            return;
        };
        let Some(group) = layer.group.take() else {
            return;
        };
        self.saved.truncate(group.saved);
        self.masks = group.below;
        self.refresh();
        if let (Some(backdrop), Some(shadow)) = (&group.backdrop, &layer.shadow) {
            remove_backdrop(&mut layer.pixmap, backdrop, shadow);
        }
        self.draw_pixmap(
            &layer.pixmap,
            layer.origin.0,
            layer.origin.1,
            group.params.alpha,
            group.params.blend,
        );
    }

    /// How many groups are open.
    pub fn group_depth(&self) -> usize {
        self.layers.len() - 1
    }

    /// Ends groups until `depth` are open, for a form whose groups were
    /// cut short.
    pub fn pop_groups_to(&mut self, depth: usize) {
        while self.group_depth() > depth {
            self.pop_group();
        }
    }

    /// The page's pixels, which a finished picture is made of.
    pub fn into_pixmap(mut self) -> Pixmap {
        self.pop_groups_to(0);
        self.layers.swap_remove(0).pixmap
    }

    /// The pixels as straight (not premultiplied) RGBA8, row by row from
    /// the top. The page starts opaque white and is only ever painted onto,
    /// so every alpha is 255 and premultiplied equals straight.
    pub fn into_rgba(self) -> Vec<u8> {
        self.into_pixmap().take()
    }
}

/// Takes the backdrop's share out of the colour of a group that is not
/// isolated (11.4.8): with `C` the colour the group's layer has, `C0` and
/// `a0` the backdrop's colour and alpha, and `ag` the alpha of the group's
/// own marks, the group's colour is
///
/// `C + (C - C0) * (a0 / ag - a0)`
///
/// and its alpha `ag`. `layer` becomes the premultiplied result.
fn remove_backdrop(layer: &mut Pixmap, backdrop: &Pixmap, shadow: &Pixmap) {
    let pixels = layer.data_mut().as_chunks_mut::<4>().0.iter_mut();
    for ((pixel, back), alone) in pixels
        .zip(backdrop.data().as_chunks::<4>().0)
        .zip(shadow.data().as_chunks::<4>().0)
    {
        let group_alpha = f32::from(alone[3]) / 255.0;
        if group_alpha <= 0.0 {
            pixel.fill(0);
            continue;
        }
        let total = f32::from(pixel[3]) / 255.0;
        let back_alpha = f32::from(back[3]) / 255.0;
        for channel in 0..3 {
            let colour = if total > 0.0 {
                f32::from(pixel[channel]) / 255.0 / total
            } else {
                0.0
            };
            let backdrop_colour = if back_alpha > 0.0 {
                f32::from(back[channel]) / 255.0 / back_alpha
            } else {
                0.0
            };
            let result =
                colour + (colour - backdrop_colour) * (back_alpha / group_alpha - back_alpha);
            let result = result.clamp(0.0, 1.0);
            pixel[channel] = (result * group_alpha * 255.0).round() as u8;
        }
        pixel[3] = alone[3];
    }
}
