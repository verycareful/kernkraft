// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 the Qyra authors (github.com/zParik/Qyra)
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Extracting pages and splitting a document into parts.
//!
//! The bookmark walk (top-level outline items through `/First` and
//! `/Next`, each item's page from its `/Dest` or `/A` GoTo action) comes
//! from Qyra's split command. Destinations follow ISO 32000-1:2008, 12.3.2:
//! an explicit destination is an array whose first element is the page; a
//! named destination is a name (looked up in the catalog's `/Dests`) or a
//! string (looked up in the `/Dests` name tree under `/Names`, 7.9.6), and
//! its value is an explicit destination or a dictionary holding one in
//! `/D`.

use std::collections::{HashMap, HashSet};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::tree::{checked_count, flatten, keep_pages, page_ids};
use crate::{PageRange, PdfError};
use kernkraft_fs::{FsError, Progress};

/// Title of the part holding the pages before the first bookmark.
pub const START_PART_TITLE: &str = "Start";

/// Bound on outline items and name-tree nodes visited, so a cyclic or
/// absurd outline ends.
const MAX_WALK: usize = 1 << 16;

/// A new document holding `pages` (1-based) of `doc`, in the order given.
///
/// Inherited page attributes are kept (the tree is flattened first). Every
/// reference to a page left out becomes `null` and its form fields leave
/// the `/AcroForm`, so no content of those pages survives in the result. The outline (bookmarks) is not carried
/// over: its items would point at pages that may not be there.
pub fn extract(doc: &Document, pages: &[u32]) -> Result<Document, PdfError> {
    let count = checked_count(doc)?;
    if pages.is_empty() {
        return Err(PdfError::Invalid("no pages to extract".to_string()));
    }
    let mut seen = HashSet::new();
    for &p in pages {
        if p == 0 || p > count {
            return Err(PdfError::PageOutOfRange { page: p, count });
        }
        if !seen.insert(p) {
            return Err(PdfError::DuplicatePage(p));
        }
    }
    let mut out = doc.clone();
    let (root, all) = flatten(&mut out)?;
    let keep: Vec<ObjectId> = pages.iter().map(|&p| all[p as usize - 1]).collect();
    remove_outline(&mut out)?;
    keep_pages(&mut out, root, &all, &keep)?;
    Ok(out)
}

/// One document per range, each as [`extract`] makes it.
pub fn split_ranges(doc: &Document, ranges: &[PageRange]) -> Result<Vec<Document>, PdfError> {
    if ranges.is_empty() {
        return Err(PdfError::Invalid("no page ranges".to_string()));
    }
    ranges
        .iter()
        .map(|r| {
            if r.start > r.end {
                return Err(PdfError::BadRange(format!("{}-{}", r.start, r.end)));
            }
            extract(doc, &(r.start..=r.end).collect::<Vec<_>>())
        })
        .collect()
}

/// One single-page document per page. `progress` gets pages done out of
/// the page count; a cancel stops with `PdfError::Fs(FsError::Cancelled)`.
pub fn split_per_page(doc: &Document, progress: &dyn Progress) -> Result<Vec<Document>, PdfError> {
    let count = checked_count(doc)?;
    let mut parts = Vec::with_capacity(count as usize);
    for page in 1..=count {
        if progress.is_cancelled() {
            return Err(FsError::Cancelled.into());
        }
        parts.push(extract(doc, &[page])?);
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
pub fn split_by_bookmarks(doc: &Document) -> Result<Vec<(String, Document)>, PdfError> {
    let page_of: HashMap<ObjectId, u32> = page_ids(doc)?.into_iter().zip(1..).collect();
    let mut starts = top_level_bookmarks(doc)
        .into_iter()
        .filter_map(|(dest, title)| page_of.get(&dest).map(|&p| (p, title)))
        .collect::<Vec<_>>();
    if starts.is_empty() {
        return Err(PdfError::Invalid("no bookmarks".to_string()));
    }
    starts.sort_by_key(|(p, _)| *p);
    starts.dedup_by_key(|(p, _)| *p);
    if starts[0].0 > 1 {
        starts.insert(0, (1, START_PART_TITLE.to_string()));
    }

    let count = checked_count(doc)?;
    let mut parts = Vec::with_capacity(starts.len());
    for (i, (start, title)) in starts.iter().enumerate() {
        let end = starts.get(i + 1).map_or(count, |(next, _)| next - 1);
        let pages: Vec<u32> = (*start..=end).collect();
        parts.push((title.clone(), extract(doc, &pages)?));
    }
    Ok(parts)
}

/// `(destination page id, title)` of each top-level outline item, in
/// outline order. Items without a resolvable page are left out; an item
/// without a usable title gets `Section <n>`.
fn top_level_bookmarks(doc: &Document) -> Vec<(ObjectId, String)> {
    let Some(mut next) = doc
        .catalog()
        .ok()
        .and_then(|c| deref_dict(doc, c.get(b"Outlines").ok()?))
        .and_then(|o| o.get(b"First").ok()?.as_reference().ok())
    else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut visited = HashSet::new();
    while visited.len() < MAX_WALK && visited.insert(next) {
        let Ok(item) = doc.get_dictionary(next) else {
            break;
        };
        if let Some(page) = item_page(doc, item) {
            let title = item
                .get(b"Title")
                .ok()
                .and_then(|t| lopdf::decode_text_string(deref(doc, t)).ok())
                .unwrap_or_else(|| format!("Section {}", out.len() + 1));
            out.push((page, title));
        }
        match item.get(b"Next").and_then(Object::as_reference) {
            Ok(id) => next = id,
            Err(_) => break,
        }
    }
    out
}

fn item_page(doc: &Document, item: &Dictionary) -> Option<ObjectId> {
    if let Ok(dest) = item.get(b"Dest") {
        return dest_page(doc, dest);
    }
    let action = deref_dict(doc, item.get(b"A").ok()?)?;
    if action.get(b"S").and_then(Object::as_name).ok()? != b"GoTo" {
        return None;
    }
    dest_page(doc, action.get(b"D").ok()?)
}

/// The page an explicit or named destination points at.
fn dest_page(doc: &Document, dest: &Object) -> Option<ObjectId> {
    match deref(doc, dest) {
        Object::Array(items) => items.first()?.as_reference().ok(),
        Object::Dictionary(d) => dest_page_explicit(doc, d.get(b"D").ok()?),
        Object::Name(name) => {
            let dests = deref_dict(doc, doc.catalog().ok()?.get(b"Dests").ok()?)?;
            dest_page_explicit(doc, dests.get(name).ok()?)
        }
        Object::String(key, _) => {
            let names = deref_dict(doc, doc.catalog().ok()?.get(b"Names").ok()?)?;
            let tree = deref_dict(doc, names.get(b"Dests").ok()?)?;
            dest_page_explicit(doc, name_tree_lookup(doc, tree, key)?)
        }
        _ => None,
    }
}

/// A named destination's value: an explicit destination array, or a
/// dictionary holding one in `/D`.
fn dest_page_explicit(doc: &Document, value: &Object) -> Option<ObjectId> {
    match deref(doc, value) {
        Object::Array(items) => items.first()?.as_reference().ok(),
        Object::Dictionary(d) => match deref(doc, d.get(b"D").ok()?) {
            Object::Array(items) => items.first()?.as_reference().ok(),
            _ => None,
        },
        _ => None,
    }
}

/// Looks `key` up in a name tree (ISO 32000-1:2008, 7.9.6): leaves hold
/// `/Names [key value key value ...]`, inner nodes `/Kids`.
fn name_tree_lookup<'a>(doc: &'a Document, root: &'a Dictionary, key: &[u8]) -> Option<&'a Object> {
    let mut stack = vec![root];
    let mut visited = 0usize;
    while let Some(node) = stack.pop() {
        visited += 1;
        if visited > MAX_WALK {
            return None;
        }
        if let Ok(names) = node.get(b"Names").and_then(|n| deref(doc, n).as_array()) {
            for [k, value] in names.as_chunks::<2>().0 {
                if let Object::String(k, _) = deref(doc, k) {
                    if k.as_slice() == key {
                        return Some(value);
                    }
                }
            }
        }
        if let Ok(kids) = node.get(b"Kids").and_then(|k| deref(doc, k).as_array()) {
            stack.extend(kids.iter().filter_map(|k| deref_dict(doc, k)));
        }
    }
    None
}

fn deref<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
    match object {
        Object::Reference(id) => doc.get_object(*id).unwrap_or(&Object::Null),
        other => other,
    }
}

fn deref_dict<'a>(doc: &'a Document, object: &'a Object) -> Option<&'a Dictionary> {
    deref(doc, object).as_dict().ok()
}

/// Drops the catalog's `/Outlines`; the outline objects are pruned with the
/// rest of what nothing references.
pub(crate) fn remove_outline(doc: &mut Document) -> Result<(), PdfError> {
    doc.catalog_mut()
        .map_err(|e| PdfError::Parse(format!("catalog: {e}")))?
        .remove(b"Outlines");
    Ok(())
}
