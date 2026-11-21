// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! LZWDecode: ISO 32000-1:2008, 7.4.4.2, and the `EarlyChange` parameter of
//! 7.4.4.3, Table 8. The codes are decoded by the `weezl` crate.

use weezl::decode::Decoder;
use weezl::{BitOrder, LzwError, LzwStatus};

use super::output::Output;
use super::Stop;
use crate::PdfError;

/// The bits of one character of the decoded data: codes 0 to 255 stand for
/// themselves, so the first code length is one bit more (7.4.4.2).
const SYMBOL_BITS: u8 = 8;

/// The bytes decoded between two checks of the output's limit.
const BLOCK_BYTES: usize = 32 * 1024;

/// Decodes codes packed high-order bit first. With `early_change` the code
/// length grows one code sooner, as in TIFF.
pub(super) fn decode(input: &[u8], early_change: bool, out: &mut Output) -> Result<Stop, PdfError> {
    let mut decoder = if early_change {
        Decoder::with_tiff_size_switch(BitOrder::Msb, SYMBOL_BITS)
    } else {
        Decoder::new(BitOrder::Msb, SYMBOL_BITS)
    };
    let mut block = vec![0; BLOCK_BYTES];
    let mut rest = input;
    loop {
        let result = decoder.decode_bytes(rest, &mut block);
        // `get` rather than an index: the counts are the decoder's.
        out.extend(block.get(..result.consumed_out).unwrap_or(&block))?;
        rest = rest.get(result.consumed_in..).unwrap_or(&[]);
        let progressed = result.consumed_in > 0 || result.consumed_out > 0;
        match result.status {
            Ok(LzwStatus::Done) => return Ok(Stop::EndOfData),
            Err(LzwError::InvalidCode) => return Ok(Stop::Damaged),
            Ok(LzwStatus::Ok) if progressed => {}
            // Nothing read and nothing written: the codes ran out before
            // the end-of-data code.
            Ok(LzwStatus::Ok | LzwStatus::NoProgress) => {
                return Ok(if rest.is_empty() {
                    Stop::MissingEndOfData
                } else {
                    Stop::Damaged
                });
            }
        }
    }
}
