// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Type 2 functions: ISO 32000-1:2008, 7.10.3, Table 40.

use super::{number, Kind, Loaded};
use crate::doc::Document;
use crate::PdfError;

/// Table 40, `C0`: the default is `[0.0]`.
const DEFAULT_C0: f64 = 0.0;
/// Table 40, `C1`: the default is `[1.0]`.
const DEFAULT_C1: f64 = 1.0;

#[derive(Debug, Clone)]
pub(crate) struct Exponential {
    c0: Vec<f64>,
    c1: Vec<f64>,
    n: f64,
}

pub(super) fn read(
    doc: &Document,
    loaded: &Loaded,
    domain: &[[f64; 2]],
    range: Option<&[[f64; 2]]>,
) -> Result<(Kind, usize), PdfError> {
    let invalid = |key| PdfError::InvalidEntry {
        object: loaded.id,
        key,
    };
    if domain.len() != 1 {
        return Err(invalid("Domain"));
    }
    let c0 = doc
        .colour_numbers(loaded.id, &loaded.dictionary, "C0")?
        .unwrap_or_else(|| vec![DEFAULT_C0]);
    let c1 = doc
        .colour_numbers(loaded.id, &loaded.dictionary, "C1")?
        .unwrap_or_else(|| vec![DEFAULT_C1]);
    if c0.is_empty() || c0.len() != c1.len() {
        return Err(invalid("C1"));
    }
    let n = match loaded.dictionary.get(b"N") {
        Some(object) => number(doc, object)?.ok_or_else(|| invalid("N"))?,
        None => return Err(invalid("N")),
    };
    if !n.is_finite() {
        return Err(invalid("N"));
    }
    if range.is_some_and(|range| range.len() != c0.len()) {
        return Err(invalid("Range"));
    }
    let outputs = c0.len();
    Ok((Kind::Exponential(Exponential { c0, c1, n }), outputs))
}

impl Exponential {
    /// `y_j = C0_j + x^N * (C1_j - C0_j)`. A power that is not a finite
    /// number (a negative base with a fractional exponent, or zero to a
    /// negative one) makes the outputs non-finite, which
    /// [`Function::eval`](super::Function::eval) reports as undefined.
    pub(super) fn eval(&self, input: &[f64], output: &mut Vec<f64>) {
        let power = input[0].powf(self.n);
        output.extend(
            self.c0
                .iter()
                .zip(&self.c1)
                .map(|(c0, c1)| c0 + power * (c1 - c0)),
        );
    }
}
