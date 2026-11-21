// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Annotations: ISO 32000-1:2008, 12.5.2 (Table 164: the entries every
//! annotation has, Table 165: the flags, Table 166: `Border` and `BS`),
//! 12.5.5 (appearance streams), 12.5.6.4 (Table 172: text annotations),
//! 12.5.6.10 (Table 179: text markup annotations), 12.5.6.13 (Table 184:
//! ink annotations) and 8.4.5 (Table 58: `CA`, `ca` and `BM` of an
//! `ExtGState`).
//!
//! [`Document::annotations`](crate::doc::Document::annotations) lists the annotations of a page. The library
//! writes five kinds, each with the appearance stream a reader draws: a
//! highlight, an underline and a strikeout over text ([`NewShape`]), an
//! ink stroke and a note. [`add_annotation`] makes one, [`edit_annotation`]
//! changes one and [`delete_annotation`] takes one away, each giving a new
//! snapshot of the document for [`EditSession::push`](crate::EditSession::push).
//!
//! Geometry is given and returned as fractions of the displayed page
//! ([`Rect`], the `(x, y)` pairs of a stroke): `(0, 0)` is its top left
//! corner and `y` grows downwards, as for the text layer and the links.
//! The file holds user space numbers; the page's `CropBox` and `Rotate`
//! are applied in both directions.
//!
//! An edit changes only what it names: every other entry of the
//! annotation, those this library does not understand included, stays as
//! it was. Changing what the appearance shows (colour, opacity, line
//! width, position) draws the appearance again, which replaces the one the
//! file had; that is refused for a kind the library does not write.

mod appearance;
mod geometry;
mod read;
mod write;

use crate::colour::Rgb;
use crate::doc::Date;
use crate::syntax::ObjectId;
use crate::Rect;

pub use write::{add_annotation, delete_annotation, edit_annotation};

/// Table 165: the bit of each flag, counted from 1.
const FLAG_PRINT: i64 = 1 << 2;
const FLAG_NO_ZOOM: i64 = 1 << 3;
const FLAG_NO_ROTATE: i64 = 1 << 4;

/// The kind of an annotation, by its `Subtype`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnnotationKind {
    Highlight,
    Underline,
    StrikeOut,
    Ink,
    /// A text annotation: a note shown as an icon.
    Note,
    /// Any other `Subtype`, by name.
    Other(String),
}

impl AnnotationKind {
    /// Whether the library draws the appearance of this kind, so that
    /// [`edit_annotation`] can change what it shows.
    pub fn is_drawn_by_library(&self) -> bool {
        !matches!(self, Self::Other(_))
    }
}

/// An annotation of a page, as [`Document::annotations`](crate::doc::Document::annotations) gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct Annotation {
    /// The annotation's object. `None` for one written directly in the
    /// page's `Annots`, which no operation can name.
    pub id: Option<ObjectId>,
    pub kind: AnnotationKind,
    /// Where the annotation is shown.
    pub rect: Rect,
    /// A highlight, underline or strikeout: the box of each `QuadPoints`
    /// quadrilateral.
    pub quads: Vec<Rect>,
    /// An ink annotation: the points of each stroke.
    pub strokes: Vec<Vec<(f64, f64)>>,
    /// `Contents`: the text of a note, or the text a markup carries.
    pub contents: Option<String>,
    /// `T`: who made it.
    pub author: Option<String>,
    /// `M`: when it was last changed.
    pub modified: Option<Date>,
    /// `C`, when it is a colour of one, three or four components.
    pub colour: Option<Rgb>,
    /// `CA`, 1 when the file gives none.
    pub opacity: f64,
    /// An ink annotation's line width in points (`BS`'s `W`, else
    /// `Border`'s third number, else 1).
    pub line_width: Option<f64>,
    /// `F`.
    pub flags: i64,
}

/// What a new annotation draws.
#[derive(Debug, Clone, PartialEq)]
pub enum NewShape {
    /// The text under each rectangle (the `rects` of a
    /// [`Selection`](crate::text::Selection)) shaded, as a highlighter
    /// does: the colour is multiplied with what is below it.
    Highlight { quads: Vec<Rect> },
    /// A line along the bottom of each rectangle.
    Underline { quads: Vec<Rect> },
    /// A line through the middle of each rectangle.
    StrikeOut { quads: Vec<Rect> },
    /// Freehand strokes, each at least one point, drawn `width` points
    /// wide with round caps and joins.
    Ink {
        strokes: Vec<Vec<(f64, f64)>>,
        width: f64,
    },
    /// A note, its icon's top left corner at the point. The icon keeps its
    /// size and stays upright whatever the zoom and the page's rotation
    /// (the `NoZoom` and `NoRotate` flags).
    Note { at: (f64, f64) },
}

/// An annotation to add with [`add_annotation`].
#[derive(Debug, Clone, PartialEq)]
pub struct NewAnnotation {
    pub shape: NewShape,
    /// Each component 0 to 1.
    pub colour: Rgb,
    /// 0 (nothing shows) to 1 (opaque).
    pub opacity: f64,
    /// The text of a note, or a comment on a markup.
    pub contents: Option<String>,
    pub author: Option<String>,
    /// The time of the change: the library reads no clock.
    pub modified: Date,
}

/// What [`add_annotation`] made.
#[derive(Debug, Clone)]
pub struct Added {
    /// The document with the annotation on its page.
    pub document: crate::doc::Document,
    /// The annotation's object.
    pub id: ObjectId,
}

/// What [`edit_annotation`] changes: each field that is `Some` replaces
/// the annotation's value.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnnotationEdit {
    pub colour: Option<Rgb>,
    pub opacity: Option<f64>,
    /// An ink annotation's line width, in points.
    pub line_width: Option<f64>,
    /// The new `Contents`; an empty text takes the entry away.
    pub contents: Option<String>,
    /// The new `T`; an empty text takes the entry away.
    pub author: Option<String>,
    /// How far to move the annotation, as fractions of the displayed
    /// page.
    pub translate: Option<(f64, f64)>,
}
