// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Composite fonts: ISO 32000-1:2008, 9.7. A Type 0 font has a CMap that
//! turns the bytes of a string into character codes and CIDs, and one
//! descendant CIDFont that gives the glyph widths and, in its font
//! program, the glyphs.

use std::sync::Arc;

use super::cmap::{self, collection_text, CMap, CidSystemInfo};
use super::descriptor::FontDescriptor;
use super::util::{entry_dictionary, entry_integer, entry_name, entry_string};
use super::widths::CidWidths;
use super::{decode_stream, MAX_CMAP_BYTES};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

/// The character collections of Adobe whose CIDs have a Unicode CMap
/// (9.10.2).
const COLLECTION_ORDERINGS: [&str; 4] = ["GB1", "CNS1", "Japan1", "Korea1"];
const ADOBE: &str = "Adobe";
/// A surrogate pair's code is two units of two bytes.
const PAIR_CODE_BYTES: usize = 4;
const UNIT_CODE_BYTES: usize = 2;

pub(crate) struct Composite {
    pub(crate) cmap: Arc<CMap>,
    pub(crate) widths: CidWidths,
    pub(crate) system_info: Option<CidSystemInfo>,
    pub(crate) cid_to_gid: Option<ObjectId>,
    /// How the code of a Unicode CMap (`UniJIS-UCS2-H`, `UniGB-UTF16-V`)
    /// is read as text.
    unicode_codes: Option<UnicodeCodes>,
}

#[derive(Clone, Copy)]
enum UnicodeCodes {
    /// A two-byte code is a Basic Multilingual Plane character.
    Ucs2,
    /// A two-byte code is a UTF-16 unit and a four-byte one a pair.
    Utf16,
}

impl Composite {
    /// The text of the code `code` of `len` bytes when the CMap or the
    /// character collection gives it one (9.10.2).
    pub(crate) fn text(&self, code: u32, len: usize) -> Option<String> {
        if let Some(codes) = self.unicode_codes {
            return unicode_code(codes, code, len);
        }
        let info = self.system_info.as_ref()?;
        if info.registry != ADOBE || !COLLECTION_ORDERINGS.contains(&info.ordering.as_str()) {
            return None;
        }
        let cid = self.cmap.cid(code, len);
        (cid != 0)
            .then(|| collection_text(&info.registry, &info.ordering, cid))
            .flatten()
    }

    /// Reads the Type 0 font `dictionary`: its CMap and descendant. Returns
    /// the descendant's font descriptor too.
    pub(crate) fn read(
        doc: &Document,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<(Composite, Option<FontDescriptor>), PdfError> {
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        let cmap = Self::read_encoding(doc, id, dictionary, diagnostics)?;

        let descendants = super::util::entry_array(doc, dictionary, b"DescendantFonts")?
            .ok_or(invalid("DescendantFonts"))?;
        if descendants.len() != 1 {
            diagnostics.push(DiagnosticKind::DescendantFontCount, 0);
        }
        let first = descendants.first().ok_or(invalid("DescendantFonts"))?;
        let resolved = doc.resolve(first)?;
        let descendant = resolved
            .dictionary()
            .ok_or(invalid("DescendantFonts"))?
            .clone();
        let subtype = entry_name(doc, &descendant, b"Subtype")?;
        let is_type2 = match subtype.as_deref() {
            Some("CIDFontType0") => false,
            Some("CIDFontType2") => true,
            _ => return Err(invalid("DescendantFonts")),
        };

        let descriptor = match entry_dictionary(doc, &descendant, b"FontDescriptor")? {
            Some(descriptor) => Some(FontDescriptor::read(doc, &descriptor)?),
            None => None,
        };
        let widths = CidWidths::read(doc, &descendant, diagnostics)?;
        let system_info = read_system_info(doc, &descendant)?;
        let cid_to_gid = if is_type2 {
            match descendant.get(b"CIDToGIDMap").map(Object::value) {
                Some(Value::Reference(stream)) => {
                    // A stream that is not there falls back to the identity
                    // the entry's absence means, said once.
                    if doc.object(*stream)?.is_some_and(|o| o.stream().is_some()) {
                        Some(*stream)
                    } else {
                        diagnostics.push(DiagnosticKind::CidToGidUnreadable, 0);
                        None
                    }
                }
                _ => None,
            }
        } else {
            None
        };
        let unicode_codes = cmap.name().and_then(unicode_codes_of);
        Ok((
            Composite {
                cmap,
                widths,
                system_info,
                cid_to_gid,
                unicode_codes,
            },
            descriptor,
        ))
    }

    /// The font's `Encoding`: the name of a predefined CMap or a CMap
    /// stream (Table 121).
    fn read_encoding(
        doc: &Document,
        id: Option<ObjectId>,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<Arc<CMap>, PdfError> {
        let invalid = || PdfError::InvalidEntry {
            object: id,
            key: "Encoding",
        };
        let entry = dictionary.get(b"Encoding").ok_or_else(invalid)?;
        if let Value::Reference(stream) = entry.value() {
            let object = doc.required(*stream)?;
            if let Some(stream_data) = object.stream() {
                return Self::read_cmap_stream(doc, *stream, &stream_data.dictionary, diagnostics);
            }
        }
        match doc.resolve(entry)?.value() {
            Value::Name(name) => {
                let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                CMap::predefined(&name).ok_or(PdfError::UnknownCMap { name })
            }
            _ => Err(invalid()),
        }
    }

    /// An embedded encoding CMap: the stream's `UseCMap` and `WMode` entries
    /// (Table 120) and its data.
    fn read_cmap_stream(
        doc: &Document,
        stream: ObjectId,
        stream_dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<Arc<CMap>, PdfError> {
        let parent = match stream_dictionary.get(b"UseCMap").map(Object::value) {
            Some(Value::Name(name)) => {
                let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                Some(CMap::predefined(&name).ok_or(PdfError::UnknownCMap { name })?)
            }
            Some(Value::Reference(parent)) => {
                let object = doc.required(*parent)?;
                let parent_stream = object
                    .stream()
                    .ok_or(PdfError::WrongKind { object: *parent })?;
                Some(Self::read_cmap_stream(
                    doc,
                    *parent,
                    &parent_stream.dictionary,
                    diagnostics,
                )?)
            }
            _ => None,
        };
        let data = decode_stream(doc, stream, MAX_CMAP_BYTES)?;
        let mut cmap = cmap::parse(&data, parent, &|name| CMap::predefined(name), diagnostics)?;
        if let Some(mode) = entry_integer(doc, stream_dictionary, b"WMode")? {
            cmap.vertical = mode == 1;
        }
        if cmap.name.is_none() {
            cmap.name = entry_name(doc, stream_dictionary, b"CMapName")?;
        }
        Ok(Arc::new(cmap))
    }
}

/// `CIDSystemInfo` of a CIDFont (Table 116).
fn read_system_info(
    doc: &Document,
    descendant: &Dictionary,
) -> Result<Option<CidSystemInfo>, PdfError> {
    let Some(info) = entry_dictionary(doc, descendant, b"CIDSystemInfo")? else {
        return Ok(None);
    };
    let (Some(registry), Some(ordering)) = (
        entry_string(doc, &info, b"Registry")?,
        entry_string(doc, &info, b"Ordering")?,
    ) else {
        return Ok(None);
    };
    Ok(Some(CidSystemInfo {
        registry,
        ordering,
        supplement: entry_integer(doc, &info, b"Supplement")?.unwrap_or(0),
    }))
}

/// Whether the code of the predefined CMap `name` is Unicode: the CMaps of
/// Table 118 named `Uni...-UCS2-...` and `Uni...-UTF16-...`.
fn unicode_codes_of(name: &str) -> Option<UnicodeCodes> {
    if !name.starts_with("Uni") {
        return None;
    }
    if name.contains("-UCS2-") {
        Some(UnicodeCodes::Ucs2)
    } else if name.contains("-UTF16-") {
        Some(UnicodeCodes::Utf16)
    } else {
        None
    }
}

fn unicode_code(codes: UnicodeCodes, code: u32, len: usize) -> Option<String> {
    let c = match (codes, len) {
        (_, UNIT_CODE_BYTES) => char::from_u32(code),
        (UnicodeCodes::Utf16, PAIR_CODE_BYTES) => {
            let units = [(code >> 16) as u16, code as u16];
            char::decode_utf16(units).next().and_then(Result::ok)
        }
        _ => None,
    }?;
    Some(c.to_string())
}
