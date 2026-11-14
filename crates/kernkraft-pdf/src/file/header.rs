// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The file header: ISO 32000-1:2008, 7.5.2.
//!
//! The header is `%PDF-` and a version number. 7.5.2 places it on the
//! first line; it is looked for in the first [`HEADER_SEARCH_BYTES`] of
//! the file, and bytes before it are reported.

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::scan::find_in;
use crate::{PdfError, Source};

/// How far into the file the header is looked for. Adobe's PDF Reference,
/// sixth edition, Appendix H, implementation note 13 (on 3.4.1, File
/// Header): the header may appear anywhere within the first 1024 bytes.
pub const HEADER_SEARCH_BYTES: usize = 1024;

const HEADER_MARKER: &[u8] = b"%PDF-";

/// The version of the format a file's header names (7.5.2). The catalog's
/// `Version` entry, when a file has one, takes its place.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u8,
    pub minor: u8,
}

/// The header and where it was found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    pub version: Version,
    /// The offset of the percent sign of `%PDF-`.
    pub offset: u64,
}

/// Finds the header. A file with none is [`PdfError::MissingHeader`].
pub(crate) fn read_header(
    source: &dyn Source,
    diagnostics: &mut Diagnostics,
) -> Result<Header, PdfError> {
    let len = usize::try_from(source.len())
        .map_or(HEADER_SEARCH_BYTES, |size| size.min(HEADER_SEARCH_BYTES));
    let start = source.read(0, len)?;
    let mut from = 0;
    while let Some(position) = find_in(&start, HEADER_MARKER, from) {
        let after = start.get(position + HEADER_MARKER.len()..).unwrap_or(&[]);
        if let Some(version) = version(after) {
            // A position inside the search window fits a u64.
            let offset = position as u64;
            if offset != 0 {
                diagnostics.push(DiagnosticKind::HeaderNotAtStart, offset);
            }
            return Ok(Header { version, offset });
        }
        from = position + 1;
    }
    Err(PdfError::MissingHeader)
}

/// The version number that starts `bytes`: decimal digits, a period and
/// decimal digits, each part a value a `u8` holds.
fn version(bytes: &[u8]) -> Option<Version> {
    let (major, rest) = number(bytes)?;
    let rest = rest.strip_prefix(b".")?;
    let (minor, _) = number(rest)?;
    Some(Version { major, minor })
}

/// The decimal number that starts `bytes`, and the bytes after it.
fn number(bytes: &[u8]) -> Option<(u8, &[u8])> {
    let digits = bytes
        .iter()
        .take_while(|byte| byte.is_ascii_digit())
        .count();
    let (number, rest) = bytes.split_at(digits);
    let value = std::str::from_utf8(number).ok()?.parse().ok()?;
    Some((value, rest))
}
