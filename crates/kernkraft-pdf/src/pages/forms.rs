// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The interactive form of a document that loses pages: ISO 32000-1:2008,
//! 12.7.3.
//!
//! A field is shown by widget annotations, each listed in its page's
//! `Annots`. The fields hang off the form's `Fields` as a tree through
//! `Kids`, and `CO` lists the calculation order. A field merged with its
//! only widget is one dictionary. The widgets of a gone page leave the
//! form, and so does every field left with nothing to show, or the form
//! would keep them and their appearance streams alive.

use std::collections::HashSet;

use super::references::Gone;
use super::rewrite::{reference, Rewriter};
use crate::syntax::{Object, ObjectId};
use crate::PdfError;

const ACRO_FORM_KEY: &[u8] = b"AcroForm";
const FIELDS_KEY: &[u8] = b"Fields";
const CALCULATION_ORDER_KEY: &[u8] = b"CO";
const KIDS_KEY: &[u8] = b"Kids";
/// Deepest field tree walked; real forms nest a few levels.
const MAX_DEPTH: usize = 256;

/// Takes the widgets of the pages in `gone` out of the form, and the
/// fields that have none left.
pub(crate) fn prune(
    rewriter: &mut Rewriter<'_>,
    catalog: ObjectId,
    gone: &Gone,
) -> Result<(), PdfError> {
    if gone.annotations.is_empty() {
        return Ok(());
    }
    rewriter.edit(catalog, |rewriter, catalog| {
        let Some(form) = catalog.get_mut(ACRO_FORM_KEY) else {
            return Ok(());
        };
        rewriter.follow(form, |rewriter, form| {
            let Some(form) = form.value_mut().as_dictionary_mut() else {
                return Ok(());
            };
            let mut removed = HashSet::new();
            let mut visited = HashSet::new();
            if let Some(fields) = form.get_mut(FIELDS_KEY) {
                let roots: Vec<Object> = rewriter
                    .follow(fields, |_, fields| {
                        Ok(super::rewrite::array(fields).map(<[Object]>::to_vec))
                    })?
                    .flatten()
                    .unwrap_or_default();
                for root in &roots {
                    if let Some(id) = reference(root) {
                        mark(rewriter, id, gone, 0, &mut visited, &mut removed)?;
                    }
                }
            }
            for key in [FIELDS_KEY, CALCULATION_ORDER_KEY] {
                if let Some(list) = form.get_mut(key) {
                    rewriter.follow(list, |_, list| {
                        if let Some(items) = list.value_mut().as_array_mut() {
                            items.retain(|item| {
                                !reference(item).is_some_and(|id| removed.contains(&id))
                            });
                        }
                        Ok(())
                    })?;
                }
            }
            Ok(())
        })?;
        Ok(())
    })?;
    Ok(())
}

/// Whether field `id` goes: it is a widget of a gone page, or it had kids
/// and all of them go. Fields whose kids partly go lose those kids.
fn mark(
    rewriter: &mut Rewriter<'_>,
    id: ObjectId,
    gone: &Gone,
    depth: usize,
    visited: &mut HashSet<ObjectId>,
    removed: &mut HashSet<ObjectId>,
) -> Result<bool, PdfError> {
    if depth > MAX_DEPTH {
        return Err(PdfError::InvalidTree { object: Some(id) });
    }
    if !visited.insert(id) {
        return Ok(removed.contains(&id));
    }
    if gone.annotations.contains(&id) {
        removed.insert(id);
        return Ok(true);
    }
    let Some(field) = rewriter.read_dictionary(id)? else {
        return Ok(false);
    };
    let kids: Vec<Object> = match field.get(KIDS_KEY) {
        Some(kids) => {
            let mut kids = kids.clone();
            rewriter
                .follow(&mut kids, |_, kids| {
                    Ok(super::rewrite::array(kids).map(<[Object]>::to_vec))
                })?
                .flatten()
                .unwrap_or_default()
        }
        None => Vec::new(),
    };
    if kids.is_empty() {
        return Ok(false);
    }
    let mut all_gone = true;
    let mut dropped_any = false;
    for kid in &kids {
        let Some(kid_id) = reference(kid) else {
            all_gone = false;
            continue;
        };
        if mark(rewriter, kid_id, gone, depth + 1, visited, removed)? {
            dropped_any = true;
        } else {
            all_gone = false;
        }
    }
    if all_gone {
        removed.insert(id);
        return Ok(true);
    }
    if dropped_any {
        rewriter.edit(id, |rewriter, field| {
            if let Some(list) = field.get_mut(KIDS_KEY) {
                rewriter.follow(list, |_, list| {
                    if let Some(items) = list.value_mut().as_array_mut() {
                        items.retain(|item| {
                            !reference(item).is_some_and(|kid| removed.contains(&kid))
                        });
                    }
                    Ok(())
                })?;
            }
            Ok(())
        })?;
    }
    Ok(false)
}
