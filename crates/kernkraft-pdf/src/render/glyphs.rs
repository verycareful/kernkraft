// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The outline of a glyph a text-showing operator paints: ISO 32000-1:2008,
//! 9.6.6 (character encoding of simple fonts, 9.6.6.2 for Type 1 and 9.6.6.4
//! for TrueType), 9.7.4.2 (glyph selection in CIDFonts) and 9.6.2.2 (a font
//! that is not embedded). The font programs are read by `kernkraft-font`;
//! this module chooses the glyph in them.
//!
//! A font with an embedded program draws with it. One without, or whose
//! program cannot be read, draws with a face from the list the application
//! gave, matched on the name and flags in its descriptor; with no list, or
//! no face that fits, its text is not drawn and a diagnostic says so.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use kernkraft_font::{CmapSelector, FaceInfo, FontList, GlyphId, Program, Query, Variations};
use tiny_skia::{Path, Transform};

use crate::content::{PlacedGlyph, TextRun};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::font::{text_for_name, BaseEncoding, Font, FontKind, ProgramKind};
use crate::render::shape;
use crate::syntax::Name;

/// The `cmap` subtables 9.6.6.4 names: (platform, encoding).
const WINDOWS_SYMBOL: CmapSelector = CmapSelector {
    platform: 3,
    encoding: 0,
};
const MACINTOSH_ROMAN: CmapSelector = CmapSelector {
    platform: 1,
    encoding: 0,
};
/// A symbolic TrueType font's `cmap` (3, 0) may place a code in the
/// private-use area at one of these offsets (9.6.6.4).
const SYMBOL_OFFSETS: [u32; 4] = [0, 0xF000, 0xF100, 0xF200];
/// The weight and width the matcher is asked for: Regular and Bold
/// (OpenType `usWeightClass`), and the normal width.
const REGULAR: u16 = 400;
const BOLD: u16 = 700;
const NORMAL_WIDTH: u8 = 5;
/// A subset font's name starts with six capital letters and a plus (9.6.4).
const SUBSET_PREFIX_LETTERS: usize = 6;

const SANS: &[&str] = &[
    "Helvetica",
    "Arial",
    "Liberation Sans",
    "Nimbus Sans",
    "Nimbus Sans L",
    "DejaVu Sans",
    "Noto Sans",
];
const SERIF: &[&str] = &[
    "Times New Roman",
    "Times",
    "Liberation Serif",
    "Nimbus Roman",
    "Nimbus Roman No9 L",
    "DejaVu Serif",
    "Noto Serif",
];
const MONO: &[&str] = &[
    "Courier New",
    "Courier",
    "Liberation Mono",
    "Nimbus Mono PS",
    "Nimbus Mono L",
    "DejaVu Sans Mono",
    "Noto Sans Mono",
];
const SYMBOL: &[&str] = &["Symbol", "Standard Symbols PS", "Standard Symbols L"];
const DINGBATS: &[&str] = &["ZapfDingbats", "Dingbats", "D050000L"];

/// Where a font's glyphs come from.
enum Origin {
    /// The font's own embedded program.
    Embedded(Vec<u8>),
    /// A face of the application's list: a font of it, opened at the named
    /// instance of a variable font when the face is one.
    Substitute {
        data: Vec<u8>,
        index: u32,
        variations: Variations,
    },
    /// Nothing to draw with.
    Unavailable,
}

struct Entry {
    /// Kept so the pointer that keys the entry is never reused.
    font: Arc<Font>,
    origin: Origin,
    cid_to_gid: Option<Vec<u16>>,
    /// Glyph paths in em units (y up), by character code.
    glyphs: HashMap<u32, Option<Arc<Path>>>,
}

/// The glyph outlines of a page's fonts, read as they are first shown.
pub(crate) struct Glyphs<'d> {
    doc: &'d Document,
    list: Option<&'d FontList>,
    entries: HashMap<*const Font, Entry>,
    /// The fonts a missing glyph has been reported for.
    reported: HashSet<*const Font>,
}

impl<'d> Glyphs<'d> {
    pub fn new(doc: &'d Document, list: Option<&'d FontList>) -> Self {
        Self {
            doc,
            list,
            entries: HashMap::new(),
            reported: HashSet::new(),
        }
    }

    /// The outline of `glyph` of `run`, in em units with y up: map it with
    /// the glyph's [`matrix`](PlacedGlyph::matrix). `None` for a glyph that
    /// has no marks (a space) and for one that cannot be drawn, which is
    /// reported.
    pub fn outline(
        &mut self,
        run: &TextRun,
        glyph: &PlacedGlyph,
        diagnostics: &mut Diagnostics,
    ) -> Option<Arc<Path>> {
        let key = Arc::as_ptr(&run.font);
        if !self.entries.contains_key(&key) {
            let entry = self.open(&run.font, &run.font_name, diagnostics);
            self.entries.insert(key, entry);
        }
        let entry = self.entries.get_mut(&key)?;
        if let Some(known) = entry.glyphs.get(&glyph.code) {
            return known.clone();
        }
        let found = draw(entry, glyph);
        let outline = match found {
            Drawn::Path(path) => Some(Arc::new(path)),
            Drawn::Blank => None,
            Drawn::Missing => {
                if self.reported.insert(key) {
                    diagnostics.push(
                        DiagnosticKind::GlyphMissing {
                            name: run.font_name.clone(),
                        },
                        0,
                    );
                }
                None
            }
        };
        entry.glyphs.insert(glyph.code, outline.clone());
        outline
    }

    /// Whether the font has marks for `glyph`, a blank one or none at all.
    pub fn presence(
        &mut self,
        run: &TextRun,
        glyph: &PlacedGlyph,
        diagnostics: &mut Diagnostics,
    ) -> Presence {
        let key = Arc::as_ptr(&run.font);
        if !self.entries.contains_key(&key) {
            let entry = self.open(&run.font, &run.font_name, diagnostics);
            self.entries.insert(key, entry);
        }
        match self.entries.get(&key).map(|entry| draw(entry, glyph)) {
            Some(Drawn::Path(_)) => Presence::Marks,
            Some(Drawn::Blank) => Presence::Blank,
            Some(Drawn::Missing) | None => Presence::Missing,
        }
    }

    fn open(&self, font: &Arc<Font>, name: &Name, diagnostics: &mut Diagnostics) -> Entry {
        let cid_to_gid = font.cid_to_gid(self.doc).ok().flatten();
        let embedded = match font.embedded_program(self.doc) {
            Ok(Some(program)) => {
                if Program::parse(&program.data, 0).is_ok() {
                    Some(program.data)
                } else {
                    diagnostics.push(DiagnosticKind::FontProgramUnreadable, 0);
                    None
                }
            }
            Ok(None) => None,
            Err(_) => {
                diagnostics.push(DiagnosticKind::FontProgramUnreadable, 0);
                None
            }
        };
        let origin = match embedded {
            Some(data) => Origin::Embedded(data),
            None => match self.substitute(font) {
                Some((data, index, variations, family)) => {
                    diagnostics.push(
                        DiagnosticKind::FontSubstituted {
                            name: name.clone(),
                            family,
                        },
                        0,
                    );
                    Origin::Substitute {
                        data,
                        index,
                        variations,
                    }
                }
                None => {
                    diagnostics.push(DiagnosticKind::FontUnavailable { name: name.clone() }, 0);
                    Origin::Unavailable
                }
            },
        };
        Entry {
            font: Arc::clone(font),
            origin,
            cid_to_gid,
            glyphs: HashMap::new(),
        }
    }

    /// The bytes, collection index, variations and first family name of the
    /// list's face for `font`.
    fn substitute(&self, font: &Font) -> Option<(Vec<u8>, u32, Variations, String)> {
        let list = self.list?;
        let wanted = wanted_style(font);
        let query = Query {
            families: wanted.families,
            weight: if wanted.bold { BOLD } else { REGULAR },
            width: NORMAL_WIDTH,
            italic: wanted.italic,
        };
        let face: &FaceInfo = list.best(&query)?;
        let data = std::fs::read(&face.path).ok()?;
        let variations = face
            .instance
            .as_ref()
            .map(kernkraft_font::Instance::variations)
            .unwrap_or_default();
        Program::parse(&data, face.index)
            .ok()?
            .with_variations(&variations)
            .ok()?;
        let family = face.families.first().cloned().unwrap_or_default();
        Some((data, face.index, variations, family))
    }
}

/// What a font has for a character.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Presence {
    Marks,
    Blank,
    Missing,
}

enum Drawn {
    Path(Path),
    /// A glyph with no marks.
    Blank,
    Missing,
}

fn draw(entry: &Entry, glyph: &PlacedGlyph) -> Drawn {
    let (data, index, substituted, variations) = match &entry.origin {
        Origin::Embedded(data) => (data, 0, false, None),
        Origin::Substitute {
            data,
            index,
            variations,
        } => (data, *index, true, Some(variations)),
        Origin::Unavailable => return Drawn::Blank,
    };
    let Ok(program) = Program::parse(data, index) else {
        return Drawn::Missing;
    };
    let Ok(program) = program.with_variations(variations.unwrap_or(&Variations::new())) else {
        return Drawn::Missing;
    };
    let found = if substituted {
        by_unicode(&program, glyph)
    } else {
        embedded_glyph(&entry.font, &program, glyph, entry.cid_to_gid.as_deref())
    };
    let Some(id) = found else {
        return Drawn::Missing;
    };
    let Ok(outline) = program.outline(id) else {
        return Drawn::Missing;
    };
    if outline.is_empty() {
        return Drawn::Blank;
    }
    let Some(path) = shape::outline(&outline) else {
        return Drawn::Blank;
    };
    let [a, b, c, d, e, f] = program.font_matrix();
    match path.transform(Transform::from_row(a, b, c, d, e, f)) {
        Some(path) => Drawn::Path(path),
        None => Drawn::Missing,
    }
}

/// The glyph of an embedded program for `glyph`.
fn embedded_glyph(
    font: &Font,
    program: &Program<'_>,
    glyph: &PlacedGlyph,
    cid_to_gid: Option<&[u16]>,
) -> Option<GlyphId> {
    if font.kind() == FontKind::Type0 {
        return by_cid(program, glyph.cid, cid_to_gid);
    }
    match font.descriptor().and_then(|d| d.font_file).map(|f| f.kind) {
        Some(ProgramKind::TrueType) => truetype_glyph(font, program, glyph),
        _ if program.format() == kernkraft_font::Format::TrueType => {
            truetype_glyph(font, program, glyph)
        }
        _ => named_glyph(program, glyph),
    }
}

/// A CID's glyph (9.7.4.2): a TrueType program through `CIDToGIDMap`, a
/// CID-keyed CFF program through its charset, and a CFF program that is not
/// CID-keyed by the CID taken as the glyph number.
fn by_cid(program: &Program<'_>, cid: u32, map: Option<&[u16]>) -> Option<GlyphId> {
    let cid = u16::try_from(cid).ok()?;
    match program.format() {
        kernkraft_font::Format::TrueType => match map {
            Some(map) => map.get(usize::from(cid)).copied().map(GlyphId),
            None => Some(GlyphId(cid)),
        },
        _ => program.glyph_by_cid(cid).or(Some(GlyphId(cid))),
    }
}

/// A Type 1 or CFF glyph (9.6.6.2): by the name the encoding gives the
/// code, else by the program's own encoding.
fn named_glyph(program: &Program<'_>, glyph: &PlacedGlyph) -> Option<GlyphId> {
    if let Some(found) = glyph
        .glyph_name
        .as_deref()
        .and_then(|name| program.glyph_by_name(name))
    {
        return Some(found);
    }
    let code = u8::try_from(glyph.code).ok()?;
    program.glyph_by_builtin_code(code).or_else(|| {
        glyph
            .glyph_name
            .as_deref()
            .and_then(text_for_name)
            .and_then(|text| text.chars().next())
            .and_then(|c| program.glyph_by_unicode(c))
    })
}

/// A simple TrueType font's glyph (9.6.6.4).
fn truetype_glyph(font: &Font, program: &Program<'_>, glyph: &PlacedGlyph) -> Option<GlyphId> {
    let code = glyph.code;
    let by_symbol_cmap = || {
        SYMBOL_OFFSETS
            .iter()
            .find_map(|offset| program.glyph_by_cmap(WINDOWS_SYMBOL, offset + code))
            .or_else(|| program.glyph_by_cmap(MACINTOSH_ROMAN, code))
    };
    if font.truetype_uses_cmap_directly() {
        return by_symbol_cmap().or_else(|| program.glyph_by_unicode(char::from_u32(code)?));
    }
    let name = glyph.glyph_name.as_deref();
    let by_unicode = name
        .and_then(text_for_name)
        .and_then(|text| text.chars().next())
        .and_then(|c| program.glyph_by_unicode(c));
    let by_mac = || {
        let mac = BaseEncoding::MacRoman.code_of(name?)?;
        program.glyph_by_cmap(MACINTOSH_ROMAN, u32::from(mac))
    };
    by_unicode
        .or_else(by_mac)
        .or_else(|| name.and_then(|n| program.glyph_by_name(n)))
        .or_else(by_symbol_cmap)
}

/// The glyph of a substitute face: by the character's Unicode value.
fn by_unicode(program: &Program<'_>, glyph: &PlacedGlyph) -> Option<GlyphId> {
    let c = glyph
        .unicode
        .as_deref()
        .and_then(|text| text.chars().next())
        .or_else(|| {
            glyph
                .glyph_name
                .as_deref()
                .and_then(text_for_name)
                .and_then(|text| text.chars().next())
        })?;
    program.glyph_by_unicode(c)
}

/// What a descriptor and a name ask of a substitute.
pub(crate) struct Wanted {
    pub families: &'static [&'static str],
    pub bold: bool,
    pub italic: bool,
}

/// The family list and style for a font with no program of its own: the
/// standard 14 names, then the descriptor's flags (Table 123).
pub(crate) fn wanted_style(font: &Font) -> Wanted {
    let full = font.base_font().unwrap_or_default();
    let base = match full.split_once('+') {
        Some((prefix, rest))
            if prefix.len() == SUBSET_PREFIX_LETTERS
                && prefix.bytes().all(|b| b.is_ascii_uppercase()) =>
        {
            rest
        }
        _ => full,
    };
    let name: String = base
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase();
    let flags = font.descriptor().map(|d| d.flags).unwrap_or_default();
    let families = if name.contains("zapfdingbats") || name.contains("dingbats") {
        DINGBATS
    } else if name.contains("symbol") {
        SYMBOL
    } else if name.contains("courier") || name.contains("mono") || flags.fixed_pitch() {
        MONO
    } else if name.contains("arial") || name.contains("helvetica") || name.contains("sans") {
        SANS
    } else if name.contains("times") || name.contains("serif") || flags.serif() {
        SERIF
    } else {
        SANS
    };
    let angle = font.descriptor().map_or(0.0, |d| d.italic_angle);
    Wanted {
        families,
        bold: ["bold", "black", "heavy", "demi"]
            .iter()
            .any(|word| name.contains(word))
            || flags.force_bold(),
        italic: name.contains("italic")
            || name.contains("oblique")
            || flags.italic()
            || angle != 0.0,
    }
}
