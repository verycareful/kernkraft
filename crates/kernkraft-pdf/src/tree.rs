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

use std::collections::{HashMap, HashSet, VecDeque};

use lopdf::{Dictionary, Document, Object, ObjectId};

use crate::fields::drop_fields_of;
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
    let (pages, _) = walk(doc, root)?;

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

/// The page ids in document order, as every operation numbers them. A
/// malformed tree (cyclic, a node reached twice, nested too deeply, a kid
/// that is not a dictionary) is [`PdfError::Parse`].
pub(crate) fn page_ids(doc: &Document) -> Result<Vec<ObjectId>, PdfError> {
    let (pages, _) = walk(doc, root_pages_id(doc)?)?;
    Ok(pages.into_iter().map(|(id, _)| id).collect())
}

/// [`page_ids`]'s count, for the operations that check page numbers
/// against it.
pub(crate) fn checked_count(doc: &Document) -> Result<u32, PdfError> {
    let n = page_ids(doc)?.len();
    u32::try_from(n).map_err(|_| PdfError::Invalid(format!("{n} pages")))
}

/// Checks the page tree and gives every node the `/Type` ISO 32000-1:2008
/// requires (`/Page` for a leaf, `/Pages` for a node with kids, Tables 29
/// and 30) where it is missing, so lopdf, MuPDF and the operations here
/// all see the same pages. Returns the page count.
pub(crate) fn check_and_type_pages(doc: &mut Document) -> Result<usize, PdfError> {
    let (pages, nodes) = walk(doc, root_pages_id(doc)?)?;
    let leaves: HashSet<ObjectId> = pages.iter().map(|(id, _)| *id).collect();
    for id in nodes {
        let dict = doc
            .get_dictionary_mut(id)
            .map_err(|e| PdfError::Parse(format!("page tree node {id:?}: {e}")))?;
        if !dict.has(b"Type") {
            let kind = if leaves.contains(&id) {
                "Page"
            } else {
                "Pages"
            };
            dict.set("Type", Object::Name(kind.as_bytes().to_vec()));
        }
    }
    Ok(pages.len())
}

/// A page id with the inheritable attributes its ancestors give it.
type InheritedPage = (ObjectId, Dictionary);

/// Walks the tree under `root`: the pages with their inherited attributes,
/// in document order, and every node visited (pages included).
fn walk(
    doc: &Document,
    root: ObjectId,
) -> Result<(Vec<InheritedPage>, HashSet<ObjectId>), PdfError> {
    let mut pages = Vec::new();
    let mut visited = HashSet::new();
    collect(doc, root, &Dictionary::new(), 0, &mut visited, &mut pages)?;
    Ok((pages, visited))
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
/// used (content streams, images, fonts) are pruned from the result. Their
/// form fields leave the form first (see [`drop_fields_of`]).
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
        drop_fields_of(doc, &dropped)?;
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
    pages: &mut Vec<InheritedPage>,
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

/// Gives every object a new id, consecutive from `start` in the order of
/// the old ids, generation 0, and rewrites every reference (objects and
/// trailer) to match. Returns the first id left free.
pub(crate) fn renumber(doc: &mut Document, start: u32) -> Result<u32, PdfError> {
    let overflow = || PdfError::Invalid("too many objects to renumber".to_string());
    let mut map: HashMap<ObjectId, ObjectId> = HashMap::with_capacity(doc.objects.len());
    let mut next = start;
    for &old in doc.objects.keys() {
        map.insert(old, (next, 0));
        next = next.checked_add(1).ok_or_else(overflow)?;
    }
    let objects = std::mem::take(&mut doc.objects);
    for (old, mut object) in objects {
        remap_references(&mut object, &map);
        doc.objects.insert(map[&old], object);
    }
    for (_, value) in doc.trailer.iter_mut() {
        remap_references(value, &map);
    }
    doc.max_id = next.saturating_sub(1);
    Ok(next)
}

/// Rewrites references through `map`. A reference to an object the
/// document does not hold has no new id; it becomes `null`, which is what
/// ISO 32000-1:2008, 7.3.10 says such a reference means.
fn remap_references(object: &mut Object, map: &HashMap<ObjectId, ObjectId>) {
    match object {
        Object::Reference(id) => match map.get(id) {
            Some(new) => *id = *new,
            None => *object = Object::Null,
        },
        Object::Array(items) => items.iter_mut().for_each(|o| remap_references(o, map)),
        Object::Dictionary(dict) => dict.iter_mut().for_each(|(_, o)| remap_references(o, map)),
        Object::Stream(stream) => stream
            .dict
            .iter_mut()
            .for_each(|(_, o)| remap_references(o, map)),
        _ => {}
    }
}
