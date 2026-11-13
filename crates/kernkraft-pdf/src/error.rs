// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::fmt;

use crate::syntax::limits::{MAX_NAME_BYTES, MAX_NESTING_DEPTH, MAX_STRING_BYTES, MAX_TOKEN_BYTES};

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
}

impl Limit {
    /// The largest value the limit allows.
    pub fn max(self) -> u64 {
        let max = match self {
            Limit::NestingDepth => MAX_NESTING_DEPTH,
            Limit::NameBytes => MAX_NAME_BYTES,
            Limit::TokenBytes => MAX_TOKEN_BYTES,
            Limit::StringBytes => MAX_STRING_BYTES,
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
}
