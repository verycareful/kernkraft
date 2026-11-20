// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use crate::variation::AxisTag;
use crate::GlyphId;

/// What went wrong reading a font program.
#[derive(Debug, Clone, thiserror::Error, PartialEq, Eq)]
pub enum FontError {
    /// The bytes are not a font program this crate reads.
    #[error("not a font program this crate reads")]
    UnknownFormat,
    /// A TrueType or OpenType file whose structure is damaged.
    #[error("damaged TrueType or OpenType font: {0}")]
    Sfnt(&'static str),
    /// A collection holds fewer fonts than the index asked for.
    #[error("font index {index} is past the {count} fonts of the collection")]
    CollectionIndex { index: u32, count: u32 },
    /// A bare CFF font program whose structure is damaged.
    #[error("damaged CFF font program")]
    Cff,
    /// A Type 1 font program whose structure is damaged.
    #[error("damaged Type 1 font program: {0}")]
    Type1(&'static str),
    /// A font file could not be read.
    #[error("cannot read the font file: {kind}")]
    Io { kind: std::io::ErrorKind },
    /// The caller cancelled the indexing of a font list.
    #[error("cancelled")]
    Cancelled,
    /// The font has no glyph with this number.
    #[error("no glyph {} in the font", .0.0)]
    NoSuchGlyph(GlyphId),
    /// The glyph has no vector outline: the font draws it from bitmaps (a
    /// colour emoji font) or has outlines for no glyph at all.
    #[error("glyph {} has no vector outline", .0.0)]
    NoOutline(GlyphId),
    /// The font cannot be reduced to a subset, for the reason given.
    #[error("cannot subset the font: {0}")]
    Subset(&'static str),
    /// An instance names an axis, and the font has no variation axes.
    #[error("the font is not a variable font")]
    NoVariationAxes,
    /// An instance names an axis the font does not have.
    #[error("the font has no axis {}", tag_text(.tag))]
    UnknownAxis { tag: AxisTag },
    /// An instance names an axis twice.
    #[error("the axis {} is given twice", tag_text(.tag))]
    DuplicateAxis { tag: AxisTag },
    /// An axis value outside the axis's range, or not a number. The range is
    /// the one [`crate::Program::axes`] gives for the axis.
    #[error("the value given for the axis {} is outside its range", tag_text(.tag))]
    AxisOutOfRange { tag: AxisTag },
    /// A glyph description that cannot be turned into an outline.
    #[error("damaged outline of glyph {}: {reason}", .glyph.0)]
    Outline {
        glyph: GlyphId,
        reason: &'static str,
    },
}

fn tag_text(tag: &AxisTag) -> String {
    String::from_utf8_lossy(tag).into_owned()
}
