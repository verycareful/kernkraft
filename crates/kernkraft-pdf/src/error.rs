// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::fmt;

use crate::syntax::limits::{
    MAX_FILTERS, MAX_NAME_BYTES, MAX_NESTING_DEPTH, MAX_STRING_BYTES, MAX_TOKEN_BYTES,
};

/// A hard limit of the reader and the writer. Input that passes one is
/// refused with [`PdfError::LimitExceeded`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Arrays and dictionaries nested one inside another.
    NestingDepth,
    /// The bytes of one name.
    NameBytes,
    /// The bytes of one keyword or number.
    TokenBytes,
    /// The bytes of one string.
    StringBytes,
    /// The filters one stream chains.
    FilterCount,
    /// The array items and dictionary entries of one object, nested ones
    /// included. The caller sets `max`
    /// ([`ReadLimits`](crate::file::ReadLimits)).
    ObjectElements { max: u64 },
    /// The cross-reference entries of one file, over all its sections. The
    /// caller sets `max` ([`ReadLimits`](crate::file::ReadLimits)).
    XrefEntries { max: u64 },
}

impl Limit {
    /// The largest value the limit allows.
    pub fn max(self) -> u64 {
        let max = match self {
            Limit::NestingDepth => MAX_NESTING_DEPTH,
            Limit::NameBytes => MAX_NAME_BYTES,
            Limit::TokenBytes => MAX_TOKEN_BYTES,
            Limit::StringBytes => MAX_STRING_BYTES,
            Limit::FilterCount => MAX_FILTERS,
            Limit::ObjectElements { max } | Limit::XrefEntries { max } => return max,
        };
        // A usize is at most 64 bits wide on every supported target.
        max as u64
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Limit::NestingDepth => "nesting depth",
            Limit::NameBytes => "name length",
            Limit::TokenBytes => "token length",
            Limit::StringBytes => "string length",
            Limit::FilterCount => "filter count",
            Limit::ObjectElements { .. } => "element count of an object",
            Limit::XrefEntries { .. } => "cross-reference entry count",
        })
    }
}

/// Every failure `kernkraft-pdf` reports.
#[derive(Debug, thiserror::Error)]
pub enum PdfError {
    #[error("not a readable PDF: {0}")]
    Parse(String),
    #[error("the PDF is encrypted")]
    Encrypted,
    #[error("the PDF has no pages")]
    NoPages,
    #[error("page {page} is out of range 1-{count}")]
    PageOutOfRange { page: u32, count: u32 },
    #[error("page {0} is listed twice")]
    DuplicatePage(u32),
    #[error("bad page range {0:?}")]
    BadRange(String),
    #[error("{0}")]
    Invalid(String),
    #[error(transparent)]
    Fs(#[from] kernkraft_fs::FsError),
    #[error("render failed: {0}")]
    Render(String),
    /// A read that does not lie inside the source: its range starts or ends
    /// past the last byte, or its end does not fit in a `u64`.
    #[error("read of {len} bytes at offset {offset} is outside the file's {size} bytes")]
    OutOfBounds { offset: u64, len: u64, size: u64 },
    /// The bytes end inside a string, an array, a dictionary or an indirect
    /// object. `offset` is where the unfinished construct starts.
    #[error("the PDF ends inside a construct that starts at offset {offset}")]
    UnexpectedEnd { offset: u64 },
    /// A token that cannot stand where it was found. `found` holds the start
    /// of the token as text.
    #[error("unexpected {found} at offset {offset}")]
    UnexpectedToken { offset: u64, found: String },
    /// A construct that passes a hard limit. `offset` is where the construct
    /// starts in the source, or how many bytes had been written when the
    /// limit was passed while writing.
    #[error("{limit} at offset {offset} is above the limit of {}", .limit.max())]
    LimitExceeded { offset: u64, limit: Limit },
    /// A filter name that is not one this library decodes. `name` holds
    /// the start of the name as text.
    #[error("{name} is not a filter this library decodes")]
    UnknownFilter { name: String },
    /// A `Filter` or `DecodeParms` entry of the wrong shape, or a filter
    /// parameter outside the values its filter defines. `filter` is the
    /// filter's name, or `the stream` for the two entries themselves.
    #[error("the {key} of {filter} is not valid")]
    FilterParameters {
        filter: &'static str,
        key: &'static str,
    },
    /// Decoding a stream would give more bytes than the caller allowed.
    #[error("decoded data is larger than the {cap} bytes allowed")]
    DecodedTooLarge { cap: u64 },
    /// No `%PDF-` header with a version number in the first bytes of the
    /// file, where a reader looks for one.
    #[error("the file has no PDF header")]
    MissingHeader,
    /// Cross-reference data that cannot be read: `offset` is where the
    /// fault lies. Opening a file answers this by rebuilding the
    /// cross-reference, so it is returned only where that is not done.
    #[error("the cross-reference data at offset {offset} is not valid")]
    BadXref { offset: u64 },
    /// The cross-reference places an object at an offset where another
    /// object, or none, starts.
    #[error("object {number} {generation} is not at offset {offset}, where the cross-reference places it")]
    XrefMismatch {
        number: u32,
        generation: u16,
        offset: u64,
    },
    /// An object stream (7.5.7) whose dictionary or data cannot be read, or
    /// that does not hold the object the cross-reference places in it.
    #[error("object stream {stream} is not valid")]
    ObjectStream { stream: u32 },
    /// The cross-reference had to be rebuilt and nothing in the file names
    /// or is a document catalog.
    #[error("the PDF has no document catalog")]
    NoCatalog,
    /// The file is encrypted by a security handler other than the standard
    /// one (7.6.3), a public-key handler (7.6.4) among them. `name` holds
    /// the start of the handler's name as text.
    #[error("the PDF is encrypted by the security handler {name}, which is not supported")]
    UnsupportedSecurityHandler { name: String },
    /// The file is encrypted by the standard security handler with an
    /// algorithm (`V`) and revision (`R`) this library does not read
    /// together.
    #[error("the PDF is encrypted with algorithm {version} of revision {revision}, which is not supported")]
    UnsupportedEncryption { version: i64, revision: i64 },
    /// A crypt filter (7.6.5) that the encryption dictionary does not
    /// define, or whose method is not one the file's algorithm allows.
    /// `name` holds the start of the filter's name as text.
    #[error("the crypt filter {name} is not supported")]
    UnsupportedCryptFilter { name: String },
    /// An entry of the encryption dictionary (Tables 20 and 21), or the
    /// trailer's `Encrypt` itself, that is missing or is not what its
    /// table defines.
    #[error("the {key} of the encryption dictionary is not valid")]
    EncryptionDictionary { key: &'static str },
    /// The password is neither the user password nor the owner password
    /// of the file.
    #[error("the password does not open the PDF")]
    WrongPassword,
}
