// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What the interpreter tells a consumer: the [`Device`] trait and the
//! events it receives. ISO 32000-1:2008, 8.5 (path construction and
//! painting), 9.4 (text objects), 8.8 (images), 8.10 (form XObjects),
//! 14.6 (marked content) and 8.11 (optional content).
//!
//! The text layer, the renderer and the text editor are all devices. The
//! interpreter owns the graphics state and passes it, as it stands, with
//! every event; a device that needs a clipping stack or a group stack keeps
//! it from the [`Device::save`], [`Device::restore`], [`Device::clip`] and
//! form events.

use std::sync::Arc;

use crate::content::matrix::Matrix;
use crate::content::operation::InlineImage;
use crate::content::state::GraphicsState;
use crate::font::{Char, Font};
use crate::syntax::{Dictionary, Name, Object, ObjectId, Span};

/// Which content stream an operation is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum StreamId {
    /// The `Contents` of a page, its streams joined (7.8.2): the page object.
    Page(ObjectId),
    /// A stream of its own: a form XObject, a tiling pattern, a Type 3
    /// glyph description or an annotation's appearance.
    Object(ObjectId),
}

/// Where in a content stream an operation is, for an editor to find it
/// again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OperatorSpan {
    pub stream: StreamId,
    /// The bytes of the operation (its operands and its operator) in the
    /// decoded stream; for the `Contents` of a page, in the decoded streams
    /// joined by one line feed each.
    pub range: Span,
    /// The operation's place in its stream, counted from 0.
    pub index: u32,
}

/// A segment of a path (8.5.2). A rectangle (`re`) is the four segments
/// and the close the format defines it as.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PathSegment {
    MoveTo(f64, f64),
    LineTo(f64, f64),
    /// Two control points and the end point.
    CurveTo(f64, f64, f64, f64, f64, f64),
    Close,
}

/// A path in user space: the coordinates the operators gave. The
/// matrix that maps them to the device is [`GraphicsState::ctm`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Path {
    pub segments: Vec<PathSegment>,
}

impl Path {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }
}

/// The rule that tells inside from outside (8.5.3.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FillRule {
    NonZeroWinding,
    EvenOdd,
}

/// What a path-painting operator does to the path (Table 59).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PathPaint {
    pub fill: Option<FillRule>,
    pub stroke: bool,
}

/// What every event carries beside its own data.
#[derive(Debug, Clone, Copy)]
pub struct Context<'a> {
    /// The operation that caused the event.
    pub span: OperatorSpan,
    /// The graphics state as it was when the operation ran, after the
    /// operation's own changes for a state operator and before them for a
    /// painting one.
    pub state: &'a GraphicsState,
    /// The marked-content sequences open, outermost first.
    pub marked: &'a [MarkedContent],
    /// False when optional content hides what the event paints (8.11): the
    /// event is still sent, so that a text layer or an editor can see
    /// hidden content.
    pub visible: bool,
}

/// A marked-content sequence that is open (14.6).
#[derive(Debug, Clone, PartialEq)]
pub struct MarkedContent {
    /// The tag.
    pub tag: Name,
    /// The property list, resolved: the inline dictionary of `BDC`, or the
    /// dictionary the `Properties` resource names. `None` for `BMC`.
    pub properties: Option<Dictionary>,
    /// The `Properties` name, when the list came from there.
    pub properties_name: Option<Name>,
    /// The object of the property list, when it is an indirect one (an
    /// optional-content group or membership dictionary is).
    pub properties_id: Option<ObjectId>,
    /// Whether this sequence hides its content by optional content
    /// (`/OC` tag with a group that is off).
    pub hidden: bool,
}

impl MarkedContent {
    /// The marked-content identifier (`MCID`) of a tagged-content
    /// sequence (14.7.4.4), when the property list has one.
    pub fn mcid(&self) -> Option<i64> {
        match self.properties.as_ref()?.get(b"MCID")?.value() {
            crate::syntax::Value::Integer(mcid) => Some(*mcid),
            _ => None,
        }
    }
}

/// One glyph shown by a text-showing operator, placed.
#[derive(Debug, Clone)]
pub struct PlacedGlyph {
    /// The character code, as the string's bytes give it.
    pub code: u32,
    /// The bytes the code takes in the string.
    pub len: usize,
    /// The CID of a composite font's character; the code for a simple font.
    pub cid: u32,
    pub glyph_name: Option<String>,
    /// The Unicode text of the character, when it can be told.
    pub unicode: Option<String>,
    /// The horizontal advance, in thousandths of a text space unit.
    pub width: f64,
    /// True for the single-byte code 32.
    pub is_space: bool,
    /// The text rendering matrix of the glyph (9.4.4): it maps the
    /// font's em square (a glyph's design units divided by
    /// [`GLYPH_UNITS_PER_EM`](crate::content::state::GLYPH_UNITS_PER_EM),
    /// or a Type 3 glyph's by its `FontMatrix`) to the space the CTM
    /// leads to, the glyph's origin at its translation.
    pub matrix: Matrix,
    /// How far the glyph moves the text matrix, in text space: the
    /// translation `(tx, ty)` of 9.4.4, spacing included.
    pub advance: (f64, f64),
    /// Which string of the operator the glyph is from: always 0 for
    /// `Tj`, `'` and `"`, and the index in the array operand of `TJ`.
    pub operand: u32,
}

/// The text a showing operator (`Tj`, `TJ`, `'`, `"`) paints.
#[derive(Debug, Clone)]
pub struct TextRun {
    pub font_name: Name,
    pub font: Arc<Font>,
    pub font_size: f64,
    /// The glyphs, in the order shown.
    pub glyphs: Vec<PlacedGlyph>,
    /// The number items of a `TJ` array, each with the index of the
    /// operand it was (the same numbering as [`PlacedGlyph::operand`]) and
    /// the displacement it made in text space.
    pub adjustments: Vec<(u32, f64)>,
    /// The text matrix before the operator and after it.
    pub matrix_before: Matrix,
    pub matrix_after: Matrix,
}

/// An image to paint (8.9): it fills the unit square of user space.
#[derive(Debug, Clone)]
pub enum ImageSource {
    /// An image XObject, by its name in the `XObject` resources, and its
    /// object.
    XObject { name: Name, id: ObjectId },
    /// An inline image (8.9.7).
    Inline(InlineImage),
}

/// A form XObject being run (8.10).
#[derive(Debug, Clone)]
pub struct FormInfo {
    pub id: ObjectId,
    /// The name it was invoked by, when it was invoked with `Do`.
    pub name: Option<Name>,
    /// `BBox`, as written, when it is a rectangle.
    pub bbox: Option<[f64; 4]>,
    /// `Matrix`: the identity when absent.
    pub matrix: Matrix,
    /// The transparency group attributes, `Group`, when the form has them
    /// (11.6.6).
    pub group: Option<Dictionary>,
}

/// What an operator that selects an extended graphics state (`gs`) did.
#[derive(Debug, Clone)]
pub struct ExtGStateInfo {
    pub name: Name,
    pub dictionary: Dictionary,
}

/// A consumer of content: every method has an empty default, so a device
/// implements what it needs.
///
/// The interpreter calls a method as an operation runs and never calls it
/// again for the same operation. A device that wants to stop the
/// interpretation says so through the [`Progress`](kernkraft_fs::Progress)
/// the caller gave the interpreter.
#[allow(unused_variables)]
pub trait Device {
    /// `q` (after the state is saved).
    fn save(&mut self, ctx: &Context<'_>) {}

    /// `Q` (after the state is restored).
    fn restore(&mut self, ctx: &Context<'_>) {}

    /// A path was painted by `S`, `s`, `f`, `F`, `f*`, `B`, `B*`, `b` or
    /// `b*`. The path ended by `n` is not painted: it sends no event
    /// unless `W` or `W*` made it a clip ([`Self::clip`]).
    ///
    /// `construction` spans from the path's first construction operator to
    /// the painting operator, in the one stream; `ctx.span` is the
    /// painting operator.
    fn path(
        &mut self,
        ctx: &Context<'_>,
        path: &Path,
        paint: PathPaint,
        construction: OperatorSpan,
    ) {
    }

    /// The clipping path is intersected with `path` (`W` or `W*`, applied
    /// by the next path-painting operator, 8.5.4). It is sent after that
    /// operator's [`Self::path`] event, since the clip takes effect once
    /// the painting is done.
    fn clip(&mut self, ctx: &Context<'_>, path: &Path, rule: FillRule) {}

    /// `BT`.
    fn begin_text(&mut self, ctx: &Context<'_>) {}

    /// `ET`. The glyphs of modes 4 to 7 have been added to the clipping
    /// path by the device's own bookkeeping.
    fn end_text(&mut self, ctx: &Context<'_>) {}

    /// A text-showing operator.
    fn text(&mut self, ctx: &Context<'_>, run: &TextRun) {}

    /// `Do` of an image XObject, or an inline image.
    fn image(&mut self, ctx: &Context<'_>, image: &ImageSource) {}

    /// `sh`: the shading named `name` in the `Shading` resources, `object`
    /// as the resource gives it, fills the current clipping path.
    fn shading(&mut self, ctx: &Context<'_>, name: &Name, object: &Object) {}

    /// A form XObject starts (`Do`): the state has been saved, the
    /// form's matrix concatenated and its `BBox` sent as a clip.
    fn begin_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {}

    /// The form XObject ends; the state is restored after this call.
    fn end_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {}

    /// `BMC` or `BDC`: `ctx.marked` already holds the new sequence last.
    fn begin_marked(&mut self, ctx: &Context<'_>) {}

    /// `EMC`: `ctx.marked` no longer holds the closed sequence; `closed`
    /// is it.
    fn end_marked(&mut self, ctx: &Context<'_>, closed: &MarkedContent) {}

    /// `MP` or `DP`: a marked-content point.
    fn marked_point(&mut self, ctx: &Context<'_>, tag: &Name, properties: Option<&Dictionary>) {}

    /// `gs` set the state from the extended graphics state `info`; the
    /// state in `ctx` already has the changes.
    fn ext_gstate(&mut self, ctx: &Context<'_>, info: &ExtGStateInfo) {}

    /// `BX` and `EX`: whether unknown operators are tolerated is the
    /// interpreter's; the device sees only the operator.
    fn compatibility(&mut self, ctx: &Context<'_>, begin: bool) {}
}

/// A [`Char`] as a [`PlacedGlyph`] with no placement yet.
pub(crate) fn glyph_of(ch: &Char<'_>, operand: u32) -> PlacedGlyph {
    PlacedGlyph {
        code: ch.code,
        len: ch.len,
        cid: ch.cid,
        glyph_name: ch.glyph_name.map(str::to_owned),
        unicode: ch.unicode.as_ref().map(|text| text.to_string()),
        width: ch.width,
        is_space: ch.is_space,
        matrix: Matrix::IDENTITY,
        advance: (0.0, 0.0),
        operand,
    }
}
