// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The bytes a password is checked as.
//!
//! Security handlers of revisions 2 to 4 take a password in
//! PDFDocEncoding (ISO 32000-1:2008, 7.6.3.3; the encoding is Table D.2).
//! Revisions 5 and 6 take its UTF-8 bytes after the SASLprep profile
//! (RFC 4013) of stringprep (RFC 3454), at most 127 of them (ISO
//! 32000-2:2020, 7.6.4.3.3).

use super::dictionary::Revision;

/// The most bytes of a password that revisions 5 and 6 use.
pub(crate) const MAX_UTF8_PASSWORD_BYTES: usize = 127;

/// The characters of PDFDocEncoding whose code is not their Unicode value
/// (Table D.2): codes 0x18 to 0x1F, then 0x80 to 0x9E, then 0xA0.
const LOW_ACCENTS: [char; 8] = [
    '\u{02D8}', '\u{02C7}', '\u{02C6}', '\u{02D9}', '\u{02DD}', '\u{02DB}', '\u{02DA}', '\u{02DC}',
];
const LOW_ACCENTS_START: u8 = 0x18;
const HIGH_SYMBOLS: [char; 31] = [
    '\u{2022}', '\u{2020}', '\u{2021}', '\u{2026}', '\u{2014}', '\u{2013}', '\u{0192}', '\u{2044}',
    '\u{2039}', '\u{203A}', '\u{2212}', '\u{2030}', '\u{201E}', '\u{201C}', '\u{201D}', '\u{2018}',
    '\u{2019}', '\u{201A}', '\u{2122}', '\u{FB01}', '\u{FB02}', '\u{0141}', '\u{0152}', '\u{0160}',
    '\u{0178}', '\u{017D}', '\u{0131}', '\u{0142}', '\u{0153}', '\u{0161}', '\u{017E}',
];
const HIGH_SYMBOLS_START: u8 = 0x80;
const EURO: char = '\u{20AC}';
const EURO_CODE: u8 = 0xA0;
/// The code Table D.2 leaves undefined among those above the euro sign.
const UNDEFINED_HIGH: u8 = 0xAD;

/// The PDFDocEncoding code of `character`, when it has one.
fn pdf_doc_code(character: char) -> Option<u8> {
    let in_table = |table: &[char], start: u8| {
        (start..)
            .zip(table)
            .find_map(|(code, &listed)| (listed == character).then_some(code))
    };
    match character {
        // The three control characters Table D.2 defines, and the codes
        // that are the same in ASCII.
        '\t' | '\n' | '\r' | ' '..='~' => u8::try_from(character).ok(),
        EURO => Some(EURO_CODE),
        // The codes that are the same in Latin-1.
        '\u{A1}'..='\u{FF}' => u8::try_from(character)
            .ok()
            .filter(|&code| code != UNDEFINED_HIGH),
        _ => in_table(&LOW_ACCENTS, LOW_ACCENTS_START)
            .or_else(|| in_table(&HIGH_SYMBOLS, HIGH_SYMBOLS_START)),
    }
}

/// `text` in PDFDocEncoding, or `None` when a character of it has no code
/// there.
pub(crate) fn pdf_doc_encode(text: &str) -> Option<Vec<u8>> {
    text.chars().map(pdf_doc_code).collect()
}

/// `text` after SASLprep, or `None` when the profile forbids it.
fn saslprep(text: &str) -> Option<String> {
    stringprep::saslprep(text)
        .ok()
        .map(|prepared| prepared.into_owned())
}

/// The byte strings `password` is tried as, in order, each with whether it
/// is the form the revision defines: that form when the password has
/// one, then its UTF-8 bytes when they differ from it. Writers exist that
/// encrypt with the UTF-8 bytes whatever the revision.
pub(crate) fn candidates(revision: Revision, password: &str) -> Vec<(Vec<u8>, bool)> {
    let defined = if revision.uses_utf8_passwords() {
        saslprep(password).map(String::into_bytes)
    } else {
        pdf_doc_encode(password)
    };
    let typed = password.as_bytes().to_vec();
    match defined {
        Some(defined) if defined == typed => vec![(typed, true)],
        Some(defined) => vec![(defined, true), (typed, false)],
        None => vec![(typed, false)],
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn pdf_doc_encoding_of_ascii_and_latin_1() {
        assert_eq!(pdf_doc_encode("user 42~").unwrap(), b"user 42~");
        assert_eq!(pdf_doc_encode("\t\n\r").unwrap(), b"\t\n\r");
        // U+00A1 to U+00FF keep their value as the code.
        assert_eq!(
            pdf_doc_encode("\u{A1}\u{E4}\u{F6}\u{FF}").unwrap(),
            [0xA1, 0xE4, 0xF6, 0xFF]
        );
    }

    /// Rows of Table D.2 where the code is not the Unicode value: the
    /// first and the last of each run, and the euro sign.
    #[test]
    fn pdf_doc_encoding_of_the_remapped_rows() {
        let rows = [
            ('\u{02D8}', 0x18), // BREVE
            ('\u{02DC}', 0x1F), // SMALL TILDE
            ('\u{2022}', 0x80), // BULLET
            ('\u{2122}', 0x92), // TRADE MARK SIGN
            ('\u{0141}', 0x95), // LATIN CAPITAL LETTER L WITH STROKE
            ('\u{017E}', 0x9E), // LATIN SMALL LETTER Z WITH CARON
            ('\u{20AC}', 0xA0), // EURO SIGN
        ];
        for (character, code) in rows {
            assert_eq!(pdf_doc_code(character), Some(code), "{character:?}");
        }
    }

    /// Every code is given by one character at most, and the three codes
    /// Table D.2 marks undefined above the control range (0x7F, 0x9F,
    /// 0xAD) are given by none.
    #[test]
    fn pdf_doc_codes_are_distinct_and_skip_the_undefined() {
        let mut seen = [false; 256];
        for character in (0..=0xFFFFu32).filter_map(char::from_u32) {
            if let Some(code) = pdf_doc_code(character) {
                assert!(!seen[usize::from(code)], "code {code:#x} twice");
                seen[usize::from(code)] = true;
            }
        }
        for undefined in [0x7Fusize, 0x9F, 0xAD] {
            assert!(!seen[undefined]);
        }
        // 3 control characters, 0x18 to 0x7E, 0x80 to 0x9E, 0xA0 to 0xFF
        // less 0xAD.
        let defined = 3 + (0x7E - 0x18 + 1) + (0x9E - 0x80 + 1) + (0xFF - 0xA0 + 1) - 1;
        assert_eq!(seen.iter().filter(|&&given| given).count(), defined);
    }

    #[test]
    fn characters_outside_pdf_doc_encoding() {
        for text in ["\u{0}", "\u{7F}", "\u{AD}", "\u{0416}", "\u{1F600}"] {
            assert_eq!(pdf_doc_encode(text), None, "{text:?}");
        }
    }

    /// The examples of RFC 4013, section 3.
    #[test]
    fn saslprep_examples_of_rfc_4013() {
        assert_eq!(saslprep("I\u{AD}X").as_deref(), Some("IX"));
        assert_eq!(saslprep("user").as_deref(), Some("user"));
        assert_eq!(saslprep("USER").as_deref(), Some("USER"));
        assert_eq!(saslprep("\u{AA}").as_deref(), Some("a"));
        assert_eq!(saslprep("\u{2168}").as_deref(), Some("IX"));
        assert_eq!(saslprep("\u{7}"), None);
        assert_eq!(saslprep("\u{627}\u{31}"), None);
    }

    #[test]
    fn candidates_for_revisions_2_to_4() {
        let revision = Revision::R3;
        assert_eq!(candidates(revision, "user"), [(b"user".to_vec(), true)]);
        // One byte in PDFDocEncoding, three in UTF-8.
        assert_eq!(
            candidates(revision, "\u{20AC}"),
            [(vec![0xA0], true), ("\u{20AC}".as_bytes().to_vec(), false)]
        );
        // No PDFDocEncoding form: the UTF-8 bytes alone.
        assert_eq!(
            candidates(revision, "\u{0416}"),
            [("\u{0416}".as_bytes().to_vec(), false)]
        );
    }

    #[test]
    fn candidates_for_revisions_5_and_6() {
        let revision = Revision::R6;
        assert_eq!(candidates(revision, "user"), [(b"user".to_vec(), true)]);
        // SASLprep maps a no-break space to a space.
        assert_eq!(
            candidates(revision, "a\u{A0}b"),
            [
                (b"a b".to_vec(), true),
                ("a\u{A0}b".as_bytes().to_vec(), false)
            ]
        );
        // SASLprep forbids a control character.
        assert_eq!(candidates(revision, "a\u{7}"), [(b"a\x07".to_vec(), false)]);
    }
}
