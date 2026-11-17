// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Character encodings of simple fonts: ISO 32000-1:2008, 9.6.6 (Table 114
//! and the sub-clauses on Type 1, Type 3 and TrueType fonts) and Annex D.
//!
//! An [`Encoding`] gives each of the 256 codes of a simple font the name
//! of a glyph, or none. It starts from a base (a predefined encoding, the
//! font program's own, or the built-in encoding of a standard font) and
//! applies the `Differences` of an encoding dictionary.

use std::borrow::Cow;

use super::data::encodings;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::syntax::{Object, Value};
use crate::PdfError;

pub(crate) const CODES: usize = 256;

/// The predefined encodings a font dictionary names (Annex D).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BaseEncoding {
    /// `StandardEncoding`: the base of a nonsymbolic font that has none.
    Standard,
    MacRoman,
    WinAnsi,
    MacExpert,
}

impl BaseEncoding {
    /// The encoding a name of 9.6.6 stands for. `StandardEncoding` is not
    /// a value the format allows for `Encoding` (only `BaseEncoding`
    /// names Standard implicitly), but writers use it.
    pub fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"StandardEncoding" => BaseEncoding::Standard,
            b"MacRomanEncoding" => BaseEncoding::MacRoman,
            b"WinAnsiEncoding" => BaseEncoding::WinAnsi,
            b"MacExpertEncoding" => BaseEncoding::MacExpert,
            _ => return None,
        })
    }

    /// The code the encoding gives the glyph `name`, when it has one.
    pub(crate) fn code_of(self, name: &str) -> Option<u8> {
        let at = self.table().iter().position(|entry| *entry == name)?;
        u8::try_from(at).ok()
    }

    fn table(self) -> &'static [&'static str; CODES] {
        match self {
            BaseEncoding::Standard => &encodings::STANDARD,
            BaseEncoding::MacRoman => &encodings::MAC_ROMAN,
            BaseEncoding::WinAnsi => &encodings::WIN_ANSI,
            BaseEncoding::MacExpert => &encodings::MAC_EXPERT,
        }
    }
}

/// The glyph name of each code of a simple font.
#[derive(Debug, Clone)]
pub(crate) struct Encoding {
    names: Vec<Option<Cow<'static, str>>>,
}

fn from_table(table: &'static [&'static str; CODES]) -> Encoding {
    Encoding {
        names: table
            .iter()
            .map(|name| (!name.is_empty()).then_some(Cow::Borrowed(*name)))
            .collect(),
    }
}

impl Encoding {
    /// An encoding that names no glyph.
    pub(crate) fn empty() -> Self {
        Self {
            names: vec![None; CODES],
        }
    }

    pub(crate) fn base(base: BaseEncoding) -> Self {
        from_table(base.table())
    }

    /// The built-in encoding of Symbol.
    pub(crate) fn symbol() -> Self {
        from_table(&encodings::SYMBOL)
    }

    /// The built-in encoding of ZapfDingbats.
    pub(crate) fn zapf_dingbats() -> Self {
        from_table(&encodings::ZAPF_DINGBATS)
    }

    /// An encoding of glyph names read from a font program.
    pub(crate) fn from_names(names: Vec<Option<String>>) -> Self {
        let mut encoding = Self::empty();
        for (slot, name) in encoding.names.iter_mut().zip(names) {
            *slot = name.map(Cow::Owned);
        }
        encoding
    }

    pub(crate) fn name(&self, code: u8) -> Option<&str> {
        self.names[usize::from(code)].as_deref()
    }

    pub(crate) fn names(&self) -> impl Iterator<Item = Option<&str>> {
        self.names.iter().map(|name| name.as_deref())
    }

    /// Gives every code that has no name the one it has in `base`.
    pub(crate) fn fill_from(&mut self, base: BaseEncoding) {
        for (slot, name) in self.names.iter_mut().zip(base.table()) {
            if slot.is_none() && !name.is_empty() {
                *slot = Some(Cow::Borrowed(*name));
            }
        }
    }

    /// Applies a `Differences` array (Table 114): a code starts a run of
    /// names, each for the next code. A code outside 0 to 255, a name that
    /// is not one, and a run past code 255 are
    /// [`DiagnosticKind::EncodingDifferenceInvalid`] and skipped.
    pub(crate) fn apply_differences(
        &mut self,
        doc: &Document,
        differences: &[Object],
        diagnostics: &mut Diagnostics,
    ) -> Result<(), PdfError> {
        let mut next: Option<usize> = None;
        for item in differences {
            match doc.resolve(item)?.value() {
                Value::Integer(code) => match usize::try_from(*code).ok().filter(|c| *c < CODES) {
                    Some(code) => next = Some(code),
                    None => {
                        next = None;
                        diagnostics.push(DiagnosticKind::EncodingDifferenceInvalid, 0);
                    }
                },
                Value::Name(name) => match next {
                    Some(code) if code < CODES => {
                        let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                        self.names[code] = Some(Cow::Owned(name));
                        next = Some(code + 1);
                    }
                    _ => diagnostics.push(DiagnosticKind::EncodingDifferenceInvalid, 0),
                },
                _ => diagnostics.push(DiagnosticKind::EncodingDifferenceInvalid, 0),
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Annex D, Table D.2, from the rows of the standard Latin character
    /// set that each encoding differs on.
    #[test]
    fn the_latin_encodings_differ_where_annex_d_says() {
        let standard = Encoding::base(BaseEncoding::Standard);
        let mac = Encoding::base(BaseEncoding::MacRoman);
        let win = Encoding::base(BaseEncoding::WinAnsi);
        // quoteright is 047 in StandardEncoding and quotesingle in the others.
        assert_eq!(standard.name(0o47), Some("quoteright"));
        assert_eq!(win.name(0o47), Some("quotesingle"));
        assert_eq!(mac.name(0o47), Some("quotesingle"));
        // Aacute: absent from Standard; 0347 in Mac, 0301 in WinAnsi.
        assert!(standard.names().flatten().all(|name| name != "Aacute"));
        assert_eq!(mac.name(0o347), Some("Aacute"));
        assert_eq!(win.name(0o301), Some("Aacute"));
        // Euro is WinAnsi 0200; MacRoman keeps currency at 0333.
        assert_eq!(win.name(0o200), Some("Euro"));
        assert_eq!(mac.name(0o333), Some("currency"));
    }

    /// The notes under Table D.2: the soft hyphen and the non-breaking
    /// spaces, and the bullet for the unused codes of WinAnsiEncoding.
    #[test]
    fn the_notes_under_table_d2_apply() {
        let win = Encoding::base(BaseEncoding::WinAnsi);
        assert_eq!(win.name(0o255), Some("hyphen"));
        assert_eq!(win.name(0o240), Some("space"));
        assert_eq!(win.name(0o225), Some("bullet"));
        assert_eq!(win.name(0o201), Some("bullet"));
        assert_eq!(win.name(0o40), Some("space"));
        assert_eq!(win.name(0o37), None);
        let mac = Encoding::base(BaseEncoding::MacRoman);
        assert_eq!(mac.name(0o312), Some("space"));
    }

    #[test]
    fn standard_encoding_defines_149_characters() {
        let standard = Encoding::base(BaseEncoding::Standard);
        assert_eq!(standard.names().flatten().count(), 149);
    }

    #[test]
    fn the_symbol_and_zapf_dingbats_encodings() {
        // Table D.5: Alpha is 0101. Table D.6: a1 is 041.
        assert_eq!(Encoding::symbol().name(0o101), Some("Alpha"));
        assert_eq!(Encoding::zapf_dingbats().name(0o41), Some("a1"));
        assert_eq!(Encoding::symbol().name(0o240), Some("Euro"));
    }

    #[test]
    fn mac_expert_has_the_small_capitals() {
        // Table D.4: Asmall is 0141.
        assert_eq!(
            Encoding::base(BaseEncoding::MacExpert).name(0o141),
            Some("Asmall")
        );
    }

    #[test]
    fn base_names_are_recognised() {
        assert_eq!(
            BaseEncoding::from_name(b"WinAnsiEncoding"),
            Some(BaseEncoding::WinAnsi)
        );
        assert_eq!(
            BaseEncoding::from_name(b"MacRomanEncoding"),
            Some(BaseEncoding::MacRoman)
        );
        assert_eq!(BaseEncoding::from_name(b"Identity-H"), None);
    }

    #[test]
    fn filling_leaves_named_codes_alone() {
        let mut encoding = Encoding::from_names(vec![None; CODES]);
        encoding.names[0o101] = Some(Cow::Borrowed("Q"));
        encoding.fill_from(BaseEncoding::Standard);
        assert_eq!(encoding.name(0o101), Some("Q"));
        assert_eq!(encoding.name(0o102), Some("B"));
    }
}
