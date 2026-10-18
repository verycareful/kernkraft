// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 the Qyra authors (github.com/zParik/Qyra)
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Rotating, reordering and removing pages in place, after Qyra's rotate,
//! reorder and remove commands. Each first flattens the page tree, so a
//! page's inherited attributes (its rotation among them) are on the page
//! itself before anything moves.

use std::collections::HashSet;

use lopdf::{Document, Object, ObjectId};

use crate::tree::{flatten, keep_pages, set_pages};
use crate::{page_count, PdfError};

/// Degrees in a quarter turn; `/Rotate` must be a multiple of it
/// (ISO 32000-1:2008, Table 30).
const QUARTER_TURN: i64 = 90;
const FULL_TURN: i64 = 360;

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

/// Turns `pages` (all pages when empty) clockwise by `by`, on top of the
/// rotation each page already has, inherited or its own. The stored value
/// is normalised to 0, 90, 180 or 270. A page whose rotation is not a
/// multiple of 90 is [`PdfError::Invalid`] and nothing is changed.
pub fn rotate(doc: &mut Document, pages: &[u32], by: Rotation) -> Result<(), PdfError> {
    let count = page_count(doc);
    let targets: Vec<u32> = if pages.is_empty() {
        (1..=count).collect()
    } else {
        check_pages(pages, count)?;
        pages.to_vec()
    };
    let (root, all) = flatten(doc)?;
    let mut updates = Vec::with_capacity(targets.len());
    for &n in &targets {
        let id = all[n as usize - 1];
        let current = current_rotation(doc, id, n)?;
        updates.push((id, (current + by.degrees()).rem_euclid(FULL_TURN)));
    }
    for (id, degrees) in updates {
        page_dict(doc, id)?.set("Rotate", degrees);
    }
    set_pages(doc, root, &all)
}

/// Puts the pages in a new order: `order[i]` is the old page number of the
/// page that ends up at position `i + 1`. `order` must name every page
/// exactly once.
pub fn reorder(doc: &mut Document, order: &[u32]) -> Result<(), PdfError> {
    let count = page_count(doc);
    if order.len() != count as usize {
        return Err(PdfError::Invalid(format!(
            "the new order lists {} pages, the document has {count}",
            order.len()
        )));
    }
    check_pages(order, count)?;
    let (root, all) = flatten(doc)?;
    let kids: Vec<ObjectId> = order.iter().map(|&n| all[n as usize - 1]).collect();
    set_pages(doc, root, &kids)
}

/// Removes `pages`, keeping at least one page. Every reference to a removed
/// page becomes `null`, so nothing only it used survives. The outline is
/// kept: a bookmark that led to a removed page keeps its title and no
/// longer leads anywhere.
pub fn remove(doc: &mut Document, pages: &[u32]) -> Result<(), PdfError> {
    let count = page_count(doc);
    if pages.is_empty() {
        return Err(PdfError::Invalid("no pages to remove".to_string()));
    }
    check_pages(pages, count)?;
    if pages.len() >= count as usize {
        return Err(PdfError::Invalid("cannot remove every page".to_string()));
    }
    let (root, all) = flatten(doc)?;
    let removed: HashSet<u32> = pages.iter().copied().collect();
    let keep: Vec<ObjectId> = (1..=count)
        .filter(|n| !removed.contains(n))
        .map(|n| all[n as usize - 1])
        .collect();
    keep_pages(doc, root, &all, &keep)
}

/// Every page in `1..=count`, none twice.
fn check_pages(pages: &[u32], count: u32) -> Result<(), PdfError> {
    let mut seen = HashSet::new();
    for &p in pages {
        if p == 0 || p > count {
            return Err(PdfError::PageOutOfRange { page: p, count });
        }
        if !seen.insert(p) {
            return Err(PdfError::DuplicatePage(p));
        }
    }
    Ok(())
}

fn current_rotation(doc: &Document, id: ObjectId, n: u32) -> Result<i64, PdfError> {
    let value = match doc
        .get_dictionary(id)
        .map_err(|e| PdfError::Parse(format!("page {n}: {e}")))?
        .get(b"Rotate")
    {
        Ok(Object::Reference(r)) => doc.get_object(*r).ok(),
        Ok(other) => Some(other),
        Err(_) => None,
    };
    let degrees = match value {
        None => 0,
        Some(Object::Integer(i)) => *i,
        Some(Object::Real(r)) if r.fract() == 0.0 => *r as i64,
        Some(other) => {
            return Err(PdfError::Invalid(format!(
                "page {n} has rotation {other:?}, not a multiple of 90"
            )))
        }
    };
    if degrees % QUARTER_TURN != 0 {
        return Err(PdfError::Invalid(format!(
            "page {n} has rotation {degrees}, not a multiple of 90"
        )));
    }
    Ok(degrees)
}

fn page_dict(doc: &mut Document, id: ObjectId) -> Result<&mut lopdf::Dictionary, PdfError> {
    doc.get_dictionary_mut(id)
        .map_err(|e| PdfError::Parse(format!("page {id:?}: {e}")))
}
