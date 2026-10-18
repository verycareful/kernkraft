// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Interactive-form fields of pages that leave a document.
//!
//! A form field (ISO 32000-1:2008, 12.7.3) is shown by widget annotations,
//! each listed in its page's `/Annots`; the fields themselves hang off the
//! catalog's `/AcroForm /Fields` as a tree through `/Kids`, and `/CO` lists
//! the calculation order. A field merged with its only widget is one
//! dictionary. When a page is dropped, its widgets must go and so must
//! every field left with nothing to show, or the form keeps them, and their
//! appearance streams, alive.

use std::collections::HashSet;

use lopdf::{Document, Object, ObjectId};

use crate::PdfError;

/// Deepest field tree walked; real forms nest a few levels.
const MAX_FIELD_DEPTH: usize = 256;

/// Removes from the form every widget annotation on a page in `dropped`,
/// and every field whose widgets were all among them.
pub(crate) fn drop_fields_of(
    doc: &mut Document,
    dropped: &HashSet<ObjectId>,
) -> Result<(), PdfError> {
    let widgets = annotations_of(doc, dropped);
    let Some(form) = form_id(doc) else {
        return Ok(());
    };
    if widgets.is_empty() {
        return Ok(());
    }
    let roots = field_refs(doc, form.dict(doc), b"Fields");
    let mut removed = HashSet::new();
    let mut parents = Vec::new();
    let mut visited = HashSet::new();
    for id in roots {
        mark(
            doc,
            id,
            &widgets,
            0,
            &mut visited,
            &mut removed,
            &mut parents,
        )?;
    }
    if removed.is_empty() {
        return Ok(());
    }
    for id in parents {
        retain_refs(doc, Holder::Object(id), b"Kids", &removed);
    }
    for key in [&b"Fields"[..], b"CO"] {
        retain_refs(doc, form.holder(), key, &removed);
    }
    Ok(())
}

/// Where the `/AcroForm` dictionary lives: its own object, or directly in
/// the catalog.
#[derive(Clone, Copy)]
enum Form {
    Object(ObjectId),
    InCatalog,
}

impl Form {
    fn dict<'a>(&self, doc: &'a Document) -> Option<&'a lopdf::Dictionary> {
        match self {
            Form::Object(id) => doc.get_dictionary(*id).ok(),
            Form::InCatalog => doc.catalog().ok()?.get(b"AcroForm").ok()?.as_dict().ok(),
        }
    }

    fn holder(&self) -> Holder {
        match self {
            Form::Object(id) => Holder::Object(*id),
            Form::InCatalog => Holder::CatalogForm,
        }
    }
}

#[derive(Clone, Copy)]
enum Holder {
    Object(ObjectId),
    CatalogForm,
}

fn form_id(doc: &Document) -> Option<Form> {
    match doc.catalog().ok()?.get(b"AcroForm").ok()? {
        Object::Reference(id) => Some(Form::Object(*id)),
        Object::Dictionary(_) => Some(Form::InCatalog),
        _ => None,
    }
}

/// Ids of the annotations listed in the `/Annots` of each page in `pages`.
fn annotations_of(doc: &Document, pages: &HashSet<ObjectId>) -> HashSet<ObjectId> {
    pages
        .iter()
        .filter_map(|&page| doc.get_dictionary(page).ok())
        .flat_map(|page| field_refs(doc, Some(page), b"Annots"))
        .collect()
}

/// The references in array `key` of `dict`, the array given directly or
/// by reference.
fn field_refs(doc: &Document, dict: Option<&lopdf::Dictionary>, key: &[u8]) -> Vec<ObjectId> {
    let Some(value) = dict.and_then(|d| d.get(key).ok()) else {
        return Vec::new();
    };
    let array = match value {
        Object::Reference(id) => doc.get_object(*id).ok().and_then(|o| o.as_array().ok()),
        other => other.as_array().ok(),
    };
    array
        .map(|items| items.iter().filter_map(|o| o.as_reference().ok()).collect())
        .unwrap_or_default()
}

/// Marks field `id` removed when it is one of `widgets`, or when it had
/// kids and every one of them is removed. Records each field with kids in
/// `parents`, whose `/Kids` the caller then filters.
fn mark(
    doc: &Document,
    id: ObjectId,
    widgets: &HashSet<ObjectId>,
    depth: usize,
    visited: &mut HashSet<ObjectId>,
    removed: &mut HashSet<ObjectId>,
    parents: &mut Vec<ObjectId>,
) -> Result<bool, PdfError> {
    if depth > MAX_FIELD_DEPTH {
        return Err(PdfError::Parse(
            "form field tree nested too deeply".to_string(),
        ));
    }
    if !visited.insert(id) {
        return Ok(removed.contains(&id));
    }
    if widgets.contains(&id) {
        removed.insert(id);
        return Ok(true);
    }
    let kids = field_refs(doc, doc.get_dictionary(id).ok(), b"Kids");
    if kids.is_empty() {
        return Ok(false);
    }
    parents.push(id);
    let mut all_gone = true;
    for kid in kids {
        all_gone &= mark(doc, kid, widgets, depth + 1, visited, removed, parents)?;
    }
    if all_gone {
        removed.insert(id);
    }
    Ok(all_gone)
}

/// Drops every reference to a `removed` id from array `key` of the
/// dictionary `holder` names, following the array through one reference.
fn retain_refs(doc: &mut Document, holder: Holder, key: &[u8], removed: &HashSet<ObjectId>) {
    let dict = match holder {
        Holder::Object(id) => doc.get_dictionary(id).ok(),
        Holder::CatalogForm => doc
            .catalog()
            .ok()
            .and_then(|c| c.get(b"AcroForm").ok())
            .and_then(|f| f.as_dict().ok()),
    };
    let target = match dict.and_then(|d| d.get(key).ok()) {
        Some(Object::Reference(array)) => Some(*array),
        Some(_) => None,
        None => return,
    };
    let keep = |o: &Object| !o.as_reference().is_ok_and(|r| removed.contains(&r));
    let array = match target {
        Some(array) => doc.get_object_mut(array).ok(),
        None => match holder {
            Holder::Object(id) => doc
                .get_dictionary_mut(id)
                .ok()
                .and_then(|d| d.get_mut(key).ok()),
            Holder::CatalogForm => doc
                .catalog_mut()
                .ok()
                .and_then(|c| c.get_mut(b"AcroForm").ok())
                .and_then(|f| f.as_dict_mut().ok())
                .and_then(|d| d.get_mut(key).ok()),
        },
    };
    if let Some(Object::Array(items)) = array {
        items.retain(keep);
    }
}
