// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! JBIG2Decode: ISO 32000-1:2008, 7.4.7 (Table 12), over ITU-T T.88, which
//! the `hayro-jbig2` crate decodes.
//!
//! The stream holds the segments of one page in the embedded organisation
//! (T.88 Annex D.3), without file header, end-of-page or end-of-file
//! segments; the segments shared between pages are in the stream
//! `JBIG2Globals` names. Pixels come out one bit each, rows padded to a
//! whole byte, in the polarity of PDF image data: JBIG2 calls black 1, and
//! PDF image data calls it 0 in a DeviceGray image, so the bits are
//! inverted.

use std::sync::Arc;

use hayro_jbig2::{DecodeSettings, Decoder, Globals, Page};

use super::output::Output;
use super::Stop;
use crate::syntax::{Dictionary, ObjectId, Value};
use crate::PdfError;

const FILTER: &str = "JBIG2Decode";
const GLOBALS_KEY: &str = "JBIG2Globals";
const BITS_PER_BYTE: u32 = 8;
const WHITE_BYTE: u8 = 0xFF;

/// The segments shared between pages (Table 12).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Jbig2Globals {
    /// The filter names no `JBIG2Globals`.
    None,
    /// The stream is object `id`; its decoded data is to be given with
    /// [`Pipeline::provide_jbig2_globals`](super::Pipeline::provide_jbig2_globals)
    /// before the filter runs.
    Reference(ObjectId),
    /// The decoded bytes of the stream.
    Data(Arc<[u8]>),
}

impl Jbig2Globals {
    pub(super) fn read(parms: Option<&Dictionary>) -> Result<Self, PdfError> {
        let entry = parms.and_then(|parms| parms.get(GLOBALS_KEY.as_bytes()));
        match entry.map(|object| object.value()) {
            None | Some(Value::Null) => Ok(Self::None),
            Some(Value::Reference(id)) => Ok(Self::Reference(*id)),
            Some(_) => Err(PdfError::FilterParameters {
                filter: FILTER,
                key: GLOBALS_KEY,
            }),
        }
    }
}

fn settings() -> DecodeSettings {
    DecodeSettings { strict: true }
}

/// The globals as the decoder takes them: `Ok(None)` for none, `Err(())`
/// when their data cannot be read, and the caller's mistake of a stream
/// not yet given is `PdfError::FilterParameters`.
fn parsed_globals(globals: &Jbig2Globals) -> Result<Result<Option<Globals>, ()>, PdfError> {
    match globals {
        Jbig2Globals::None => Ok(Ok(None)),
        Jbig2Globals::Data(bytes) => Ok(Globals::new(bytes, &settings()).map(Some).map_err(|_| ())),
        Jbig2Globals::Reference(_) => Err(PdfError::FilterParameters {
            filter: FILTER,
            key: GLOBALS_KEY,
        }),
    }
}

/// Packs the pixels the decoder finds into the rows of `out`.
struct Rows<'o> {
    out: &'o mut Output,
    row: Vec<u8>,
    /// Pixels in the row so far.
    filled: u32,
    width: u32,
    /// The first error `out` gave, after which rows are dropped.
    error: Option<PdfError>,
}

impl Rows<'_> {
    fn put(&mut self, black: bool) {
        if self.filled < self.width {
            let (byte, shift) = (
                (self.filled / BITS_PER_BYTE) as usize,
                BITS_PER_BYTE - 1 - self.filled % BITS_PER_BYTE,
            );
            // PDF image data: 0 is black.
            if black {
                self.row[byte] &= !(1 << shift);
            }
        }
        self.filled += 1;
    }
}

impl Decoder for Rows<'_> {
    fn push_pixel(&mut self, black: bool) {
        self.put(black);
    }

    fn push_pixel_chunk(&mut self, black: bool, chunk_count: u32) {
        for _ in 0..chunk_count.saturating_mul(BITS_PER_BYTE) {
            self.put(black);
        }
    }

    fn next_line(&mut self) {
        if self.error.is_none() {
            if let Err(error) = self.out.extend(&self.row) {
                self.error = Some(error);
            }
        }
        self.row.fill(WHITE_BYTE);
        self.filled = 0;
    }
}

pub(super) fn decode(
    input: &[u8],
    globals: &Jbig2Globals,
    out: &mut Output,
) -> Result<(Stop, u32, u32), PdfError> {
    let Ok(globals) = parsed_globals(globals)? else {
        return Ok((Stop::Damaged, 0, 0));
    };
    let page = match Page::new_embedded(input, globals.as_ref(), &settings()) {
        Ok(page) => page,
        Err(_) => return Ok((Stop::Damaged, 0, 0)),
    };
    let width = page.width();
    let row_bytes = width.div_ceil(BITS_PER_BYTE) as usize;
    // A page that cannot fit is refused before it is decoded.
    out.check_room(row_bytes.saturating_mul(page.height() as usize))?;
    let mut rows = Rows {
        out,
        row: vec![WHITE_BYTE; row_bytes],
        filled: 0,
        width,
        error: None,
    };
    let result = page.decode(&mut rows);
    if let Some(error) = rows.error {
        return Err(error);
    }
    let stop = match result {
        Ok(()) => Stop::EndOfData,
        Err(_) => Stop::Damaged,
    };
    Ok((stop, width, page.height()))
}
