// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Font programs, glyph outlines and metrics.
//!
//! [`Program::parse`] reads TrueType, OpenType, bare CFF and Type 1 font
//! programs behind one interface: glyph lookup by name, Unicode, `cmap`
//! subtable, built-in encoding or CID, horizontal advances, outlines as path
//! segments in design units, and font-wide metrics. The crate knows nothing
//! of PDF.
//!
//! [`subset`] reduces a TrueType or OpenType font to a [`GlyphSet`] for
//! embedding in a document.
//!
//! [`FontList::index`] reads the fonts an application hands in (paths) and
//! [`FontList::best`], [`FontList::by_family`] and [`FontList::covering`]
//! choose from them by family, width, slant, weight and character coverage.

mod coverage;
mod error;
mod list;
mod matching;
mod outline;
mod program;
mod subset;
pub mod type1;
mod variation;

pub use coverage::Coverage;
pub use error::FontError;
pub use list::{FaceInfo, FontList, Instance, Skipped};
pub use matching::Query;
pub use outline::{Outline, Point, Segment};
pub use program::{CmapSelector, Format, GlyphId, Metrics, Names, Program};
pub use subset::{subset, GlyphSet, Subset};
pub use variation::{Axis, AxisTag, Variations, AXIS_ITALIC, AXIS_SLANT, AXIS_WEIGHT};
