// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Rewriting name trees and number trees: ISO 32000-1:2008, 7.9.6 and
//! 7.9.7 (Tables 36 and 37).
//!
//! A tree is filtered in place, keeping its shape: the pairs a test
//! refuses leave their leaf, a node left with nothing leaves its parent,
//! and the `Limits` of the nodes that stay are made to say what is in
//! them. A tree is built from sorted pairs as a balanced tree of nodes of
//! at most [`NODE_WIDTH`] entries.

use std::collections::HashSet;

use super::rewrite::{direct, reference, reference_object, Rewriter};
use crate::syntax::{Name, Object, Value};
use crate::PdfError;

const KIDS_KEY: &[u8] = b"Kids";
const LIMITS_KEY: &[u8] = b"Limits";
/// The `Names` array of a name tree leaf.
pub(crate) const NAMES_KEY: &[u8] = b"Names";
/// The `Nums` array of a number tree leaf.
pub(crate) const NUMS_KEY: &[u8] = b"Nums";

/// The most pairs in a leaf and the most kids in an inner node of a tree
/// this module builds.
const NODE_WIDTH: usize = 64;
/// Deepest tree walked; the trees of real files are a few levels deep.
const MAX_DEPTH: usize = 256;

/// The least and greatest key under a node.
type Limits = (Object, Object);

/// Keeps the pairs of the tree whose root is `root` that `keep` accepts,
/// given the key and the value. Returns whether the tree is empty
/// afterwards. A node reached twice or nested too deeply is
/// [`PdfError::InvalidTree`].
pub(crate) fn filter(
    rewriter: &mut Rewriter<'_>,
    root: &mut Object,
    leaf_key: &'static [u8],
    keep: &mut dyn FnMut(&mut Rewriter<'_>, &Object, &mut Object) -> Result<bool, PdfError>,
) -> Result<bool, PdfError> {
    let mut seen = HashSet::new();
    let limits = filter_node(rewriter, root, leaf_key, keep, 0, &mut seen)?;
    if let Some(root) = root.value_mut().as_dictionary_mut() {
        root.remove(LIMITS_KEY);
    }
    Ok(limits.is_none())
}

fn filter_node(
    rewriter: &mut Rewriter<'_>,
    node: &mut Object,
    leaf_key: &'static [u8],
    keep: &mut dyn FnMut(&mut Rewriter<'_>, &Object, &mut Object) -> Result<bool, PdfError>,
    depth: usize,
    seen: &mut HashSet<crate::syntax::ObjectId>,
) -> Result<Option<Limits>, PdfError> {
    let id = reference(node);
    if depth > MAX_DEPTH || id.is_some_and(|id| !seen.insert(id)) {
        return Err(PdfError::InvalidTree { object: id });
    }
    let document = rewriter.document;
    let result = rewriter.follow(node, |rewriter, node| {
        let Some(dict) = node.value_mut().as_dictionary_mut() else {
            return Ok(None);
        };
        let mut low: Option<Object> = None;
        let mut high: Option<Object> = None;
        if let Some(entry) = dict.get_mut(leaf_key) {
            let kept = rewriter.follow(entry, |rewriter, entry| {
                let Some(items) = entry.value_mut().as_array_mut() else {
                    return Err(PdfError::InvalidTree { object: id });
                };
                if items.len() % 2 != 0 {
                    return Err(PdfError::InvalidTree { object: id });
                }
                let mut kept: Vec<Object> = Vec::with_capacity(items.len());
                for pair in items.as_chunks_mut::<2>().0 {
                    let key = document.resolve(&pair[0])?.object().clone();
                    if keep(rewriter, &key, &mut pair[1])? {
                        kept.extend_from_slice(pair);
                    }
                }
                let keys: Vec<Object> = kept
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|pair| pair[0].clone())
                    .collect();
                *items = kept;
                Ok(keys)
            })?;
            if let Some(keys) = kept {
                low = keys.first().cloned();
                high = keys.last().cloned();
            }
        }
        if let Some(entry) = dict.get_mut(KIDS_KEY) {
            let mut kid_limits: Vec<Limits> = Vec::new();
            rewriter.follow(entry, |rewriter, entry| {
                let Some(kids) = entry.value_mut().as_array_mut() else {
                    return Err(PdfError::InvalidTree { object: id });
                };
                let mut kept = Vec::with_capacity(kids.len());
                for mut kid in std::mem::take(kids) {
                    if let Some(limits) =
                        filter_node(rewriter, &mut kid, leaf_key, keep, depth + 1, seen)?
                    {
                        kid_limits.push(limits);
                        kept.push(kid);
                    }
                }
                *kids = kept;
                Ok(())
            })?;
            if let (Some(first), Some(last)) = (kid_limits.first(), kid_limits.last()) {
                if low.is_none() {
                    low = Some(first.0.clone());
                }
                high = Some(last.1.clone());
            }
        }
        match (low, high) {
            (Some(low), Some(high)) => {
                dict.insert(
                    Name::new(LIMITS_KEY),
                    direct(Value::Array(vec![low.clone(), high.clone()])),
                );
                Ok(Some((low, high)))
            }
            _ => Ok(None),
        }
    })?;
    Ok(result.flatten())
}

/// A tree of `pairs`, which are in the order of their keys, as a root
/// node: one leaf when they fit in one, else a balanced tree whose
/// inner nodes are objects of their own.
pub(crate) fn build(
    rewriter: &mut Rewriter<'_>,
    leaf_key: &'static [u8],
    pairs: Vec<(Object, Object)>,
) -> Result<Object, PdfError> {
    let mut level: Vec<(Object, Limits)> = Vec::new();
    for chunk in pairs.chunks(NODE_WIDTH) {
        let mut items = Vec::with_capacity(chunk.len() * 2);
        for (key, value) in chunk {
            items.push(key.clone());
            items.push(value.clone());
        }
        let limits = (chunk[0].0.clone(), chunk[chunk.len() - 1].0.clone());
        let mut node = crate::syntax::Dictionary::new();
        node.insert(Name::new(leaf_key), direct(Value::Array(items)));
        level.push((direct(Value::Dictionary(node)), limits));
    }
    if level.len() <= 1 {
        return Ok(level
            .pop()
            .map_or_else(|| empty(leaf_key), |(node, _)| node));
    }
    loop {
        let mut next: Vec<(Object, Limits)> = Vec::new();
        let last_level = level.len() <= NODE_WIDTH;
        for chunk in level.chunks(NODE_WIDTH) {
            let mut kids = Vec::with_capacity(chunk.len());
            for (node, limits) in chunk {
                let mut node = node.clone();
                if let Some(dict) = node.value_mut().as_dictionary_mut() {
                    dict.insert(
                        Name::new(LIMITS_KEY),
                        direct(Value::Array(vec![limits.0.clone(), limits.1.clone()])),
                    );
                }
                kids.push(reference_object(rewriter.add(node)?));
            }
            let limits = (chunk[0].1 .0.clone(), chunk[chunk.len() - 1].1 .1.clone());
            let mut node = crate::syntax::Dictionary::new();
            node.insert(Name::new(KIDS_KEY), direct(Value::Array(kids)));
            next.push((direct(Value::Dictionary(node)), limits));
        }
        if last_level {
            let (root, _) = next.remove(0);
            return Ok(root);
        }
        level = next;
    }
}

/// A tree with nothing in it.
fn empty(leaf_key: &'static [u8]) -> Object {
    let mut node = crate::syntax::Dictionary::new();
    node.insert(Name::new(leaf_key), direct(Value::Array(Vec::new())));
    direct(Value::Dictionary(node))
}
