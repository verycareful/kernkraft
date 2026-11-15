// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Extracting pages and splitting a document into parts.
//!
//! A part is a new snapshot of the same file whose page tree holds the
//! pages asked for; it shares every object with the document it was cut
//! from, and a compacted copy of it holds only what its pages use. The
//! outline, destinations, form, structure and labels of a part follow
//! [`super`]: what led to a page the part does not have is gone.
//! [`split_by_bookmarks`] cuts at the top-level items of the outline
//! ([`Document::outline`]), whose pages come from their destinations.

use kernkraft_fs::Progress;

use super::select::{check, pages_in_order, scan};
use crate::doc::Document;
use crate::{PageRange, PdfError};

/// Title of the part holding the pages before the first bookmark.
pub const START_PART_TITLE: &str = "Start";

/// A document of `pages` (1-based) of `document`, in the order given.
pub fn extract(document: &Document, pages: &[u32]) -> Result<Document, PdfError> {
    let count = document.page_count()?;
    check(pages, count)?;
    pages_in_order(document, pages, &scan(document)?)
}

/// One document per range, each as [`extract`] makes it.
pub fn split_ranges(document: &Document, ranges: &[PageRange]) -> Result<Vec<Document>, PdfError> {
    if ranges.is_empty() {
        return Err(PdfError::Invalid("no page ranges".to_string()));
    }
    let count = document.page_count()?;
    let referrers = scan(document)?;
    ranges
        .iter()
        .map(|range| {
            if range.start > range.end {
                return Err(PdfError::BadRange(format!("{}-{}", range.start, range.end)));
            }
            let pages: Vec<u32> = (range.start..=range.end).collect();
            check(&pages, count)?;
            pages_in_order(document, &pages, &referrers)
        })
        .collect()
}

/// One single-page document per page. `progress` gets pages done out of
/// the page count; a cancel is [`PdfError::Cancelled`].
pub fn split_per_page(
    document: &Document,
    progress: &dyn Progress,
) -> Result<Vec<Document>, PdfError> {
    let count = document.page_count()?;
    let referrers = scan(document)?;
    let mut parts = Vec::with_capacity(count as usize);
    for page in 1..=count {
        if progress.is_cancelled() {
            return Err(PdfError::Cancelled);
        }
        parts.push(pages_in_order(document, &[page], &referrers)?);
        progress.report(u64::from(page), u64::from(count), "Splitting");
    }
    Ok(parts)
}

/// Splits at the top-level bookmarks: each bookmark starts a part that runs
/// to the page before the next bookmark's page, titled with the bookmark's
/// title. Pages before the first bookmark form a part titled
/// [`START_PART_TITLE`]. Bookmarks on the same page as an earlier one, or
/// whose destination is not a page of this document, start no part.
/// [`PdfError::Invalid`] when no bookmark leads to a page.
pub fn split_by_bookmarks(document: &Document) -> Result<Vec<(String, Document)>, PdfError> {
    let mut starts: Vec<(u32, String)> = document
        .outline()?
        .into_iter()
        .filter(|item| item.level == 0)
        .filter_map(|item| match item.destination?.page {
            crate::DestinationPage::Page(page) => Some((page, item.title.text)),
            _ => None,
        })
        .collect();
    if starts.is_empty() {
        return Err(PdfError::Invalid("no bookmarks".to_string()));
    }
    starts.sort_by_key(|(page, _)| *page);
    starts.dedup_by_key(|(page, _)| *page);
    if starts[0].0 > 1 {
        starts.insert(0, (1, START_PART_TITLE.to_string()));
    }
    let count = document.page_count()?;
    let referrers = scan(document)?;
    let mut parts = Vec::with_capacity(starts.len());
    for (at, (start, title)) in starts.iter().enumerate() {
        let end = starts.get(at + 1).map_or(count, |(next, _)| next - 1);
        let pages: Vec<u32> = (*start..=end).collect();
        parts.push((title.clone(), pages_in_order(document, &pages, &referrers)?));
    }
    Ok(parts)
}
