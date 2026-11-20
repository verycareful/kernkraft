// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! One font program behind one interface: TrueType and OpenType through
//! `skrifa`, a bare CFF font program (the `FontFile3` of a PDF) through
//! `read-fonts`' CFF reader, and Type 1 through [`crate::type1`].
//!
//! Formats: OpenType 1.9 (`head`, `maxp`, `hmtx`, `cmap`, `glyf`, `loca`,
//! `CFF `, `post`, `name`, `OS/2`); Adobe Technical Note 5176, The Compact
//! Font Format Specification; Adobe Type 1 Font Format.

use std::collections::HashMap;
use std::sync::OnceLock;

use read_fonts::ps::cff::{CffFontRef, Subfont};
use read_fonts::ps::string::Sid;
use read_fonts::types::Tag;
use read_fonts::{FileRef, ReadError, TableProvider};
use skrifa::instance::{Location, Size};
use skrifa::outline::{
    DrawError, DrawSettings, OutlineGlyphCollection, OutlineGlyphFormat, OutlinePen,
};
use skrifa::string::StringId;
use skrifa::{FontRef, MetadataProvider};

use crate::error::FontError;
use crate::outline::Outline;
use crate::type1::Type1Font;
use crate::variation::{
    arbitrary_instance_name, ascii_alphanumeric, named_instance_name, Axis, Variations,
    AXIS_ITALIC, AXIS_SLANT, AXIS_WEIGHT,
};

/// A glyph number in a font program: the glyph's index in its outline table
/// (`loca`, the CFF CharStrings INDEX, or the order of a Type 1 font's
/// CharStrings).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlyphId(pub u16);

impl GlyphId {
    fn to_skrifa(self) -> skrifa::GlyphId {
        skrifa::GlyphId::new(u32::from(self.0))
    }

    fn from_skrifa(g: skrifa::GlyphId) -> Option<GlyphId> {
        u16::try_from(g.to_u32()).ok().map(GlyphId)
    }
}

/// The kind of font program the bytes held.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// An sfnt with `glyf` outlines.
    TrueType,
    /// An sfnt with a `CFF ` or `CFF2` table.
    OpenTypeCff,
    /// An sfnt with no vector outlines: its glyphs are bitmaps.
    SfntNoOutlines,
    /// A CFF font program with no sfnt around it.
    Cff,
    /// An Adobe Type 1 font program (PFA, PFB, or the `FontFile` of a PDF).
    Type1,
}

/// The names a font gives itself. A field is `None` when the font has no
/// such name.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Names {
    pub family: Option<String>,
    pub subfamily: Option<String>,
    pub full: Option<String>,
    pub postscript: Option<String>,
}

/// Font-wide measurements in design units (y up). `None` where the format
/// does not carry the value.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Metrics {
    pub ascent: Option<f32>,
    pub descent: Option<f32>,
    pub cap_height: Option<f32>,
    pub x_height: Option<f32>,
    /// Degrees counter-clockwise from the vertical (negative for a
    /// forward slant).
    pub italic_angle: Option<f32>,
    /// `[min_x, min_y, max_x, max_y]`.
    pub bbox: Option<[f32; 4]>,
    pub is_monospaced: Option<bool>,
    pub is_italic: Option<bool>,
    /// 1 to 1000 (OS/2 `usWeightClass`).
    pub weight: Option<u16>,
}

/// Which `cmap` subtable of an sfnt to look in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CmapSelector {
    pub platform: u16,
    pub encoding: u16,
}

/// Axis values are compared as the 16.16 fixed numbers a font stores.
const FIXED_ONE: f64 = 65536.0;

type NameMap = OnceLock<HashMap<String, GlyphId>>;

struct Sfnt<'a> {
    font: FontRef<'a>,
    /// The `CFF ` table of an OpenType font with CFF outlines, read as the
    /// bare program it holds.
    cff: Option<CffFontRef<'a>>,
    outlines: OutlineGlyphCollection<'a>,
    glyph_count: u16,
    units_per_em: u16,
    names: NameMap,
    /// The instance outlines, advances and metrics are read at: normalised
    /// coordinates, all zero (the default instance) until
    /// [`Program::with_variations`].
    location: Location,
    variations: Variations,
}

struct BareCff<'a> {
    cff: CffFontRef<'a>,
    /// One per font DICT; a name-keyed font has one.
    subfonts: Vec<Subfont>,
    names: NameMap,
}

enum Inner<'a> {
    Sfnt(Box<Sfnt<'a>>),
    Cff(Box<BareCff<'a>>),
    Type1(Box<Type1Font>),
}

/// A parsed font program. It borrows the bytes of TrueType, OpenType and
/// CFF programs; a Type 1 program is decoded into owned data.
pub struct Program<'a> {
    inner: Inner<'a>,
}

/// The face of `data` that `index` picks. Only a TrueType collection holds
/// more than one font; for any other file the index is ignored (fontconfig
/// numbers a variable font's named instances in the high bits of an index).
pub(crate) fn face_index(data: &[u8], index: u32) -> u32 {
    if data.starts_with(b"ttcf") {
        index
    } else {
        0
    }
}

impl<'a> Program<'a> {
    /// Reads a font program, telling the format from its first bytes.
    /// `index` picks a font out of a TrueType collection and is ignored
    /// for everything else.
    pub fn parse(data: &'a [u8], index: u32) -> Result<Program<'a>, FontError> {
        match data {
            [0x00, 0x01, 0x00, 0x00, ..]
            | [b't', b'r', b'u', b'e', ..]
            | [b'O', b'T', b'T', b'O', ..] => Self::sfnt(data, 0),
            [b't', b't', b'c', b'f', ..] => Self::sfnt(data, face_index(data, index)),
            [0x01, _, hdr, off, ..] if *hdr >= 4 && (1..=4).contains(off) => Self::cff(data),
            [b'%', b'!', ..] | [0x80, 0x01, ..] => Ok(Program {
                inner: Inner::Type1(Box::new(Type1Font::parse(data)?)),
            }),
            _ => Err(FontError::UnknownFormat),
        }
    }

    fn sfnt(data: &'a [u8], index: u32) -> Result<Program<'a>, FontError> {
        let font = FontRef::from_index(data, index).map_err(|e| match e {
            ReadError::InvalidCollectionIndex(_) => FontError::CollectionIndex {
                index,
                count: match FileRef::new(data) {
                    Ok(FileRef::Collection(c)) => c.len(),
                    _ => 0,
                },
            },
            ReadError::InvalidSfnt(_) | ReadError::InvalidTtc(_) => FontError::UnknownFormat,
            other => sfnt_reason(&other),
        })?;
        let units_per_em = font.head().map_err(|e| sfnt_reason(&e))?.units_per_em();
        let glyph_count = font.maxp().map_err(|e| sfnt_reason(&e))?.num_glyphs();
        let outlines = font.outline_glyphs();
        let cff = font
            .table_data(Tag::new(b"CFF "))
            .and_then(|table| CffFontRef::new(table.as_bytes(), 0, None).ok());
        Ok(Program {
            inner: Inner::Sfnt(Box::new(Sfnt {
                font,
                cff,
                outlines,
                glyph_count,
                units_per_em,
                names: OnceLock::new(),
                location: Location::default(),
                variations: Variations::new(),
            })),
        })
    }

    fn cff(data: &'a [u8]) -> Result<Program<'a>, FontError> {
        let cff = CffFontRef::new(data, 0, None).map_err(|_| FontError::Cff)?;
        let count = cff.num_subfonts().max(1);
        let mut subfonts = Vec::with_capacity(usize::from(count));
        for i in 0..count {
            subfonts.push(cff.subfont(i, &[]).map_err(|_| FontError::Cff)?);
        }
        if cff.num_glyphs() > u32::from(u16::MAX) + 1 {
            return Err(FontError::Cff);
        }
        Ok(Program {
            inner: Inner::Cff(Box::new(BareCff {
                cff,
                subfonts,
                names: OnceLock::new(),
            })),
        })
    }

    /// The axes of a variable font, in the order of its `fvar` table; empty
    /// for a font with none and for a bare CFF or Type 1 program.
    pub fn axes(&self) -> Vec<Axis> {
        match &self.inner {
            Inner::Sfnt(s) => s
                .font
                .axes()
                .iter()
                .map(|a| Axis {
                    tag: a.tag().to_be_bytes(),
                    min: a.min_value(),
                    default: a.default_value(),
                    max: a.max_value(),
                })
                .collect(),
            Inner::Cff(_) | Inner::Type1(_) => Vec::new(),
        }
    }

    /// The program at the instance `variations` picks: [`Self::advance`],
    /// [`Self::outline`] and [`Self::metrics`] describe that instance from
    /// then on, and [`Self::names`] gives its PostScript name (Adobe TN 5902).
    /// An empty `variations` is the default instance.
    ///
    /// The errors of [`Variations::check`].
    pub fn with_variations(mut self, variations: &Variations) -> Result<Program<'a>, FontError> {
        variations.check(&self.axes())?;
        if let Inner::Sfnt(s) = &mut self.inner {
            s.location = s.font.axes().location(
                variations
                    .settings()
                    .iter()
                    .map(|&(tag, value)| (Tag::new(&tag), value)),
            );
            s.variations = variations.clone();
        }
        Ok(self)
    }

    pub fn format(&self) -> Format {
        match &self.inner {
            Inner::Sfnt(s) => match s.outlines.format() {
                Some(OutlineGlyphFormat::Glyf | OutlineGlyphFormat::Varc) => Format::TrueType,
                Some(OutlineGlyphFormat::Cff | OutlineGlyphFormat::Cff2) => Format::OpenTypeCff,
                None => Format::SfntNoOutlines,
            },
            Inner::Cff(_) => Format::Cff,
            Inner::Type1(_) => Format::Type1,
        }
    }

    pub fn glyph_count(&self) -> u16 {
        match &self.inner {
            Inner::Sfnt(s) => s.glyph_count,
            // Held to 65536 glyphs when the program is read; a count of
            // exactly 65536 is reported as the largest a GlyphId can number.
            Inner::Cff(c) => u16::try_from(c.cff.num_glyphs()).unwrap_or(u16::MAX),
            Inner::Type1(t) => t.glyph_count(),
        }
    }

    /// The matrix from glyph design units to text space (the units of a
    /// 1-point font): `[a b c d e f]` as in a PDF matrix. For an sfnt it is
    /// `1/unitsPerEm` on the diagonal. A CFF program's own `FontMatrix`
    /// (and its font DICTs') is applied to the outlines, which then come
    /// out in units of `1/upem` with `upem` the number this matrix inverts.
    /// Type 1 carries its own `FontMatrix`, applied by the caller.
    pub fn font_matrix(&self) -> [f32; 6] {
        match &self.inner {
            Inner::Sfnt(s) => {
                let k = 1.0 / f32::from(s.units_per_em);
                [k, 0.0, 0.0, k, 0.0, 0.0]
            }
            Inner::Cff(c) => {
                let k = 1.0 / c.cff.upem() as f32;
                [k, 0.0, 0.0, k, 0.0, 0.0]
            }
            Inner::Type1(t) => t.font_matrix(),
        }
    }

    /// The glyph named `name` (a `post` name, a CFF charset name, or a Type 1
    /// CharStrings key).
    pub fn glyph_by_name(&self, name: &str) -> Option<GlyphId> {
        match &self.inner {
            Inner::Sfnt(s) => s
                .names
                .get_or_init(|| sfnt_names(&s.font))
                .get(name)
                .copied(),
            Inner::Cff(c) => c.names.get_or_init(|| cff_names(&c.cff)).get(name).copied(),
            Inner::Type1(t) => t.glyph_by_name(name),
        }
    }

    pub fn glyph_name(&self, glyph: GlyphId) -> Option<String> {
        match &self.inner {
            Inner::Sfnt(s) => {
                let name = s.font.glyph_names().get(glyph.to_skrifa())?;
                (!name.is_synthesized()).then(|| name.as_str().to_owned())
            }
            Inner::Cff(c) => cff_glyph_name(&c.cff, glyph),
            Inner::Type1(t) => t.glyph_name(glyph).map(str::to_owned),
        }
    }

    /// The glyph the font's Unicode `cmap` gives for `c`. `None` for CFF and
    /// Type 1 programs, which have no `cmap`.
    pub fn glyph_by_unicode(&self, c: char) -> Option<GlyphId> {
        match &self.inner {
            Inner::Sfnt(s) => s.font.charmap().map(c).and_then(GlyphId::from_skrifa),
            _ => None,
        }
    }

    /// The glyph subtable `sel` of the `cmap` gives for `code`. `None` for
    /// CFF and Type 1 programs.
    pub fn glyph_by_cmap(&self, sel: CmapSelector, code: u32) -> Option<GlyphId> {
        let Inner::Sfnt(s) = &self.inner else {
            return None;
        };
        let cmap = s.font.cmap().ok()?;
        let record = cmap
            .encoding_records()
            .iter()
            .find(|r| r.platform_id() as u16 == sel.platform && r.encoding_id() == sel.encoding)?;
        let sub = record.subtable(cmap.offset_data()).ok()?;
        // Glyph 0 is `.notdef`: the code is not mapped.
        sub.map_codepoint(code)
            .filter(|g| g.to_u32() != 0)
            .and_then(GlyphId::from_skrifa)
    }

    /// The glyph the font's own encoding gives for a one-byte code: the
    /// `Encoding` of a Type 1 program or of a name-keyed CFF program.
    /// `None` for an sfnt, for a CID-keyed CFF program, and for a code the
    /// encoding does not assign.
    pub fn glyph_by_builtin_code(&self, code: u8) -> Option<GlyphId> {
        match &self.inner {
            Inner::Cff(c) => {
                if c.cff.is_cid() {
                    return None;
                }
                c.cff.encoding()?.map(code).and_then(GlyphId::from_skrifa)
            }
            Inner::Type1(t) => t.glyph_by_code(code),
            Inner::Sfnt(_) => None,
        }
    }

    /// The glyph of a CID in a CID-keyed CFF program, bare or inside an
    /// OpenType font (through its charset). `None` for any other program,
    /// and for a CID the font does not hold.
    pub fn glyph_by_cid(&self, cid: u16) -> Option<GlyphId> {
        let cff = match &self.inner {
            Inner::Cff(c) => &c.cff,
            Inner::Sfnt(s) => s.cff.as_ref()?,
            Inner::Type1(_) => return None,
        };
        if !cff.is_cid() {
            return None;
        }
        let g = cff.charset()?.glyph_id(Sid::new(cid)).ok()?;
        GlyphId::from_skrifa(g)
    }

    /// The horizontal advance in design units.
    pub fn advance(&self, glyph: GlyphId) -> Option<f32> {
        if glyph.0 >= self.glyph_count() {
            return None;
        }
        match &self.inner {
            Inner::Sfnt(s) => s
                .font
                .glyph_metrics(Size::unscaled(), &s.location)
                .advance_width(glyph.to_skrifa()),
            Inner::Cff(c) => c.draw(glyph, &mut Outline::default()).ok().flatten(),
            Inner::Type1(t) => t.advance(glyph),
        }
    }

    /// The outline of a glyph in design units. A glyph with no marks gives
    /// an empty outline. A glyph number past the font's glyphs is
    /// [`FontError::NoSuchGlyph`].
    pub fn outline(&self, glyph: GlyphId) -> Result<Outline, FontError> {
        if glyph.0 >= self.glyph_count() {
            return Err(FontError::NoSuchGlyph(glyph));
        }
        let mut out = Outline::default();
        match &self.inner {
            Inner::Sfnt(s) => {
                let Some(g) = s.outlines.get(glyph.to_skrifa()) else {
                    return Err(FontError::NoOutline(glyph));
                };
                g.draw(
                    DrawSettings::unhinted(Size::unscaled(), &s.location),
                    &mut out,
                )
                .map_err(|e| FontError::Outline {
                    glyph,
                    reason: draw_reason(&e),
                })?;
            }
            Inner::Cff(c) => {
                c.draw(glyph, &mut out)
                    .map_err(|reason| FontError::Outline { glyph, reason })?;
            }
            Inner::Type1(t) => return t.outline(glyph),
        }
        Ok(out)
    }

    pub fn metrics(&self) -> Metrics {
        match &self.inner {
            Inner::Sfnt(s) => {
                let m = s.font.metrics(Size::unscaled(), &s.location);
                let attrs = s.font.attributes();
                let named = |tag| s.variations.get(tag);
                // The OS/2 weight and the italic flags describe the default
                // instance; an axis the caller set moves them.
                let weight = named(AXIS_WEIGHT).unwrap_or_else(|| attrs.weight.value());
                // `slnt` is the instance's italic angle (OpenType, registered
                // axes: it matches `post` italicAngle at the default and
                // "can be used" for an instance), whatever its value, zero
                // included. `ital` is 0 for Roman and 1 for italic, and
                // nothing between is defined: a value that is not Roman is
                // italic. An axis the caller left alone does not decide.
                let slant = named(AXIS_SLANT);
                let italic = match (named(AXIS_ITALIC), slant) {
                    (Some(ital), _) => ital > 0.0,
                    (None, Some(slnt)) => slnt != 0.0,
                    (None, None) => !matches!(attrs.style, skrifa::attribute::Style::Normal),
                };
                Metrics {
                    ascent: Some(m.ascent),
                    descent: Some(m.descent),
                    cap_height: m.cap_height,
                    x_height: m.x_height,
                    italic_angle: Some(slant.unwrap_or(m.italic_angle)),
                    bbox: m.bounds.map(|b| [b.x_min, b.y_min, b.x_max, b.y_max]),
                    is_monospaced: Some(m.is_monospace),
                    is_italic: Some(italic),
                    weight: Some(weight.round().clamp(1.0, 1000.0) as u16),
                }
            }
            Inner::Cff(c) => match c.cff.metadata() {
                Some(m) => {
                    let angle = m.italic_angle().to_f32();
                    let b = [
                        m.bbox().x_min,
                        m.bbox().y_min,
                        m.bbox().x_max,
                        m.bbox().y_max,
                    ]
                    .map(|v| v.to_f32());
                    Metrics {
                        italic_angle: Some(angle),
                        // A Top DICT without FontBBox reads as the default
                        // 0 0 0 0, which is no box.
                        bbox: (b != [0.0; 4]).then_some(b),
                        is_monospaced: Some(m.is_fixed_pitch()),
                        is_italic: Some(angle != 0.0),
                        ..Metrics::default()
                    }
                }
                None => Metrics::default(),
            },
            Inner::Type1(t) => t.metrics(),
        }
    }

    pub fn names(&self) -> Names {
        match &self.inner {
            Inner::Sfnt(s) => {
                let get = |id: StringId| {
                    s.font
                        .localized_strings(id)
                        .english_or_first()
                        .map(|n| n.chars().collect::<String>())
                        .filter(|n| !n.is_empty())
                };
                Names {
                    family: get(StringId::TYPOGRAPHIC_FAMILY_NAME)
                        .or_else(|| get(StringId::FAMILY_NAME)),
                    subfamily: get(StringId::TYPOGRAPHIC_SUBFAMILY_NAME)
                        .or_else(|| get(StringId::SUBFAMILY_NAME)),
                    full: get(StringId::FULL_NAME),
                    postscript: s
                        .instance_postscript_name()
                        .or_else(|| get(StringId::POSTSCRIPT_NAME)),
                }
            }
            Inner::Cff(c) => match c.cff.metadata() {
                Some(m) => Names {
                    family: m.family_name().map(str::to_owned),
                    subfamily: m.weight().map(str::to_owned),
                    full: m.full_name().map(str::to_owned),
                    postscript: m.name().map(str::to_owned),
                },
                None => Names::default(),
            },
            Inner::Type1(t) => t.names(),
        }
    }
}

impl Sfnt<'_> {
    /// The PostScript name of the instance chosen, by Adobe TN 5902; `None`
    /// at the default instance (the font's own name, `name` ID 6, is that
    /// instance's) and for a font with no family name to build one from.
    ///
    /// A named instance (an `fvar` instance whose coordinates are the ones
    /// chosen) keeps the PostScript name the font gives it, else the prefix
    /// and the instance's subfamily name; any other instance is the prefix
    /// and, for each axis off its default, its value and tag. The prefix is
    /// `name` ID 25, else the US English typographic family name (ID 16, as
    /// ID 1 stands in for it).
    fn instance_postscript_name(&self) -> Option<String> {
        let fixed = |v: f32| (f64::from(v) * FIXED_ONE).round() as i64;
        let axes = self.font.axes();
        let chosen: Vec<_> = axes
            .iter()
            .map(|a| {
                let tag = a.tag().to_be_bytes();
                let default = a.default_value();
                (tag, self.variations.get(tag).unwrap_or(default), default)
            })
            .collect();
        if chosen.iter().all(|&(_, v, d)| fixed(v) == fixed(d)) {
            return None;
        }
        let string = |id: StringId| {
            self.font
                .localized_strings(id)
                .english_or_first()
                .map(|n| n.chars().collect::<String>())
                .filter(|n| !n.is_empty())
        };
        let prefix = string(StringId::VARIATIONS_POSTSCRIPT_NAME_PREFIX)
            .or_else(|| string(StringId::TYPOGRAPHIC_FAMILY_NAME))
            .or_else(|| string(StringId::FAMILY_NAME))
            .map(|n| ascii_alphanumeric(&n))
            .filter(|n| !n.is_empty())?;
        let instances = self.font.named_instances();
        for i in 0..instances.len() {
            let Some(instance) = instances.get(i) else {
                continue;
            };
            let same = instance.user_coords().count() == chosen.len()
                && instance
                    .user_coords()
                    .zip(&chosen)
                    .all(|(c, &(_, v, _))| fixed(c) == fixed(v));
            if !same {
                continue;
            }
            if let Some(name) = instance.postscript_name_id().and_then(string) {
                return Some(name);
            }
            if let Some(subfamily) = string(instance.subfamily_name_id()) {
                return Some(named_instance_name(&prefix, &subfamily));
            }
        }
        let off_default: Vec<_> = chosen
            .iter()
            .filter(|&&(_, v, d)| fixed(v) != fixed(d))
            .map(|&(tag, v, _)| (tag, v))
            .collect();
        Some(arbitrary_instance_name(&prefix, &off_default))
    }
}

impl BareCff<'_> {
    /// Draws a glyph through its font DICT's matrix, to `pen`; returns the
    /// advance the charstring gave, in the same units.
    fn draw(&self, glyph: GlyphId, pen: &mut impl OutlinePen) -> Result<Option<f32>, &'static str> {
        let gid = glyph.to_skrifa();
        let index = self
            .cff
            .subfont_index(gid)
            .ok_or("the glyph has no font DICT")?;
        let subfont = self
            .subfonts
            .get(usize::from(index))
            .ok_or("the glyph's font DICT does not exist")?;
        self.cff
            .draw(subfont, gid, &[], None, pen)
            .map_err(|_| "a damaged charstring")
    }
}

fn sfnt_reason(e: &ReadError) -> FontError {
    FontError::Sfnt(match e {
        ReadError::OutOfBounds => "a table runs past the end of the file",
        ReadError::TableIsMissing(_) | ReadError::MetricIsMissing(_) => {
            "a required table is missing"
        }
        ReadError::InvalidFormat(_) => "a table has a format this crate does not read",
        ReadError::NullOffset => "a table offset is missing",
        _ => "a table is damaged",
    })
}

fn draw_reason(e: &DrawError) -> &'static str {
    match e {
        DrawError::NoSources => "the font has no outlines",
        DrawError::GlyphNotFound(_) => "the glyph is not in the font",
        DrawError::InsufficientMemory => "the glyph needs more memory than allowed",
        DrawError::RecursionLimitExceeded(_) => "composite glyphs nest too deep",
        DrawError::TooManyPoints(_) => "the glyph has too many points",
        DrawError::HintingFailed(_) => "hinting failed",
        DrawError::InvalidAnchorPoint(..) => "a composite glyph has a bad anchor point",
        DrawError::PostScript(_) => "a damaged charstring",
        DrawError::ToPath(_) => "the glyph cannot be turned into a path",
        DrawError::Read(_) => "the glyph data is damaged",
        #[allow(unreachable_patterns)]
        _ => "the glyph cannot be drawn",
    }
}

fn sfnt_names(font: &FontRef<'_>) -> HashMap<String, GlyphId> {
    let mut map = HashMap::new();
    for (g, name) in font.glyph_names().iter() {
        if name.is_synthesized() {
            continue;
        }
        if let Some(id) = GlyphId::from_skrifa(g) {
            map.entry(name.as_str().to_owned()).or_insert(id);
        }
    }
    map
}

fn cff_glyph_name(cff: &CffFontRef<'_>, glyph: GlyphId) -> Option<String> {
    if cff.is_cid() {
        return None;
    }
    let sid = cff.charset()?.string_id(glyph.to_skrifa()).ok()?;
    std::str::from_utf8(cff.string(sid)?)
        .ok()
        .map(str::to_owned)
}

fn cff_names(cff: &CffFontRef<'_>) -> HashMap<String, GlyphId> {
    let mut map = HashMap::new();
    if cff.is_cid() {
        return map;
    }
    for g in 0..cff.num_glyphs().min(u32::from(u16::MAX) + 1) {
        let id = GlyphId(g as u16);
        if let Some(name) = cff_glyph_name(cff, id) {
            map.entry(name).or_insert(id);
        }
    }
    map
}
