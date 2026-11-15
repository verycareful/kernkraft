// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page labels of a document whose pages change: ISO 32000-1:2008,
//! 12.4.2 (Table 159).
//!
//! A page keeps its label when it moves or when the pages around it go. A
//! labelling range is a run of pages numbered one after another, so the
//! labels of the new page order are written again as the runs they now
//! make: a page whose label does not follow the previous page's begins a
//! range of its own, with the label dictionary the page had and the
//! number it had in it (`St`).

use super::rewrite::{direct, integer, Rewriter};
use super::trees::{self, NUMS_KEY};
use crate::doc::NumberTree;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const PAGE_LABELS_KEY: &[u8] = b"PageLabels";
const START_KEY: &[u8] = b"St";
/// The number the first page of a range has when `St` says nothing
/// (Table 159).
const DEFAULT_START: i64 = 1;

/// A labelling range of the document: its first page's index and its
/// label dictionary.
struct Range {
    first: i64,
    dictionary: Dictionary,
}

/// The page label of one page of the source: the range it is in and its
/// number there.
#[derive(Clone, PartialEq)]
struct Label {
    range: usize,
    number: i64,
}

/// Writes the labels so that the page at index `i` of the new document
/// (counted from 0) has the label page `order[i]` (counted from 1) has in
/// the document, for `catalog`'s `PageLabels`.
pub(crate) fn remap(
    rewriter: &mut Rewriter<'_>,
    catalog: ObjectId,
    order: &[u32],
) -> Result<(), PdfError> {
    let document = rewriter.document;
    let Some(root) = rewriter
        .read_dictionary(catalog)?
        .and_then(|catalog| catalog.get(PAGE_LABELS_KEY).cloned())
    else {
        return Ok(());
    };
    if document.resolve(&root)?.dictionary().is_none() {
        return Ok(());
    }
    let mut ranges = Vec::new();
    for (first, value) in NumberTree::new(document, root).entries()? {
        let resolved = document.resolve(&value)?;
        let dictionary = resolved.dictionary().cloned().unwrap_or_default();
        ranges.push(Range { first, dictionary });
    }
    ranges.sort_by_key(|range| range.first);
    let mut pairs: Vec<(Object, Object)> = Vec::new();
    let mut previous: Option<(Option<Label>, i64)> = None;
    for (index, &page) in order.iter().enumerate() {
        let source = i64::from(page) - 1;
        let label = label_of(document, &ranges, source)?;
        let continues = match (&previous, &label) {
            (Some((Some(before), page_before)), Some(now)) => {
                before.range == now.range
                    && now.number == before.number + 1
                    && source == page_before + 1
            }
            (Some((None, _)), None) => true,
            _ => false,
        };
        if !continues {
            // A range for a page that has no label is the label with
            // nothing in it, which a viewer shows as the page's own number.
            let dictionary = match &label {
                Some(label) => with_start(&ranges[label.range].dictionary, label.number),
                None => Dictionary::new(),
            };
            pairs.push((
                integer(i64::try_from(index).unwrap_or(i64::MAX)),
                direct(Value::Dictionary(dictionary)),
            ));
        }
        previous = Some((label, source));
    }
    let tree = trees::build(rewriter, NUMS_KEY, pairs)?;
    rewriter.edit(catalog, |_, catalog| {
        catalog.insert(Name::new(PAGE_LABELS_KEY), tree);
        Ok(())
    })?;
    Ok(())
}

/// The label of the page at `index` of the source, or `None` when no
/// range covers it.
fn label_of(
    document: &crate::doc::Document,
    ranges: &[Range],
    index: i64,
) -> Result<Option<Label>, PdfError> {
    let at = ranges.partition_point(|range| range.first <= index);
    let Some(range) = at.checked_sub(1) else {
        return Ok(None);
    };
    let start = match ranges[range].dictionary.get(START_KEY) {
        Some(start) => match document.resolve(start)?.value() {
            Value::Integer(start) if *start >= 1 => *start,
            _ => {
                return Err(PdfError::InvalidEntry {
                    object: None,
                    key: "St",
                })
            }
        },
        None => DEFAULT_START,
    };
    Ok(Some(Label {
        range,
        number: start + (index - ranges[range].first),
    }))
}

/// `dictionary` with `St` set to `number`, taken out when it is the
/// default.
fn with_start(dictionary: &Dictionary, number: i64) -> Dictionary {
    let mut dictionary = dictionary.clone();
    if number == DEFAULT_START {
        dictionary.remove(START_KEY);
    } else {
        dictionary.insert(Name::new(START_KEY), integer(number));
    }
    dictionary
}

/// Whether the document has page labels.
pub(crate) fn has_labels(rewriter: &Rewriter<'_>, catalog: ObjectId) -> Result<bool, PdfError> {
    Ok(rewriter
        .read_dictionary(catalog)?
        .is_some_and(|catalog| catalog.get(PAGE_LABELS_KEY).is_some()))
}
