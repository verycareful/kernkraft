// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The structure tree of a tagged document that loses pages: ISO
//! 32000-1:2008, 14.7 (Tables 322 to 326).
//!
//! A structure element's `K` lists its children: marked-content
//! identifiers (integers), marked-content references (`MCR`), object
//! references (`OBJR`) and child elements. A content item belongs to the
//! page of the element that holds it, or to the `Pg` it gives itself
//! (Table 324); an item on a gone page leaves its element. An element that
//! had content and has none left leaves its parent, and the `ParentTree`
//! and `IDTree` forget it. The `ParentTree` also forgets the keys of the
//! gone pages (`StructParents`) and annotations (`StructParent`).

use std::collections::{HashMap, HashSet};

use super::references::Gone;
use super::rewrite::{direct, name, reference, Rewriter};
use super::trees::{self, NAMES_KEY, NUMS_KEY};
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

const STRUCT_TREE_ROOT_KEY: &[u8] = b"StructTreeRoot";
const K_KEY: &[u8] = b"K";
const PG_KEY: &[u8] = b"Pg";
const OBJ_KEY: &[u8] = b"Obj";
const TYPE_KEY: &[u8] = b"Type";
const PARENT_TREE_KEY: &[u8] = b"ParentTree";
const ID_TREE_KEY: &[u8] = b"IDTree";
const STRUCT_PARENTS_KEY: &[u8] = b"StructParents";
const STRUCT_PARENT_KEY: &[u8] = b"StructParent";
const MCR: &[u8] = b"MCR";
const OBJR: &[u8] = b"OBJR";
/// Deepest element nesting followed.
const MAX_DEPTH: usize = 256;

/// What the walk of the tree knows.
struct Pruner<'a> {
    gone: &'a Gone,
    /// The elements that left, and the verdict on each one visited (an
    /// element reached twice is not walked again).
    decided: HashMap<ObjectId, bool>,
    dropped: HashSet<ObjectId>,
}

/// Prunes the structure tree of the document for the pages and
/// annotations in `gone`. `page_keys` and `annotation_keys` are the
/// `ParentTree` keys of the gone pages and annotations.
pub(crate) fn prune(
    rewriter: &mut Rewriter<'_>,
    catalog: ObjectId,
    gone: &Gone,
) -> Result<(), PdfError> {
    let Some(root) = rewriter
        .read_dictionary(catalog)?
        .and_then(|catalog| catalog.get(STRUCT_TREE_ROOT_KEY).and_then(reference))
    else {
        return Ok(());
    };
    let mut keys = HashSet::new();
    for page in &gone.pages {
        if let Some(key) = integer_entry(rewriter, *page, STRUCT_PARENTS_KEY)? {
            keys.insert(key);
        }
    }
    for annotation in &gone.annotations {
        if let Some(key) = integer_entry(rewriter, *annotation, STRUCT_PARENT_KEY)? {
            keys.insert(key);
        }
    }
    let mut pruner = Pruner {
        gone,
        decided: HashMap::new(),
        dropped: HashSet::new(),
    };
    pruner.decided.insert(root, true);
    rewriter.edit(root, |rewriter, root_dictionary| {
        pruner.children(rewriter, root_dictionary, None, 0)?;
        if let Some(tree) = root_dictionary.get_mut(PARENT_TREE_KEY) {
            let dropped = &pruner.dropped;
            let emptied = trees::filter(rewriter, tree, NUMS_KEY, &mut |rewriter, key, value| {
                if matches!(key.value(), Value::Integer(key) if keys.contains(key)) {
                    return Ok(false);
                }
                keep_parents(rewriter, value, dropped)
            })?;
            if emptied {
                root_dictionary.remove(PARENT_TREE_KEY);
            }
        }
        if let Some(tree) = root_dictionary.get_mut(ID_TREE_KEY) {
            let dropped = &pruner.dropped;
            let emptied = trees::filter(rewriter, tree, NAMES_KEY, &mut |_, _, value| {
                Ok(!reference(value).is_some_and(|id| dropped.contains(&id)))
            })?;
            if emptied {
                root_dictionary.remove(ID_TREE_KEY);
            }
        }
        Ok(())
    })?;
    Ok(())
}

fn integer_entry(
    rewriter: &Rewriter<'_>,
    id: ObjectId,
    key: &[u8],
) -> Result<Option<i64>, PdfError> {
    let Some(dictionary) = rewriter.read_dictionary(id)? else {
        return Ok(None);
    };
    Ok(match dictionary.get(key).map(Object::value) {
        Some(Value::Integer(integer)) => Some(*integer),
        _ => None,
    })
}

/// Whether a `ParentTree` value stays: an array indexed by marked-content
/// identifier loses the elements that left (their places hold null) and
/// stays if it has any element; a single element stays unless it left.
fn keep_parents(
    rewriter: &mut Rewriter<'_>,
    value: &mut Object,
    dropped: &HashSet<ObjectId>,
) -> Result<bool, PdfError> {
    if reference(value).is_some_and(|id| dropped.contains(&id)) {
        return Ok(false);
    }
    let kept = rewriter.follow(value, |_, value| {
        let Some(items) = value.value_mut().as_array_mut() else {
            return Ok(true);
        };
        let mut any = false;
        for item in items.iter_mut() {
            if reference(item).is_some_and(|id| dropped.contains(&id)) {
                *item = direct(Value::Null);
            }
            any |= reference(item).is_some();
        }
        Ok(any)
    })?;
    Ok(kept.unwrap_or(true))
}

impl Pruner<'_> {
    /// Takes the content that is gone out of the `K` of `holder`, an
    /// element or the tree's root, whose own page is `page`. Returns
    /// whether `holder` had children and has none now.
    fn children(
        &mut self,
        rewriter: &mut Rewriter<'_>,
        holder: &mut Dictionary,
        page: Option<ObjectId>,
        depth: usize,
    ) -> Result<bool, PdfError> {
        if depth > MAX_DEPTH {
            return Err(PdfError::StructureTree {
                object: page.unwrap_or(ObjectId {
                    number: 0,
                    generation: 0,
                }),
            });
        }
        let Some(k) = holder.get_mut(K_KEY) else {
            return Ok(false);
        };
        let emptied = match k.value() {
            Value::Array(_) => {
                let Some(items) = k.value_mut().as_array_mut() else {
                    return Ok(false);
                };
                let before = items.len();
                let mut kept = Vec::with_capacity(before);
                for mut item in std::mem::take(items) {
                    if self.keep_child(rewriter, &mut item, page, depth)? {
                        kept.push(item);
                    }
                }
                *items = kept;
                before > 0 && items.is_empty()
            }
            _ => !self.keep_child(rewriter, k, page, depth)?,
        };
        if emptied {
            holder.remove(K_KEY);
        }
        Ok(emptied)
    }

    /// Whether the content item or child element `item` stays.
    fn keep_child(
        &mut self,
        rewriter: &mut Rewriter<'_>,
        item: &mut Object,
        page: Option<ObjectId>,
        depth: usize,
    ) -> Result<bool, PdfError> {
        match item.value() {
            Value::Integer(_) => Ok(!page.is_some_and(|page| self.gone.pages.contains(&page))),
            Value::Dictionary(_) => {
                let Some(dictionary) = item.value_mut().as_dictionary_mut() else {
                    return Ok(true);
                };
                self.keep_dictionary(rewriter, dictionary, None, page, depth)
            }
            Value::Reference(id) => {
                let id = *id;
                if let Some(&verdict) = self.decided.get(&id) {
                    return Ok(verdict);
                }
                let Some(mut dictionary) = rewriter.read_dictionary(id)? else {
                    return Ok(true);
                };
                let keep =
                    self.keep_dictionary(rewriter, &mut dictionary, Some(id), page, depth)?;
                rewriter.set(id, direct(Value::Dictionary(dictionary)))?;
                if !keep {
                    self.dropped.insert(id);
                }
                self.decided.insert(id, keep);
                Ok(keep)
            }
            _ => Ok(true),
        }
    }

    /// Whether the dictionary `dictionary` (object `id` when indirect)
    /// stays, as a content reference or an element; `parent_page` is the
    /// page of the element that holds it.
    fn keep_dictionary(
        &mut self,
        rewriter: &mut Rewriter<'_>,
        dictionary: &mut Dictionary,
        id: Option<ObjectId>,
        parent_page: Option<ObjectId>,
        depth: usize,
    ) -> Result<bool, PdfError> {
        let own_page = dictionary.get(PG_KEY).and_then(reference);
        let kind = dictionary.get(TYPE_KEY).and_then(name);
        if kind == Some(MCR) || kind == Some(OBJR) {
            let page = own_page.or(parent_page);
            if page.is_some_and(|page| self.gone.pages.contains(&page)) {
                return Ok(false);
            }
            if kind == Some(OBJR) {
                let object = dictionary.get(OBJ_KEY).and_then(reference);
                if object.is_some_and(|object| self.gone.annotations.contains(&object)) {
                    return Ok(false);
                }
            }
            return Ok(true);
        }
        let had_children = dictionary.get(K_KEY).is_some();
        if let Some(id) = id {
            // A cycle through this element ends here.
            self.decided.insert(id, true);
        }
        let emptied = self.children(rewriter, dictionary, own_page, depth + 1)?;
        if own_page.is_some_and(|page| self.gone.pages.contains(&page)) {
            dictionary.remove(PG_KEY);
        }
        Ok(!(had_children && emptied))
    }
}
