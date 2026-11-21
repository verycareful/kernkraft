// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! PDFDocEncoding: ISO 32000-1:2008, Annex D, Table D.2.
//!
//! The encoding of text strings that have no byte order mark (7.9.2.2),
//! and of the passwords of security handler revisions 2 to 4 (7.6.3.3).

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
pub(crate) fn pdf_doc_code(character: char) -> Option<u8> {
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

/// The character PDFDocEncoding gives `code`, when Table D.2 defines one.
pub(crate) fn pdf_doc_char(code: u8) -> Option<char> {
    match code {
        b'\t' | b'\n' | b'\r' | b' '..=b'~' => Some(char::from(code)),
        EURO_CODE => Some(EURO),
        UNDEFINED_HIGH => None,
        0xA1..=0xFF => Some(char::from(code)),
        _ => code
            .checked_sub(HIGH_SYMBOLS_START)
            .and_then(|index| HIGH_SYMBOLS.get(usize::from(index)))
            .or_else(|| {
                code.checked_sub(LOW_ACCENTS_START)
                    .and_then(|index| LOW_ACCENTS.get(usize::from(index)))
            })
            .copied(),
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

    /// Decoding is the inverse of encoding on every code Table D.2
    /// defines, and gives nothing for the codes it leaves undefined.
    #[test]
    fn pdf_doc_char_inverts_pdf_doc_code() {
        let mut defined = 0;
        for code in 0..=u8::MAX {
            match pdf_doc_char(code) {
                Some(character) => {
                    assert_eq!(pdf_doc_code(character), Some(code), "{code:#x}");
                    defined += 1;
                }
                None => {
                    assert!((0..=0xFFFFu32)
                        .filter_map(char::from_u32)
                        .all(|character| pdf_doc_code(character) != Some(code)));
                }
            }
        }
        assert_eq!(
            defined,
            3 + (0x7E - 0x18 + 1) + (0x9E - 0x80 + 1) + (0xFF - 0xA0 + 1) - 1
        );
    }
}
