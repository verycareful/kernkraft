// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Named destinations of a document that loses pages: the catalog's
//! `Dests` dictionary (PDF 1.1) and the `Dests` name tree of the name
//! dictionary (ISO 32000-1:2008, 12.3.2.3, 7.7.4).
//!
//! A name whose destination is a gone page is taken out.

use std::collections::HashSet;

use super::rewrite::{reference, Rewriter};
use super::trees::{self, NAMES_KEY};
use crate::doc::Document;
use crate::syntax::{Object, ObjectId};
use crate::PdfError;

const DESTS_KEY: &[u8] = b"Dests";
const NAMES_KEY_OF_CATALOG: &[u8] = b"Names";
const D_KEY: &[u8] = b"D";

/// Takes out of the document's named destinations those that lead to a
/// page in `gone`.
pub(crate) fn prune(
    rewriter: &mut Rewriter<'_>,
    catalog: ObjectId,
    gone: &HashSet<ObjectId>,
) -> Result<(), PdfError> {
    rewriter.edit(catalog, |rewriter, catalog| {
        if let Some(dests) = catalog.get_mut(DESTS_KEY) {
            rewriter.follow(dests, |rewriter, dests| {
                let document = rewriter.document;
                let Some(entries) = dests.value_mut().as_dictionary_mut() else {
                    return Ok(());
                };
                let mut doomed = Vec::new();
                for (name, value) in entries.iter() {
                    if leads_to(document, value, gone)? {
                        doomed.push(name.clone());
                    }
                }
                for name in doomed {
                    entries.remove(name.as_bytes());
                }
                Ok(())
            })?;
        }
        if let Some(names) = catalog.get_mut(NAMES_KEY_OF_CATALOG) {
            rewriter.follow(names, |rewriter, names| {
                let Some(names) = names.value_mut().as_dictionary_mut() else {
                    return Ok(());
                };
                let Some(tree) = names.get_mut(DESTS_KEY) else {
                    return Ok(());
                };
                let emptied =
                    trees::filter(rewriter, tree, NAMES_KEY, &mut |rewriter, _, value| {
                        Ok(!leads_to(rewriter.document, value, gone)?)
                    })?;
                if emptied {
                    names.remove(DESTS_KEY);
                }
                Ok(())
            })?;
        }
        Ok(())
    })?;
    Ok(())
}

/// Whether the destination `value` names (an array, or a dictionary whose
/// `D` is one) has a page in `gone`.
pub(crate) fn leads_to(
    document: &Document,
    value: &Object,
    gone: &HashSet<ObjectId>,
) -> Result<bool, PdfError> {
    let resolved = document.resolve(value)?;
    let target = if let Some(dest) = resolved.dictionary() {
        document.get(dest, D_KEY)?.array().map(<[Object]>::to_vec)
    } else {
        resolved.array().map(<[Object]>::to_vec)
    };
    Ok(target
        .as_deref()
        .and_then(<[Object]>::first)
        .and_then(reference)
        .is_some_and(|page| gone.contains(&page)))
}
