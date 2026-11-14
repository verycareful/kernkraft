// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! SASLprep, the stringprep profile of RFC 4013 (sections 2.1 to 2.5)
//! over the tables of RFC 3454, which revisions 5 and 6 of the standard
//! security handler apply to a password (ISO 32000-2:2020, 7.6.4.3.3).
//!
//! The profile names Unicode 3.2 (RFC 4013, section 7). Normalization
//! form KC comes from `unicode-normalization`, which follows the current
//! version of Unicode; for every code point Unicode 3.2 assigned it gives
//! the 3.2 result but for the five that Unicode 4.0 corrected (Corrigendum
//! #4), which are mapped to their 3.2 decompositions first. Code points
//! that Unicode 3.2 leaves unassigned (Table A.1) are refused, as a stored
//! string refuses them (RFC 3454, section 7), so that a version of Unicode
//! later than 3.2 never decides the bytes of a password.

use unicode_normalization::UnicodeNormalization;

use super::saslprep_tables::{
    ASCII_CONTROL, CHANGE_DISPLAY_OR_DEPRECATED, INAPPROPRIATE_FOR_CANONICAL,
    INAPPROPRIATE_FOR_PLAIN_TEXT, L_CAT, MAPPED_TO_NOTHING, NON_ASCII_CONTROL, NON_ASCII_SPACE,
    NON_CHARACTER, PRIVATE_USE, RAND_AL_CAT, SURROGATE, TAGGING, UNASSIGNED,
};

/// The tables of prohibited output (RFC 4013, 2.3).
const PROHIBITED: [&[(u32, u32)]; 10] = [
    NON_ASCII_SPACE,
    ASCII_CONTROL,
    NON_ASCII_CONTROL,
    PRIVATE_USE,
    NON_CHARACTER,
    SURROGATE,
    INAPPROPRIATE_FOR_PLAIN_TEXT,
    INAPPROPRIATE_FOR_CANONICAL,
    CHANGE_DISPLAY_OR_DEPRECATED,
    TAGGING,
];

/// The decompositions Unicode 3.2 gave the code points Unicode 4.0
/// corrected (Unicode Corrigendum #4).
const UNICODE_3_2_DECOMPOSITIONS: [(char, char); 5] = [
    ('\u{2F868}', '\u{2136A}'),
    ('\u{2F874}', '\u{5F33}'),
    ('\u{2F91F}', '\u{43AB}'),
    ('\u{2F95F}', '\u{7AAE}'),
    ('\u{2F9BF}', '\u{4D57}'),
];

fn in_table(table: &[(u32, u32)], character: char) -> bool {
    let code = u32::from(character);
    table
        .binary_search_by(|&(first, last)| {
            if last < code {
                std::cmp::Ordering::Less
            } else if first > code {
                std::cmp::Ordering::Greater
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .is_ok()
}

/// `text` after SASLprep, or `None` when the profile refuses it.
pub(crate) fn saslprep(text: &str) -> Option<String> {
    if text
        .chars()
        .any(|character| in_table(UNASSIGNED, character))
    {
        return None;
    }
    // 2.1: non-ASCII spaces become a space; B.1 maps to nothing. U+200B
    // is in both tables and the profile does not say which comes first:
    // it becomes a space, as pdf.js maps it.
    let mapped = text.chars().filter_map(|character| {
        if in_table(NON_ASCII_SPACE, character) {
            Some(' ')
        } else if in_table(MAPPED_TO_NOTHING, character) {
            None
        } else {
            Some(unicode_3_2(character))
        }
    });
    // 2.2: normalization form KC.
    let normalized: String = mapped.nfkc().collect();
    // 2.3: prohibited output.
    if normalized
        .chars()
        .any(|character| PROHIBITED.iter().any(|table| in_table(table, character)))
    {
        return None;
    }
    // 2.4: RFC 3454, section 6.
    bidi_allowed(&normalized).then_some(normalized)
}

fn unicode_3_2(character: char) -> char {
    UNICODE_3_2_DECOMPOSITIONS
        .iter()
        .find_map(|&(from, to)| (from == character).then_some(to))
        .unwrap_or(character)
}

/// RFC 3454, section 6: a string holding a character of Table D.1 holds
/// none of Table D.2, and starts and ends with one of Table D.1. The
/// characters of section 5.8 (Table C.8) are refused with the rest of
/// the prohibited output.
fn bidi_allowed(text: &str) -> bool {
    if !text
        .chars()
        .any(|character| in_table(RAND_AL_CAT, character))
    {
        return true;
    }
    let first_and_last = text
        .chars()
        .next()
        .zip(text.chars().next_back())
        .is_some_and(|(first, last)| in_table(RAND_AL_CAT, first) && in_table(RAND_AL_CAT, last));
    first_and_last && !text.chars().any(|character| in_table(L_CAT, character))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// RFC 4013, section 3: the seven examples.
    #[test]
    fn examples_of_rfc_4013() {
        assert_eq!(saslprep("I\u{AD}X").as_deref(), Some("IX"));
        assert_eq!(saslprep("user").as_deref(), Some("user"));
        assert_eq!(saslprep("USER").as_deref(), Some("USER"));
        assert_eq!(saslprep("\u{AA}").as_deref(), Some("a"));
        assert_eq!(saslprep("\u{2168}").as_deref(), Some("IX"));
        assert_eq!(saslprep("\u{7}"), None);
        assert_eq!(saslprep("\u{627}\u{31}"), None);
    }

    /// RFC 3454, section 6: "aleph 1 beh" is allowed where "aleph 1" is
    /// not; a right-to-left string with a left-to-right letter is not.
    #[test]
    fn bidirectional_rules() {
        assert_eq!(
            saslprep("\u{627}\u{31}\u{628}").as_deref(),
            Some("\u{627}\u{31}\u{628}")
        );
        assert_eq!(saslprep("\u{627}a\u{628}"), None);
        // Table C.8: a right-to-left mark is prohibited output.
        assert_eq!(saslprep("a\u{200F}"), None);
    }

    /// 2.1: a non-ASCII space (C.1.2) becomes a space and stays one, since
    /// only non-ASCII spaces are prohibited.
    #[test]
    fn non_ascii_space_is_mapped() {
        assert_eq!(saslprep("a\u{A0}b\u{3000}c").as_deref(), Some("a b c"));
    }

    /// 2.1: each character of Table B.1 maps to nothing, but for the one
    /// that is also a non-ASCII space (C.1.2).
    #[test]
    fn mapped_to_nothing() {
        for &(first, last) in MAPPED_TO_NOTHING {
            for code in first..=last {
                let character = char::from_u32(code).unwrap();
                let expected = if in_table(NON_ASCII_SPACE, character) {
                    "a b"
                } else {
                    "ab"
                };
                assert_eq!(
                    saslprep(&format!("a{character}b")).as_deref(),
                    Some(expected)
                );
            }
        }
    }

    /// U+200B is in Table C.1.2 and in Table B.1. RFC 4013 does not order
    /// the two mappings of 2.1; it becomes a space, as pdf.js maps it.
    #[test]
    fn zero_width_space_becomes_a_space() {
        assert_eq!(saslprep("a\u{200B}b").as_deref(), Some("a b"));
    }

    /// 2.3: one character of each prohibited table that normalization
    /// keeps as it is.
    #[test]
    fn prohibited_output() {
        // C.2.2, C.3, C.4, C.6, C.7, C.9.
        for character in [
            '\u{80}',
            '\u{E000}',
            '\u{FDD0}',
            '\u{FFF9}',
            '\u{2FF0}',
            '\u{E0001}',
        ] {
            assert_eq!(saslprep(&format!("a{character}")), None, "{character:?}");
        }
    }

    /// 2.5 with the policy of a stored string: a code point Unicode 3.2
    /// leaves unassigned is refused, whatever a later Unicode gives it.
    #[test]
    fn unassigned_is_refused() {
        // U+0221 is unassigned in Unicode 3.2 (the first entry of A.1).
        assert_eq!(saslprep("a\u{221}"), None);
    }

    /// Corrigendum #4: the five ideographs normalize as Unicode 3.2 did.
    #[test]
    fn corrected_ideographs_keep_their_unicode_3_2_form() {
        for (from, to) in UNICODE_3_2_DECOMPOSITIONS {
            assert_eq!(saslprep(&from.to_string()), Some(to.to_string()));
        }
    }

    #[test]
    fn empty_is_empty() {
        assert_eq!(saslprep("").as_deref(), Some(""));
    }
}
