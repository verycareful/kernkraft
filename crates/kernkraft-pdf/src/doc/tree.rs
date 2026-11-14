// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name trees (ISO 32000-1:2008, 7.9.6, Table 36) and number trees
//! (7.9.7, Table 37).
//!
//! A node has `Kids`, references to its children, or `Names` (`Nums` in
//! a number tree), its keys and values in pairs; every node below the root
//! has `Limits`, its least and greatest key. A lookup skips a child whose
//! `Limits` leave the key out and searches every other one, so a tree
//! whose `Limits` are missing is still searched whole. Name keys compare
//! byte by byte, a shorter key before a longer one it starts (7.9.6), as
//! byte slices compare.
//!
//! A node reached twice, a node that is not a dictionary, or a pair whose
//! key is not of the tree's key type is [`PdfError::InvalidTree`].

use std::collections::HashSet;

use super::{Document, Resolved};
use crate::syntax::{Object, ObjectId, Value};
use crate::PdfError;

const KIDS_KEY: &[u8] = b"Kids";
const LIMITS_KEY: &[u8] = b"Limits";
const NAMES_KEY: &[u8] = b"Names";
const NUMS_KEY: &[u8] = b"Nums";

/// What tells a name tree from a number tree.
trait Keys {
    type Key: Ord + Clone;
    const LEAF_KEY: &'static [u8];
    fn key(value: &Value) -> Option<Self::Key>;
}

struct Strings;
impl Keys for Strings {
    type Key = Vec<u8>;
    const LEAF_KEY: &'static [u8] = NAMES_KEY;
    fn key(value: &Value) -> Option<Vec<u8>> {
        match value {
            Value::String(string) => Some(string.as_bytes().to_vec()),
            _ => None,
        }
    }
}

struct Integers;
impl Keys for Integers {
    type Key = i64;
    const LEAF_KEY: &'static [u8] = NUMS_KEY;
    fn key(value: &Value) -> Option<i64> {
        match value {
            Value::Integer(integer) => Some(*integer),
            _ => None,
        }
    }
}

/// A name tree of a document, from its root node.
#[derive(Debug, Clone)]
pub struct NameTree<'d> {
    document: &'d Document,
    root: Object,
}

/// A number tree of a document, from its root node.
#[derive(Debug, Clone)]
pub struct NumberTree<'d> {
    document: &'d Document,
    root: Object,
}

impl<'d> NameTree<'d> {
    /// The tree whose root is `root`: the node dictionary, or a reference
    /// to it.
    pub fn new(document: &'d Document, root: Object) -> Self {
        Self { document, root }
    }

    /// The value of `key`, as the tree holds it: maybe a reference.
    pub fn get(&self, key: &[u8]) -> Result<Option<Object>, PdfError> {
        lookup::<Strings>(self.document, &self.root, &key.to_vec())
    }

    /// Every key and value, in the tree's order.
    pub fn entries(&self) -> Result<Vec<(Vec<u8>, Object)>, PdfError> {
        entries::<Strings>(self.document, &self.root)
    }
}

impl<'d> NumberTree<'d> {
    pub fn new(document: &'d Document, root: Object) -> Self {
        Self { document, root }
    }

    pub fn get(&self, key: i64) -> Result<Option<Object>, PdfError> {
        lookup::<Integers>(self.document, &self.root, &key)
    }

    pub fn entries(&self) -> Result<Vec<(i64, Object)>, PdfError> {
        entries::<Integers>(self.document, &self.root)
    }
}

/// A node, followed from `object`, and its object number when it was a
/// reference.
fn node<'a>(
    document: &Document,
    object: &'a Object,
    seen: &mut HashSet<ObjectId>,
) -> Result<Resolved<'a>, PdfError> {
    let resolved = document.resolve(object)?;
    if let Some(id) = resolved.id() {
        if !seen.insert(id) {
            return Err(PdfError::InvalidTree { object: Some(id) });
        }
    }
    if resolved.dictionary().is_none() {
        return Err(PdfError::InvalidTree {
            object: resolved.id(),
        });
    }
    Ok(resolved)
}

/// The keys and values of a leaf, in order.
type Pairs<K> = Vec<(<K as Keys>::Key, Object)>;

/// The pairs of a leaf's `Names` or `Nums` array.
fn pairs<K: Keys>(document: &Document, node: &Resolved<'_>) -> Result<Option<Pairs<K>>, PdfError> {
    let Some(dictionary) = node.dictionary() else {
        return Ok(None);
    };
    let leaf = document.get(dictionary, K::LEAF_KEY)?;
    if matches!(leaf, Resolved::Null) {
        return Ok(None);
    }
    let bad = || PdfError::InvalidTree { object: node.id() };
    let items = leaf.array().ok_or_else(bad)?;
    if items.len() % 2 != 0 {
        return Err(bad());
    }
    let mut pairs = Vec::with_capacity(items.len() / 2);
    for [key, value] in items.as_chunks::<2>().0 {
        let key = K::key(document.resolve(key)?.value()).ok_or_else(bad)?;
        pairs.push((key, value.clone()));
    }
    Ok(Some(pairs))
}

fn kids(document: &Document, node: &Resolved<'_>) -> Result<Vec<Object>, PdfError> {
    let Some(dictionary) = node.dictionary() else {
        return Ok(Vec::new());
    };
    let kids = document.get(dictionary, KIDS_KEY)?;
    match kids {
        Resolved::Null => Ok(Vec::new()),
        _ => kids
            .array()
            .map(<[Object]>::to_vec)
            .ok_or(PdfError::InvalidTree { object: node.id() }),
    }
}

/// Whether `key` may lie under a node, by its `Limits`: yes when the node
/// has none that can be read.
fn may_hold<K: Keys>(
    document: &Document,
    node: &Resolved<'_>,
    key: &K::Key,
) -> Result<bool, PdfError> {
    let Some(dictionary) = node.dictionary() else {
        return Ok(true);
    };
    let limits = document.get(dictionary, LIMITS_KEY)?;
    let Some([low, high]) = limits.array() else {
        return Ok(true);
    };
    let low = K::key(document.resolve(low)?.value());
    let high = K::key(document.resolve(high)?.value());
    Ok(match (low, high) {
        (Some(low), Some(high)) => low <= *key && *key <= high,
        _ => true,
    })
}

fn lookup<K: Keys>(
    document: &Document,
    root: &Object,
    key: &K::Key,
) -> Result<Option<Object>, PdfError> {
    let mut seen = HashSet::new();
    let mut stack = vec![root.clone()];
    while let Some(object) = stack.pop() {
        let node = node(document, &object, &mut seen)?;
        if let Some(pairs) = pairs::<K>(document, &node)? {
            if let Some((_, value)) = pairs.into_iter().find(|(found, _)| found == key) {
                return Ok(Some(value));
            }
        }
        for kid in kids(document, &node)?.into_iter().rev() {
            let child = document.resolve(&kid)?;
            if may_hold::<K>(document, &child, key)? {
                stack.push(kid);
            }
        }
    }
    Ok(None)
}

fn entries<K: Keys>(document: &Document, root: &Object) -> Result<Vec<(K::Key, Object)>, PdfError> {
    let mut seen = HashSet::new();
    let mut stack = vec![root.clone()];
    let mut out = Vec::new();
    while let Some(object) = stack.pop() {
        let node = node(document, &object, &mut seen)?;
        if let Some(pairs) = pairs::<K>(document, &node)? {
            out.extend(pairs);
        }
        stack.extend(kids(document, &node)?.into_iter().rev());
    }
    Ok(out)
}
