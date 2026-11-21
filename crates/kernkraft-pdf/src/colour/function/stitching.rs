// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Type 3 functions: ISO 32000-1:2008, 7.10.4, Table 41.

use super::{interpolate, Function, Kind, Loaded};
use crate::diagnostic::Diagnostics;
use crate::doc::Document;
use crate::syntax::Value;
use crate::PdfError;

#[derive(Debug, Clone)]
pub(crate) struct Stitching {
    functions: Vec<Function>,
    bounds: Vec<f64>,
    encode: Vec<f64>,
}

pub(super) fn read(
    doc: &Document,
    loaded: &Loaded,
    domain: &[[f64; 2]],
    range: Option<&[[f64; 2]]>,
    depth: usize,
    diagnostics: &mut Diagnostics,
) -> Result<(Kind, usize), PdfError> {
    let invalid = |key| PdfError::InvalidEntry {
        object: loaded.id,
        key,
    };
    if domain.len() != 1 {
        return Err(invalid("Domain"));
    }
    let listed = doc.get(&loaded.dictionary, b"Functions")?;
    let items = listed
        .array()
        .filter(|items| !items.is_empty())
        .ok_or_else(|| invalid("Functions"))?;
    let mut functions = Vec::with_capacity(items.len());
    for item in items {
        if matches!(item.value(), Value::Null) {
            return Err(invalid("Functions"));
        }
        functions.push(doc.read_function(item, depth + 1, diagnostics)?);
        diagnostics.set_object(loaded.id);
    }
    let count = functions.len();
    let bounds = doc
        .colour_numbers(loaded.id, &loaded.dictionary, "Bounds")?
        .filter(|bounds| bounds.len() == count - 1)
        .ok_or_else(|| invalid("Bounds"))?;
    let encode = doc
        .colour_numbers(loaded.id, &loaded.dictionary, "Encode")?
        .filter(|encode| encode.len() == 2 * count)
        .ok_or_else(|| invalid("Encode"))?;
    if functions.iter().any(|function| function.inputs() != 1) {
        return Err(invalid("Functions"));
    }
    let outputs = functions[0].outputs();
    if functions
        .iter()
        .any(|function| function.outputs() != outputs)
    {
        return Err(invalid("Functions"));
    }
    if range.is_some_and(|range| range.len() != outputs) {
        return Err(invalid("Range"));
    }
    Ok((
        Kind::Stitching(Stitching {
            functions,
            bounds,
            encode,
        }),
        outputs,
    ))
}

impl Stitching {
    pub(super) fn eval(
        &self,
        domain: &[[f64; 2]],
        input: &[f64],
        output: &mut Vec<f64>,
    ) -> Result<(), PdfError> {
        let x = input[0];
        let [low, high] = domain[0];
        // The subdomains are half-open on the right, the last one closed.
        let last = self.functions.len() - 1;
        let index = self.bounds.iter().take_while(|bound| x >= **bound).count();
        let index = index.min(last);
        let start = if index == 0 {
            low
        } else {
            self.bounds[index - 1]
        };
        let end = if index == last {
            high
        } else {
            self.bounds[index]
        };
        let (e0, e1) = (self.encode[2 * index], self.encode[2 * index + 1]);
        // 7.10.4: a subdomain of no width (the last bound at the top of the
        // domain) is encoded as `Encode_2i`.
        let encoded = interpolate(x, start, end, e0, e1);
        self.functions[index].eval(&[encoded], output)
    }
}
