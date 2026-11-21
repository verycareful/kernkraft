// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! References to pages that leave a document.
//!
//! A page is referred to from many places besides the page tree: a link's
//! destination, an outline item, a structure element, a form field's
//! widget, the open action, an article's bead. The structures a reader
//! shows (outline, named destinations, form, structure tree, page labels)
//! are rewritten by their own modules; what is left, a reference to a
//! page that is gone, is made null, which is what ISO 32000-1:2008,
//! 7.3.10 says a reference to a missing object means. A destination or
//! go-to action that is written directly and leads to a gone page is
//! taken out instead, so that the link has no destination rather than a
//! broken one.

use std::collections::{HashMap, HashSet};

use kernkraft_fs::NoProgress;

use super::rewrite::{array, dictionary, direct, name, reference, Rewriter};
use crate::doc::{Content, Document};
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::write::reachable;
use crate::PdfError;

const DEST_KEY: &[u8] = b"Dest";
const OPEN_ACTION_KEY: &[u8] = b"OpenAction";
const ACTION_KEY: &[u8] = b"A";
const NEXT_KEY: &[u8] = b"Next";
const SUBTYPE_KEY: &[u8] = b"S";
const GO_TO: &[u8] = b"GoTo";
const D_KEY: &[u8] = b"D";

/// Entries whose value, written directly, may be a destination.
const DESTINATION_KEYS: [&[u8]; 2] = [DEST_KEY, OPEN_ACTION_KEY];

/// The pages that leave, and what lies on them.
pub(crate) struct Gone {
    pub(crate) pages: HashSet<ObjectId>,
    /// The annotations listed in the `Annots` of the pages.
    pub(crate) annotations: HashSet<ObjectId>,
}

/// Which objects refer to which page, found once so that an operation
/// that makes many documents from one (a split) reads the file once.
pub(crate) struct Referrers {
    by_page: HashMap<ObjectId, Vec<ObjectId>>,
}

impl Referrers {
    /// Every object the trailer reaches that holds a reference to one of
    /// `pages`.
    pub(crate) fn scan(document: &Document, pages: &HashSet<ObjectId>) -> Result<Self, PdfError> {
        let mut by_page: HashMap<ObjectId, Vec<ObjectId>> = HashMap::new();
        for id in reachable(document, document.trailer(), &NoProgress)? {
            let object = document.required(id)?;
            let mut found = Vec::new();
            match &object.content {
                Content::Object(value) => collect(value, pages, &mut found),
                Content::Stream(stream) => {
                    for (_, value) in stream.dictionary.iter() {
                        collect(value, pages, &mut found);
                    }
                }
            }
            found.sort_unstable();
            found.dedup();
            for page in found {
                by_page.entry(page).or_default().push(id);
            }
        }
        Ok(Self { by_page })
    }

    /// No references known: for an operation that removes no page.
    pub(crate) fn none() -> Self {
        Self {
            by_page: HashMap::new(),
        }
    }

    /// The objects that refer to any page of `gone`, once each.
    fn to_any(&self, gone: &HashSet<ObjectId>) -> Vec<ObjectId> {
        let mut found: Vec<ObjectId> = gone
            .iter()
            .filter_map(|page| self.by_page.get(page))
            .flatten()
            .copied()
            .collect();
        found.sort_unstable();
        found.dedup();
        found
    }
}

fn collect(object: &Object, pages: &HashSet<ObjectId>, out: &mut Vec<ObjectId>) {
    let mut stack = vec![object];
    while let Some(object) = stack.pop() {
        match object.value() {
            Value::Reference(id) if pages.contains(id) => out.push(*id),
            Value::Array(items) => stack.extend(items.iter()),
            Value::Dictionary(dictionary) => stack.extend(dictionary.iter().map(|(_, v)| v)),
            _ => {}
        }
    }
}

/// Makes every reference to a page of `gone` null, or takes out the
/// destination that holds it, in the objects `referrers` knows hold one.
/// Objects in `skip` (the pages that go, the nodes of the old page tree)
/// are left as they are.
pub(crate) fn scrub_all(
    rewriter: &mut Rewriter<'_>,
    referrers: &Referrers,
    gone: &HashSet<ObjectId>,
    skip: &dyn Fn(ObjectId) -> bool,
) -> Result<(), PdfError> {
    for id in referrers.to_any(gone) {
        if skip(id) {
            continue;
        }
        if let Some(mut object) = rewriter.read(id)? {
            scrub(&mut object, gone);
            rewriter.set(id, object)?;
            continue;
        }
        // A stream: only its dictionary can hold a reference.
        let document = rewriter.document;
        let Some(stream) = document.object(id)?.and_then(|o| o.stream().cloned()) else {
            continue;
        };
        let mut as_object = direct(Value::Dictionary(stream.dictionary.clone()));
        scrub(&mut as_object, gone);
        if let Value::Dictionary(changed) = as_object.into_value() {
            if changed != stream.dictionary {
                rewriter.set_stream_dictionary(id, changed)?;
            }
        }
    }
    Ok(())
}

/// Makes every reference in `object` to a page of `gone` null.
pub(crate) fn scrub(object: &mut Object, gone: &HashSet<ObjectId>) {
    if let Value::Reference(id) = object.value() {
        if gone.contains(id) {
            *object = direct(Value::Null);
        }
        return;
    }
    match object.value_mut() {
        Value::Array(items) => items.iter_mut().for_each(|item| scrub(item, gone)),
        Value::Dictionary(dictionary) => scrub_dictionary(dictionary, gone),
        _ => {}
    }
}

fn scrub_dictionary(dictionary: &mut Dictionary, gone: &HashSet<ObjectId>) {
    for key in DESTINATION_KEYS {
        if dictionary
            .get(key)
            .is_some_and(|value| leads_to(value, gone))
        {
            dictionary.remove(key);
        }
    }
    while dictionary
        .get(ACTION_KEY)
        .is_some_and(|action| is_go_to(action, gone))
    {
        // The action goes; what it chains to takes its place.
        let next = dictionary
            .get(ACTION_KEY)
            .and_then(dictionary_of)
            .and_then(|action| action.get(NEXT_KEY))
            .and_then(first_action)
            .cloned();
        match next {
            Some(next) => {
                dictionary.insert(crate::syntax::Name::new(ACTION_KEY), next);
            }
            None => {
                dictionary.remove(ACTION_KEY);
            }
        }
    }
    for (_, value) in dictionary.iter_mut() {
        scrub(value, gone);
    }
}

fn dictionary_of(object: &Object) -> Option<&Dictionary> {
    dictionary(object)
}

/// The action `next` is, or the first of the actions it lists.
fn first_action(next: &Object) -> Option<&Object> {
    match next.value() {
        Value::Dictionary(_) => Some(next),
        Value::Array(items) => items
            .iter()
            .find(|item| matches!(item.value(), Value::Dictionary(_))),
        _ => None,
    }
}

/// Whether `object` is a direct go-to action that leads to a gone page.
fn is_go_to(object: &Object, gone: &HashSet<ObjectId>) -> bool {
    let Some(action) = dictionary(object) else {
        return false;
    };
    action.get(SUBTYPE_KEY).and_then(name) == Some(GO_TO)
        && action.get(D_KEY).is_some_and(|d| leads_to(d, gone))
}

/// Whether `object` is a direct destination array whose page is gone.
fn leads_to(object: &Object, gone: &HashSet<ObjectId>) -> bool {
    array(object)
        .and_then(<[Object]>::first)
        .and_then(reference)
        .is_some_and(|page| gone.contains(&page))
}

/// The pages that leave and their annotations.
pub(crate) fn gone_of(document: &Document, pages: HashSet<ObjectId>) -> Result<Gone, PdfError> {
    let mut annotations = HashSet::new();
    for page in &pages {
        let Some(page) = document.object(*page)? else {
            continue;
        };
        let Some(dictionary) = page.dictionary() else {
            continue;
        };
        let annots = document.get(dictionary, b"Annots")?;
        for item in annots.array().unwrap_or_default() {
            if let Some(id) = reference(item) {
                annotations.insert(id);
            }
        }
    }
    Ok(Gone { pages, annotations })
}
