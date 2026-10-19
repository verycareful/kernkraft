// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text back to bytes.

use crate::decode::{LineEnding, UTF8_BOM};

/// The bytes of `text` (whose line breaks are `\n`) with `ending` as its
/// line break and, when `bom` is set, a UTF-8 byte order mark in front.
/// The inverse of [`crate::decode`] for any file that decoded without a
/// [`crate::ReadOnly`] mark.
pub fn encode(text: &str, ending: LineEnding, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(text.len() + if bom { UTF8_BOM.len() } else { 0 });
    if bom {
        out.extend_from_slice(&UTF8_BOM);
    }
    match ending {
        LineEnding::Lf => out.extend_from_slice(text.as_bytes()),
        LineEnding::CrLf => {
            for part in text.split_inclusive('\n') {
                match part.strip_suffix('\n') {
                    Some(line) => {
                        out.extend_from_slice(line.as_bytes());
                        out.extend_from_slice(b"\r\n");
                    }
                    None => out.extend_from_slice(part.as_bytes()),
                }
            }
        }
    }
    out
}
