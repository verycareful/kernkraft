// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The font new text is set in: ISO 32000-1:2008, 9.6 (simple fonts),
//! 9.7 (composite fonts), 9.3.3 (word spacing applies to the single-byte
//! code 32 only), 9.8 (Table 122: `Ascent`, `Descent`, `Leading`) and
//! 9.9 (embedded font programs).
//!
//! The font the block was set in is used again when it can show every
//! character of the new text: a simple font whose encoding gives each
//! character a code and, when the font program is embedded, whose program
//! has marks for that code (a subset has only the glyphs the page used).
//! Otherwise a face of the application's font list is embedded.

use std::collections::HashMap;

use kernkraft_font::{FontList, Query};

use crate::content::glyph_of;
use crate::diagnostic::Diagnostics;
use crate::doc::Document;
use crate::edit_text::collect::ShowOp;
use crate::font::{FontEmbedder, FontKind};
use crate::render::glyphs::{Glyphs, Presence};
use crate::syntax::Name;
use crate::text::collect::{FALLBACK_ASCENT, FALLBACK_DESCENT};
use crate::PdfError;

/// The usual weight of a bold face and of a regular one (CSS Fonts 4,
/// 5.2: 700 and 400).
pub(super) const WEIGHT_BOLD: u16 = 700;
pub(super) const WEIGHT_REGULAR: u16 = 400;
/// 9.6.4: a subset tag is six capital letters.
const SUBSET_TAG_LETTERS: usize = 6;

/// A font the page already has.
pub(super) struct Reused {
    /// The name the page's `Font` resources give it.
    pub name: Name,
    codes: HashMap<char, (u8, f64)>,
    /// `Ascent`, `Descent` and `Leading` of Table 122, in thousandths of
    /// an em.
    line: f64,
}

/// A face of the font list, to be embedded.
pub(super) struct Embedded {
    pub embedder: FontEmbedder,
    pub family: String,
}

pub(super) enum Face {
    Reused(Reused),
    Embedded(Embedded),
}

impl Face {
    /// The width of `c` in thousandths of a text space unit.
    pub(super) fn width(&self, c: char) -> Result<f64, PdfError> {
        match self {
            Face::Reused(reused) => reused
                .codes
                .get(&c)
                .map(|(_, width)| *width)
                .ok_or(PdfError::NoGlyphForCharacter { character: c }),
            Face::Embedded(embedded) => embedded.embedder.width(c.encode_utf8(&mut [0; 4])),
        }
    }

    /// Whether word spacing moves the glyph of `c` (9.3.3: the single-byte
    /// code 32 only).
    pub(super) fn takes_word_spacing(&self, c: char) -> bool {
        matches!(self, Face::Reused(reused) if reused.codes.get(&c).is_some_and(|(code, _)| *code == 32))
    }

    pub(super) fn encode(&mut self, text: &str) -> Result<Vec<u8>, PdfError> {
        match self {
            Face::Reused(reused) => text
                .chars()
                .map(|c| {
                    reused
                        .codes
                        .get(&c)
                        .map(|(code, _)| *code)
                        .ok_or(PdfError::NoGlyphForCharacter { character: c })
                })
                .collect(),
            Face::Embedded(embedded) => embedded.embedder.encode(text),
        }
    }

    /// The distance between lines of this font, in thousandths of an em.
    pub(super) fn line_height(&self) -> Result<f64, PdfError> {
        match self {
            Face::Reused(reused) => Ok(reused.line),
            Face::Embedded(embedded) => embedded.embedder.line_height(),
        }
    }
}

/// The font of `show` again, when it can show every character of `text`.
pub(super) fn reuse(
    doc: &Document,
    show: &ShowOp,
    text: &str,
    diagnostics: &mut Diagnostics,
) -> Result<Option<Reused>, PdfError> {
    let font = &show.run.font;
    if !matches!(
        font.kind(),
        FontKind::Type1 | FontKind::MMType1 | FontKind::TrueType
    ) {
        return Ok(None);
    }
    let embedded = font.embedded_program(doc).ok().flatten().is_some();
    let mut codes: HashMap<char, (u8, f64)> = HashMap::new();
    for code in 0..=u8::MAX {
        let bytes = [code];
        let Some(ch) = font.chars(&bytes).next() else {
            continue;
        };
        let Some(unicode) = ch.unicode.as_deref() else {
            continue;
        };
        // A code the font states no width for takes `MissingWidth`, often 0:
        // text set with it would overlap its neighbours.
        if !font.has_width(code) {
            continue;
        }
        let mut chars = unicode.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            codes.entry(c).or_insert((code, ch.width));
        }
    }
    let mut glyphs = Glyphs::new(doc, None);
    for c in text.chars() {
        let Some(&(code, _)) = codes.get(&c) else {
            return Ok(None);
        };
        if embedded {
            let bytes = [code];
            let Some(ch) = font.chars(&bytes).next() else {
                return Ok(None);
            };
            let glyph = glyph_of(&ch, 0);
            match glyphs.presence(&show.run, &glyph, diagnostics) {
                Presence::Marks => {}
                Presence::Blank if c.is_whitespace() => {}
                Presence::Blank | Presence::Missing => return Ok(None),
            }
        }
    }
    let descriptor = font.descriptor();
    let (ascent, descent, leading) =
        descriptor.map_or((FALLBACK_ASCENT, FALLBACK_DESCENT, 0.0), |descriptor| {
            (
                descriptor.ascent.abs(),
                -descriptor.descent.abs(),
                descriptor.leading,
            )
        });
    Ok(Some(Reused {
        name: show.run.font_name.clone(),
        codes,
        line: ascent - descent + leading,
    }))
}

/// The PostScript name of `show`'s font without its subset tag (9.6.4: six
/// capitals and a plus), which names the face the font was made from.
pub(super) fn postscript_name(show: &ShowOp) -> Option<String> {
    let name = show.run.font.base_font()?;
    Some(match name.split_once('+') {
        Some((tag, rest))
            if tag.len() == SUBSET_TAG_LETTERS && tag.bytes().all(|b| b.is_ascii_uppercase()) =>
        {
            rest.to_owned()
        }
        _ => name.to_owned(),
    })
}

/// A face of `list` with a glyph for every character of `text`: the one
/// whose PostScript name is `postscript`, else of the families, weight and
/// slant of `query`.
pub(super) fn embed(
    list: &FontList,
    query: &Query<'_>,
    text: &str,
    postscript: Option<&str>,
) -> Result<Embedded, PdfError> {
    let covers_all = |face: &&kernkraft_font::FaceInfo| text.chars().all(|c| face.covers(c));
    // The face the block's font was made from, when the list has it.
    let mut face = postscript.and_then(|wanted| {
        list.faces()
            .iter()
            .filter(|face| {
                face.postscript_name
                    .as_deref()
                    .is_some_and(|name| name.eq_ignore_ascii_case(wanted))
            })
            .find(covers_all)
    });
    if face.is_none() {
        face = list.by_family(query).into_iter().find(covers_all);
    }
    if face.is_none() {
        // A family that lacks a character: the closest face that has it.
        let missing = text.chars().find(|c| {
            !list
                .by_family(query)
                .first()
                .is_some_and(|face| face.covers(*c))
        });
        if let Some(c) = missing {
            face = list.covering(c, query).filter(covers_all);
        }
    }
    let Some(face) = face else {
        let character = text
            .chars()
            .find(|c| list.covering(*c, query).is_none())
            .or_else(|| text.chars().next())
            .unwrap_or(' ');
        return Err(PdfError::NoGlyphForCharacter { character });
    };
    let data = std::fs::read(&face.path).map_err(|source| {
        PdfError::Fs(kernkraft_fs::FsError::Io {
            path: face.path.clone(),
            source,
        })
    })?;
    let variations = face
        .instance
        .as_ref()
        .map(kernkraft_font::Instance::variations)
        .unwrap_or_default();
    let embedder = FontEmbedder::with_variations(data, face.index, &variations)?;
    for character in text.chars() {
        if !embedder.covers(character)? {
            return Err(PdfError::NoGlyphForCharacter { character });
        }
    }
    let family = face.families.first().cloned().unwrap_or_default();
    Ok(Embedded { embedder, family })
}
