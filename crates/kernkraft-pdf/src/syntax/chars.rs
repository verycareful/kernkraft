// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The three character classes of ISO 32000-1:2008, 7.2.2 (Tables 1 and 2),
//! which decide how bytes group into tokens.

/// Table 1: NUL, HT, LF, FF, CR and SP.
pub(crate) fn is_whitespace(byte: u8) -> bool {
    matches!(byte, 0x00 | 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// Table 2: `( ) < > [ ] { } / %`.
pub(crate) fn is_delimiter(byte: u8) -> bool {
    matches!(
        byte,
        b'(' | b')' | b'<' | b'>' | b'[' | b']' | b'{' | b'}' | b'/' | b'%'
    )
}

/// Every byte that is neither white space nor a delimiter, bytes outside
/// ASCII included.
pub(crate) fn is_regular(byte: u8) -> bool {
    !is_whitespace(byte) && !is_delimiter(byte)
}

/// The value of a hexadecimal digit in either case.
pub(crate) fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    /// Table 1, by its hexadecimal column.
    const WHITESPACE: [u8; 6] = [0x00, 0x09, 0x0A, 0x0C, 0x0D, 0x20];
    /// Table 2, by its glyph column.
    const DELIMITERS: [u8; 10] = *b"()<>[]{}/%";

    #[test]
    fn classes_partition_every_byte() {
        for byte in 0..=u8::MAX {
            let classes = [is_whitespace(byte), is_delimiter(byte), is_regular(byte)];
            assert_eq!(
                classes.iter().filter(|&&c| c).count(),
                1,
                "byte {byte:#04x}"
            );
            assert_eq!(
                is_whitespace(byte),
                WHITESPACE.contains(&byte),
                "byte {byte:#04x}"
            );
            assert_eq!(
                is_delimiter(byte),
                DELIMITERS.contains(&byte),
                "byte {byte:#04x}"
            );
        }
    }

    #[test]
    fn hex_value_covers_both_cases() {
        for (value, digit) in (0u8..).zip(*b"0123456789") {
            assert_eq!(hex_value(digit), Some(value));
        }
        for (value, (upper, lower)) in (10u8..).zip(b"ABCDEF".iter().zip(b"abcdef")) {
            assert_eq!(hex_value(*upper), Some(value));
            assert_eq!(hex_value(*lower), Some(value));
        }
        // The neighbours of each digit range in ASCII.
        for byte in *b"/:@G`g" {
            assert_eq!(hex_value(byte), None, "byte {byte:#04x}");
        }
    }
}
