// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! ASCII85Decode: ISO 32000-1:2008, 7.4.3.

use super::output::Output;
use super::Stop;
use crate::syntax::chars::is_whitespace;
use crate::PdfError;

/// The character of digit 0.
const FIRST_DIGIT: u8 = b'!';
/// The character of digit 84.
const LAST_DIGIT: u8 = b'u';
/// Stands for a group of five zero digits.
const ZERO_GROUP: u8 = b'z';
/// The two characters of the end-of-data marker, `~>`.
const END_OF_DATA_FIRST: u8 = b'~';
const END_OF_DATA_SECOND: u8 = b'>';
const BASE: u64 = 85;
/// The value of the character `u`, the largest digit.
const MAX_DIGIT: u64 = BASE - 1;
/// The characters of a whole group, and the bytes it stands for.
const GROUP_CHARS: usize = 5;
const GROUP_BYTES: usize = 4;

/// Four bytes for each group of five characters, white space ignored. A
/// final group of `n + 1` characters gives `n` bytes.
pub(super) fn decode(input: &[u8], out: &mut Output) -> Result<Stop, PdfError> {
    let mut value: u64 = 0;
    let mut count: usize = 0;
    let mut stop = Stop::MissingEndOfData;
    let mut bytes = input.iter().copied();
    while let Some(byte) = bytes.next() {
        if is_whitespace(byte) {
            continue;
        }
        match byte {
            FIRST_DIGIT..=LAST_DIGIT => {
                value = value * BASE + u64::from(byte - FIRST_DIGIT);
                count += 1;
                if count == GROUP_CHARS {
                    let Ok(group) = u32::try_from(value) else {
                        return Ok(Stop::Damaged);
                    };
                    out.extend(&group.to_be_bytes())?;
                    value = 0;
                    count = 0;
                }
            }
            ZERO_GROUP if count == 0 => out.repeat(0, GROUP_BYTES)?,
            END_OF_DATA_FIRST => {
                if bytes.next() != Some(END_OF_DATA_SECOND) {
                    return Ok(Stop::Damaged);
                }
                stop = Stop::EndOfData;
                break;
            }
            _ => return Ok(Stop::Damaged),
        }
    }
    match count {
        0 => Ok(stop),
        // One character cannot carry a byte.
        1 => Ok(Stop::Damaged),
        _ => {
            // The encoder filled the group with zero bytes and cut the five
            // characters to `count`. Filling the cut characters with the
            // largest digit gives a value whose leading bytes are the ones
            // encoded, whatever the cut characters were.
            for _ in count..GROUP_CHARS {
                value = value * BASE + MAX_DIGIT;
            }
            let Ok(group) = u32::try_from(value) else {
                return Ok(Stop::Damaged);
            };
            out.extend(&group.to_be_bytes()[..count - 1])?;
            Ok(stop)
        }
    }
}
