// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Embedding a font: ISO 32000-1:2008, 9.6.4 (the subset tag), 9.7
//! (composite fonts, Tables 121 and 117), 9.7.4.2 (glyph selection), 9.7.4.3
//! (`W`), 9.8 (font descriptors, Tables 122 and 123), 9.9 (embedded font
//! programs, Table 126) and 9.10.3 (ToUnicode CMaps); Adobe Technical Note
//! 5411 (ToUnicode mapping file tutorial).
//!
//! A [`FontEmbedder`] holds a TrueType or OpenType font, the instance of it
//! when it is a variable font, and the characters shown with it so far.
//! [`FontEmbedder::encode`] turns text into the bytes of a string for `Tj`,
//! and [`FontEmbedder::write`] writes the font as a Type 0 font with an
//! `Identity-H` encoding: every character is two bytes, the glyph's number in
//! the subset, which is also its CID. The descendant is a CIDFontType2
//! (TrueType outlines, `CIDToGIDMap` `Identity`) or a CIDFontType0 (CFF
//! outlines). The subset keeps only the glyphs shown, and a ToUnicode CMap
//! keeps their text so that the font can be searched and copied from.
//!
//! A variable font is embedded at the one instance chosen, as a static font:
//! a PDF font program has no variation tables. A `CFF2` font, and a `glyf`
//! font at an instance other than its default, come out as `glyf` outlines
//! (a CIDFontType2). The widths, the descriptor and the font's name (Adobe
//! Technical Note 5902) are those of the instance.

use std::collections::BTreeMap;

use kernkraft_font::{subset, Format, GlyphId, GlyphSet, Outline, Program, Segment, Variations};
use md5::{Digest, Md5};

use super::descriptor::FontFlags;
use crate::build::{number, ObjectBuilder, StreamEncoding};
use crate::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Value};
use crate::PdfError;

/// 9.2.4: glyph space is a thousandth of a text space unit.
const GLYPH_UNITS: f64 = 1000.0;
/// 9.6.4: a subset tag is six capital letters.
const TAG_LETTERS: usize = 6;
const LETTERS: u8 = 26;
/// Table 125: Adobe's registry and the ordering of a CIDFont whose CIDs are
/// glyph numbers, with the supplement 0 of Table 116.
const REGISTRY: &[u8] = b"Adobe";
const ORDERING_IDENTITY: &[u8] = b"Identity";
const ORDERING_UCS: &[u8] = b"UCS";
const SUPPLEMENT: i64 = 0;
const ENCODING_IDENTITY_H: &[u8] = b"Identity-H";
/// A character code of an `Identity-H` string is two bytes (9.7.5.2).
const CODE_BYTES: usize = 2;
/// Annex C: a `bfchar` section holds at most 100 entries.
const BFCHAR_PER_SECTION: usize = 100;
/// Samples taken along a curve when the outline is scanned for a stem.
const CURVE_STEPS: u32 = 16;
/// The glyphs that show a vertical stem on its own (a Latin lower-case l,
/// capital I and the vertical bar), tried in this order for `StemV`.
const STEM_GLYPHS: [char; 3] = ['l', 'I', '|'];
/// The glyph whose top is the cap height (Table 122, `CapHeight`).
const CAP_GLYPH: char = 'H';

/// A font being embedded. See the module documentation.
#[derive(Debug, Clone)]
pub struct FontEmbedder {
    program: Vec<u8>,
    index: u32,
    /// The instance of a variable font; empty for the default instance.
    variations: Variations,
    glyphs: GlyphSet,
    /// The character that first selected each glyph of the subset, for the
    /// ToUnicode CMap: a glyph two characters select (a space and a no-break
    /// space) keeps the first.
    text: BTreeMap<u16, char>,
}

impl FontEmbedder {
    /// A font of `program`, a TrueType or OpenType file (`index` picks a font
    /// out of a collection) with `glyf`, `CFF ` or `CFF2` outlines, at its
    /// default instance if it is a variable font.
    ///
    /// [`PdfError::Font`] for a program that cannot be read or that cannot be
    /// reduced to a subset: a bare CFF or Type 1 program, a font with no
    /// outlines.
    pub fn new(program: Vec<u8>, index: u32) -> Result<FontEmbedder, PdfError> {
        Self::with_variations(program, index, &Variations::new())
    }

    /// As [`Self::new`], for the instance `variations` picks of a variable
    /// font ([`Program::axes`] lists the axes).
    ///
    /// [`PdfError::Font`] also for an instance the font does not have: an
    /// axis it lacks or a font with no axes, an axis given twice, a value
    /// outside the axis's range.
    pub fn with_variations(
        program: Vec<u8>,
        index: u32,
        variations: &Variations,
    ) -> Result<FontEmbedder, PdfError> {
        let parsed = Program::parse(&program, index)?;
        match parsed.format() {
            Format::TrueType | Format::OpenTypeCff => {}
            // The reason is the one `kernkraft_font::subset` gives.
            _ => {
                subset(
                    &program,
                    index,
                    &GlyphSet::new(parsed.glyph_count()),
                    variations,
                )?;
            }
        }
        let glyphs = GlyphSet::new(parsed.glyph_count());
        parsed.with_variations(variations)?;
        Ok(FontEmbedder {
            program,
            index,
            variations: variations.clone(),
            glyphs,
            text: BTreeMap::new(),
        })
    }

    /// The font program at the instance chosen.
    fn parsed(&self) -> Result<Program<'_>, PdfError> {
        Ok(Program::parse(&self.program, self.index)?.with_variations(&self.variations)?)
    }

    /// Whether the font has a glyph for `character`.
    pub fn covers(&self, character: char) -> Result<bool, PdfError> {
        let program = self.parsed()?;
        Ok(glyph_of(&program, character).is_some())
    }

    /// The bytes that show `text` with the font written by
    /// [`Self::write`]: two for each character. Each glyph is kept in the
    /// subset.
    ///
    /// [`PdfError::NoGlyphForCharacter`] for a character the font has no
    /// glyph for; nothing is added then.
    pub fn encode(&mut self, text: &str) -> Result<Vec<u8>, PdfError> {
        let program = self.parsed()?;
        let found = text
            .chars()
            .map(|c| {
                glyph_of(&program, c)
                    .map(|glyph| (c, glyph))
                    .ok_or(PdfError::NoGlyphForCharacter { character: c })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let mut codes = Vec::with_capacity(found.len() * CODE_BYTES);
        for (character, glyph) in found {
            let cid = self.glyphs.add(glyph)?;
            self.text.entry(cid).or_insert(character);
            codes.extend_from_slice(&cid.to_be_bytes());
        }
        Ok(codes)
    }

    /// The distance between two lines of text set in the font, in
    /// thousandths of an em: the ascent less the descent (Table 122) of
    /// the font's own metrics.
    pub fn line_height(&self) -> Result<f64, PdfError> {
        let program = self.parsed()?;
        let metrics = program.metrics();
        let scale = glyph_scale(&program);
        let ascent = f64::from(metrics.ascent.unwrap_or(0.0));
        let descent = f64::from(metrics.descent.unwrap_or(0.0));
        Ok((ascent - descent) * scale)
    }

    /// The width of `text` in thousandths of a text space unit (the unit of
    /// the `W` array): the sum of the advances of its characters' glyphs.
    /// Nothing is added to the subset.
    ///
    /// [`PdfError::NoGlyphForCharacter`] as for [`Self::encode`].
    pub fn width(&self, text: &str) -> Result<f64, PdfError> {
        let program = self.parsed()?;
        let scale = glyph_scale(&program);
        let mut total = 0.0;
        for character in text.chars() {
            let glyph =
                glyph_of(&program, character).ok_or(PdfError::NoGlyphForCharacter { character })?;
            total += advance(&program, glyph, scale);
        }
        Ok(total)
    }

    /// Writes the font through `builder`: the Type 0 font, its CIDFont, the
    /// descriptor, the subset program and the ToUnicode CMap. Gives the Type
    /// 0 font's object, for a page's `Font` resources. The output is a
    /// function of the font and of the glyphs encoded, in the order they were.
    pub fn write(&self, builder: &mut ObjectBuilder) -> Result<ObjectId, PdfError> {
        let program = self.parsed()?;
        let reduced = subset(&self.program, self.index, &self.glyphs, &self.variations)?;
        let truetype = reduced.format == Format::TrueType;
        let base = format!(
            "{}+{}",
            subset_tag(&reduced.data),
            postscript_name(&program)
        );

        let font_file = {
            let mut dictionary = Dictionary::new();
            let key: &[u8] = if truetype {
                // Table 126: the length of the program before it is encoded.
                dictionary.insert(
                    Name::new(&b"Length1"[..]),
                    Object::new(Value::Integer(length(reduced.data.len())?)),
                );
                b"FontFile2"
            } else {
                dictionary.insert(Name::new(&b"Subtype"[..]), name_object(b"OpenType"));
                b"FontFile3"
            };
            let id = builder.add_stream(dictionary, reduced.data, StreamEncoding::Flate)?;
            (key, id)
        };

        let descriptor = {
            let mut dictionary = self.descriptor(&program, &base)?;
            dictionary.insert(Name::new(font_file.0), reference(font_file.1));
            builder.add(Object::new(Value::Dictionary(dictionary)))?
        };

        let to_unicode = builder.add_stream(
            Dictionary::new(),
            to_unicode_cmap(&self.text),
            StreamEncoding::Flate,
        )?;

        let widths = self.widths(&program)?;
        let descendant = {
            let mut dictionary = Dictionary::new();
            put(&mut dictionary, b"Type", name_object(b"Font"));
            put(
                &mut dictionary,
                b"Subtype",
                name_object(if truetype {
                    b"CIDFontType2"
                } else {
                    b"CIDFontType0"
                }),
            );
            put(&mut dictionary, b"BaseFont", name_object(base.as_bytes()));
            put(
                &mut dictionary,
                b"CIDSystemInfo",
                system_info(ORDERING_IDENTITY),
            );
            put(&mut dictionary, b"FontDescriptor", reference(descriptor));
            put(&mut dictionary, b"W", widths);
            if truetype {
                // 9.7.4.2: the CID is the glyph number.
                put(&mut dictionary, b"CIDToGIDMap", name_object(b"Identity"));
            }
            builder.add(Object::new(Value::Dictionary(dictionary)))?
        };

        let mut dictionary = Dictionary::new();
        put(&mut dictionary, b"Type", name_object(b"Font"));
        put(&mut dictionary, b"Subtype", name_object(b"Type0"));
        // Table 121: a CIDFontType2's name is the Type 0 font's; a
        // CIDFontType0's has the CMap's name after a hyphen.
        let type0_name = if truetype {
            base
        } else {
            format!("{base}-{}", String::from_utf8_lossy(ENCODING_IDENTITY_H))
        };
        put(
            &mut dictionary,
            b"BaseFont",
            name_object(type0_name.as_bytes()),
        );
        put(
            &mut dictionary,
            b"Encoding",
            name_object(ENCODING_IDENTITY_H),
        );
        put(
            &mut dictionary,
            b"DescendantFonts",
            Object::new(Value::Array(vec![reference(descendant)])),
        );
        put(&mut dictionary, b"ToUnicode", reference(to_unicode));
        builder.add(Object::new(Value::Dictionary(dictionary)))
    }

    /// The `W` array (9.7.4.3): one run from CID 0 with the width of every
    /// glyph of the set.
    fn widths(&self, program: &Program<'_>) -> Result<Object, PdfError> {
        let scale = glyph_scale(program);
        let widths = self
            .glyphs
            .glyphs()
            .map(|glyph| number(round_to(advance(program, glyph, scale), WIDTH_DECIMALS)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Object::new(Value::Array(vec![
            Object::new(Value::Integer(0)),
            Object::new(Value::Array(widths)),
        ])))
    }

    /// The font descriptor (9.8, Table 122) without its font file.
    fn descriptor(&self, program: &Program<'_>, base: &str) -> Result<Dictionary, PdfError> {
        let metrics = program.metrics();
        let scale = glyph_scale(program);
        let mut flags = FontFlags::SYMBOLIC;
        if metrics.is_monospaced == Some(true) {
            flags |= FontFlags::FIXED_PITCH;
        }
        if metrics.is_italic == Some(true) {
            flags |= FontFlags::ITALIC;
        }
        let bounds = self.bounds(program);
        let cap_height = metrics.cap_height.map(f64::from).or_else(|| {
            glyph_of(program, CAP_GLYPH)
                .and_then(|glyph| program.outline(glyph).ok())
                .and_then(|outline| outline.control_box())
                .map(|b| f64::from(b[3]))
        });

        let mut dictionary = Dictionary::new();
        put(&mut dictionary, b"Type", name_object(b"FontDescriptor"));
        put(&mut dictionary, b"FontName", name_object(base.as_bytes()));
        put(
            &mut dictionary,
            b"Flags",
            Object::new(Value::Integer(i64::from(flags))),
        );
        put(
            &mut dictionary,
            b"FontBBox",
            Object::new(Value::Array(
                bounds
                    .iter()
                    .enumerate()
                    // The lower left corner is rounded down, the upper right
                    // up, so that the box still holds every glyph.
                    .map(|(i, v)| {
                        let scaled = v * scale;
                        number(if i < 2 { scaled.floor() } else { scaled.ceil() })
                    })
                    .collect::<Result<Vec<_>, _>>()?,
            )),
        );
        put(
            &mut dictionary,
            b"ItalicAngle",
            number(f64::from(metrics.italic_angle.unwrap_or(0.0)))?,
        );
        put(
            &mut dictionary,
            b"Ascent",
            number((f64::from(metrics.ascent.unwrap_or(0.0)) * scale).round())?,
        );
        put(
            &mut dictionary,
            b"Descent",
            number((f64::from(metrics.descent.unwrap_or(0.0)) * scale).round())?,
        );
        if let Some(cap) = cap_height {
            put(
                &mut dictionary,
                b"CapHeight",
                number((cap * scale).round())?,
            );
        }
        put(
            &mut dictionary,
            b"StemV",
            number((stem_width(program).unwrap_or(0.0) * scale).round())?,
        );
        Ok(dictionary)
    }

    /// The box, in design units, that holds the outlines of the glyphs of
    /// the set (`[0 0 0 0]` when none has marks).
    fn bounds(&self, program: &Program<'_>) -> [f64; 4] {
        let mut found: Option<[f32; 4]> = None;
        for glyph in self.glyphs.glyphs() {
            let Some(b) = program
                .outline(glyph)
                .ok()
                .and_then(|outline| outline.control_box())
            else {
                continue;
            };
            found = Some(match found {
                None => b,
                Some(a) => [
                    a[0].min(b[0]),
                    a[1].min(b[1]),
                    a[2].max(b[2]),
                    a[3].max(b[3]),
                ],
            });
        }
        found.unwrap_or([0.0; 4]).map(f64::from)
    }
}

/// The ToUnicode CMap (9.10.3): the character each glyph was first chosen
/// for, as UTF-16BE.
fn to_unicode_cmap(text: &BTreeMap<u16, char>) -> Vec<u8> {
    let mut out = String::new();
    out.push_str("/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n");
    out.push_str(&format!(
        "/CIDSystemInfo << /Registry ({}) /Ordering ({}) /Supplement {} >> def\n",
        String::from_utf8_lossy(REGISTRY),
        String::from_utf8_lossy(ORDERING_UCS),
        SUPPLEMENT
    ));
    out.push_str("/CMapName /Adobe-Identity-UCS def\n/CMapType 2 def\n");
    out.push_str("1 begincodespacerange\n<0000> <FFFF>\nendcodespacerange\n");
    let entries: Vec<(&u16, &char)> = text.iter().collect();
    for section in entries.chunks(BFCHAR_PER_SECTION) {
        out.push_str(&format!("{} beginbfchar\n", section.len()));
        for (cid, character) in section {
            let mut units = [0u16; 2];
            let hex: String = character
                .encode_utf16(&mut units)
                .iter()
                .map(|unit| format!("{unit:04X}"))
                .collect();
            out.push_str(&format!("<{cid:04X}> <{hex}>\n"));
        }
        out.push_str("endbfchar\n");
    }
    out.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    out.into_bytes()
}

/// Decimals kept in a width: an advance of a font of 2048 units to the em
/// is a multiple of 1000/2048, which a thousandth keeps to within a
/// hundred-thousandth of a text space unit a glyph.
const WIDTH_DECIMALS: i32 = 3;

fn round_to(value: f64, decimals: i32) -> f64 {
    let k = 10f64.powi(decimals);
    (value * k).round() / k
}

/// The glyph of `character` through the font's Unicode `cmap`; none for a
/// character it maps to `.notdef`.
fn glyph_of(program: &Program<'_>, character: char) -> Option<GlyphId> {
    program
        .glyph_by_unicode(character)
        .filter(|glyph| glyph.0 != 0)
}

/// Glyph space units (thousandths of an em) per design unit. The matrix of
/// an sfnt is the single-precision `1/unitsPerEm`; the units to the em are
/// a whole number, which the division recovers exactly.
fn glyph_scale(program: &Program<'_>) -> f64 {
    let units_per_em = (1.0 / f64::from(program.font_matrix()[0])).round();
    GLYPH_UNITS / units_per_em
}

fn advance(program: &Program<'_>, glyph: GlyphId, scale: f64) -> f64 {
    f64::from(program.advance(glyph).unwrap_or(0.0)) * scale
}

/// The PostScript name of the font with the characters 9.6.4 and 7.3.5 do
/// not allow in a name removed: a name given by the font, else its full or
/// family name.
fn postscript_name(program: &Program<'_>) -> String {
    let names = program.names();
    let chosen = names
        .postscript
        .or(names.full)
        .or(names.family)
        .unwrap_or_default();
    let clean: String = chosen
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        .collect();
    if clean.is_empty() {
        "Embedded".to_string()
    } else {
        clean
    }
}

/// Six capital letters from the subset program's digest (9.6.4): the same
/// glyphs of the same font always get the same tag, and a different subset
/// of a font another.
fn subset_tag(subset: &[u8]) -> String {
    let digest = Md5::digest(subset);
    digest
        .iter()
        .take(TAG_LETTERS)
        .map(|byte| char::from(b'A' + byte % LETTERS))
        .collect()
}

/// The thickness of the vertical stem of the first of [`STEM_GLYPHS`] the
/// font has, in design units: the distance between the two edges the glyph's
/// outline has at half its height. `None` when no glyph gives exactly two.
fn stem_width(program: &Program<'_>) -> Option<f64> {
    STEM_GLYPHS.iter().find_map(|&c| {
        let outline = program.outline(glyph_of(program, c)?).ok()?;
        let [_, bottom, _, top] = outline.control_box()?;
        match crossings(&outline, (bottom + top) / 2.0)[..] {
            [left, right] => Some(f64::from(right - left)),
            _ => None,
        }
    })
}

/// The x of every place the outline crosses the horizontal line at `y`,
/// ascending; curves are followed as [`CURVE_STEPS`] straight pieces.
fn crossings(outline: &Outline, y: f32) -> Vec<f32> {
    let mut xs = Vec::new();
    let mut edge = |from: (f32, f32), to: (f32, f32)| {
        if (from.1 <= y) != (to.1 <= y) {
            let t = (y - from.1) / (to.1 - from.1);
            xs.push(from.0 + t * (to.0 - from.0));
        }
    };
    let (mut start, mut at) = ((0.0, 0.0), (0.0, 0.0));
    for segment in &outline.segments {
        match *segment {
            Segment::MoveTo(p) => {
                edge(at, start);
                start = (p.x, p.y);
                at = start;
            }
            Segment::LineTo(p) => {
                edge(at, (p.x, p.y));
                at = (p.x, p.y);
            }
            Segment::QuadTo(c, p) => {
                let from = at;
                let mut previous = from;
                for step in 1..=CURVE_STEPS {
                    let t = step as f32 / CURVE_STEPS as f32;
                    let u = 1.0 - t;
                    let point = (
                        u * u * from.0 + 2.0 * u * t * c.x + t * t * p.x,
                        u * u * from.1 + 2.0 * u * t * c.y + t * t * p.y,
                    );
                    edge(previous, point);
                    previous = point;
                }
                at = (p.x, p.y);
            }
            Segment::CubicTo(a, b, p) => {
                let from = at;
                let mut previous = from;
                for step in 1..=CURVE_STEPS {
                    let t = step as f32 / CURVE_STEPS as f32;
                    let u = 1.0 - t;
                    let point = (
                        u * u * u * from.0
                            + 3.0 * u * u * t * a.x
                            + 3.0 * u * t * t * b.x
                            + t * t * t * p.x,
                        u * u * u * from.1
                            + 3.0 * u * u * t * a.y
                            + 3.0 * u * t * t * b.y
                            + t * t * t * p.y,
                    );
                    edge(previous, point);
                    previous = point;
                }
                at = (p.x, p.y);
            }
            Segment::Close => {
                edge(at, start);
                at = start;
            }
        }
    }
    edge(at, start);
    xs.sort_by(f32::total_cmp);
    xs
}

fn put(dictionary: &mut Dictionary, key: &[u8], value: Object) {
    dictionary.insert(Name::new(key), value);
}

fn name_object(bytes: &[u8]) -> Object {
    Object::new(Value::Name(Name::new(bytes)))
}

fn reference(id: ObjectId) -> Object {
    Object::new(Value::Reference(id))
}

fn length(len: usize) -> Result<i64, PdfError> {
    i64::try_from(len).map_err(|_| PdfError::Invalid("a font program of this size".to_string()))
}

/// A `CIDSystemInfo` dictionary (Table 116) of the Adobe registry.
fn system_info(ordering: &[u8]) -> Object {
    let mut dictionary = Dictionary::new();
    put(
        &mut dictionary,
        b"Registry",
        Object::new(Value::String(PdfString::new(REGISTRY))),
    );
    put(
        &mut dictionary,
        b"Ordering",
        Object::new(Value::String(PdfString::new(ordering))),
    );
    put(
        &mut dictionary,
        b"Supplement",
        Object::new(Value::Integer(SUPPLEMENT)),
    );
    Object::new(Value::Dictionary(dictionary))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn a_character_past_the_bmp_is_a_surrogate_pair() {
        let text = BTreeMap::from([(1u16, 'A'), (2, '\u{1F600}')]);
        let cmap = String::from_utf8(to_unicode_cmap(&text)).unwrap();
        assert!(cmap.contains("2 beginbfchar\n<0001> <0041>\n<0002> <D83DDE00>\nendbfchar\n"));
        assert!(cmap.contains("<0000> <FFFF>\nendcodespacerange"));
    }

    #[test]
    fn a_bfchar_section_holds_at_most_a_hundred_entries() {
        let text: BTreeMap<u16, char> = (1..=101u16).map(|cid| (cid, 'x')).collect();
        let cmap = String::from_utf8(to_unicode_cmap(&text)).unwrap();
        assert!(cmap.contains("100 beginbfchar"));
        assert!(cmap.contains("\n1 beginbfchar\n<0065> <0078>\nendbfchar"));
        assert_eq!(cmap.matches("beginbfchar").count(), 2);
    }

    #[test]
    fn a_subset_tag_is_six_capitals_that_follow_the_bytes() {
        let tag = subset_tag(b"one");
        assert_eq!(tag.len(), TAG_LETTERS);
        assert!(tag.bytes().all(|b| b.is_ascii_uppercase()));
        assert_eq!(tag, subset_tag(b"one"));
        assert_ne!(tag, subset_tag(b"two"));
    }

    #[test]
    fn widths_keep_a_thousandth() {
        assert_eq!(round_to(0.48828125 * 1024.0, WIDTH_DECIMALS), 500.0);
        assert_eq!(round_to(1.0 / 3.0, WIDTH_DECIMALS), 0.333);
    }
}
