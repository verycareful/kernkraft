// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! ASCIIHexDecode: ISO 32000-1:2008, 7.4.2.

use super::output::Output;
use super::Stop;
use crate::syntax::chars::{hex_value, is_whitespace};
use crate::PdfError;

/// The end-of-data marker.
const END_OF_DATA: u8 = b'>';

/// One byte for each pair of hexadecimal digits, white space ignored. An odd
/// last digit is read as if a 0 followed it.
pub(super) fn decode(input: &[u8], out: &mut Output) -> Result<Stop, PdfError> {
    let mut high: Option<u8> = None;
    let mut stop = Stop::MissingEndOfData;
    for &byte in input {
        if is_whitespace(byte) {
            continue;
        }
        if byte == END_OF_DATA {
            stop = Stop::EndOfData;
            break;
        }
        let Some(digit) = hex_value(byte) else {
            return Ok(Stop::Damaged);
        };
        match high.take() {
            Some(first) => out.push(first << 4 | digit)?,
            None => high = Some(digit),
        }
    }
    if let Some(first) = high {
        out.push(first << 4)?;
    }
    Ok(stop)
}
