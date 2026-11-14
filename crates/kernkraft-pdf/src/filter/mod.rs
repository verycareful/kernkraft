// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Stream filters: ISO 32000-1:2008, 7.4, and the `Filter` and
//! `DecodeParms` entries of a stream dictionary (7.3.8.2, Table 5).
//!
//! A [`Pipeline`] is the filters of one stream in the order they decode.
//! [`Pipeline::decode`] runs encoded bytes through them and never produces
//! more than the size its caller allows, at any stage: a few bytes of
//! encoded data can stand for gigabytes.
//!
//! The filters decoded here are those of Table 6 that are not image codecs:
//! ASCIIHexDecode, ASCII85Decode and RunLengthDecode, decoded by this
//! module, LZWDecode, whose codes the `weezl` crate decodes, and
//! FlateDecode, whose DEFLATE blocks the `flate2` crate decodes.
//!
//! The Crypt filter (7.4.10) is read and changes no data here: it names
//! how the security handler decrypts the stream, which
//! [`crypt`](crate::crypt) does before the data reaches a pipeline.
//! [`crypt_filter_name`] reads that name.
//!
//! Data that ends without its end-of-data marker, or that cannot be decoded
//! past some point, is not refused: what was decoded is returned, the fault
//! is reported through [`Diagnostics`], and [`Decoded::complete`] says
//! whether anything is missing.

mod ascii85;
mod ascii_hex;
mod flate;
mod lzw;
mod output;
mod predictor;
mod run_length;

use std::borrow::Cow;
use std::fmt;

pub use predictor::{Predictor, PredictorFunction};

use crate::syntax::lexer::shown;
use crate::syntax::limits::MAX_FILTERS;
use crate::syntax::{Dictionary, Name, Object, Value};
use crate::{DiagnosticKind, Diagnostics, Limit, PdfError};
use output::Output;

/// What an error names when the `Filter` or `DecodeParms` entry itself is
/// at fault, and no one filter.
const THE_STREAM: &str = "the stream";
const FILTER_KEY: &str = "Filter";
const PARMS_KEY: &str = "DecodeParms";
const EARLY_CHANGE_KEY: &str = "EarlyChange";
const NAME_KEY: &str = "Name";
/// The crypt filter of a Crypt filter that names none (Table 14), which
/// passes data through unchanged (Table 26).
const IDENTITY: &[u8] = b"Identity";

/// A filter this module decodes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterKind {
    AsciiHex,
    Ascii85,
    Lzw,
    Flate,
    RunLength,
    /// Decrypted by the security handler, not decoded (7.4.10).
    Crypt,
}

impl FilterKind {
    const ALL: [FilterKind; 6] = [
        FilterKind::AsciiHex,
        FilterKind::Ascii85,
        FilterKind::Lzw,
        FilterKind::Flate,
        FilterKind::RunLength,
        FilterKind::Crypt,
    ];

    /// The filter's name in Table 6.
    pub fn name(self) -> &'static str {
        match self {
            FilterKind::AsciiHex => "ASCIIHexDecode",
            FilterKind::Ascii85 => "ASCII85Decode",
            FilterKind::Lzw => "LZWDecode",
            FilterKind::Flate => "FlateDecode",
            FilterKind::RunLength => "RunLengthDecode",
            FilterKind::Crypt => "Crypt",
        }
    }

    fn from_name(name: &[u8]) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.name().as_bytes() == name)
    }
}

impl fmt::Display for FilterKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// One filter of a stream, with its parameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Filter {
    AsciiHex,
    Ascii85,
    RunLength,
    Lzw {
        /// Whether the code length grows one code early (Table 8,
        /// `EarlyChange`).
        early_change: bool,
        predictor: Predictor,
    },
    Flate {
        predictor: Predictor,
    },
    /// The data was decrypted before it came to the pipeline, which
    /// passes it on as it is.
    Crypt,
}

impl Filter {
    pub fn kind(&self) -> FilterKind {
        match self {
            Filter::AsciiHex => FilterKind::AsciiHex,
            Filter::Ascii85 => FilterKind::Ascii85,
            Filter::RunLength => FilterKind::RunLength,
            Filter::Lzw { .. } => FilterKind::Lzw,
            Filter::Flate { .. } => FilterKind::Flate,
            Filter::Crypt => FilterKind::Crypt,
        }
    }

    fn predictor(&self) -> Option<&Predictor> {
        match self {
            Filter::Lzw { predictor, .. } | Filter::Flate { predictor } => Some(predictor),
            Filter::AsciiHex | Filter::Ascii85 | Filter::RunLength | Filter::Crypt => None,
        }
    }

    /// The filter named `name` with the parameters of `parms`. A filter of
    /// Table 6 that takes no parameters reads none.
    fn read(name: &[u8], parms: Option<&Dictionary>) -> Result<Self, PdfError> {
        let kind = FilterKind::from_name(name)
            .ok_or_else(|| PdfError::UnknownFilter { name: shown(name) })?;
        let predictor = || match parms {
            Some(parms) => Predictor::read(parms, kind.name()),
            None => Ok(Predictor::default()),
        };
        Ok(match kind {
            FilterKind::AsciiHex => Filter::AsciiHex,
            FilterKind::Ascii85 => Filter::Ascii85,
            FilterKind::RunLength => Filter::RunLength,
            FilterKind::Crypt => Filter::Crypt,
            FilterKind::Flate => Filter::Flate {
                predictor: predictor()?,
            },
            FilterKind::Lzw => {
                let early_change = match parms {
                    None => None,
                    Some(parms) => predictor::integer(parms, EARLY_CHANGE_KEY, kind.name())?,
                };
                let early_change = match early_change {
                    // Table 8: the default is 1.
                    None | Some(1) => true,
                    Some(0) => false,
                    Some(_) => {
                        return Err(PdfError::FilterParameters {
                            filter: kind.name(),
                            key: EARLY_CHANGE_KEY,
                        })
                    }
                };
                Filter::Lzw {
                    early_change,
                    predictor: predictor()?,
                }
            }
        })
    }
}

/// How a decoder came to stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stop {
    /// At the filter's end-of-data marker.
    EndOfData,
    /// At the end of the input, with every unit of it whole and no marker.
    MissingEndOfData,
    /// At the end of Flate's last block, with no checksum that matches.
    ChecksumMismatch,
    /// At bytes the filter cannot decode, or inside a unit the input cuts
    /// short.
    Damaged,
}

/// Decoded stream data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decoded<'a> {
    /// The decoded bytes; the encoded ones themselves when the stream has
    /// no filter.
    pub data: Cow<'a, [u8]>,
    /// False when a filter met data it could not decode. `data` then holds
    /// what was decoded before that, and a [`DiagnosticKind::FilterDataDamaged`]
    /// names the filter.
    pub complete: bool,
}

/// The filters of one stream, in the order they decode its data.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Pipeline {
    filters: Vec<Filter>,
}

impl Pipeline {
    /// Reads the `Filter` and `DecodeParms` entries of a stream dictionary.
    pub fn from_dictionary(dictionary: &Dictionary) -> Result<Self, PdfError> {
        Self::from_objects(
            dictionary.get(FILTER_KEY.as_bytes()),
            dictionary.get(PARMS_KEY.as_bytes()),
        )
    }

    /// Builds the pipeline from the values of `Filter` and `DecodeParms`
    /// (Table 5). Both are read as direct objects: an indirect reference in
    /// either is refused, so the caller resolves them first.
    pub fn from_objects(filter: Option<&Object>, parms: Option<&Object>) -> Result<Self, PdfError> {
        let stages = stages(filter, parms)?;
        // 7.4.10: the Crypt filter is the first of the array.
        if stages
            .iter()
            .skip(1)
            .any(|(name, _)| *name == FilterKind::Crypt.name().as_bytes())
        {
            return Err(PdfError::FilterParameters {
                filter: FilterKind::Crypt.name(),
                key: FILTER_KEY,
            });
        }
        let filters = stages
            .into_iter()
            .map(|(name, parms)| Filter::read(name, parms))
            .collect::<Result<_, _>>()?;
        Ok(Self { filters })
    }

    pub fn filters(&self) -> &[Filter] {
        &self.filters
    }

    /// Decodes `data` through every filter in turn.
    ///
    /// `max_output` is the most bytes any filter may produce; data that
    /// would pass it is refused with [`PdfError::DecodedTooLarge`].
    /// `offset` is where the stream's data starts in its source, and is the
    /// offset of every diagnostic raised here.
    pub fn decode<'a>(
        &self,
        data: &'a [u8],
        max_output: usize,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Decoded<'a>, PdfError> {
        let mut decoding = self
            .filters
            .iter()
            .filter(|filter| !matches!(filter, Filter::Crypt))
            .peekable();
        if data.len() > max_output && decoding.peek().is_none() {
            return Err(output::too_large(max_output));
        }
        let mut data = Cow::Borrowed(data);
        let mut complete = true;
        for filter in decoding {
            let (decoded, stage_complete) = run(filter, &data, max_output, offset, diagnostics)?;
            data = Cow::Owned(decoded);
            complete &= stage_complete;
        }
        Ok(Decoded { data, complete })
    }
}

/// A filter's name and its parameters, as a stream dictionary gives them.
type Stage<'a> = (&'a [u8], Option<&'a Dictionary>);

/// The filter names of a stream with the parameters of each, in the order
/// they decode: the values of `Filter` and `DecodeParms` (Table 5).
fn stages<'a>(
    filter: Option<&'a Object>,
    parms: Option<&'a Object>,
) -> Result<Vec<Stage<'a>>, PdfError> {
    let invalid = |key| PdfError::FilterParameters {
        filter: THE_STREAM,
        key,
    };

    let names: Vec<&[u8]> = match filter.map(Object::value) {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Name(name)) => vec![name.as_bytes()],
        Some(Value::Array(items)) => {
            if items.len() > MAX_FILTERS {
                return Err(PdfError::LimitExceeded {
                    offset: filter.and_then(Object::span).map_or(0, |span| span.start),
                    limit: Limit::FilterCount,
                });
            }
            items
                .iter()
                .map(|item| match item.value() {
                    Value::Name(name) => Ok(name.as_bytes()),
                    _ => Err(invalid(FILTER_KEY)),
                })
                .collect::<Result<_, _>>()?
        }
        Some(_) => return Err(invalid(FILTER_KEY)),
    };

    let parms: Vec<Option<&Dictionary>> = match parms.map(Object::value) {
        None | Some(Value::Null) => vec![None; names.len()],
        Some(Value::Dictionary(dictionary)) if names.len() == 1 => vec![Some(dictionary)],
        Some(Value::Array(items)) if items.len() == names.len() => items
            .iter()
            .map(|item| match item.value() {
                Value::Null => Ok(None),
                Value::Dictionary(dictionary) => Ok(Some(dictionary)),
                _ => Err(invalid(PARMS_KEY)),
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err(invalid(PARMS_KEY)),
    };
    Ok(names.into_iter().zip(parms).collect())
}

/// The crypt filter a stream names for its own data: the `Name` of the
/// Crypt filter that heads its `Filter` (7.4.10, Table 14), `Identity`
/// when the filter gives no name. `None` for a stream with no Crypt
/// filter, which the document's default crypt filter decrypts.
///
/// `Filter` and `DecodeParms` are read as direct objects, as
/// [`Pipeline::from_objects`] reads them.
pub fn crypt_filter_name(dictionary: &Dictionary) -> Result<Option<Name>, PdfError> {
    let stages = stages(
        dictionary.get(FILTER_KEY.as_bytes()),
        dictionary.get(PARMS_KEY.as_bytes()),
    )?;
    let crypt = FilterKind::Crypt.name();
    let Some((_, parms)) = stages.first().filter(|(name, _)| *name == crypt.as_bytes()) else {
        return Ok(None);
    };
    let name = parms.and_then(|parms| parms.get(NAME_KEY.as_bytes()));
    match name.map(Object::value) {
        None | Some(Value::Null) => Ok(Some(Name::new(IDENTITY))),
        Some(Value::Name(name)) => Ok(Some(name.clone())),
        Some(_) => Err(PdfError::FilterParameters {
            filter: crypt,
            key: NAME_KEY,
        }),
    }
}

/// Runs one filter over `input`. Returns the decoded bytes and whether all
/// of the input was decoded.
fn run(
    filter: &Filter,
    input: &[u8],
    max_output: usize,
    offset: u64,
    diagnostics: &mut Diagnostics,
) -> Result<(Vec<u8>, bool), PdfError> {
    let kind = filter.kind();
    let removed = filter
        .predictor()
        .map_or(0, |predictor| predictor.removed_bytes(max_output));
    let mut out = Output::new(max_output, removed);
    let stop = match filter {
        Filter::AsciiHex => ascii_hex::decode(input, &mut out)?,
        Filter::Ascii85 => ascii85::decode(input, &mut out)?,
        Filter::RunLength => run_length::decode(input, &mut out)?,
        Filter::Lzw { early_change, .. } => lzw::decode(input, *early_change, &mut out)?,
        Filter::Flate { .. } => flate::decode(input, &mut out)?,
        // `Pipeline::decode` runs no Crypt filter: its data is passed on.
        Filter::Crypt => return Ok((input.to_vec(), true)),
    };
    let mut complete = true;
    match stop {
        Stop::EndOfData => {}
        Stop::MissingEndOfData => {
            diagnostics.push(DiagnosticKind::MissingEndOfData { filter: kind }, offset);
        }
        Stop::ChecksumMismatch => diagnostics.push(DiagnosticKind::FlateChecksum, offset),
        Stop::Damaged => complete = false,
    }

    let mut decoded = out.into_vec();
    if let Some(predictor) = filter.predictor() {
        complete &= predictor.undo(&mut decoded);
        if decoded.len() > max_output {
            return Err(output::too_large(max_output));
        }
    }
    if !complete {
        diagnostics.push(DiagnosticKind::FilterDataDamaged { filter: kind }, offset);
    }
    Ok((decoded, complete))
}
