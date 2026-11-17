// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Simple fonts: Type 1, MMType1, TrueType (ISO 32000-1:2008, 9.6.2, 9.6.3)
//! and Type 3 (9.6.5). One byte of a string is one character code, which
//! the font's encoding (9.6.6) turns into a glyph name.

use std::collections::BTreeMap;

use super::descriptor::{FontDescriptor, ProgramKind};
use super::encoding::{BaseEncoding, Encoding, CODES};
use super::glyphlist::text_for_name;
use super::standard::standard_metrics;
use super::util::{entry_array, entry_dictionary, number};
use super::widths::SimpleWidths;
use super::{decode_stream, FontKind, Type3, MAX_PROGRAM_BYTES, WIDTH_UNIT};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::font::data::standard14::StandardMetrics;
use crate::syntax::{Dictionary, ObjectId, Value};
use crate::PdfError;

/// Table 113: the matrix of a font whose glyph space is a thousandth of
/// text space.
const DEFAULT_FONT_MATRIX: [f64; 6] = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];

pub(crate) struct Simple {
    pub(crate) encoding: Encoding,
    texts: Vec<Option<String>>,
    widths: SimpleWidths,
    missing_width: f64,
    standard: Option<&'static StandardMetrics>,
    /// Widths are in glyph space: thousandths of text space for every font
    /// but a Type 3 one, whose `FontMatrix` scales them.
    scale: f64,
    pub(crate) cmap_directly: bool,
}

impl Simple {
    /// The width of `code`: the font's `Widths`, else the metrics of the
    /// standard font it is (or is named like) when it has no `Widths`,
    /// else its `MissingWidth` (Table 111, 9.6.2.2).
    pub(crate) fn width(&self, code: u8) -> f64 {
        if let Some(width) = self.widths.get(code) {
            return width * self.scale;
        }
        if !self.widths.present {
            if let Some(width) = self
                .standard
                .zip(self.encoding.name(code))
                .and_then(|(font, name)| font.width(name))
            {
                return f64::from(width);
            }
        }
        self.missing_width * self.scale
    }

    /// The text the glyph name of `code` stands for.
    pub(crate) fn text(&self, code: u32) -> Option<&str> {
        self.texts.get(usize::try_from(code).ok()?)?.as_deref()
    }

    pub(crate) fn read(
        doc: &Document,
        id: Option<ObjectId>,
        kind: FontKind,
        base_font: Option<&str>,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<(Simple, Option<FontDescriptor>, Option<Type3>), PdfError> {
        let descriptor = match entry_dictionary(doc, dictionary, b"FontDescriptor")? {
            Some(descriptor) => Some(FontDescriptor::read(doc, &descriptor)?),
            None => None,
        };
        let type3 = (kind == FontKind::Type3)
            .then(|| read_type3(doc, id, dictionary))
            .transpose()?;
        let standard = if kind == FontKind::Type3 {
            None
        } else {
            base_font.and_then(standard_metrics)
        };
        let (encoding, cmap_directly) = build_encoding(
            doc,
            kind,
            descriptor.as_ref(),
            standard,
            dictionary,
            diagnostics,
        )?;
        let widths = SimpleWidths::read(doc, dictionary, diagnostics)?;
        if !widths.present && standard.is_none() && kind != FontKind::Type3 {
            diagnostics.push(DiagnosticKind::WidthsMissing, 0);
        }
        let texts = encoding
            .names()
            .map(|name| name.and_then(text_for_name))
            .collect();
        let scale = match &type3 {
            Some(type3) => type3.font_matrix[0] * WIDTH_UNIT,
            None => 1.0,
        };
        let missing_width = descriptor.as_ref().map_or(0.0, |d| d.missing_width);
        Ok((
            Simple {
                encoding,
                texts,
                widths,
                missing_width,
                standard,
                scale,
                cmap_directly,
            },
            descriptor,
            type3,
        ))
    }
}

/// The glyph names of an embedded Type 1 or CFF program's own encoding.
fn builtin_names(
    doc: &Document,
    descriptor: Option<&FontDescriptor>,
    diagnostics: &mut Diagnostics,
) -> Result<Option<Encoding>, PdfError> {
    let Some(file) = descriptor.and_then(|d| d.font_file) else {
        return Ok(None);
    };
    if file.kind == ProgramKind::TrueType {
        return Ok(None);
    }
    let data = match decode_stream(doc, file.stream, MAX_PROGRAM_BYTES) {
        Ok(data) => data,
        Err(error @ PdfError::Fs(_)) => return Err(error),
        Err(_) => {
            diagnostics.push(DiagnosticKind::FontProgramUnreadable, 0);
            return Ok(None);
        }
    };
    let Ok(program) = kernkraft_font::Program::parse(&data, 0) else {
        diagnostics.push(DiagnosticKind::FontProgramUnreadable, 0);
        return Ok(None);
    };
    let names = (0..=u8::MAX)
        .map(|code| {
            program
                .glyph_by_builtin_code(code)
                .and_then(|glyph| program.glyph_name(glyph))
                .filter(|name| name != ".notdef")
        })
        .collect();
    Ok(Some(Encoding::from_names(names)))
}

/// The encoding of a simple font (9.6.6.1, Table 114, 9.6.6.2) and whether
/// its codes reach glyphs through the program's `cmap` (9.6.6.4).
fn build_encoding(
    doc: &Document,
    kind: FontKind,
    descriptor: Option<&FontDescriptor>,
    standard: Option<&'static StandardMetrics>,
    dictionary: &Dictionary,
    diagnostics: &mut Diagnostics,
) -> Result<(Encoding, bool), PdfError> {
    let flags = descriptor.map(|d| d.flags);
    let symbolic = flags.is_some_and(|f| f.symbolic());
    let entry = dictionary.get(b"Encoding");
    let resolved = entry.map(|object| doc.resolve(object)).transpose()?;
    let encoding_value = resolved.as_ref().map(|r| r.value());

    let named = match encoding_value {
        Some(Value::Name(name)) => {
            let base = BaseEncoding::from_name(name.as_bytes());
            if base.is_none() {
                diagnostics.push(DiagnosticKind::EncodingUnknown { name: name.clone() }, 0);
            }
            base
        }
        _ => None,
    };
    let encoding_dictionary = resolved.as_ref().and_then(|r| r.dictionary());

    // 9.6.6.4: a TrueType font reaches its glyphs by name through the
    // table below when its Encoding names MacRoman or WinAnsi or it is
    // nonsymbolic; otherwise a code selects a glyph through the cmap.
    let cmap_directly = kind == FontKind::TrueType
        && !matches!(named, Some(BaseEncoding::MacRoman | BaseEncoding::WinAnsi))
        && !(flags.is_some_and(|f| f.nonsymbolic()) || descriptor.is_none());

    // The base the Differences describe differences from, and the base of
    // an Encoding that is absent: the program's own, a standard font's
    // built-in one, or Standard for a nonsymbolic font.
    let implicit = |diagnostics: &mut Diagnostics| -> Result<Encoding, PdfError> {
        if kind == FontKind::Type3 {
            return Ok(Encoding::empty());
        }
        if let Some(builtin) = builtin_names(doc, descriptor, diagnostics)? {
            return Ok(builtin);
        }
        Ok(match standard {
            Some(font) if font.name == "Symbol" => Encoding::symbol(),
            Some(font) if font.name == "ZapfDingbats" => Encoding::zapf_dingbats(),
            _ if symbolic || cmap_directly => Encoding::empty(),
            _ => Encoding::base(BaseEncoding::Standard),
        })
    };

    let mut encoding = if let Some(base) = named {
        Encoding::base(base)
    } else if let Some(encoding_dictionary) = encoding_dictionary {
        let base = match super::util::entry_name(doc, encoding_dictionary, b"BaseEncoding")? {
            Some(name) => {
                let base = BaseEncoding::from_name(name.as_bytes());
                if base.is_none() {
                    diagnostics.push(
                        DiagnosticKind::EncodingUnknown {
                            name: crate::syntax::Name::new(name.into_bytes()),
                        },
                        0,
                    );
                }
                base
            }
            None => None,
        };
        let mut encoding = match base {
            Some(base) => Encoding::base(base),
            None => implicit(diagnostics)?,
        };
        if let Some(differences) = entry_array(doc, encoding_dictionary, b"Differences")? {
            encoding.apply_differences(doc, &differences, diagnostics)?;
        }
        // 9.6.6.4: a TrueType font's undefined entries are Standard's.
        if kind == FontKind::TrueType && !cmap_directly {
            encoding.fill_from(BaseEncoding::Standard);
        }
        encoding
    } else {
        implicit(diagnostics)?
    };
    if kind == FontKind::Type3 && encoding_dictionary.is_none() && named.is_none() {
        encoding = Encoding::empty();
    }
    debug_assert_eq!(encoding.names().count(), CODES);
    Ok((encoding, cmap_directly))
}

/// Table 113: the entries of a Type 3 font.
fn read_type3(
    doc: &Document,
    id: Option<ObjectId>,
    dictionary: &Dictionary,
) -> Result<Type3, PdfError> {
    let invalid = |key| PdfError::InvalidEntry { object: id, key };
    let font_matrix = match entry_array(doc, dictionary, b"FontMatrix")? {
        Some(items) if items.len() == 6 => {
            let mut matrix = [0.0; 6];
            for (slot, item) in matrix.iter_mut().zip(&items) {
                *slot = number(doc, item)?.ok_or(invalid("FontMatrix"))?;
            }
            matrix
        }
        Some(_) => return Err(invalid("FontMatrix")),
        None => DEFAULT_FONT_MATRIX,
    };
    let font_bbox = match entry_array(doc, dictionary, b"FontBBox")? {
        Some(items) if items.len() == 4 => {
            let mut bbox = [0.0; 4];
            for (slot, item) in bbox.iter_mut().zip(&items) {
                *slot = number(doc, item)?.unwrap_or(0.0);
            }
            Some(bbox)
        }
        _ => None,
    };
    let mut char_procs = BTreeMap::new();
    let procs = entry_dictionary(doc, dictionary, b"CharProcs")?.ok_or(invalid("CharProcs"))?;
    for (name, object) in procs.iter() {
        if let Value::Reference(stream) = object.value() {
            char_procs.insert(
                String::from_utf8_lossy(name.as_bytes()).into_owned(),
                *stream,
            );
        }
    }
    let resources = dictionary.get(b"Resources").cloned();
    Ok(Type3 {
        font_matrix,
        font_bbox,
        char_procs,
        resources,
    })
}
