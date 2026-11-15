// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the page operations, one module per source file of
//! `src/pages/`. The documents are written by `common`, byte by byte.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../common/mod.rs"]
mod common;
#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod merge;
mod remove;
mod reorder;
mod robustness;
mod rotate;
mod split;

use kernkraft_pdf::doc::{Document, NameTree, NumberTree};
use kernkraft_pdf::syntax::{Object, ObjectId, Value};
use kernkraft_pdf::DestinationPage;

pub use common::*;
pub use pdf_fixture::OPTIONS;

pub fn open(bytes: &[u8]) -> Document {
    Document::from_bytes(bytes.to_vec(), &OPTIONS, "").expect("the document opens")
}

/// The outline as `(level, title, page)`.
pub fn outline(document: &Document) -> Vec<(u32, String, Option<u32>)> {
    document
        .outline()
        .unwrap()
        .into_iter()
        .map(|item| {
            let page = match item.destination.map(|destination| destination.page) {
                Some(DestinationPage::Page(page)) => Some(page),
                _ => None,
            };
            (item.level, item.title.text, page)
        })
        .collect()
}

/// The label of each page, as shown.
pub fn labels(document: &Document) -> Option<Vec<String>> {
    document.page_labels().unwrap().map(|labels| {
        labels
            .into_iter()
            .map(|label| label.map_or(String::new(), |label| label.to_string()))
            .collect()
    })
}

/// What the catalog's `key` entry stands for, followed through a
/// reference, as an object of its own.
pub fn resolved_entry(document: &Document, key: &[u8]) -> Object {
    let entry = catalog_entry(document, key).expect("the catalog has the entry");
    document.resolve(&entry).unwrap().object().clone()
}

/// `dictionary()` and `array()` for an object the tests hold.
pub trait Shape {
    fn dictionary(&self) -> Option<&kernkraft_pdf::syntax::Dictionary>;
    fn array(&self) -> Option<&[Object]>;
}

impl Shape for Object {
    fn dictionary(&self) -> Option<&kernkraft_pdf::syntax::Dictionary> {
        match self.value() {
            Value::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }

    fn array(&self) -> Option<&[Object]> {
        match self.value() {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }
}

fn catalog_entry(document: &Document, key: &[u8]) -> Option<Object> {
    document
        .catalog()
        .unwrap()
        .dictionary()
        .unwrap()
        .get(key)
        .cloned()
}

/// The names of the top-level fields of the form, in order.
pub fn field_names(document: &Document) -> Vec<String> {
    let Some(form) = catalog_entry(document, b"AcroForm") else {
        return Vec::new();
    };
    let form = document.resolve(&form).unwrap();
    let fields = document
        .get(form.dictionary().unwrap(), b"Fields")
        .unwrap()
        .array()
        .unwrap_or_default()
        .to_vec();
    fields
        .iter()
        .map(|field| {
            let field = document.resolve(field).unwrap();
            let name = document.get(field.dictionary().unwrap(), b"T").unwrap();
            match name.value() {
                Value::String(name) => String::from_utf8_lossy(name.as_bytes()).into_owned(),
                other => panic!("a field name that is {other:?}"),
            }
        })
        .collect()
}

/// The keys of the `Dests` name tree.
pub fn dest_names(document: &Document) -> Vec<String> {
    names_of(document, b"Dests")
}

/// The keys of a name tree of the name dictionary.
pub fn names_of(document: &Document, category: &[u8]) -> Vec<String> {
    let Some(names) = catalog_entry(document, b"Names") else {
        return Vec::new();
    };
    let names = document.resolve(&names).unwrap();
    let Some(tree) = names.dictionary().unwrap().get(category) else {
        return Vec::new();
    };
    NameTree::new(document, tree.clone())
        .entries()
        .unwrap()
        .into_iter()
        .map(|(key, _)| String::from_utf8_lossy(&key).into_owned())
        .collect()
}

/// The keys of the structure tree's `ParentTree`.
pub fn parent_tree_keys(document: &Document) -> Vec<i64> {
    let Some(root) = catalog_entry(document, b"StructTreeRoot") else {
        return Vec::new();
    };
    let root = document.resolve(&root).unwrap();
    let tree = root.dictionary().unwrap().get(b"ParentTree").unwrap();
    NumberTree::new(document, tree.clone())
        .entries()
        .unwrap()
        .into_iter()
        .map(|(key, _)| key)
        .collect()
}

/// The `Pg` pages (as page numbers) of the structure tree's top-level
/// elements.
pub fn structure_pages(document: &Document) -> Vec<Option<u32>> {
    let Some(root) = catalog_entry(document, b"StructTreeRoot") else {
        return Vec::new();
    };
    let root = document.resolve(&root).unwrap();
    let k = document.get(root.dictionary().unwrap(), b"K").unwrap();
    let children: Vec<Object> = match k.array() {
        Some(items) => items.to_vec(),
        None => vec![k.object().clone()],
    };
    children
        .iter()
        .filter(|child| !matches!(child.value(), Value::Null))
        .map(|child| {
            let element = document.resolve(child).unwrap();
            let page = element.dictionary().unwrap().get(b"Pg")?;
            match page.value() {
                Value::Reference(id) => document.page_number(*id).unwrap(),
                _ => None,
            }
        })
        .collect()
}

/// The text of a string entry of object `number`.
pub fn string_entry(document: &Document, number: u32, key: &[u8]) -> Option<String> {
    let object = document.object(id(number)).unwrap()?;
    match object.dictionary()?.get(key)?.value() {
        Value::String(text) => Some(String::from_utf8_lossy(text.as_bytes()).into_owned()),
        _ => None,
    }
}

/// Whether the document has an object `number` that is not null.
pub fn has_object(document: &Document, number: u32) -> bool {
    document.object(id(number)).unwrap().is_some()
}

pub fn page_ids(document: &Document) -> Vec<ObjectId> {
    (1..=document.page_count().unwrap())
        .map(|number| document.page_id(number).unwrap())
        .collect()
}
