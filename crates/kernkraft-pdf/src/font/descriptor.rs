// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Font descriptors: ISO 32000-1:2008, 9.8 (Tables 122 and 124).

use super::util::{entry_integer, entry_name, entry_number, entry_string};
use crate::doc::Document;
use crate::syntax::{Dictionary, ObjectId, Value};
use crate::PdfError;

/// The flags of a font descriptor (Table 123), bit 1 being the lowest.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FontFlags(pub u32);

impl FontFlags {
    const FIXED_PITCH: u32 = 1 << 0;
    const SERIF: u32 = 1 << 1;
    const SYMBOLIC: u32 = 1 << 2;
    const SCRIPT: u32 = 1 << 3;
    const NONSYMBOLIC: u32 = 1 << 5;
    const ITALIC: u32 = 1 << 6;
    const ALL_CAP: u32 = 1 << 16;
    const SMALL_CAP: u32 = 1 << 17;
    const FORCE_BOLD: u32 = 1 << 18;

    pub fn fixed_pitch(self) -> bool {
        self.0 & Self::FIXED_PITCH != 0
    }
    pub fn serif(self) -> bool {
        self.0 & Self::SERIF != 0
    }
    /// The font uses glyphs outside the Adobe standard Latin set.
    pub fn symbolic(self) -> bool {
        self.0 & Self::SYMBOLIC != 0
    }
    pub fn script(self) -> bool {
        self.0 & Self::SCRIPT != 0
    }
    /// The font uses the Adobe standard Latin set.
    pub fn nonsymbolic(self) -> bool {
        self.0 & Self::NONSYMBOLIC != 0
    }
    pub fn italic(self) -> bool {
        self.0 & Self::ITALIC != 0
    }
    pub fn all_cap(self) -> bool {
        self.0 & Self::ALL_CAP != 0
    }
    pub fn small_cap(self) -> bool {
        self.0 & Self::SMALL_CAP != 0
    }
    pub fn force_bold(self) -> bool {
        self.0 & Self::FORCE_BOLD != 0
    }
}

/// What kind of font program a descriptor embeds (Table 124).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProgramKind {
    /// `FontFile`: a Type 1 font program.
    Type1,
    /// `FontFile2`: a TrueType font program.
    TrueType,
    /// `FontFile3` of subtype `Type1C` or `CIDFontType0C`: a bare CFF.
    Cff,
    /// `FontFile3` of subtype `OpenType`.
    OpenType,
}

/// The embedded font program of a descriptor: the stream it is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FontFile {
    pub stream: ObjectId,
    pub kind: ProgramKind,
}

/// A font descriptor.
#[derive(Debug, Clone, PartialEq)]
pub struct FontDescriptor {
    pub font_name: Option<String>,
    pub font_family: Option<String>,
    pub flags: FontFlags,
    pub font_bbox: Option<[f64; 4]>,
    pub italic_angle: f64,
    pub ascent: f64,
    pub descent: f64,
    pub leading: f64,
    pub cap_height: Option<f64>,
    pub x_height: Option<f64>,
    pub stem_v: f64,
    pub stem_h: f64,
    pub avg_width: f64,
    pub max_width: f64,
    /// The width of a glyph the font gives no width for (Table 122);
    /// zero when the descriptor has none.
    pub missing_width: f64,
    pub font_file: Option<FontFile>,
}

impl FontDescriptor {
    /// Reads the descriptor `dictionary`.
    pub(crate) fn read(doc: &Document, dictionary: &Dictionary) -> Result<Self, PdfError> {
        let number = |key: &[u8]| entry_number(doc, dictionary, key);
        let flags = entry_integer(doc, dictionary, b"Flags")?
            .and_then(|flags| u32::try_from(flags).ok())
            .unwrap_or(0);
        let font_bbox = match doc.get(dictionary, b"FontBBox")?.array() {
            Some([a, b, c, d]) => {
                let mut bbox = [0.0; 4];
                for (slot, item) in bbox.iter_mut().zip([a, b, c, d]) {
                    *slot = super::util::number(doc, item)?.unwrap_or(0.0);
                }
                Some(bbox)
            }
            _ => None,
        };
        Ok(FontDescriptor {
            font_name: entry_name(doc, dictionary, b"FontName")?,
            font_family: entry_string(doc, dictionary, b"FontFamily")?,
            flags: FontFlags(flags),
            font_bbox,
            italic_angle: number(b"ItalicAngle")?.unwrap_or(0.0),
            ascent: number(b"Ascent")?.unwrap_or(0.0),
            descent: number(b"Descent")?.unwrap_or(0.0),
            leading: number(b"Leading")?.unwrap_or(0.0),
            cap_height: number(b"CapHeight")?,
            x_height: number(b"XHeight")?,
            stem_v: number(b"StemV")?.unwrap_or(0.0),
            stem_h: number(b"StemH")?.unwrap_or(0.0),
            avg_width: number(b"AvgWidth")?.unwrap_or(0.0),
            max_width: number(b"MaxWidth")?.unwrap_or(0.0),
            missing_width: number(b"MissingWidth")?.unwrap_or(0.0),
            font_file: Self::font_file(doc, dictionary)?,
        })
    }

    /// The embedded program, which `FontFile`, `FontFile2` or `FontFile3`
    /// names. A reference to something that is not a stream is none.
    fn font_file(doc: &Document, dictionary: &Dictionary) -> Result<Option<FontFile>, PdfError> {
        for (key, kind) in [
            (&b"FontFile"[..], Some(ProgramKind::Type1)),
            (&b"FontFile2"[..], Some(ProgramKind::TrueType)),
            (&b"FontFile3"[..], None),
        ] {
            let Some(Value::Reference(id)) = dictionary.get(key).map(|object| object.value())
            else {
                continue;
            };
            let Some(object) = doc.object(*id)? else {
                continue;
            };
            let Some(stream) = object.stream() else {
                continue;
            };
            let kind = match kind {
                Some(kind) => kind,
                None => match entry_name(doc, &stream.dictionary, b"Subtype")?.as_deref() {
                    Some("OpenType") => ProgramKind::OpenType,
                    Some("Type1C" | "CIDFontType0C") => ProgramKind::Cff,
                    _ => continue,
                },
            };
            return Ok(Some(FontFile { stream: *id, kind }));
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Table 123: FixedPitch is bit 1, Symbolic bit 3, Nonsymbolic bit 6,
    /// Italic bit 7, ForceBold bit 19.
    #[test]
    fn flags_are_read_from_their_bits() {
        let flags = FontFlags(0b0000_0000_0000_0000_0010_0000_0010_0101);
        assert!(flags.fixed_pitch());
        assert!(!flags.serif());
        assert!(flags.symbolic());
        assert!(!flags.script());
        assert!(flags.nonsymbolic());
        assert!(!flags.italic());
        assert!(!flags.force_bold());
        let flags = FontFlags((1 << 6) | (1 << 18) | (1 << 16) | (1 << 17));
        assert!(flags.italic() && flags.force_bold() && flags.all_cap() && flags.small_cap());
    }
}
