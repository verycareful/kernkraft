// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! CCITTFaxDecode: ISO 32000-1:2008, 7.4.6 (Table 11), over ITU-T T.4
//! (Group 3) and T.6 (Group 4), which the `hayro-ccitt` crate decodes.
//!
//! The crate gives each row as the positions where the colour changes; they
//! are packed here into the rows of 7.4.6: one bit to a pixel, each row
//! padded to a whole byte, with 0 black and 1 white unless `BlackIs1`.

use hayro_ccitt::{DecodeSettings, Decoder, DecoderContext, EncodingMode};

use super::output::Output;
use super::Stop;
use crate::syntax::{Dictionary, Value};
use crate::PdfError;

const FILTER: &str = "CCITTFaxDecode";
const K_KEY: &str = "K";
const END_OF_LINE_KEY: &str = "EndOfLine";
const BYTE_ALIGN_KEY: &str = "EncodedByteAlign";
const COLUMNS_KEY: &str = "Columns";
const ROWS_KEY: &str = "Rows";
const END_OF_BLOCK_KEY: &str = "EndOfBlock";
const BLACK_IS_1_KEY: &str = "BlackIs1";
const DAMAGED_ROWS_KEY: &str = "DamagedRowsBeforeError";

/// Table 11: the default of `Columns`.
const DEFAULT_COLUMNS: u32 = 1728;
const BITS_PER_BYTE: u32 = 8;
const WHITE_ONE: u8 = 0xFF;
const WHITE_ZERO: u8 = 0x00;

/// The parameters of one CCITTFaxDecode filter (Table 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CcittParams {
    scheme: Scheme,
    end_of_line: bool,
    byte_align: bool,
    columns: u32,
    /// 0 when the height is not given.
    rows: u32,
    end_of_block: bool,
    black_is_1: bool,
}

/// The encoding `K` names (Table 11).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scheme {
    /// `K` < 0: pure two-dimensional encoding (T.6).
    Group4,
    /// `K` = 0: pure one-dimensional encoding (T.4, MH).
    Group3OneDimensional,
    /// `K` > 0: mixed one- and two-dimensional encoding (T.4, MR).
    Group3TwoDimensional,
}

impl CcittParams {
    /// The pixels in a row.
    pub fn columns(&self) -> u32 {
        self.columns
    }

    /// The rows the stream says it holds, 0 when it does not say.
    pub fn rows(&self) -> u32 {
        self.rows
    }

    /// The bytes one decoded row takes.
    pub fn row_bytes(&self) -> usize {
        self.columns.div_ceil(BITS_PER_BYTE) as usize
    }

    /// Reads `DecodeParms` of a CCITTFaxDecode filter; `None` gives
    /// Table 11's defaults.
    pub(super) fn read(parms: Option<&Dictionary>) -> Result<Self, PdfError> {
        let mut params = Self {
            scheme: Scheme::Group3OneDimensional,
            end_of_line: false,
            byte_align: false,
            columns: DEFAULT_COLUMNS,
            rows: 0,
            end_of_block: true,
            black_is_1: false,
        };
        let Some(parms) = parms else {
            return Ok(params);
        };
        if let Some(k) = integer(parms, K_KEY)? {
            params.scheme = match k {
                k if k < 0 => Scheme::Group4,
                0 => Scheme::Group3OneDimensional,
                _ => Scheme::Group3TwoDimensional,
            };
        }
        params.end_of_line = boolean(parms, END_OF_LINE_KEY)?.unwrap_or(params.end_of_line);
        params.byte_align = boolean(parms, BYTE_ALIGN_KEY)?.unwrap_or(params.byte_align);
        params.end_of_block = boolean(parms, END_OF_BLOCK_KEY)?.unwrap_or(params.end_of_block);
        params.black_is_1 = boolean(parms, BLACK_IS_1_KEY)?.unwrap_or(params.black_is_1);
        if let Some(columns) = integer(parms, COLUMNS_KEY)? {
            params.columns = u32::try_from(columns)
                .ok()
                .filter(|columns| *columns > 0)
                .ok_or(PdfError::FilterParameters {
                    filter: FILTER,
                    key: COLUMNS_KEY,
                })?;
        }
        if let Some(rows) = integer(parms, ROWS_KEY)? {
            params.rows = u32::try_from(rows).map_err(|_| PdfError::FilterParameters {
                filter: FILTER,
                key: ROWS_KEY,
            })?;
        }
        // Read for its type; a damaged row is always decoded as far as the
        // data allows (the filter makes no resynchronisation of its own).
        integer(parms, DAMAGED_ROWS_KEY)?;
        Ok(params)
    }

    fn settings(&self) -> DecodeSettings {
        DecodeSettings {
            columns: self.columns,
            rows: self.rows,
            end_of_block: self.end_of_block,
            end_of_line: self.end_of_line,
            rows_are_byte_aligned: self.byte_align,
            encoding: match self.scheme {
                Scheme::Group4 => EncodingMode::Group4,
                Scheme::Group3OneDimensional => EncodingMode::Group3_1D,
                Scheme::Group3TwoDimensional => EncodingMode::Group3_2D,
            },
            strict: false,
        }
    }
}

fn integer(parms: &Dictionary, key: &'static str) -> Result<Option<i64>, PdfError> {
    super::predictor::integer(parms, key, FILTER)
}

fn boolean(parms: &Dictionary, key: &'static str) -> Result<Option<bool>, PdfError> {
    match parms.get(key.as_bytes()).map(|object| object.value()) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Boolean(value)) => Ok(Some(*value)),
        Some(_) => Err(PdfError::FilterParameters {
            filter: FILTER,
            key,
        }),
    }
}

/// Packs the rows the decoder finds into `out`.
struct Rows<'o> {
    out: &'o mut Output,
    columns: u32,
    /// The bit a black pixel is.
    black_bit: bool,
    row: Vec<u8>,
    white: u8,
    /// The first error `out` gave, after which rows are dropped.
    error: Option<PdfError>,
}

impl Decoder for Rows<'_> {
    fn push_row(&mut self, changes: &[u32]) {
        if self.error.is_some() {
            return;
        }
        self.row.fill(self.white);
        for run in changes.chunks(2) {
            let start = run[0].min(self.columns);
            let end = run
                .get(1)
                .copied()
                .unwrap_or(self.columns)
                .min(self.columns);
            set_bits(&mut self.row, start, end, self.black_bit);
        }
        if let Err(error) = self.out.extend(&self.row) {
            self.error = Some(error);
        }
    }
}

/// Sets the bits `start..end` of `row` (most significant bit first) to
/// `bit`.
fn set_bits(row: &mut [u8], start: u32, end: u32, bit: bool) {
    for pixel in start..end {
        let (byte, shift) = (
            (pixel / BITS_PER_BYTE) as usize,
            BITS_PER_BYTE - 1 - pixel % BITS_PER_BYTE,
        );
        if bit {
            row[byte] |= 1 << shift;
        } else {
            row[byte] &= !(1 << shift);
        }
    }
}

pub(super) fn decode(
    input: &[u8],
    params: &CcittParams,
    out: &mut Output,
) -> Result<Stop, PdfError> {
    let mut context = DecoderContext::new(params.settings());
    let mut rows = Rows {
        out,
        columns: params.columns,
        black_bit: params.black_is_1,
        row: vec![0; params.row_bytes()],
        white: if params.black_is_1 {
            WHITE_ZERO
        } else {
            WHITE_ONE
        },
        error: None,
    };
    let result = hayro_ccitt::decode(input, &mut rows, &mut context);
    if let Some(error) = rows.error {
        return Err(error);
    }
    Ok(match result {
        Ok(_) => Stop::EndOfData,
        Err(_) => Stop::Damaged,
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn set_bits_marks_a_run_across_bytes() {
        let mut row = [0xFF; 3];
        set_bits(&mut row, 6, 18, false);
        assert_eq!(row, [0b1111_1100, 0x00, 0b0011_1111]);
        set_bits(&mut row, 7, 9, true);
        assert_eq!(row, [0b1111_1101, 0b1000_0000, 0b0011_1111]);
    }

    #[test]
    fn defaults_are_those_of_table_11() {
        let params = CcittParams::read(None).unwrap();
        assert_eq!(params.columns(), 1728);
        assert_eq!(params.rows(), 0);
        assert_eq!(params.row_bytes(), 216);
        assert_eq!(params.scheme, Scheme::Group3OneDimensional);
        assert!(params.end_of_block && !params.black_is_1 && !params.byte_align);
    }
}
