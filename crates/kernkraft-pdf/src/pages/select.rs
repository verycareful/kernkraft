// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Choosing the pages of a document and their order: the one operation
//! behind extract, split, remove and reorder.
//!
//! The page tree is flattened first: each page that stays is given the
//! inheritable attributes it took from the nodes above it (Table 30) and
//! hung from the root, in the order asked for, so that moving a page
//! changes nothing about it. The nodes above the pages are then
//! unreachable and a compacted copy does not hold them.
//!
//! What refers to the pages that go is then made to agree: the outline,
//! the named destinations, the form and the structure tree each by its
//! own rules, the page labels so that a page keeps its label, and every
//! other reference by [`scrub_all`].

use std::collections::HashSet;

use super::references::{gone_of, scrub_all, Referrers};
use super::rewrite::{integer, reference, reference_object, Rewriter};
use super::{destinations, forms, labels, outline, structure};
use crate::doc::Document;
use crate::syntax::{Name, ObjectId};
use crate::PdfError;

const PAGES_KEY: &[u8] = b"Pages";
const KIDS_KEY: &[u8] = b"Kids";
const COUNT_KEY: &[u8] = b"Count";
const PARENT_KEY: &[u8] = b"Parent";

/// Every page of `document` as a `Referrers` can be asked about.
pub(crate) fn scan(document: &Document) -> Result<Referrers, PdfError> {
    let count = document.page_count()?;
    let mut pages = HashSet::new();
    for number in 1..=count {
        pages.insert(document.page_id(number)?);
    }
    Referrers::scan(document, &pages)
}

/// A document of the pages `order` names (1-based, in the order they are
/// to have, none twice) of `document`. `referrers` is [`scan`] of
/// `document`.
pub(crate) fn pages_in_order(
    document: &Document,
    order: &[u32],
    referrers: &Referrers,
) -> Result<Document, PdfError> {
    let count = document.page_count()?;
    check(order, count)?;
    let list = document.page_list()?;
    let all: Vec<ObjectId> = list.pages.iter().map(|(id, _)| *id).collect();
    let stays: HashSet<u32> = order.iter().copied().collect();
    let gone_numbers: HashSet<u32> = (1..=count).filter(|n| !stays.contains(n)).collect();
    let gone_pages: HashSet<ObjectId> = gone_numbers.iter().map(|&n| all[n as usize - 1]).collect();
    let catalog = document.catalog()?.id;
    let root = document
        .catalog()?
        .dictionary()
        .and_then(|catalog| catalog.get(PAGES_KEY))
        .and_then(reference)
        .ok_or(PdfError::InvalidEntry {
            object: Some(catalog),
            key: "Pages",
        })?;
    let mut rewriter = Rewriter::new(document);

    for &number in order {
        let id = all[number as usize - 1];
        let inherited = document.inherited_entries(number)?;
        rewriter.edit(id, |_, page| {
            for (key, value) in inherited {
                page.insert(key, value);
            }
            page.insert(Name::new(PARENT_KEY), reference_object(root));
            Ok(())
        })?;
    }
    let kids: Vec<_> = order
        .iter()
        .map(|&n| reference_object(all[n as usize - 1]))
        .collect();
    let kept = i64::try_from(kids.len()).unwrap_or(i64::MAX);
    rewriter.edit(root, |_, root| {
        root.insert(
            Name::new(KIDS_KEY),
            super::rewrite::direct(crate::syntax::Value::Array(kids)),
        );
        root.insert(Name::new(COUNT_KEY), integer(kept));
        Ok(())
    })?;

    if !gone_pages.is_empty() {
        let gone = gone_of(document, gone_pages)?;
        outline::prune(&mut rewriter, catalog, &gone_numbers)?;
        destinations::prune(&mut rewriter, catalog, &gone.pages)?;
        forms::prune(&mut rewriter, catalog, &gone)?;
        structure::prune(&mut rewriter, catalog, &gone)?;
        let nodes = &list.nodes;
        scrub_all(&mut rewriter, referrers, &gone.pages, &|id| {
            gone.pages.contains(&id) || nodes.contains(&id)
        })?;
    }
    let unchanged = order.len() == all.len() && order.iter().copied().eq(1..=count);
    if !unchanged && labels::has_labels(&rewriter, catalog)? {
        labels::remap(&mut rewriter, catalog, order)?;
    }
    document.apply(rewriter.finish())
}

/// Every page number in `1..=count`, none twice, and at least one.
pub(crate) fn check(pages: &[u32], count: u32) -> Result<(), PdfError> {
    if pages.is_empty() {
        return Err(PdfError::Invalid("no pages chosen".to_string()));
    }
    let mut seen = HashSet::new();
    for &page in pages {
        if page == 0 || page > count {
            return Err(PdfError::PageOutOfRange { page, count });
        }
        if !seen.insert(page) {
            return Err(PdfError::DuplicatePage(page));
        }
    }
    Ok(())
}
