// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The predictor functions of LZWDecode and FlateDecode: ISO 32000-1:2008,
//! 7.4.4.3 (Table 8, the parameters) and 7.4.4.4 (Tables 9 and 10).
//!
//! The TIFF function is Predictor 2 of TIFF 6.0, section 14 (horizontal
//! differencing). The PNG functions are the filter types of RFC 2083,
//! section 6: None, Sub, Up, Average and Paeth.

use crate::syntax::{Dictionary, Value};
use crate::PdfError;

/// Table 10: the values of `Predictor`.
const PREDICTOR_NONE: i64 = 1;
const PREDICTOR_TIFF: i64 = 2;
const PREDICTOR_PNG_FIRST: i64 = 10;
const PREDICTOR_PNG_LAST: i64 = 15;

/// Table 8: the values `BitsPerComponent` may take.
const BITS_PER_COMPONENT: [u8; 5] = [1, 2, 4, 8, 16];
/// Table 8: the default of `BitsPerComponent`.
const DEFAULT_BITS_PER_COMPONENT: u8 = 8;
const BITS_PER_BYTE: u128 = 8;

/// RFC 2083, section 6: the tag that starts each row.
const PNG_NONE: u8 = 0;
const PNG_SUB: u8 = 1;
const PNG_UP: u8 = 2;
const PNG_AVERAGE: u8 = 3;
const PNG_PAETH: u8 = 4;
/// The bytes of a row's tag.
const PNG_TAG_BYTES: usize = 1;

/// Which group of predictor functions a filter's data was passed through
/// before it was encoded (7.4.4.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PredictorFunction {
    /// No prediction.
    None,
    /// TIFF Predictor 2: each component is the difference from the same
    /// component of the sample to its left.
    Tiff,
    /// A PNG function, named row by row in the data.
    Png,
}

/// The predictor parameters of one LZWDecode or FlateDecode filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Predictor {
    function: PredictorFunction,
    colors: u32,
    bits_per_component: u8,
    columns: u32,
    /// The bytes of one row of samples: `colors * bits_per_component *
    /// columns` bits, rounded up to whole bytes.
    row_bytes: usize,
}

impl Default for Predictor {
    /// Table 8's defaults: no prediction, one 8-bit component, one column.
    fn default() -> Self {
        Self {
            function: PredictorFunction::None,
            colors: 1,
            bits_per_component: DEFAULT_BITS_PER_COMPONENT,
            columns: 1,
            row_bytes: 1,
        }
    }
}

impl Predictor {
    pub fn function(&self) -> PredictorFunction {
        self.function
    }

    /// The interleaved colour components of one sample.
    pub fn colors(&self) -> u32 {
        self.colors
    }

    pub fn bits_per_component(&self) -> u8 {
        self.bits_per_component
    }

    /// The samples of one row.
    pub fn columns(&self) -> u32 {
        self.columns
    }

    /// Reads the predictor entries of a filter's parameter dictionary.
    /// `filter` is the filter's name, for the error.
    pub(super) fn read(parms: &Dictionary, filter: &'static str) -> Result<Self, PdfError> {
        let defaults = Self::default();
        let invalid = |key| PdfError::FilterParameters { filter, key };

        let function = match integer(parms, "Predictor", filter)? {
            None | Some(PREDICTOR_NONE) => PredictorFunction::None,
            Some(PREDICTOR_TIFF) => PredictorFunction::Tiff,
            Some(PREDICTOR_PNG_FIRST..=PREDICTOR_PNG_LAST) => PredictorFunction::Png,
            Some(_) => return Err(invalid("Predictor")),
        };
        let colors = match integer(parms, "Colors", filter)? {
            None => defaults.colors,
            Some(value) => positive(value).ok_or_else(|| invalid("Colors"))?,
        };
        let bits_per_component = match integer(parms, "BitsPerComponent", filter)? {
            None => defaults.bits_per_component,
            Some(value) => u8::try_from(value)
                .ok()
                .filter(|bits| BITS_PER_COMPONENT.contains(bits))
                .ok_or_else(|| invalid("BitsPerComponent"))?,
        };
        let columns = match integer(parms, "Columns", filter)? {
            None => defaults.columns,
            Some(value) => positive(value).ok_or_else(|| invalid("Columns"))?,
        };
        // Three factors below 2^32, 2^5 and 2^32: the product fits a u128.
        let row_bits = u128::from(colors) * u128::from(bits_per_component) * u128::from(columns);
        let row_bytes =
            usize::try_from(row_bits.div_ceil(BITS_PER_BYTE)).map_err(|_| invalid("Columns"))?;
        Ok(Self {
            function,
            colors,
            bits_per_component,
            columns,
            row_bytes,
        })
    }

    /// How many bytes the data carries, for a result of at most `cap`
    /// bytes, that undoing the prediction removes: one tag for each PNG row.
    pub(super) fn removed_bytes(&self, cap: usize) -> usize {
        match self.function {
            PredictorFunction::Png => cap.div_ceil(self.row_bytes),
            PredictorFunction::None | PredictorFunction::Tiff => 0,
        }
    }

    /// Turns predicted data back into samples, in place. Returns false when
    /// the data is not whole rows of this predictor; `data` then holds the
    /// rows before the fault and, of a row cut short, the bytes present.
    pub(super) fn undo(&self, data: &mut Vec<u8>) -> bool {
        match self.function {
            PredictorFunction::None => true,
            PredictorFunction::Tiff => self.undo_tiff(data),
            PredictorFunction::Png => self.undo_png(data),
        }
    }

    fn undo_tiff(&self, data: &mut [u8]) -> bool {
        // A colour count that does not fit is more components than any row
        // of this data holds, and then nothing has a component to its left.
        let colors = usize::try_from(self.colors).unwrap_or(usize::MAX);
        let columns = usize::try_from(self.columns).unwrap_or(usize::MAX);
        for row in data.chunks_mut(self.row_bytes) {
            match self.bits_per_component {
                8 => {
                    for i in colors..row.len() {
                        row[i] = row[i].wrapping_add(row[i - colors]);
                    }
                }
                16 => {
                    let step = colors.saturating_mul(2);
                    let mut i = step;
                    while i + 1 < row.len() {
                        let left = u16::from_be_bytes([row[i - step], row[i - step + 1]]);
                        let sum = u16::from_be_bytes([row[i], row[i + 1]]).wrapping_add(left);
                        row[i..i + 2].copy_from_slice(&sum.to_be_bytes());
                        i += 2;
                    }
                }
                bits => {
                    let in_row = colors.saturating_mul(columns);
                    undo_tiff_packed(row, colors, in_row, usize::from(bits));
                }
            }
        }
        data.len().is_multiple_of(self.row_bytes)
    }

    fn undo_png(&self, data: &mut Vec<u8>) -> bool {
        let row_bytes = self.row_bytes;
        // RFC 2083, 6.1: the bytes of a complete pixel, rounded up to one.
        // It is at most a row, so it fits.
        let pixel_bits = u128::from(self.colors) * u128::from(self.bits_per_component);
        let pixel_bytes = usize::try_from(pixel_bits / BITS_PER_BYTE)
            .unwrap_or(row_bytes)
            .max(1);

        // Rows are read at `read` and written, without their tags, at
        // `write`. The write position never passes the read position, and
        // the row above the one being written ends at `write`.
        let mut read = 0;
        let mut write = 0;
        let mut complete = true;
        while read < data.len() {
            let tag = data[read];
            if tag > PNG_PAETH {
                complete = false;
                break;
            }
            read += PNG_TAG_BYTES;
            let present = row_bytes.min(data.len() - read);
            let has_row_above = write >= row_bytes;
            for i in 0..present {
                let left = if i >= pixel_bytes {
                    data[write + i - pixel_bytes]
                } else {
                    0
                };
                let above = if has_row_above {
                    data[write + i - row_bytes]
                } else {
                    0
                };
                let above_left = if has_row_above && i >= pixel_bytes {
                    data[write + i - row_bytes - pixel_bytes]
                } else {
                    0
                };
                let predicted = match tag {
                    PNG_NONE => 0,
                    PNG_SUB => left,
                    PNG_UP => above,
                    PNG_AVERAGE => average(left, above),
                    _ => paeth(left, above, above_left),
                };
                data[write + i] = data[read + i].wrapping_add(predicted);
            }
            read += present;
            write += present;
            if present < row_bytes {
                complete = false;
                break;
            }
        }
        data.truncate(write);
        complete
    }
}

/// TIFF Predictor 2 for components of 1, 2 or 4 bits, packed into bytes
/// from the high-order bit down (7.4.4.4). A whole row holds `in_row`
/// components; the bits after them pad the row to a byte and are left as
/// they are.
fn undo_tiff_packed(row: &mut [u8], colors: usize, in_row: usize, bits: usize) {
    let mask = u8::MAX >> (u8::BITS as usize - bits);
    // 8, 4 or 2 components to a byte.
    let per_byte = u8::BITS as usize / bits;
    // The byte that holds the component at `index`, and the shift that
    // brings the component to the byte's low bits.
    let place = |index: usize| (index / per_byte, (per_byte - 1 - index % per_byte) * bits);
    // A row cut short holds fewer.
    let components = in_row.min(row.len().saturating_mul(per_byte));
    for index in colors..components {
        let (left_byte, left_shift) = place(index - colors);
        let left = row[left_byte] >> left_shift & mask;
        let (byte, shift) = place(index);
        let sum = (row[byte] >> shift & mask).wrapping_add(left) & mask;
        row[byte] = row[byte] & !(mask << shift) | sum << shift;
    }
}

/// RFC 2083, 6.5: the sum of the two neighbours halved, rounded down,
/// without overflow.
fn average(left: u8, above: u8) -> u8 {
    // The sum of two bytes halved fits a byte.
    ((u16::from(left) + u16::from(above)) / 2) as u8
}

/// RFC 2083, 6.6: whichever of left, above and upper-left is nearest to
/// `left + above - upper_left`, the earlier one on a tie.
fn paeth(left: u8, above: u8, above_left: u8) -> u8 {
    let estimate = i16::from(left) + i16::from(above) - i16::from(above_left);
    let distance = |neighbour: u8| (estimate - i16::from(neighbour)).abs();
    let (to_left, to_above, to_above_left) =
        (distance(left), distance(above), distance(above_left));
    if to_left <= to_above && to_left <= to_above_left {
        left
    } else if to_above <= to_above_left {
        above
    } else {
        above_left
    }
}

/// A parameter that counts something: at least 1.
fn positive(value: i64) -> Option<u32> {
    u32::try_from(value).ok().filter(|&count| count > 0)
}

/// The integer under `key`, or `None` when the entry is absent or null.
pub(super) fn integer(
    parms: &Dictionary,
    key: &'static str,
    filter: &'static str,
) -> Result<Option<i64>, PdfError> {
    match parms.get(key.as_bytes()).map(|object| object.value()) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Integer(value)) => Ok(Some(*value)),
        Some(_) => Err(PdfError::FilterParameters { filter, key }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each case states the estimate `left + above - upper_left` and the
    /// three distances to it, from which RFC 2083, 6.6 gives the choice.
    #[test]
    fn paeth_picks_the_nearest_and_breaks_ties_in_order() {
        // Estimate 10: distances 0, 0, 0. A three-way tie goes to left.
        assert_eq!(paeth(10, 10, 10), 10);
        // Estimate 2 + 6 - 4 = 4: distances 2, 2, 0. Upper-left is nearest.
        assert_eq!(paeth(2, 6, 4), 4);
        // Estimate 2 + 2 - 0 = 4: distances 2, 2, 4. Left ties with above.
        assert_eq!(paeth(2, 2, 0), 2);
        // Estimate 0 + 6 - 2 = 4: distances 4, 2, 2. Above ties with
        // upper-left.
        assert_eq!(paeth(0, 6, 2), 6);
        // Estimate 9 + 5 - 9 = 5: distances 4, 0, 4. Above is nearest.
        assert_eq!(paeth(9, 5, 9), 5);
        // Estimate 3 + 5 - 9 = -1: distances 4, 6, 10. Left is nearest, and
        // the estimate is below zero.
        assert_eq!(paeth(3, 5, 9), 3);
        // Estimate 255 + 255 - 0 = 510: distances 255, 255, 510. The
        // estimate is above a byte.
        assert_eq!(paeth(255, 255, 0), 255);
    }

    #[test]
    fn average_rounds_down_and_does_not_overflow() {
        assert_eq!(average(255, 255), 255);
        assert_eq!(average(255, 254), 254);
        assert_eq!(average(1, 0), 0);
    }
}
