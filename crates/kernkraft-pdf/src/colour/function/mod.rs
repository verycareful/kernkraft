// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Functions: ISO 32000-1:2008, 7.10. [`Document::function`] reads a
//! function dictionary or stream of type 0 (sampled, 7.10.2), 2
//! (exponential interpolation, 7.10.3), 3 (stitching, 7.10.4) or 4
//! (PostScript calculator, 7.10.5) into a [`Function`], and
//! [`Function::eval`] computes it.
//!
//! Every input is clipped to the function's `Domain` and, where the
//! function has a `Range`, every output to it (Table 38). A function
//! whose evaluation fails (a type 4 program that underflows its stack, a
//! result that is not a finite number) is [`PdfError::Function`]; nothing
//! is guessed in its place.

mod exponential;
mod postscript;
mod sampled;
mod stitching;

use std::sync::Arc;

use crate::diagnostic::Diagnostics;
use crate::doc::Document;
use crate::syntax::limits::MAX_FUNCTION_DEPTH;
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

pub use postscript::PostScript;

/// The most inputs a sampled function may have. One evaluation reads the
/// 2^m samples at the corners of the cell the input falls in, so the cost
/// doubles with each input; ISO 32000 sets no bound, and a table with
/// more inputs than this would also be past [`MAX_FUNCTION_BYTES`] at the
/// smallest legal size of two samples per input.
pub const MAX_SAMPLED_INPUTS: usize = 16;

/// The bytes of a sample table or of calculator code: 64 MiB. A table of
/// 8-bit samples for a four-input, four-output function at 33 samples
/// per input is 4.7 MB; this leaves room for much finer tables and bounds
/// what a stream can make the reader decode.
pub const MAX_FUNCTION_BYTES: usize = 64 << 20;

/// Why a function could not be evaluated (7.10.5.2 lists the errors of a
/// type 4 function).
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum FunctionFault {
    #[error("the function has {expected} inputs and was given {found}")]
    InputCount { expected: usize, found: usize },
    #[error("the program overflows the operand stack")]
    StackOverflow,
    #[error("the program takes more operands than the stack holds")]
    StackUnderflow,
    #[error("an operator was applied to an operand of the wrong type")]
    Type,
    #[error("an operand is outside the range its operator accepts")]
    Range,
    #[error("the result is undefined")]
    Undefined,
    #[error("the program left {found} values and the function has {expected} outputs")]
    OutputCount { expected: usize, found: usize },
}

/// A function of 7.10, with its `Domain` and `Range`.
#[derive(Debug, Clone)]
pub struct Function {
    pub(crate) domain: Vec<[f64; 2]>,
    pub(crate) range: Option<Vec<[f64; 2]>>,
    pub(crate) outputs: usize,
    pub(crate) kind: Kind,
}

#[derive(Debug, Clone)]
pub(crate) enum Kind {
    Sampled(sampled::Sampled),
    Exponential(exponential::Exponential),
    Stitching(stitching::Stitching),
    PostScript(Arc<PostScript>),
}

impl Function {
    /// How many input values the function takes.
    pub fn inputs(&self) -> usize {
        self.domain.len()
    }

    /// How many values the function returns.
    pub fn outputs(&self) -> usize {
        self.outputs
    }

    /// The `Domain`, one `[min, max]` pair per input.
    pub fn domain(&self) -> &[[f64; 2]] {
        &self.domain
    }

    /// The `Range`, one pair per output, when the function has one.
    pub fn range(&self) -> Option<&[[f64; 2]]> {
        self.range.as_deref()
    }

    /// The values of the function at `input`.
    pub fn call(&self, input: &[f64]) -> Result<Vec<f64>, PdfError> {
        let mut output = Vec::with_capacity(self.outputs);
        self.eval(input, &mut output)?;
        Ok(output)
    }

    /// Clears `output` and fills it with the values of the function at
    /// `input`, which is clipped to the domain first.
    pub fn eval(&self, input: &[f64], output: &mut Vec<f64>) -> Result<(), PdfError> {
        output.clear();
        if input.len() != self.domain.len() {
            return Err(fault(FunctionFault::InputCount {
                expected: self.domain.len(),
                found: input.len(),
            }));
        }
        let clipped: Vec<f64> = input
            .iter()
            .zip(&self.domain)
            .map(|(x, d)| clip(finite(*x), *d))
            .collect();
        match &self.kind {
            Kind::Sampled(function) => function.eval(&self.domain, &clipped, output),
            Kind::Exponential(function) => function.eval(&clipped, output),
            Kind::Stitching(function) => function.eval(&self.domain, &clipped, output)?,
            Kind::PostScript(function) => function.eval(&clipped, self.outputs, output)?,
        }
        if let Some(range) = &self.range {
            for (value, pair) in output.iter_mut().zip(range) {
                *value = clip(*value, *pair);
            }
        }
        if output.iter().any(|value| !value.is_finite()) {
            return Err(fault(FunctionFault::Undefined));
        }
        Ok(())
    }
}

pub(crate) fn fault(fault: FunctionFault) -> PdfError {
    PdfError::Function(fault)
}

/// 7.10.2: `min(max(x, lo), hi)`. A reversed pair clips to its larger end.
pub(crate) fn clip(x: f64, pair: [f64; 2]) -> f64 {
    x.max(pair[0]).min(pair[1])
}

/// The number `object` is or refers to, an integer or a real.
pub(crate) fn number(doc: &Document, object: &Object) -> Result<Option<f64>, PdfError> {
    Ok(match doc.resolve(object)?.value() {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    })
}

/// A NaN input is taken as 0, so that the clip of the domain, which
/// would pass a NaN through, always gives a number.
fn finite(x: f64) -> f64 {
    if x.is_nan() {
        0.0
    } else {
        x
    }
}

/// 7.10.2: `Interpolate(x, xmin, xmax, ymin, ymax)`. A zero-width input
/// interval gives `ymin`.
pub(crate) fn interpolate(x: f64, xmin: f64, xmax: f64, ymin: f64, ymax: f64) -> f64 {
    let width = xmax - xmin;
    if width == 0.0 {
        return ymin;
    }
    ymin + (x - xmin) * (ymax - ymin) / width
}

/// A function object read far enough to be dispatched on.
pub(crate) struct Loaded {
    pub(crate) id: Option<ObjectId>,
    pub(crate) dictionary: Dictionary,
    /// The object number of the stream, when the function is one.
    pub(crate) stream: Option<ObjectId>,
}

impl Document {
    /// Reads the function that `object` is or refers to (7.10).
    pub fn function(&self, object: &Object) -> Result<Function, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let result = self.read_function(object, 0, &mut diagnostics);
        self.record_diagnostics(diagnostics);
        result
    }

    pub(crate) fn read_function(
        &self,
        object: &Object,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Function, PdfError> {
        if depth >= MAX_FUNCTION_DEPTH {
            return Err(PdfError::LimitExceeded {
                offset: 0,
                limit: crate::Limit::FunctionNesting,
            });
        }
        let loaded = self.load_function(object)?;
        diagnostics.set_object(loaded.id);
        let invalid = |key| PdfError::InvalidEntry {
            object: loaded.id,
            key,
        };
        let dictionary = &loaded.dictionary;
        let kind = match self.function_integer(dictionary, b"FunctionType")? {
            Some(kind) => kind,
            None => return Err(invalid("FunctionType")),
        };
        let domain = self
            .colour_pairs(loaded.id, dictionary, "Domain")?
            .filter(|domain| !domain.is_empty())
            .ok_or_else(|| invalid("Domain"))?;
        let range = self.colour_pairs(loaded.id, dictionary, "Range")?;
        let (kind, outputs) = match kind {
            0 => sampled::read(self, &loaded, &domain, range.as_deref(), diagnostics)?,
            2 => exponential::read(self, &loaded, &domain, range.as_deref())?,
            3 => stitching::read(self, &loaded, &domain, range.as_deref(), depth, diagnostics)?,
            4 => postscript::read(self, &loaded, range.as_deref())?,
            _ => return Err(invalid("FunctionType")),
        };
        Ok(Function {
            domain,
            range,
            outputs,
            kind,
        })
    }

    fn load_function(&self, object: &Object) -> Result<Loaded, PdfError> {
        if let Value::Reference(id) = object.value() {
            let loaded = self.object(*id)?.ok_or(PdfError::InvalidEntry {
                object: Some(*id),
                key: "Function",
            })?;
            let dictionary = loaded.dictionary().cloned().ok_or(PdfError::InvalidEntry {
                object: Some(*id),
                key: "Function",
            })?;
            return Ok(Loaded {
                id: Some(*id),
                dictionary,
                stream: loaded.stream().map(|_| *id),
            });
        }
        match object.value() {
            Value::Dictionary(dictionary) => Ok(Loaded {
                id: None,
                dictionary: dictionary.clone(),
                stream: None,
            }),
            _ => Err(PdfError::InvalidEntry {
                object: None,
                key: "Function",
            }),
        }
    }

    pub(crate) fn function_integer(
        &self,
        dictionary: &Dictionary,
        key: &[u8],
    ) -> Result<Option<i64>, PdfError> {
        Ok(match self.get(dictionary, key)?.value() {
            Value::Integer(integer) => Some(*integer),
            _ => None,
        })
    }

    /// The numbers of the array in entry `key`; `None` when the entry is
    /// absent. An entry that is not an array of numbers is
    /// [`PdfError::InvalidEntry`] of `object`.
    pub(crate) fn colour_numbers(
        &self,
        object: Option<ObjectId>,
        dictionary: &Dictionary,
        key: &'static str,
    ) -> Result<Option<Vec<f64>>, PdfError> {
        let invalid = PdfError::InvalidEntry { object, key };
        let resolved = self.get(dictionary, key.as_bytes())?;
        let items = match resolved.value() {
            Value::Null => return Ok(None),
            Value::Array(items) => items,
            _ => return Err(invalid),
        };
        let mut numbers = Vec::with_capacity(items.len());
        for item in items {
            match number(self, item)? {
                Some(number) => numbers.push(number),
                None => return Err(invalid),
            }
        }
        Ok(Some(numbers))
    }

    /// The array in entry `key` as `[min, max]` pairs. An odd length is
    /// [`PdfError::InvalidEntry`].
    pub(crate) fn colour_pairs(
        &self,
        object: Option<ObjectId>,
        dictionary: &Dictionary,
        key: &'static str,
    ) -> Result<Option<Vec<[f64; 2]>>, PdfError> {
        let Some(numbers) = self.colour_numbers(object, dictionary, key)? else {
            return Ok(None);
        };
        if numbers.len() % 2 != 0 {
            return Err(PdfError::InvalidEntry { object, key });
        }
        Ok(Some(numbers.as_chunks::<2>().0.to_vec()))
    }

    /// The decoded data of the stream function `loaded`.
    pub(crate) fn function_data(&self, loaded: &Loaded) -> Result<Vec<u8>, PdfError> {
        let id = loaded.stream.ok_or(PdfError::InvalidEntry {
            object: loaded.id,
            key: "Function",
        })?;
        let object = self.required(id)?;
        let stream = object.stream().ok_or(PdfError::WrongKind { object: id })?;
        match self.decode_stream(id, stream, MAX_FUNCTION_BYTES) {
            Ok(decoded) => Ok(decoded.data.into_owned()),
            Err(PdfError::DecodedTooLarge { .. }) => Err(PdfError::LimitExceeded {
                offset: 0,
                limit: crate::Limit::FunctionData,
            }),
            Err(error) => Err(error),
        }
    }
}
