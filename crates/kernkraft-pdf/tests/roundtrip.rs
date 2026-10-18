// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Load and save over the whole corpus: what is read is written back.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use common::corpus::{assert_no_dangling, corpus, page_attrs, with_info, INFO_TITLE};
use common::markers;
use kernkraft_pdf::lopdf::{Document, Object, ObjectId};
use kernkraft_pdf::{load, load_bytes, page_count, save, to_bytes, FileStamp};

/// Stream types that describe how a file is laid out rather than what it
/// holds: cross-reference streams and object streams (ISO 32000-1, 7.5.8
/// and 7.5.7). A save writes them afresh.
const LAYOUT_TYPES: [&[u8]; 2] = [b"XRef", b"ObjStm"];

/// The document's objects without its layout streams.
fn content_objects(doc: &Document) -> BTreeMap<ObjectId, Object> {
    doc.objects
        .iter()
        .filter(|(_, o)| {
            o.as_stream().map_or(true, |s| {
                let kind = s.dict.get(b"Type").and_then(Object::as_name);
                !kind.is_ok_and(|k| LAYOUT_TYPES.contains(&k))
            })
        })
        .map(|(id, o)| (*id, o.clone()))
        .collect()
}

#[test]
fn corpus_entries_load() {
    for f in corpus() {
        let doc = load_bytes(&f.bytes).unwrap_or_else(|e| panic!("{}: {e}", f.name));
        assert_eq!(page_count(&doc), f.pages, "{}", f.name);
        if let Some(expected) = &f.markers {
            assert_eq!(&markers(&doc), expected, "{}", f.name);
        }
        assert_no_dangling(&doc, f.name);
    }
}

#[test]
fn save_then_load_keeps_objects() {
    for f in corpus() {
        let mut first = load_bytes(&f.bytes).unwrap();
        let second = load_bytes(&to_bytes(&mut first).unwrap()).unwrap();
        assert_eq!(
            content_objects(&second),
            content_objects(&first),
            "{}",
            f.name
        );
        for key in [&b"Root"[..], b"Info"] {
            assert_eq!(
                second.trailer.get(key).ok(),
                first.trailer.get(key).ok(),
                "{}: trailer {}",
                f.name,
                String::from_utf8_lossy(key)
            );
        }
    }
}

/// A load and save that edits nothing must not change the file: a kept
/// copy of the loaded cross-reference stream would be written again under
/// the next free object number on every save.
#[test]
fn second_round_trip_is_byte_identical() {
    for f in corpus() {
        let once = to_bytes(&mut load_bytes(&f.bytes).unwrap()).unwrap();
        let twice = to_bytes(&mut load_bytes(&once).unwrap()).unwrap();
        assert!(once == twice, "{}: second save differs", f.name);
    }
}

#[test]
fn markers_and_effective_attributes_survive() {
    for f in corpus() {
        let mut doc = load_bytes(&f.bytes).unwrap();
        let attrs = page_attrs(&doc);
        let before = f.markers.as_ref().map(|_| markers(&doc));
        let again = load_bytes(&to_bytes(&mut doc).unwrap()).unwrap();
        assert_eq!(page_attrs(&again), attrs, "{}", f.name);
        if let Some(before) = before {
            assert_eq!(markers(&again), before, "{}", f.name);
        }
    }
}

#[test]
fn atomic_save_round_trips_on_disk() {
    let dir = tempfile::tempdir().unwrap();
    for f in corpus() {
        let path = dir.path().join(format!("{}.pdf", f.name));
        let stamp = save(&mut load_bytes(&f.bytes).unwrap(), &path, None).unwrap();
        assert_eq!(stamp, FileStamp::of(&path).unwrap(), "{}", f.name);
        let back = load(&path).unwrap();
        assert_eq!(page_count(&back), f.pages, "{}", f.name);
        if let Some(expected) = &f.markers {
            assert_eq!(&markers(&back), expected, "{}", f.name);
        }
    }
}

#[test]
fn info_dictionary_survives() {
    let mut doc = load_bytes(&with_info(2)).unwrap();
    let back = load_bytes(&to_bytes(&mut doc).unwrap()).unwrap();
    let info = back.trailer.get(b"Info").unwrap().as_reference().unwrap();
    let title = back.get_dictionary(info).unwrap().get(b"Title").unwrap();
    assert_eq!(
        title,
        &Object::string_literal(INFO_TITLE),
        "the /Info title is kept"
    );
}
