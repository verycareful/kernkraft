// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The graphics state and the text state: ISO 32000-1:2008, 8.4 (graphics
//! state, Table 52), 9.3 (text state parameters, Table 104) and 11.6.4
//! (the transparency parameters of Table 58).

use std::sync::Arc;

use crate::content::matrix::Matrix;
use crate::font::Font;
use crate::syntax::{Name, Object};

/// The width of a line the state starts with (Table 52).
pub const DEFAULT_LINE_WIDTH: f64 = 1.0;
/// The miter limit the state starts with (Table 52).
pub const DEFAULT_MITER_LIMIT: f64 = 10.0;
/// The flatness tolerance the state starts with. Table 52 says a
/// device-dependent default; 1 is what 10.6.2 gives as the value 1 pixel.
pub const DEFAULT_FLATNESS: f64 = 1.0;
/// The horizontal scaling the state starts with, as a fraction: 100 percent
/// (9.3.4).
pub const DEFAULT_HORIZONTAL_SCALING: f64 = 1.0;
/// The factor that turns the percentage `Tz` and the `Tz` field of a
/// font into the fraction the text rendering matrix uses (9.3.4).
pub const PERCENT: f64 = 100.0;
/// The number of text-space units a glyph width is measured in: widths are
/// thousandths of a unit (9.2.4).
pub const GLYPH_UNITS_PER_EM: f64 = 1000.0;

/// The shape at the ends of an open subpath (8.4.3.3, `J`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineCap {
    Butt,
    Round,
    Square,
}

impl LineCap {
    pub(crate) fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Butt),
            1 => Some(Self::Round),
            2 => Some(Self::Square),
            _ => None,
        }
    }
}

/// The shape at the corners of a path (8.4.3.4, `j`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineJoin {
    Miter,
    Round,
    Bevel,
}

impl LineJoin {
    pub(crate) fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Miter),
            1 => Some(Self::Round),
            2 => Some(Self::Bevel),
            _ => None,
        }
    }
}

/// How a glyph is painted, and whether it adds to the clipping path
/// (9.3.6, Table 106, `Tr`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderMode {
    Fill,
    Stroke,
    FillStroke,
    Invisible,
    FillClip,
    StrokeClip,
    FillStrokeClip,
    Clip,
}

impl RenderMode {
    pub(crate) fn from_code(code: i64) -> Option<Self> {
        match code {
            0 => Some(Self::Fill),
            1 => Some(Self::Stroke),
            2 => Some(Self::FillStroke),
            3 => Some(Self::Invisible),
            4 => Some(Self::FillClip),
            5 => Some(Self::StrokeClip),
            6 => Some(Self::FillStrokeClip),
            7 => Some(Self::Clip),
            _ => None,
        }
    }

    /// Whether glyphs are filled.
    pub fn fills(self) -> bool {
        matches!(
            self,
            Self::Fill | Self::FillStroke | Self::FillClip | Self::FillStrokeClip
        )
    }

    /// Whether glyphs are stroked.
    pub fn strokes(self) -> bool {
        matches!(
            self,
            Self::Stroke | Self::FillStroke | Self::StrokeClip | Self::FillStrokeClip
        )
    }

    /// Whether glyph outlines are added to the clipping path at `ET`.
    pub fn clips(self) -> bool {
        matches!(
            self,
            Self::FillClip | Self::StrokeClip | Self::FillStrokeClip | Self::Clip
        )
    }
}

/// A colour space as a content stream names it (8.6). Resolving what a
/// named or array space means is the colour layer's work: here it is kept
/// as the resource gave it.
#[derive(Debug, Clone, PartialEq)]
pub enum ColorSpace {
    DeviceGray,
    DeviceRGB,
    DeviceCMYK,
    /// The Pattern space with no underlying space (8.7.3.2).
    Pattern,
    /// A space from the `ColorSpace` resources, under `name`: `object` is
    /// its definition, an array or a name, as the resource gives it.
    Resource {
        name: Name,
        object: Object,
    },
}

impl ColorSpace {
    /// The number of components a colour of a device space has, `None` for
    /// a resource space (whose definition says) and for Pattern.
    pub fn device_components(&self) -> Option<usize> {
        match self {
            Self::DeviceGray => Some(1),
            Self::DeviceRGB => Some(3),
            Self::DeviceCMYK => Some(4),
            Self::Pattern | Self::Resource { .. } => None,
        }
    }
}

/// The current colour: a colour space and the components given in it (8.6.3),
/// or, in a Pattern space, the pattern's name and the underlying
/// components of an uncoloured pattern (8.7.3).
#[derive(Debug, Clone, PartialEq)]
pub struct Paint {
    pub space: ColorSpace,
    pub components: Vec<f64>,
    /// The name of the pattern in the `Pattern` resources, and the pattern
    /// object it names, when the colour is a pattern.
    pub pattern: Option<(Name, Object)>,
}

impl Paint {
    /// Black in DeviceGray, the initial colour of both (Table 52).
    pub fn black() -> Self {
        Self {
            space: ColorSpace::DeviceGray,
            components: vec![0.0],
            pattern: None,
        }
    }
}

/// The dash pattern (8.4.3.6, `d`): an empty array is a solid line.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Dash {
    pub array: Vec<f64>,
    pub phase: f64,
}

/// The soft mask of the state (11.6.5.1): none, or the dictionary
/// `SMask` gave.
#[derive(Debug, Clone, PartialEq)]
pub enum SoftMask {
    None,
    /// A soft-mask dictionary (Table 144).
    Dictionary(Object),
}

/// The font and size of the text state, with the resource name that
/// chose the font.
#[derive(Debug, Clone)]
pub struct TextFont {
    pub name: Name,
    pub font: Arc<Font>,
    pub size: f64,
}

/// The text state parameters of Table 104. The text matrix and the text
/// line matrix belong to a text object, not to the state; the
/// interpreter keeps them.
#[derive(Debug, Clone)]
pub struct TextState {
    /// `Tc`, in unscaled text space units.
    pub char_spacing: f64,
    /// `Tw`, in unscaled text space units.
    pub word_spacing: f64,
    /// `Tz` as a fraction: 1 is 100 percent.
    pub horizontal_scaling: f64,
    /// `TL`, in unscaled text space units.
    pub leading: f64,
    /// `Tf`; `None` until a font is set.
    pub font: Option<TextFont>,
    /// `Tr`.
    pub render_mode: RenderMode,
    /// `Ts`, in unscaled text space units.
    pub rise: f64,
}

impl Default for TextState {
    fn default() -> Self {
        Self {
            char_spacing: 0.0,
            word_spacing: 0.0,
            horizontal_scaling: DEFAULT_HORIZONTAL_SCALING,
            leading: 0.0,
            font: None,
            render_mode: RenderMode::Fill,
            rise: 0.0,
        }
    }
}

/// The graphics state (8.4, Table 52), with the transparency parameters of
/// Table 58 and the text state. Colours are kept as written, not converted.
#[derive(Debug, Clone)]
pub struct GraphicsState {
    /// The current transformation matrix: user space to the space the
    /// caller started the interpreter in (default user space when it
    /// started with the identity).
    pub ctm: Matrix,
    pub fill: Paint,
    pub stroke: Paint,
    pub line_width: f64,
    pub line_cap: LineCap,
    pub line_join: LineJoin,
    pub miter_limit: f64,
    pub dash: Dash,
    /// `RI`, the rendering intent, as a name (`RelativeColorimetric` is
    /// the default).
    pub rendering_intent: Name,
    pub flatness: f64,
    pub stroke_adjustment: bool,
    /// `CA`, the stroking alpha constant.
    pub stroke_alpha: f64,
    /// `ca`, the nonstroking alpha constant.
    pub fill_alpha: f64,
    /// `BM`, the blend mode, as the name or the first name of the array
    /// the entry gave.
    pub blend_mode: Name,
    pub soft_mask: SoftMask,
    /// `AIS`, whether the alpha is a shape rather than an opacity.
    pub alpha_is_shape: bool,
    /// `TK`, the text knockout.
    pub text_knockout: bool,
    /// `OP`, overprint for stroking (and for all painting when `op` is
    /// absent).
    pub stroke_overprint: bool,
    /// `op`, overprint for nonstroking painting.
    pub fill_overprint: bool,
    /// `OPM`, the overprint mode.
    pub overprint_mode: i64,
    pub text: TextState,
}

impl GraphicsState {
    /// The state a page's content stream starts in (8.4.1), with `ctm` as
    /// the matrix.
    pub fn initial(ctm: Matrix) -> Self {
        Self {
            ctm,
            fill: Paint::black(),
            stroke: Paint::black(),
            line_width: DEFAULT_LINE_WIDTH,
            line_cap: LineCap::Butt,
            line_join: LineJoin::Miter,
            miter_limit: DEFAULT_MITER_LIMIT,
            dash: Dash::default(),
            rendering_intent: Name::new(b"RelativeColorimetric".to_vec()),
            flatness: DEFAULT_FLATNESS,
            stroke_adjustment: false,
            stroke_alpha: 1.0,
            fill_alpha: 1.0,
            blend_mode: Name::new(b"Normal".to_vec()),
            soft_mask: SoftMask::None,
            alpha_is_shape: false,
            text_knockout: true,
            stroke_overprint: false,
            fill_overprint: false,
            overprint_mode: 0,
            text: TextState::default(),
        }
    }
}
