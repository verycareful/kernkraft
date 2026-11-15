// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page operations on a [`Document`]: rotate, reorder, remove, extract,
//! split and merge.
//!
//! Page numbers are 1-based. A [`Document`] never changes, so every
//! operation gives a new one that shares the file and every object it did
//! not touch; the document it was given is left as it was. Saving is
//! [`crate::write::compact`] (or `save_compact`), which writes the pages
//! the result has and what they use, and an encrypted document stays
//! encrypted.
//!
//! A page that leaves a document takes with it what only it used. What
//! other things say about it is kept in agreement: the outline, the
//! named destinations, the form and the tagged structure each lose what
//! led to it, page labels follow the pages, and any other reference
//! becomes null (see `references`). The tagged structure, the form and
//! the labels are kept whole by a merge as well.

mod destinations;
mod forms;
mod labels;
mod merge;
mod outline;
mod references;
mod rewrite;
mod select;
mod split;
mod structure;
mod trees;

use std::collections::HashSet;

pub use merge::merge;
pub use split::{extract, split_by_bookmarks, split_per_page, split_ranges, START_PART_TITLE};

use crate::doc::Document;
use crate::syntax::Name;
use crate::PdfError;
use rewrite::{integer, Rewriter};

/// Degrees in a quarter turn; `Rotate` must be a multiple of it (Table
/// 30).
const QUARTER_TURN: i64 = 90;
const FULL_TURN: i64 = 360;
const ROTATE_KEY: &[u8] = b"Rotate";

/// A clockwise rotation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rotation {
    Cw90,
    Cw180,
    Cw270,
}

impl Rotation {
    pub fn degrees(self) -> i64 {
        match self {
            Rotation::Cw90 => QUARTER_TURN,
            Rotation::Cw180 => 2 * QUARTER_TURN,
            Rotation::Cw270 => 3 * QUARTER_TURN,
        }
    }
}

/// `document` with `pages` (all pages when empty) turned clockwise by
/// `by`, on top of the rotation each already has, its own or inherited.
/// The stored value is 0, 90, 180 or 270. A page whose rotation is not a
/// multiple of 90 is [`PdfError::InvalidEntry`] and nothing is changed.
pub fn rotate(document: &Document, pages: &[u32], by: Rotation) -> Result<Document, PdfError> {
    let count = document.page_count()?;
    let targets: Vec<u32> = if pages.is_empty() {
        (1..=count).collect()
    } else {
        select::check(pages, count)?;
        pages.to_vec()
    };
    let mut rewriter = Rewriter::new(document);
    for number in targets {
        let page = document.page(number)?;
        let degrees = (i64::from(page.rotate) + by.degrees()).rem_euclid(FULL_TURN);
        rewriter.edit(page.id, |_, dictionary| {
            dictionary.insert(Name::new(ROTATE_KEY), integer(degrees));
            Ok(())
        })?;
    }
    document.apply(rewriter.finish())
}

/// `document` with its pages in a new order: `order[i]` is the old
/// number of the page that ends up at position `i + 1`. `order` names
/// every page once.
pub fn reorder(document: &Document, order: &[u32]) -> Result<Document, PdfError> {
    let count = document.page_count()?;
    if order.len() != count as usize {
        return Err(PdfError::Invalid(format!(
            "the new order lists {} pages, the document has {count}",
            order.len()
        )));
    }
    select::check(order, count)?;
    select::pages_in_order(document, order, &references::Referrers::none())
}

/// `document` without `pages`. At least one page stays.
pub fn remove(document: &Document, pages: &[u32]) -> Result<Document, PdfError> {
    let count = document.page_count()?;
    if pages.is_empty() {
        return Err(PdfError::Invalid("no pages to remove".to_string()));
    }
    select::check(pages, count)?;
    if pages.len() >= count as usize {
        return Err(PdfError::Invalid("cannot remove every page".to_string()));
    }
    let gone: HashSet<u32> = pages.iter().copied().collect();
    let stays: Vec<u32> = (1..=count).filter(|n| !gone.contains(n)).collect();
    select::pages_in_order(document, &stays, &select::scan(document)?)
}
