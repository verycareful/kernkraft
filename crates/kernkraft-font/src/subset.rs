// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reducing a font to the glyphs a document shows, for embedding.
//!
//! Formats: OpenType 1.9 (`glyf`, `loca`, `CFF `, `CFF2`, `fvar`, `gvar`,
//! `hmtx`, `head`, `maxp`, `post`, `name`); ISO 32000-1:2008, 9.9 (embedded
//! font programs) and 9.7.4.2 (glyph selection in CIDFonts).
//!
//! A [`GlyphSet`] numbers the glyphs in the order they are added; the number
//! is the glyph's index in the subset and, for a PDF CIDFont, its CID.
//! [`subset`] keeps those glyphs and the components of any composite glyph
//! among them (components take the numbers after the set's own). The
//! subset has no `cmap`, `OS/2` or `GSUB` table: a PDF font addresses glyphs
//! through its own encoding, so it is only fit to be embedded.
//!
//! A variable font is reduced at one instance ([`Variations`]) and the subset
//! is static: a `PDF` font program has no variation tables. A `CFF2` font, and
//! a `glyf` font at an instance other than its default, are redrawn as `glyf`
//! outlines (cubic curves become quadratic, hinting is dropped).

use subsetter::GlyphRemapper;

use crate::error::FontError;
use crate::program::{face_index, Format, GlyphId, Program};
use crate::variation::Variations;

/// The glyphs to keep, numbered from 0 in the order they were added. Glyph 0,
/// `.notdef`, is always the first.
#[derive(Debug, Clone)]
pub struct GlyphSet {
    /// The glyph count of the font the set is for.
    glyph_count: u16,
    remapper: GlyphRemapper,
}

impl GlyphSet {
    /// An empty set (`.notdef` only) for a font of `glyph_count` glyphs
    /// ([`Program::glyph_count`]).
    pub fn new(glyph_count: u16) -> GlyphSet {
        GlyphSet {
            glyph_count,
            remapper: GlyphRemapper::new(),
        }
    }

    /// Adds `glyph` and gives its number in the subset; a glyph already in
    /// the set gives its number again.
    ///
    /// [`FontError::NoSuchGlyph`] when the font has no such glyph.
    pub fn add(&mut self, glyph: GlyphId) -> Result<u16, FontError> {
        if glyph.0 >= self.glyph_count {
            return Err(FontError::NoSuchGlyph(glyph));
        }
        Ok(self.remapper.remap(glyph.0))
    }

    /// The number of `glyph` in the subset, when it was added.
    pub fn get(&self, glyph: GlyphId) -> Option<u16> {
        self.remapper.get(glyph.0)
    }

    /// The glyphs in the set, `.notdef` included.
    pub fn len(&self) -> usize {
        usize::from(self.remapper.num_gids())
    }

    /// Always false: `.notdef` is in every set. Present for the
    /// `len`/`is_empty` pair.
    pub fn is_empty(&self) -> bool {
        false
    }

    /// The glyphs of the font in subset order: the one at index `n` is the
    /// subset's glyph `n`.
    pub fn glyphs(&self) -> impl Iterator<Item = GlyphId> + '_ {
        self.remapper.remapped_gids().map(GlyphId)
    }
}

/// A font reduced to a [`GlyphSet`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Subset {
    /// [`Format::TrueType`] or [`Format::OpenTypeCff`], read from the subset
    /// itself: a `CFF2` font gives [`Format::TrueType`]. An OpenType subset's
    /// CFF table is CID-keyed with CID equal to glyph number, whatever the
    /// original's charset was.
    pub format: Format,
    /// The subset, an sfnt whose glyph `n` is the set's glyph `n`.
    pub data: Vec<u8>,
}

/// Reduces the font program `data` (`index` picks a font out of a
/// collection) to the glyphs of `glyphs`, at the instance `variations` picks
/// (an empty one is the font's default instance).
///
/// [`FontError::Subset`] for a program that is not an sfnt with `glyf`,
/// `CFF ` or `CFF2` outlines (a bare CFF or Type 1 program cannot be reduced
/// here) and a set made for a font of another glyph count; the errors of
/// [`Variations::check`] for an instance the font does not have; the other
/// errors are those of [`Program::parse`].
pub fn subset(
    data: &[u8],
    index: u32,
    glyphs: &GlyphSet,
    variations: &Variations,
) -> Result<Subset, FontError> {
    let program = Program::parse(data, index)?;
    match program.format() {
        Format::TrueType | Format::OpenTypeCff => {}
        Format::SfntNoOutlines => return Err(FontError::Subset("the font has no outlines")),
        Format::Cff => return Err(FontError::Subset("a bare CFF program is not an sfnt")),
        Format::Type1 => return Err(FontError::Subset("a Type 1 program is not an sfnt")),
    }
    if program.glyph_count() != glyphs.glyph_count {
        return Err(FontError::Subset(
            "the glyph set is for a font of another glyph count",
        ));
    }
    variations.check(&program.axes())?;
    let coordinates: Vec<(subsetter::Tag, f32)> = variations
        .settings()
        .iter()
        .map(|(tag, value)| (subsetter::Tag::new(tag), *value))
        .collect();
    let bytes = subsetter::subset_with_variations(
        data,
        face_index(data, index),
        &coordinates,
        &glyphs.remapper,
    )
    .map_err(|e| {
        FontError::Subset(match e {
            subsetter::Error::Unimplemented => "the font uses a feature that cannot be reduced",
            subsetter::Error::UnknownKind => {
                "the font has no outlines of a kind that can be reduced"
            }
            subsetter::Error::MalformedFont | subsetter::Error::CFFError => "the font is damaged",
            subsetter::Error::SubsetError | subsetter::Error::OverflowError => {
                "the font could not be reduced"
            }
        })
    })?;
    let format = Program::parse(&bytes, 0)?.format();
    Ok(Subset {
        format,
        data: bytes,
    })
}
