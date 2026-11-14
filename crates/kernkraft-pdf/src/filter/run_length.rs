// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! RunLengthDecode: ISO 32000-1:2008, 7.4.5.

use super::output::Output;
use super::Stop;
use crate::PdfError;

/// The length byte that marks the end of the data.
const END_OF_DATA: u8 = 128;
/// A length byte above the marker repeats one byte `257 - length` times.
const REPEAT_BASE: usize = 257;

/// Runs of a length byte followed by the bytes to copy, or by the one byte
/// to repeat.
pub(super) fn decode(input: &[u8], out: &mut Output) -> Result<Stop, PdfError> {
    let mut rest = input;
    while let Some((&length, after)) = rest.split_first() {
        if length == END_OF_DATA {
            return Ok(Stop::EndOfData);
        }
        if length < END_OF_DATA {
            let count = usize::from(length) + 1;
            let Some((literal, after_run)) = after.split_at_checked(count) else {
                out.extend(after)?;
                return Ok(Stop::Damaged);
            };
            out.extend(literal)?;
            rest = after_run;
        } else {
            let Some((&byte, after_run)) = after.split_first() else {
                return Ok(Stop::Damaged);
            };
            out.repeat(byte, REPEAT_BASE - usize::from(length))?;
            rest = after_run;
        }
    }
    Ok(Stop::MissingEndOfData)
}
