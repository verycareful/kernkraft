// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page-range specs such as `"1-3, 5, 7-9"`.

use crate::PdfError;

/// A 1-based, inclusive page range.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageRange {
    pub start: u32,
    pub end: u32,
}

/// Parses a comma-separated list of pages (`5`) and ranges (`1-3`) and
/// checks each against `page_count`. A page is written in ASCII digits
/// only. Whitespace is ignored and empty items are skipped. A page of 0 or past the last page is
/// [`PdfError::PageOutOfRange`], never clamped; a backwards range, a token
/// that is not a page or range, or a spec with no items at all is
/// [`PdfError::BadRange`].
pub fn parse_ranges(spec: &str, page_count: u32) -> Result<Vec<PageRange>, PdfError> {
    let mut out = Vec::new();
    for token in spec.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        let bad = || PdfError::BadRange(token.to_string());
        let (start, end) = match token.split_once('-') {
            Some((a, b)) => (a.trim(), b.trim()),
            None => (token, token),
        };
        let start = page_number(start).ok_or_else(bad)?;
        let end = page_number(end).ok_or_else(bad)?;
        for page in [start, end] {
            if page == 0 || page > page_count {
                return Err(PdfError::PageOutOfRange {
                    page,
                    count: page_count,
                });
            }
        }
        if start > end {
            return Err(bad());
        }
        out.push(PageRange { start, end });
    }
    if out.is_empty() {
        return Err(PdfError::BadRange(spec.to_string()));
    }
    Ok(out)
}

/// `token` as a page number: one or more ASCII digits that fit a `u32`.
/// `u32::from_str` alone would also take a leading `+`.
fn page_number(token: &str) -> Option<u32> {
    if token.is_empty() || !token.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    token.parse().ok()
}
