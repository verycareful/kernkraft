// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page-tree flattening, the first step of every page operation.
//!
//! ISO 32000-1:2008, 7.7.3.4 (Table 30) makes four page attributes
//! inheritable: `Resources`, `MediaBox`, `CropBox` and `Rotate`. A page
//! that lacks one takes it from its nearest ancestor `Pages` node that has
//! it. Re-parenting such a page onto another node silently changes it, so
//! flattening first copies every inherited value onto the page itself, then
//! hangs all pages directly off the root `Pages` node. After that, any
//! reordering of the root's `Kids` keeps every page as it was.

use std::collections::{HashSet, VecDeque};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::PdfError;

/// The inheritable page attributes (ISO 32000-1:2008, Table 30).
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];

/// Deepest page tree walked. Real files nest a handful of levels; the bound
/// stops a malicious tree from exhausting the stack.
const MAX_TREE_DEPTH: usize = 256;

/// Flattens the page tree in place: every page carries its inherited
/// attributes and is a direct kid of the root `Pages` node; the
/// intermediate nodes, now unreferenced, are removed. Page order is kept.
pub fn flatten_page_tree(doc: &mut Document) -> Result<(), PdfError> {
    let (root, pages) = flatten(doc)?;
    set_pages(doc, root, &pages)
}

/// Flattens the tree and returns the root `Pages` id and the page ids in
/// document order. The root's `Kids` and `Count` are set to all pages;
/// nothing is pruned yet.
pub(crate) fn flatten(doc: &mut Document) -> Result<(ObjectId, Vec<ObjectId>), PdfError> {
    let root = root_pages_id(doc)?;
    let mut pages = Vec::new();
    let mut visited = HashSet::new();
    collect(doc, root, &Dictionary::new(), 0, &mut visited, &mut pages)?;

    for (id, inherited) in &pages {
        let page = doc
            .get_dictionary_mut(*id)
            .map_err(|e| PdfError::Parse(format!("page {id:?}: {e}")))?;
        for key in INHERITABLE {
            if !page.has(key) {
                if let Ok(value) = inherited.get(key) {
                    page.set(key.to_vec(), value.clone());
                }
            }
        }
        page.set("Parent", Object::Reference(root));
    }
    let ids: Vec<ObjectId> = pages.into_iter().map(|(id, _)| id).collect();
    write_kids(doc, root, &ids)?;
    Ok((root, ids))
}

/// Makes `pages` the root's kids, in that order, and drops every object no
/// longer reachable from the trailer.
pub(crate) fn set_pages(
    doc: &mut Document,
    root: ObjectId,
    pages: &[ObjectId],
) -> Result<(), PdfError> {
    write_kids(doc, root, pages)?;
    prune(doc);
    Ok(())
}

/// Keeps only `keep` (page ids from [`flatten`], in the order given) and
/// unlinks every other page: each reference to a dropped page, wherever it
/// sits (a link annotation's destination, a structure element, the open
/// action), becomes `null`, so the dropped pages and everything only they
/// used (content streams, images, fonts) are pruned from the result.
pub(crate) fn keep_pages(
    doc: &mut Document,
    root: ObjectId,
    all: &[ObjectId],
    keep: &[ObjectId],
) -> Result<(), PdfError> {
    let kept: HashSet<ObjectId> = keep.iter().copied().collect();
    let dropped: HashSet<ObjectId> = all
        .iter()
        .copied()
        .filter(|id| !kept.contains(id))
        .collect();
    if !dropped.is_empty() {
        for object in doc.objects.values_mut() {
            null_references(object, &dropped);
        }
        for (_, value) in doc.trailer.iter_mut() {
            null_references(value, &dropped);
        }
    }
    set_pages(doc, root, keep)
}

fn null_references(object: &mut Object, dropped: &HashSet<ObjectId>) {
    match object {
        Object::Reference(id) if dropped.contains(id) => *object = Object::Null,
        Object::Array(items) => items.iter_mut().for_each(|o| null_references(o, dropped)),
        Object::Dictionary(dict) => dict
            .iter_mut()
            .for_each(|(_, o)| null_references(o, dropped)),
        Object::Stream(stream) => stream
            .dict
            .iter_mut()
            .for_each(|(_, o)| null_references(o, dropped)),
        _ => {}
    }
}

/// Removes every object not reachable from the trailer. Same result as
/// `lopdf::Document::prune_objects`, in linear time.
pub(crate) fn prune(doc: &mut Document) {
    let mut reachable: HashSet<ObjectId> = HashSet::new();
    let mut queue: VecDeque<ObjectId> = VecDeque::new();
    let push_refs = |object: &Object, queue: &mut VecDeque<ObjectId>| {
        let mut stack = vec![object];
        while let Some(o) = stack.pop() {
            match o {
                Object::Reference(id) => queue.push_back(*id),
                Object::Array(items) => stack.extend(items.iter()),
                Object::Dictionary(dict) => stack.extend(dict.iter().map(|(_, v)| v)),
                Object::Stream(stream) => stack.extend(stream.dict.iter().map(|(_, v)| v)),
                _ => {}
            }
        }
    };
    for (_, value) in doc.trailer.iter() {
        push_refs(value, &mut queue);
    }
    while let Some(id) = queue.pop_front() {
        if reachable.insert(id) {
            if let Some(object) = doc.objects.get(&id) {
                push_refs(object, &mut queue);
            }
        }
    }
    doc.objects.retain(|id, _| reachable.contains(id));
}

fn write_kids(doc: &mut Document, root: ObjectId, pages: &[ObjectId]) -> Result<(), PdfError> {
    let dict = doc
        .get_dictionary_mut(root)
        .map_err(|e| PdfError::Parse(format!("page tree root: {e}")))?;
    dict.set(
        "Kids",
        pages
            .iter()
            .map(|&id| Object::Reference(id))
            .collect::<Vec<_>>(),
    );
    dict.set("Count", pages.len() as i64);
    Ok(())
}

pub(crate) fn root_pages_id(doc: &Document) -> Result<ObjectId, PdfError> {
    doc.catalog()
        .and_then(|c| c.get(b"Pages"))
        .and_then(Object::as_reference)
        .map_err(|e| PdfError::Parse(format!("no page tree: {e}")))
}

/// Depth-first walk collecting `(page id, inherited attributes)` in
/// document order. `inherited` holds the nearest ancestor value of each
/// inheritable key. A node reached twice is a malformed (cyclic or shared)
/// tree and is an error rather than a duplicated page.
fn collect(
    doc: &Document,
    node_id: ObjectId,
    inherited: &Dictionary,
    depth: usize,
    visited: &mut HashSet<ObjectId>,
    pages: &mut Vec<(ObjectId, Dictionary)>,
) -> Result<(), PdfError> {
    if depth > MAX_TREE_DEPTH {
        return Err(PdfError::Parse("page tree nested too deeply".to_string()));
    }
    if !visited.insert(node_id) {
        return Err(PdfError::Parse(format!(
            "page tree reaches object {} {} twice",
            node_id.0, node_id.1
        )));
    }
    let node = doc
        .get_dictionary(node_id)
        .map_err(|e| PdfError::Parse(format!("page tree node {node_id:?}: {e}")))?;
    let is_leaf = match node.get(b"Type").and_then(Object::as_name) {
        Ok(b"Page") => true,
        Ok(b"Pages") => false,
        _ => !node.has(b"Kids"),
    };
    if is_leaf {
        pages.push((node_id, inherited.clone()));
        return Ok(());
    }

    let mut here = inherited.clone();
    for key in INHERITABLE {
        if let Ok(value) = node.get(key) {
            here.set(key.to_vec(), value.clone());
        }
    }
    let kids = node
        .get(b"Kids")
        .and_then(|k| match k {
            Object::Reference(r) => doc.get_object(*r).and_then(Object::as_array),
            other => other.as_array(),
        })
        .map_err(|e| PdfError::Parse(format!("page tree node {node_id:?} kids: {e}")))?;
    for kid in kids {
        let kid_id = kid
            .as_reference()
            .map_err(|e| PdfError::Parse(format!("page tree kid: {e}")))?;
        collect(doc, kid_id, &here, depth + 1, visited, pages)?;
    }
    Ok(())
}
