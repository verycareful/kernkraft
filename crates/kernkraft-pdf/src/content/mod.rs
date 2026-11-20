// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The content-stream interpreter: ISO 32000-1:2008, 7.8 (content streams
//! and resources), 8 (graphics), 9 (text), 14.6 (marked content) and 8.11
//! (optional content).
//!
//! An [`Interpreter`] runs a page's content (or a form XObject) and tells a
//! [`Device`] what is painted: paths, text, images, shadings, forms and
//! marked content, each with the graphics state at that point and the
//! [`OperatorSpan`] of the operation that painted it, so that an editor can
//! find the operation again in the decoded stream
//! ([`Document::content_stream`](crate::doc::Document::content_stream)).
//! The text layer, the renderer and the text editor are devices.
//!
//! Colours are kept as the stream gave them, in the colour space it chose;
//! turning them into device colours is the colour layer's
//! ([`Document::paint_colour_space`](crate::doc::Document::paint_colour_space)
//! reads the space, [`ColourSpace::to_rgb`](crate::colour::ColourSpace::to_rgb)
//! converts a colour). Type 3 glyph
//! descriptions are not run by the interpreter: a glyph of a Type 3 font is
//! reported with the text, and a renderer runs the description it names.

mod device;
mod interpreter;
mod load;
mod matrix;
mod operation;
mod optional;
mod state;
mod text;
mod xobject;

pub(crate) use device::glyph_of;
pub use device::{
    Context, Device, ExtGStateInfo, FillRule, FormInfo, ImageSource, MarkedContent, OperatorSpan,
    Path, PathPaint, PathSegment, PlacedGlyph, StreamId, TextRun,
};
pub use interpreter::{InterpretOptions, Interpreter, PatternCell, DEFAULT_MAX_OPERATIONS};
pub use load::{ContentPart, ContentStream, DEFAULT_MAX_CONTENT_BYTES};
pub use matrix::Matrix;
pub use operation::{ContentParser, InlineImage, Operation, MAX_OPERANDS};
pub use optional::OptionalContent;
pub use state::{
    ColorSpace, Dash, GraphicsState, LineCap, LineJoin, Paint, RenderMode, SoftMask, TextFont,
    TextState, DEFAULT_FLATNESS, DEFAULT_HORIZONTAL_SCALING, DEFAULT_LINE_WIDTH,
    DEFAULT_MITER_LIMIT, GLYPH_UNITS_PER_EM, PERCENT,
};
