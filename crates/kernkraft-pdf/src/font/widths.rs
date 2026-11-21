// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Glyph widths: ISO 32000-1:2008, 9.2.4 (units), 9.6.2.1 (the `FirstChar`,
//! `LastChar` and `Widths` of a simple font, Table 111) and 9.7.4.3 (the
//! `DW`, `W`, `DW2` and `W2` of a CIDFont, Table 117).
//!
//! Every width is in thousandths of a text space unit, the glyph space of
//! a font whose `FontMatrix` is the default (9.2.4): a width of 500 is half
//! an em.

use super::range_map::RangeMap;
use super::util::{as_number, number};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::syntax::{Dictionary, Object};
use crate::PdfError;

/// Table 117, `DW`: the default width of a CIDFont's glyphs.
pub(crate) const DEFAULT_CID_WIDTH: f64 = 1000.0;
/// Table 117, `DW2`: the default vertical origin and displacement.
pub(crate) const DEFAULT_VERTICAL: [f64; 2] = [880.0, -1000.0];
/// 9.7.4.3: the horizontal component of a position vector, by default,
/// is half the glyph's width.
const HALF: f64 = 0.5;

/// The widths of a simple font's codes.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SimpleWidths {
    first: i64,
    widths: Vec<f64>,
    /// False when the dictionary has no `Widths` array.
    pub(crate) present: bool,
}

impl SimpleWidths {
    /// Reads `FirstChar` and `Widths` (Table 111). A `Widths` that has
    /// fewer items than `LastChar` makes room for is
    /// [`DiagnosticKind::WidthsShort`]; an item that is not a number is
    /// [`DiagnosticKind::WidthInvalid`] and has no width.
    pub(crate) fn read(
        doc: &Document,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self, PdfError> {
        let first = super::util::entry_integer(doc, dictionary, b"FirstChar")?.unwrap_or(0);
        let Some(items) = super::util::entry_array(doc, dictionary, b"Widths")? else {
            return Ok(Self {
                first,
                widths: Vec::new(),
                present: false,
            });
        };
        let mut widths = Vec::with_capacity(items.len());
        let mut invalid = false;
        for item in &items {
            match number(doc, item)? {
                Some(width) => widths.push(width),
                None => {
                    invalid = true;
                    widths.push(f64::NAN);
                }
            }
        }
        if invalid {
            diagnostics.push(DiagnosticKind::WidthInvalid, 0);
        }
        if let Some(last) = super::util::entry_integer(doc, dictionary, b"LastChar")? {
            let wanted = last.saturating_sub(first).saturating_add(1);
            if i64::try_from(widths.len()).is_ok_and(|have| have < wanted) {
                diagnostics.push(DiagnosticKind::WidthsShort, 0);
            }
        }
        Ok(Self {
            first,
            widths,
            present: true,
        })
    }

    /// The width of `code`, or `None` when `Widths` gives it none.
    pub(crate) fn get(&self, code: u8) -> Option<f64> {
        let index = i64::from(code).checked_sub(self.first)?;
        let width = *self.widths.get(usize::try_from(index).ok()?)?;
        (!width.is_nan()).then_some(width)
    }
}

/// The vertical metrics of a glyph (9.7.4.3, Figure 40): the vertical
/// component of its displacement vector w1 and the position vector v.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VerticalMetrics {
    /// The vertical displacement to the next glyph's origin; negative is
    /// downward.
    pub advance: f64,
    /// The position vector from the horizontal to the vertical origin.
    pub origin_x: f64,
    pub origin_y: f64,
}

/// The widths and vertical metrics of a CIDFont.
#[derive(Debug, Clone)]
pub(crate) struct CidWidths {
    default: f64,
    widths: RangeMap<f64>,
    default_vertical: [f64; 2],
    vertical: RangeMap<[f64; 3]>,
}

impl CidWidths {
    /// Reads `DW`, `W`, `DW2` and `W2` of the CIDFont `dictionary`.
    /// Items of `W` or `W2` that no format of the format allows are
    /// [`DiagnosticKind::WidthInvalid`] and end the array; ranges that
    /// overlap are [`DiagnosticKind::WidthsOverlap`], the first giving the
    /// width.
    pub(crate) fn read(
        doc: &Document,
        dictionary: &Dictionary,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self, PdfError> {
        let default =
            super::util::entry_number(doc, dictionary, b"DW")?.unwrap_or(DEFAULT_CID_WIDTH);
        let default_vertical = match super::util::entry_array(doc, dictionary, b"DW2")?.as_deref() {
            Some([origin, advance]) => [
                number(doc, origin)?.unwrap_or(DEFAULT_VERTICAL[0]),
                number(doc, advance)?.unwrap_or(DEFAULT_VERTICAL[1]),
            ],
            _ => DEFAULT_VERTICAL,
        };
        let mut result = Self {
            default,
            widths: RangeMap::new(),
            default_vertical,
            vertical: RangeMap::new(),
        };
        let mut overlap = false;
        if let Some(items) = super::util::entry_array(doc, dictionary, b"W")? {
            let ok = Self::groups(doc, &items, 1, |first, last, values| {
                overlap |= result.widths.insert(first, last, values[0]);
            })?;
            if !ok {
                diagnostics.push(DiagnosticKind::WidthInvalid, 0);
            }
        }
        if let Some(items) = super::util::entry_array(doc, dictionary, b"W2")? {
            let ok = Self::groups(doc, &items, 3, |first, last, values| {
                overlap |= result
                    .vertical
                    .insert(first, last, [values[0], values[1], values[2]]);
            })?;
            if !ok {
                diagnostics.push(DiagnosticKind::WidthInvalid, 0);
            }
        }
        if overlap {
            diagnostics.push(DiagnosticKind::WidthsOverlap, 0);
        }
        Ok(result)
    }

    /// The groups of a `W` (`width` 1) or `W2` (`width` 3) array: `c [v ...]`
    /// gives one value set to each of consecutive CIDs from `c`, and
    /// `first last v...` one to the whole range. Calls `put(first, last,
    /// values)` for each. False when an item is not what the format needs.
    fn groups(
        doc: &Document,
        items: &[Object],
        width: usize,
        mut put: impl FnMut(u64, u64, &[f64]),
    ) -> Result<bool, PdfError> {
        let mut index = 0;
        while index < items.len() {
            let Some(first) = number(doc, &items[index])?.and_then(cid_key) else {
                return Ok(false);
            };
            let Some(next) = items.get(index + 1) else {
                return Ok(false);
            };
            let resolved = doc.resolve(next)?;
            if let Some(values) = resolved.array() {
                let mut numbers = Vec::with_capacity(values.len());
                for value in values {
                    let Some(value) = number(doc, value)? else {
                        return Ok(false);
                    };
                    numbers.push(value);
                }
                if numbers.len() % width != 0 {
                    return Ok(false);
                }
                for (offset, set) in numbers.chunks_exact(width).enumerate() {
                    let at = first.saturating_add(offset as u64);
                    put(at, at, set);
                }
                index += 2;
            } else {
                let Some(last) = as_number(resolved.value()).and_then(cid_key) else {
                    return Ok(false);
                };
                let mut set = Vec::with_capacity(width);
                for offset in 0..width {
                    let Some(value) = items.get(index + 2 + offset) else {
                        return Ok(false);
                    };
                    let Some(value) = number(doc, value)? else {
                        return Ok(false);
                    };
                    set.push(value);
                }
                put(first, last, &set);
                index += 2 + width;
            }
        }
        Ok(true)
    }

    /// The width of CID `cid`.
    pub(crate) fn width(&self, cid: u32) -> f64 {
        self.widths
            .get(u64::from(cid))
            .map_or(self.default, |(_, width)| *width)
    }

    /// The vertical metrics of CID `cid` (the position vector's horizontal
    /// component, where `W2` gives none, is half the width).
    pub(crate) fn vertical(&self, cid: u32) -> VerticalMetrics {
        match self.vertical.get(u64::from(cid)) {
            Some((_, [advance, origin_x, origin_y])) => VerticalMetrics {
                advance: *advance,
                origin_x: *origin_x,
                origin_y: *origin_y,
            },
            None => VerticalMetrics {
                advance: self.default_vertical[1],
                origin_x: self.width(cid) * HALF,
                origin_y: self.default_vertical[0],
            },
        }
    }
}

/// A CID read from a number: a whole number from 0 up.
fn cid_key(value: f64) -> Option<u64> {
    (value >= 0.0 && value.fract() == 0.0 && value <= f64::from(u32::MAX)).then_some(value as u64)
}
