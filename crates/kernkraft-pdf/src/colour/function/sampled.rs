// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Type 0 functions: ISO 32000-1:2008, 7.10.2, Tables 38 and 39.
//!
//! The samples stay packed as the stream gives them and are read bit by
//! bit as an evaluation needs them, so a large table costs what its
//! stream costs.

use std::sync::Arc;

use super::{clip, interpolate, Kind, Loaded, MAX_FUNCTION_BYTES, MAX_SAMPLED_INPUTS};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::{Limit, PdfError};

/// Table 39, `BitsPerSample`: the valid sample widths.
const VALID_BITS: [u32; 8] = [1, 2, 4, 8, 12, 16, 24, 32];
/// Table 39, `Order`: linear interpolation.
const LINEAR: i64 = 1;
/// Table 39, `Order`: cubic spline interpolation.
const CUBIC: i64 = 3;
/// 7.10.2: cubic interpolation needs four samples in a dimension; below
/// that `Order` 3 is ignored.
const CUBIC_MIN_SIZE: usize = 4;
const BITS_PER_BYTE: usize = 8;

#[derive(Debug, Clone)]
pub(crate) struct Sampled {
    sizes: Vec<usize>,
    /// The distance, in sample positions, between neighbours of each
    /// dimension (the first dimension varies fastest).
    strides: Vec<usize>,
    bits: usize,
    encode: Vec<[f64; 2]>,
    decode: Vec<[f64; 2]>,
    outputs: usize,
    data: Arc<[u8]>,
}

pub(super) fn read(
    doc: &Document,
    loaded: &Loaded,
    domain: &[[f64; 2]],
    range: Option<&[[f64; 2]]>,
    diagnostics: &mut Diagnostics,
) -> Result<(Kind, usize), PdfError> {
    let invalid = |key| PdfError::InvalidEntry {
        object: loaded.id,
        key,
    };
    let dictionary = &loaded.dictionary;
    let range = range.ok_or_else(|| invalid("Range"))?;
    if range.is_empty() {
        return Err(invalid("Range"));
    }
    let inputs = domain.len();
    if inputs > MAX_SAMPLED_INPUTS {
        return Err(PdfError::LimitExceeded {
            offset: 0,
            limit: Limit::SampledInputs,
        });
    }
    let sizes: Vec<usize> = doc
        .colour_numbers(loaded.id, dictionary, "Size")?
        .filter(|sizes| sizes.len() == inputs)
        .and_then(|sizes| {
            sizes
                .iter()
                .map(|size| {
                    (size.fract() == 0.0 && *size >= 1.0 && *size <= u32::MAX as f64)
                        .then_some(*size as usize)
                })
                .collect()
        })
        .ok_or_else(|| invalid("Size"))?;
    let bits = doc
        .function_integer(dictionary, b"BitsPerSample")?
        .and_then(|bits| u32::try_from(bits).ok())
        .filter(|bits| VALID_BITS.contains(bits))
        .ok_or_else(|| invalid("BitsPerSample"))? as usize;
    match doc.function_integer(dictionary, b"Order")? {
        None | Some(LINEAR) => {}
        Some(CUBIC) => {
            if sizes.iter().all(|size| *size >= CUBIC_MIN_SIZE) {
                diagnostics.push(DiagnosticKind::FunctionOrderIgnored, 0);
            }
        }
        Some(_) => return Err(invalid("Order")),
    }
    let encode = match doc.colour_pairs(loaded.id, dictionary, "Encode")? {
        Some(encode) if encode.len() == inputs => encode,
        Some(_) => return Err(invalid("Encode")),
        None => sizes.iter().map(|size| [0.0, (*size - 1) as f64]).collect(),
    };
    let outputs = range.len();
    let decode = match doc.colour_pairs(loaded.id, dictionary, "Decode")? {
        Some(decode) if decode.len() == outputs => decode,
        Some(_) => return Err(invalid("Decode")),
        None => range.to_vec(),
    };
    let mut strides = Vec::with_capacity(inputs);
    let mut count = outputs;
    for size in &sizes {
        strides.push(count / outputs);
        count = count.checked_mul(*size).ok_or(PdfError::LimitExceeded {
            offset: 0,
            limit: Limit::FunctionData,
        })?;
    }
    let table_bits = count
        .checked_mul(bits)
        .filter(|bits| bits / BITS_PER_BYTE <= MAX_FUNCTION_BYTES)
        .ok_or(PdfError::LimitExceeded {
            offset: 0,
            limit: Limit::FunctionData,
        })?;
    let needed = table_bits.div_ceil(BITS_PER_BYTE);
    let data = doc.function_data(loaded)?;
    if data.len() < needed {
        diagnostics.push(DiagnosticKind::FunctionSamplesShort, 0);
    }
    Ok((
        Kind::Sampled(Sampled {
            sizes,
            strides,
            bits,
            encode,
            decode,
            outputs,
            data: Arc::from(data),
        }),
        outputs,
    ))
}

impl Sampled {
    /// 7.10.2: the input is encoded into the table's index space and the
    /// 2^m samples around it are blended by their distance (multilinear
    /// interpolation); the blend is decoded into the output range.
    pub(super) fn eval(&self, domain: &[[f64; 2]], input: &[f64], output: &mut Vec<f64>) {
        let inputs = input.len();
        // For each input: the lower sample index and the weight of the
        // upper neighbour.
        let mut lower = [0usize; MAX_SAMPLED_INPUTS];
        let mut weight = [0.0f64; MAX_SAMPLED_INPUTS];
        for i in 0..inputs {
            let [d0, d1] = domain[i];
            let [e0, e1] = self.encode[i];
            let top = (self.sizes[i] - 1) as f64;
            let encoded = clip(interpolate(input[i], d0, d1, e0, e1), [0.0, top]);
            let floor = encoded.floor();
            lower[i] = floor as usize;
            weight[i] = encoded - floor;
        }
        let mut sums = vec![0.0f64; self.outputs];
        for corner in 0..(1usize << inputs) {
            let mut factor = 1.0;
            let mut position = 0usize;
            for i in 0..inputs {
                let upper = corner >> i & 1 == 1;
                let w = if upper { weight[i] } else { 1.0 - weight[i] };
                if w == 0.0 {
                    factor = 0.0;
                    break;
                }
                factor *= w;
                let index = (lower[i] + usize::from(upper)).min(self.sizes[i] - 1);
                position += index * self.strides[i];
            }
            if factor == 0.0 {
                continue;
            }
            for (j, sum) in sums.iter_mut().enumerate() {
                *sum += factor * self.sample(position * self.outputs + j);
            }
        }
        let maximum = ((1u64 << self.bits) - 1) as f64;
        output.extend(
            sums.iter()
                .zip(&self.decode)
                .map(|(sum, [d0, d1])| interpolate(*sum, 0.0, maximum, *d0, *d1)),
        );
    }

    /// The `index`th sample of the table: `bits` bits, high-order bit of
    /// each byte first, with no padding between samples (7.10.2). Bytes
    /// past the end of the data read as zero.
    fn sample(&self, index: usize) -> f64 {
        let mut position = index * self.bits;
        let mut remaining = self.bits;
        let mut value = 0u64;
        while remaining > 0 {
            let byte = self
                .data
                .get(position / BITS_PER_BYTE)
                .copied()
                .unwrap_or(0);
            let available = BITS_PER_BYTE - position % BITS_PER_BYTE;
            let take = available.min(remaining);
            let chunk = u64::from(byte >> (available - take)) & ((1 << take) - 1);
            value = value << take | chunk;
            position += take;
            remaining -= take;
        }
        value as f64
    }
}
