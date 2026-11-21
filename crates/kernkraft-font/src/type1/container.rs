// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Splitting a Type 1 file into its cleartext and its `eexec` portion
//! (Adobe Type 1 Font Format, the encryption chapter: the `eexec` section
//! is binary or ASCII hexadecimal; the segment layout of a PFB file is in
//! the same book's PFB appendix).

use crate::error::FontError;

const EEXEC: &[u8] = b"eexec";
/// A PFB segment header is 0x80, a type byte and a four-byte little-endian
/// length.
const PFB_MARK: u8 = 0x80;
const PFB_ASCII: u8 = 1;
const PFB_BINARY: u8 = 2;
const PFB_EOF: u8 = 3;

/// A font program in two parts.
pub(crate) struct Parts {
    /// Everything before the `eexec` operator, as text.
    pub cleartext: Vec<u8>,
    /// The encrypted portion, as bytes (hex already decoded).
    pub encrypted: Vec<u8>,
}

pub(crate) fn split(data: &[u8]) -> Result<Parts, FontError> {
    if data.first() == Some(&PFB_MARK) {
        split_pfb(data)
    } else {
        split_flat(data)
    }
}

fn split_pfb(data: &[u8]) -> Result<Parts, FontError> {
    let mut cleartext = Vec::new();
    let mut encrypted = Vec::new();
    let mut pos = 0;
    let mut seen_binary = false;
    loop {
        let [mark, kind, ..] = data.get(pos..).unwrap_or_default() else {
            return Err(FontError::Type1("a PFB file ends without its end segment"));
        };
        if *mark != PFB_MARK {
            return Err(FontError::Type1("a PFB segment does not begin with 0x80"));
        }
        if *kind == PFB_EOF {
            break;
        }
        let len_bytes = data
            .get(pos + 2..pos + 6)
            .ok_or(FontError::Type1("a PFB segment header is cut short"))?;
        let len = u32::from_le_bytes([len_bytes[0], len_bytes[1], len_bytes[2], len_bytes[3]]);
        let len =
            usize::try_from(len).map_err(|_| FontError::Type1("a PFB segment is too long"))?;
        let body = data
            .get(pos + 6..)
            .and_then(|rest| rest.get(..len))
            .ok_or(FontError::Type1(
                "a PFB segment runs past the end of the file",
            ))?;
        match *kind {
            PFB_ASCII if !seen_binary => cleartext.extend_from_slice(body),
            // The trailer after the encrypted portion (the zeros and
            // `cleartomark`) is not read.
            PFB_ASCII => {}
            PFB_BINARY => {
                seen_binary = true;
                encrypted.extend_from_slice(body);
            }
            _ => return Err(FontError::Type1("a PFB segment of an unknown type")),
        }
        pos += 6 + len;
    }
    if !seen_binary {
        return Err(FontError::Type1("a PFB file with no encrypted portion"));
    }
    Ok(Parts {
        cleartext,
        encrypted,
    })
}

fn split_flat(data: &[u8]) -> Result<Parts, FontError> {
    let at = find(data, EEXEC).ok_or(FontError::Type1("no eexec operator"))?;
    let cleartext = data[..at].to_vec();
    let mut rest = &data[at + EEXEC.len()..];
    // The first ciphertext byte is never white space, so every leading
    // white-space byte belongs to the operator.
    while let [b' ' | b'\t' | b'\r' | b'\n', tail @ ..] = rest {
        rest = tail;
    }
    // The portion is hexadecimal unless one of its first four bytes is not
    // a hexadecimal digit.
    let is_hex = rest.len() >= 4 && rest[..4].iter().all(|b| b.is_ascii_hexdigit());
    let encrypted = if is_hex {
        decode_hex(rest)
    } else {
        rest.to_vec()
    };
    Ok(Parts {
        cleartext,
        encrypted,
    })
}

/// Decodes hexadecimal digit pairs, skipping white space, up to the first
/// byte that is neither.
fn decode_hex(text: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() / 2);
    let mut high: Option<u8> = None;
    for &b in text {
        let digit = match b {
            b'0'..=b'9' => b - b'0',
            b'a'..=b'f' => b - b'a' + 10,
            b'A'..=b'F' => b - b'A' + 10,
            b' ' | b'\t' | b'\r' | b'\n' => continue,
            _ => break,
        };
        match high.take() {
            None => high = Some(digit),
            Some(h) => out.push(h << 4 | digit),
        }
    }
    out
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn hex_skips_white_space_and_stops_at_a_non_digit() {
        assert_eq!(decode_hex(b"0a FF\n10 zz 00"), vec![0x0A, 0xFF, 0x10]);
    }

    #[test]
    fn hex_drops_a_trailing_odd_digit() {
        assert_eq!(decode_hex(b"0a1"), vec![0x0A]);
    }

    #[test]
    fn flat_file_hex_and_binary_are_told_apart() {
        let hex = split(b"%!PS\n/x 1 def\ncurrentfile eexec\n0a0b0c0d0e").unwrap();
        assert_eq!(hex.encrypted, vec![0x0A, 0x0B, 0x0C, 0x0D, 0x0E]);
        let bin = split(b"%!PS\ncurrentfile eexec\r\n\xfe\x00\x01\x02\x03").unwrap();
        assert_eq!(bin.encrypted, vec![0xFE, 0, 1, 2, 3]);
    }

    #[test]
    fn pfb_segments_are_joined_and_the_trailer_dropped() {
        let mut f = vec![0x80, 1, 3, 0, 0, 0];
        f.extend_from_slice(b"abc");
        f.extend_from_slice(&[0x80, 2, 2, 0, 0, 0, 0xAA, 0xBB]);
        f.extend_from_slice(&[0x80, 1, 2, 0, 0, 0]);
        f.extend_from_slice(b"00");
        f.extend_from_slice(&[0x80, 3]);
        let p = split(&f).unwrap();
        assert_eq!(p.cleartext, b"abc");
        assert_eq!(p.encrypted, vec![0xAA, 0xBB]);
    }

    #[test]
    fn pfb_cut_short_is_an_error() {
        assert!(split(&[0x80, 1, 9, 0, 0, 0, b'a']).is_err());
        assert!(split(&[0x80]).is_err());
        assert!(split(&[0x80, 1, 1, 0, 0, 0, b'a']).is_err());
    }
}
