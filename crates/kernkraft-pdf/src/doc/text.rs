// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text strings (ISO 32000-1:2008, 7.9.2.2) and dates (7.9.4).
//!
//! A text string is UTF-16BE when it starts with the byte order mark FE
//! FF, and PDFDocEncoding (Annex D) otherwise. ISO 32000-2:2020 adds UTF-8
//! after the mark EF BB BF, which is read too. An escape sequence in a
//! Unicode string (U+001B, a language code, maybe a country code, U+001B)
//! names the language of what follows; it is taken out of the text and
//! the first one is kept as the string's language.

use crate::pdf_doc::{pdf_doc_char, pdf_doc_encode};

const UTF16_MARK: [u8; 2] = [0xFE, 0xFF];
const UTF8_MARK: [u8; 3] = [0xEF, 0xBB, 0xBF];
const ESCAPE: char = '\u{1B}';
/// A language code of ISO 639 is two characters, and the optional country
/// code of ISO 3166 two more.
const LANGUAGE_CODE_CHARS: usize = 2;
const LANGUAGE_AND_COUNTRY_CHARS: usize = 4;

/// A text string decoded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TextString {
    pub text: String,
    /// The language the first escape sequence names, as written: a
    /// language code, maybe followed by a country code.
    pub language: Option<String>,
    /// False when some bytes had no character in their encoding and were
    /// read as U+FFFD.
    pub exact: bool,
}

/// Decodes the bytes of a text string.
pub fn decode_text_string(bytes: &[u8]) -> TextString {
    let (text, exact, unicode) = if let Some(rest) = bytes.strip_prefix(&UTF16_MARK) {
        let units: Vec<u16> = rest
            .chunks(2)
            .map(|pair| match pair {
                [high, low] => u16::from_be_bytes([*high, *low]),
                // An odd byte at the end is no code unit.
                _ => u16::MAX,
            })
            .collect();
        let odd = rest.len() % 2 == 1;
        let mut exact = !odd;
        let mut text: String = char::decode_utf16(units.iter().copied().take(rest.len() / 2))
            .map(|unit| {
                unit.unwrap_or_else(|_| {
                    exact = false;
                    char::REPLACEMENT_CHARACTER
                })
            })
            .collect();
        if odd {
            text.push(char::REPLACEMENT_CHARACTER);
        }
        (text, exact, true)
    } else if let Some(rest) = bytes.strip_prefix(&UTF8_MARK) {
        let text = String::from_utf8_lossy(rest);
        let exact = matches!(text, std::borrow::Cow::Borrowed(_));
        (text.into_owned(), exact, true)
    } else {
        let mut exact = true;
        let text = bytes
            .iter()
            .map(|&code| {
                pdf_doc_char(code).unwrap_or_else(|| {
                    exact = false;
                    char::REPLACEMENT_CHARACTER
                })
            })
            .collect();
        (text, exact, false)
    };
    if !unicode {
        return TextString {
            text,
            language: None,
            exact,
        };
    }
    let (text, language) = take_escapes(&text);
    TextString {
        text,
        language,
        exact,
    }
}

/// Takes the language escape sequences out of `text`: an escape, two or
/// four ASCII letters, an escape. An escape that does not start one is
/// left in the text.
fn take_escapes(text: &str) -> (String, Option<String>) {
    let mut out = String::with_capacity(text.len());
    let mut language = None;
    let mut rest = text;
    while let Some(start) = rest.find(ESCAPE) {
        out.push_str(&rest[..start]);
        let after = &rest[start + ESCAPE.len_utf8()..];
        let code = after.find(ESCAPE).map(|end| &after[..end]).filter(|code| {
            matches!(code.len(), LANGUAGE_CODE_CHARS | LANGUAGE_AND_COUNTRY_CHARS)
                && code.bytes().all(|byte| byte.is_ascii_alphabetic())
        });
        match code {
            Some(code) => {
                language.get_or_insert_with(|| code.to_string());
                rest = &after[code.len() + ESCAPE.len_utf8()..];
            }
            None => {
                out.push(ESCAPE);
                rest = after;
            }
        }
    }
    out.push_str(rest);
    (out, language)
}

/// `text` as the bytes of a text string: PDFDocEncoding when every
/// character has a code there and the string does not start as a byte
/// order mark would, UTF-16BE after its mark otherwise.
pub fn encode_text_string(text: &str) -> Vec<u8> {
    if let Some(bytes) = pdf_doc_encode(text) {
        if !bytes.starts_with(&UTF16_MARK) && !bytes.starts_with(&UTF8_MARK) {
            return bytes;
        }
    }
    let mut bytes = UTF16_MARK.to_vec();
    for unit in text.encode_utf16() {
        bytes.extend_from_slice(&unit.to_be_bytes());
    }
    bytes
}

/// A date (7.9.4): local time, and how far it is ahead of universal time
/// when that is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Date {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// Minutes ahead of universal time (behind when negative); `None`
    /// when the date does not say. 7.9.4 takes such a date as GMT.
    pub offset_minutes: Option<i16>,
}

const DATE_PREFIX: &str = "D:";
const MINUTES_PER_HOUR: i16 = 60;
const LAST_MONTH: u8 = 12;
const LAST_DAY: u8 = 31;
const LAST_HOUR: u8 = 23;
const LAST_MINUTE: u8 = 59;
const LAST_SECOND: u8 = 59;

impl Date {
    /// Reads a date as 7.9.4 writes it: `D:` and the year, then the month,
    /// day, hour, minute, second and the offset from universal time, each
    /// present only when the ones before it are. Omitted fields take the
    /// defaults 7.9.4 gives (month and day 1, the rest 0). The apostrophe
    /// that writers of earlier versions put after the offset's minutes is
    /// accepted. `None` for anything else.
    pub fn parse(text: &str) -> Option<Self> {
        let mut rest = text.strip_prefix(DATE_PREFIX)?;
        let mut field = |digits: usize| -> Option<Option<u16>> {
            if rest.is_empty() || !rest.as_bytes()[0].is_ascii_digit() {
                return Some(None);
            }
            let value = rest.get(..digits)?;
            if !value.bytes().all(|byte| byte.is_ascii_digit()) {
                return None;
            }
            rest = &rest[digits..];
            value.parse().ok().map(Some)
        };
        let year = field(4)??;
        let mut date = Date {
            year,
            month: 1,
            day: 1,
            hour: 0,
            minute: 0,
            second: 0,
            offset_minutes: None,
        };
        let parts = [
            (&mut date.month, 1, LAST_MONTH),
            (&mut date.day, 1, LAST_DAY),
            (&mut date.hour, 0, LAST_HOUR),
            (&mut date.minute, 0, LAST_MINUTE),
            (&mut date.second, 0, LAST_SECOND),
        ];
        for (slot, first, last) in parts {
            match field(2)? {
                Some(value) => {
                    let value = u8::try_from(value)
                        .ok()
                        .filter(|v| (first..=last).contains(v))?;
                    *slot = value;
                }
                None => break,
            }
        }
        date.offset_minutes = parse_offset(rest)?;
        Some(date)
    }

    /// The date as 7.9.4 writes it, every field present.
    pub fn format(&self) -> String {
        let mut text = format!(
            "{DATE_PREFIX}{:04}{:02}{:02}{:02}{:02}{:02}",
            self.year, self.month, self.day, self.hour, self.minute, self.second
        );
        match self.offset_minutes {
            None => {}
            Some(0) => text.push('Z'),
            Some(offset) => {
                let sign = if offset < 0 { '-' } else { '+' };
                let offset = offset.unsigned_abs();
                let per_hour = MINUTES_PER_HOUR.unsigned_abs();
                text.push_str(&format!(
                    "{sign}{:02}'{:02}",
                    offset / per_hour,
                    offset % per_hour
                ));
            }
        }
        text
    }
}

/// The offset field of a date: nothing, `Z`, or a sign, two digits of
/// hours and maybe an apostrophe and two digits of minutes. `None` when it
/// is none of these; `Some(None)` when it is absent.
fn parse_offset(text: &str) -> Option<Option<i16>> {
    let mut chars = text.chars();
    let sign = match chars.next() {
        None => return Some(None),
        Some('Z') => {
            let rest = chars.as_str();
            // `Z` may be followed by a zero offset, written by some.
            return match rest {
                "" | "00'00" | "00'00'" | "00" | "00'" => Some(Some(0)),
                _ => None,
            };
        }
        Some('+') => 1,
        Some('-') => -1,
        Some(_) => return None,
    };
    let rest = chars.as_str();
    let digits = |text: &str| -> Option<i16> {
        (text.len() == 2 && text.bytes().all(|byte| byte.is_ascii_digit()))
            .then(|| text.parse().ok())
            .flatten()
    };
    let hours = digits(rest.get(..2)?)?;
    let rest = &rest[2..];
    let minutes = match rest {
        "" | "'" => 0,
        _ => {
            let minutes = rest.strip_prefix('\'')?;
            let minutes = minutes.strip_suffix('\'').unwrap_or(minutes);
            digits(minutes)?
        }
    };
    if hours > i16::from(LAST_HOUR) || minutes > i16::from(LAST_MINUTE) {
        return None;
    }
    Some(Some(sign * (hours * MINUTES_PER_HOUR + minutes)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn pdf_doc_encoded() {
        let decoded = decode_text_string(b"Gr\xFC\xDFe \xA0");
        assert_eq!(decoded.text, "Grüße €");
        assert!(decoded.exact);
        // 0x7F is undefined in Table D.2.
        let decoded = decode_text_string(b"a\x7F");
        assert_eq!(decoded.text, "a\u{FFFD}");
        assert!(!decoded.exact);
    }

    #[test]
    fn utf16_with_a_surrogate_pair() {
        let decoded = decode_text_string(&[0xFE, 0xFF, 0x00, 0x41, 0xD8, 0x3D, 0xDE, 0x00]);
        assert_eq!(decoded.text, "A\u{1F600}");
        assert!(decoded.exact);
        // A lone high surrogate, then an odd byte.
        let decoded = decode_text_string(&[0xFE, 0xFF, 0xD8, 0x3D, 0x00]);
        assert_eq!(decoded.text, "\u{FFFD}\u{FFFD}");
        assert!(!decoded.exact);
    }

    #[test]
    fn utf8_after_its_mark() {
        let mut bytes = UTF8_MARK.to_vec();
        bytes.extend_from_slice("Ж".as_bytes());
        assert_eq!(decode_text_string(&bytes).text, "Ж");
    }

    /// 7.9.2.2: the escape sequence names a language and is no text.
    #[test]
    fn language_escape_is_taken_out() {
        let mut bytes = UTF16_MARK.to_vec();
        for unit in "\u{1B}jaJP\u{1B}X\u{1B}en\u{1B}Y".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        let decoded = decode_text_string(&bytes);
        assert_eq!(decoded.text, "XY");
        assert_eq!(decoded.language.as_deref(), Some("jaJP"));
        // An escape that starts no sequence stays.
        let mut bytes = UTF16_MARK.to_vec();
        for unit in "a\u{1B}b".encode_utf16() {
            bytes.extend_from_slice(&unit.to_be_bytes());
        }
        assert_eq!(decode_text_string(&bytes).text, "a\u{1B}b");
    }

    #[test]
    fn encode_chooses_the_encoding() {
        assert_eq!(encode_text_string("Grüße"), b"Gr\xFC\xDFe");
        assert_eq!(encode_text_string("Ж"), [0xFE, 0xFF, 0x04, 0x16]);
        // PDFDocEncoding of thorn and y-dieresis is the byte order mark.
        assert_eq!(
            encode_text_string("\u{FE}\u{FF}"),
            [0xFE, 0xFF, 0x00, 0xFE, 0x00, 0xFF]
        );
        for text in ["", "plain", "Grüße", "Ж and \u{1F600}"] {
            assert_eq!(decode_text_string(&encode_text_string(text)).text, text);
        }
    }

    /// The example of 7.9.4.
    #[test]
    fn date_of_the_example() {
        let date = Date::parse("D:199812231952-08'00").unwrap();
        assert_eq!(
            date,
            Date {
                year: 1998,
                month: 12,
                day: 23,
                hour: 19,
                minute: 52,
                second: 0,
                offset_minutes: Some(-480),
            }
        );
        assert_eq!(date.format(), "D:19981223195200-08'00");
        assert_eq!(Date::parse(&date.format()), Some(date));
    }

    #[test]
    fn date_defaults_and_forms() {
        let year = Date::parse("D:2026").unwrap();
        assert_eq!((year.month, year.day, year.hour), (1, 1, 0));
        assert_eq!(year.offset_minutes, None);
        assert_eq!(Date::parse("D:20261114Z").unwrap().offset_minutes, Some(0));
        assert_eq!(
            Date::parse("D:20261114203000+05'30'")
                .unwrap()
                .offset_minutes,
            Some(330)
        );
        assert_eq!(
            Date::parse("D:2026111420+05").unwrap().offset_minutes,
            Some(300)
        );
        for bad in [
            "2026",
            "D:",
            "D:26",
            "D:20261314",
            "D:2026010125",
            "D:2026x",
            "D:2026+5",
        ] {
            assert_eq!(Date::parse(bad), None, "{bad}");
        }
    }
}
