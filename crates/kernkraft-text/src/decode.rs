// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Bytes to text, with the facts a save needs to put the file back.

/// UTF-8 byte order mark: U+FEFF encoded as UTF-8 (Unicode Standard 23.8).
pub(crate) const UTF8_BOM: [u8; 3] = [0xEF, 0xBB, 0xBF];
/// UTF-16 little endian byte order mark (Unicode Standard 23.8).
const UTF16_LE_BOM: [u8; 2] = [0xFF, 0xFE];
/// UTF-16 big endian byte order mark (Unicode Standard 23.8).
const UTF16_BE_BOM: [u8; 2] = [0xFE, 0xFF];

/// The line break a file uses throughout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

/// Why a file is shown but must not be saved.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReadOnly {
    /// Not valid UTF-8 (RFC 3629); `offset` is the byte offset in the file
    /// of the first invalid sequence.
    NotUtf8 { offset: usize },
    /// Starts with a UTF-16 byte order mark.
    Utf16,
    /// More than one kind of line break, or a lone CR; counts of each.
    /// `crlf` counts CR LF pairs, `lf` counts LF not preceded by CR, `cr`
    /// counts CR not followed by LF.
    MixedLineEndings { lf: usize, crlf: usize, cr: usize },
    /// Above [`crate::EDITABLE_LIMIT`] bytes.
    TooLarge { len: u64, limit: u64 },
    /// The current user cannot write the file.
    NoWritePermission,
}

impl ReadOnly {
    /// Whether the reason is in the file's content (not UTF-8, UTF-16,
    /// mixed line endings): text shown for such a file is a lossy copy.
    /// `TooLarge` and `NoWritePermission` are not; the text is exact.
    pub fn is_content(&self) -> bool {
        matches!(
            self,
            ReadOnly::NotUtf8 { .. } | ReadOnly::Utf16 | ReadOnly::MixedLineEndings { .. }
        )
    }
}

impl std::fmt::Display for ReadOnly {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadOnly::NotUtf8 { offset } => write!(f, "not valid UTF-8 from byte {offset}"),
            ReadOnly::Utf16 => write!(f, "UTF-16 text"),
            ReadOnly::MixedLineEndings { lf, crlf, cr } => {
                write!(f, "mixed line endings ({lf} LF, {crlf} CRLF, {cr} CR)")
            }
            ReadOnly::TooLarge { len, limit } => {
                write!(f, "{len} bytes, above the {limit} byte limit for editing")
            }
            ReadOnly::NoWritePermission => write!(f, "not writable by this user"),
        }
    }
}

/// A decoded file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Decoded {
    /// The text with `\n` as the only line break and no byte order mark.
    /// When `read_only` is set it is a lossy display copy: invalid UTF-8
    /// is replaced by U+FFFD, UTF-16 is decoded with unpaired surrogates
    /// replaced, and every CR LF or lone CR is shown as `\n`.
    pub text: String,
    pub ending: LineEnding,
    /// The file began with a UTF-8 byte order mark.
    pub bom: bool,
    pub read_only: Option<ReadOnly>,
}

/// Decodes `bytes`. A file with no line break gets [`LineEnding::Lf`].
///
/// When several read-only reasons apply, the first of this order is
/// reported: UTF-16, not UTF-8, mixed line endings. (`TooLarge` and
/// `NoWritePermission` come from the file system, see [`crate::open`].)
/// NUL bytes are valid UTF-8 and stay in the text.
pub fn decode(bytes: &[u8]) -> Decoded {
    if bytes.starts_with(&UTF16_LE_BOM) || bytes.starts_with(&UTF16_BE_BOM) {
        return decode_utf16(bytes);
    }
    let (body, bom) = match bytes.strip_prefix(&UTF8_BOM) {
        Some(rest) => (rest, true),
        None => (bytes, false),
    };
    let counts = count_breaks(body);
    let ending = if counts.crlf > 0 && counts.lf == 0 && counts.cr == 0 {
        LineEnding::CrLf
    } else {
        LineEnding::Lf
    };
    let mixed = counts.cr > 0 || (counts.lf > 0 && counts.crlf > 0);

    match std::str::from_utf8(body) {
        Ok(text) if !mixed => {
            let text = match ending {
                LineEnding::Lf => text.to_owned(),
                LineEnding::CrLf => text.replace("\r\n", "\n"),
            };
            Decoded {
                text,
                ending,
                bom,
                read_only: None,
            }
        }
        Ok(text) => Decoded {
            text: normalise_breaks(text),
            ending,
            bom,
            read_only: Some(ReadOnly::MixedLineEndings {
                lf: counts.lf,
                crlf: counts.crlf,
                cr: counts.cr,
            }),
        },
        Err(e) => Decoded {
            text: normalise_breaks(&String::from_utf8_lossy(body)),
            ending,
            bom,
            read_only: Some(ReadOnly::NotUtf8 {
                offset: e.valid_up_to() + if bom { UTF8_BOM.len() } else { 0 },
            }),
        },
    }
}

fn decode_utf16(bytes: &[u8]) -> Decoded {
    let big_endian = bytes.starts_with(&UTF16_BE_BOM);
    let units = bytes[UTF16_LE_BOM.len()..]
        .chunks(2)
        .map(|pair| match *pair {
            [a, b] if big_endian => u16::from_be_bytes([a, b]),
            [a, b] => u16::from_le_bytes([a, b]),
            // A trailing odd byte is not a code unit.
            _ => 0xFFFD,
        });
    let text: String = char::decode_utf16(units)
        .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect();
    Decoded {
        text: normalise_breaks(&text),
        ending: LineEnding::Lf,
        bom: false,
        read_only: Some(ReadOnly::Utf16),
    }
}

struct BreakCounts {
    lf: usize,
    crlf: usize,
    cr: usize,
}

fn count_breaks(bytes: &[u8]) -> BreakCounts {
    let mut counts = BreakCounts {
        lf: 0,
        crlf: 0,
        cr: 0,
    };
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => {
                counts.crlf += 1;
                i += 1;
            }
            b'\r' => counts.cr += 1,
            b'\n' => counts.lf += 1,
            _ => {}
        }
        i += 1;
    }
    counts
}

/// CR LF and lone CR become LF.
fn normalise_breaks(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}
