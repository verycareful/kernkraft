// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! PDF fonts, read side: ISO 32000-1:2008, clause 9 (text), apart from
//! the font programs themselves, which `kernkraft-font` reads.
//!
//! [`Document::font`] reads a font dictionary into a [`Font`]: its
//! descriptor, its encoding, its widths and, for a composite font, its
//! CMap and descendant CIDFont. [`Font::chars`] then splits the bytes of a
//! string a text-showing operator gives into characters: each has its
//! code, CID, glyph name, Unicode text and width. Choosing the glyph in
//! the font program belongs to the renderer.
//!
//! The text of a character comes from, in this order (9.10.2): the font's
//! ToUnicode CMap; for a simple font, its glyph name through the Adobe
//! Glyph List; for a composite font whose CMap is a Unicode one, the code
//! itself; and for a CID font of an Adobe character collection, the
//! collection's Unicode CMap. A character with none has no text.
//!
//! [`FontEmbedder`] is the write side: it embeds a TrueType or OpenType font
//! as a subset, with the descriptor, widths and ToUnicode map a reader
//! needs, and gives the string bytes that show text with it.
//!
//! Widths are in thousandths of a text space unit. A font whose dictionary
//! has no `Widths` and that is, or is named like, one of the standard 14
//! gets the widths of Adobe's metrics files (9.6.2.2).

mod cmap;
mod cmap_parse;
mod composite;
mod data;
mod descriptor;
mod embed;
mod encoding;
mod glyphlist;
mod range_map;
mod simple;
mod standard;
mod util;
mod widths;

use std::borrow::Cow;
use std::collections::BTreeMap;
use std::sync::Arc;

pub use cmap::{CMap, CidSystemInfo};
pub use descriptor::{FontDescriptor, FontFile, FontFlags, ProgramKind};
pub use embed::FontEmbedder;
pub use encoding::BaseEncoding;
pub(crate) use glyphlist::text_for_name;
pub use widths::VerticalMetrics;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

/// The most bytes of a decoded font program.
const MAX_PROGRAM_BYTES: usize = 128 << 20;
/// The most bytes of a decoded CMap.
pub(crate) const MAX_CMAP_BYTES: usize = 4 << 20;
/// A `CIDToGIDMap` has two bytes for each of the 65536 CIDs.
const MAX_CID_TO_GID_BYTES: usize = 2 << 16;
/// 9.3.3: word spacing applies to the single-byte code 32.
const SPACE_CODE: u32 = 32;
/// 9.2.4: the thousandths of a unit that widths are given in.
const WIDTH_UNIT: f64 = 1000.0;

/// What a font dictionary's `Subtype` says (Table 109).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FontKind {
    Type1,
    /// A multiple master font (9.6.2.3), read as a Type 1 font.
    MMType1,
    TrueType,
    /// A font whose glyphs are content streams (9.6.5).
    Type3,
    /// A composite font (9.7).
    Type0,
}

/// An embedded font program, decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmbeddedProgram {
    pub kind: ProgramKind,
    pub data: Vec<u8>,
}

/// The glyphs of a Type 3 font (9.6.5, Table 113).
#[derive(Debug, Clone)]
pub struct Type3 {
    /// The matrix from glyph space to text space.
    pub font_matrix: [f64; 6],
    pub font_bbox: Option<[f64; 4]>,
    /// The glyph description stream of each glyph name.
    pub char_procs: BTreeMap<String, ObjectId>,
    /// The resources of the glyph descriptions, as the font gives them.
    pub resources: Option<Object>,
}

/// A character of a string shown with a font.
#[derive(Debug, Clone, PartialEq)]
pub struct Char<'a> {
    /// The character code, as the string's bytes give it (9.7.6.2).
    pub code: u32,
    /// The bytes the code takes in the string.
    pub len: usize,
    /// The CID of a composite font's character; the code itself for a
    /// simple font's.
    pub cid: u32,
    /// The glyph name a simple font's encoding gives the code.
    pub glyph_name: Option<&'a str>,
    /// The Unicode text of the character, when it can be told (see the
    /// module docs).
    pub unicode: Option<Cow<'a, str>>,
    /// The horizontal advance, in thousandths of a text space unit.
    pub width: f64,
    /// The vertical metrics, when the font writes vertically.
    pub vertical: Option<VerticalMetrics>,
    /// True for the single-byte code 32, which word spacing adds to
    /// (9.3.3).
    pub is_space: bool,
}

pub(crate) enum Body {
    Simple(simple::Simple),
    Composite(composite::Composite),
}

/// A font, read from its dictionary.
pub struct Font {
    id: Option<ObjectId>,
    kind: FontKind,
    base_font: Option<String>,
    descriptor: Option<FontDescriptor>,
    to_unicode: Option<Arc<CMap>>,
    type3: Option<Type3>,
    body: Body,
}

impl std::fmt::Debug for Font {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Font")
            .field("id", &self.id)
            .field("kind", &self.kind)
            .field("base_font", &self.base_font)
            .finish_non_exhaustive()
    }
}

impl Font {
    pub fn kind(&self) -> FontKind {
        self.kind
    }

    /// The `BaseFont` name, with a subset prefix if it has one.
    pub fn base_font(&self) -> Option<&str> {
        self.base_font.as_deref()
    }

    /// The font descriptor; a composite font's is its descendant's. A
    /// standard font may have none.
    pub fn descriptor(&self) -> Option<&FontDescriptor> {
        self.descriptor.as_ref()
    }

    /// The glyph descriptions of a Type 3 font.
    pub fn type3(&self) -> Option<&Type3> {
        self.type3.as_ref()
    }

    /// The CMap of a composite font's `Encoding`.
    pub fn encoding_cmap(&self) -> Option<&CMap> {
        match &self.body {
            Body::Composite(composite) => Some(&composite.cmap),
            Body::Simple(_) => None,
        }
    }

    /// The character collection of a composite font's descendant.
    pub fn system_info(&self) -> Option<&CidSystemInfo> {
        match &self.body {
            Body::Composite(composite) => composite.system_info.as_ref(),
            Body::Simple(_) => None,
        }
    }

    /// True when the font writes vertically: a composite font whose CMap
    /// says so (9.7.4.3).
    pub fn is_vertical(&self) -> bool {
        matches!(&self.body, Body::Composite(composite) if composite.cmap.is_vertical())
    }

    /// The glyph name a simple font's encoding gives `code`.
    pub fn glyph_name(&self, code: u8) -> Option<&str> {
        match &self.body {
            Body::Simple(simple) => simple.encoding.name(code),
            Body::Composite(_) => None,
        }
    }

    /// True for a simple TrueType font whose codes select glyphs through
    /// the program's `cmap` and not through glyph names (9.6.6.4: a
    /// symbolic font with no named Mac or Windows encoding).
    pub fn truetype_uses_cmap_directly(&self) -> bool {
        matches!(&self.body, Body::Simple(simple) if simple.cmap_directly)
    }

    /// The characters of `bytes`, a string a text-showing operator gives.
    pub fn chars<'a>(&'a self, bytes: &'a [u8]) -> Chars<'a> {
        Chars { font: self, bytes }
    }

    /// The embedded font program, decoded; `None` when the font has none.
    pub fn embedded_program(&self, doc: &Document) -> Result<Option<EmbeddedProgram>, PdfError> {
        let Some(file) = self.descriptor.as_ref().and_then(|d| d.font_file) else {
            return Ok(None);
        };
        Ok(Some(EmbeddedProgram {
            kind: file.kind,
            data: decode_stream(doc, file.stream, MAX_PROGRAM_BYTES)?,
        }))
    }

    /// The `CIDToGIDMap` of a Type 2 CIDFont: the glyph index of each CID
    /// from 0 up (9.7.4.2), or `None` when the map is the identity or the
    /// font is not a CIDFont.
    pub fn cid_to_gid(&self, doc: &Document) -> Result<Option<Vec<u16>>, PdfError> {
        let Body::Composite(composite) = &self.body else {
            return Ok(None);
        };
        let Some(stream) = composite.cid_to_gid else {
            return Ok(None);
        };
        let data = decode_stream(doc, stream, MAX_CID_TO_GID_BYTES)?;
        Ok(Some(
            data.as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_be_bytes(*pair))
                .collect(),
        ))
    }

    fn unicode_of(&self, code: u32, len: usize) -> Option<Cow<'_, str>> {
        if let Some(map) = &self.to_unicode {
            if let Some(text) = map.to_unicode(code, len) {
                return Some(Cow::Owned(text));
            }
            // A simple font's ToUnicode may be written with two-byte codes.
            if matches!(self.body, Body::Simple(_)) && len == 1 {
                if let Some(text) = map.to_unicode(code, 2) {
                    return Some(Cow::Owned(text));
                }
            }
        }
        match &self.body {
            Body::Simple(simple) => simple.text(code).map(Cow::Borrowed),
            Body::Composite(composite) => composite.text(code, len).map(Cow::Owned),
        }
    }
}

/// The characters of a string; see [`Font::chars`].
pub struct Chars<'a> {
    font: &'a Font,
    bytes: &'a [u8],
}

impl<'a> Iterator for Chars<'a> {
    type Item = Char<'a>;

    fn next(&mut self) -> Option<Char<'a>> {
        let font = self.font;
        let first = *self.bytes.first()?;
        let (code, len, cid, glyph_name, width, vertical) = match &font.body {
            Body::Simple(simple) => {
                let (len, cid) = (1, u32::from(first));
                (
                    cid,
                    len,
                    cid,
                    simple.encoding.name(first),
                    simple.width(first),
                    None,
                )
            }
            Body::Composite(composite) => {
                let (code, len) = composite.cmap.read_code(self.bytes);
                let cid = composite.cmap.cid(code, len);
                let vertical = composite
                    .cmap
                    .is_vertical()
                    .then(|| composite.widths.vertical(cid));
                (code, len, cid, None, composite.widths.width(cid), vertical)
            }
        };
        self.bytes = &self.bytes[len..];
        Some(Char {
            code,
            len,
            cid,
            glyph_name,
            unicode: font.unicode_of(code, len),
            width,
            vertical,
            is_space: len == 1 && code == SPACE_CODE,
        })
    }
}

/// The decoded data of the stream `id`, at most `max` bytes.
fn decode_stream(doc: &Document, id: ObjectId, max: usize) -> Result<Vec<u8>, PdfError> {
    let object = doc.required(id)?;
    let stream = object.stream().ok_or(PdfError::WrongKind { object: id })?;
    Ok(doc.decode_stream(id, stream, max)?.data.into_owned())
}

impl Document {
    /// Reads the font that `object` is or refers to: a font dictionary
    /// (9.6, 9.7). A dictionary whose `Subtype` is none of those of
    /// Table 109 is [`PdfError::InvalidEntry`].
    pub fn font(&self, object: &Object) -> Result<Font, PdfError> {
        let resolved = self.resolve(object)?;
        let id = resolved.id();
        let dictionary = resolved
            .dictionary()
            .ok_or(PdfError::InvalidEntry {
                object: id,
                key: "Font",
            })?
            .clone();
        let mut diagnostics = Diagnostics::new();
        diagnostics.set_object(id);
        let result = Font::read(self, id, &dictionary, &mut diagnostics);
        self.record_diagnostics(diagnostics);
        result
    }
}

impl Font {
    fn read(
        doc: &Document,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<Font, PdfError> {
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        let kind = match util::entry_name(doc, dictionary, b"Subtype")?.as_deref() {
            Some("Type1") => FontKind::Type1,
            Some("MMType1") => FontKind::MMType1,
            Some("TrueType") => FontKind::TrueType,
            Some("Type3") => FontKind::Type3,
            Some("Type0") => FontKind::Type0,
            _ => return Err(invalid("Subtype")),
        };
        let base_font = util::entry_name(doc, dictionary, b"BaseFont")?;
        let to_unicode = Self::read_to_unicode(doc, dictionary, diagnostics)?;
        if kind == FontKind::Type0 {
            let (composite, descriptor) =
                composite::Composite::read(doc, id, dictionary, diagnostics)?;
            return Ok(Font {
                id,
                kind,
                base_font,
                descriptor,
                to_unicode,
                type3: None,
                body: Body::Composite(composite),
            });
        }
        let (simple, descriptor, type3) =
            simple::Simple::read(doc, id, kind, base_font.as_deref(), dictionary, diagnostics)?;
        Ok(Font {
            id,
            kind,
            base_font,
            descriptor,
            to_unicode,
            type3,
            body: Body::Simple(simple),
        })
    }

    /// The font's `ToUnicode` CMap. A stream that cannot be read is
    /// [`DiagnosticKind::ToUnicodeUnreadable`] and the font has none.
    fn read_to_unicode(
        doc: &Document,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Arc<CMap>>, PdfError> {
        let Some(Value::Reference(stream)) = dictionary.get(b"ToUnicode").map(Object::value) else {
            return Ok(None);
        };
        let data = match decode_stream(doc, *stream, MAX_CMAP_BYTES) {
            Ok(data) => data,
            Err(error @ PdfError::Fs(_)) => return Err(error),
            Err(_) => {
                diagnostics.push(DiagnosticKind::ToUnicodeUnreadable, 0);
                return Ok(None);
            }
        };
        match cmap::parse(&data, None, &|name| CMap::predefined(name), diagnostics) {
            Ok(map) => Ok(Some(Arc::new(map))),
            Err(_) => {
                diagnostics.push(DiagnosticKind::ToUnicodeUnreadable, 0);
                Ok(None)
            }
        }
    }
}
