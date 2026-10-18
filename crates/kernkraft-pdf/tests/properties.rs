// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page operations over the whole corpus, checked by properties that hold
//! for any input: an operation and its inverse restore the document, and
//! every result is a self-contained file.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::corpus::{
    assert_no_dangling, corpus, effective, page_attrs, shared_image, SHARED_IMAGE,
};
use kernkraft_fs::NoProgress;
use kernkraft_pdf::lopdf::{Document, Object};
use kernkraft_pdf::{
    extract, load_bytes, merge, page_count, remove, reorder, rotate, split_per_page, split_ranges,
    to_bytes, PageRange, Rotation,
};

const FULL_TURN: i64 = 360;

/// Each page's decoded content, in page order: what a page shows, whatever
/// object ids it ends up with.
fn contents(doc: &Document) -> Vec<Vec<u8>> {
    doc.get_pages()
        .values()
        .map(|&id| doc.get_page_content(id))
        .collect()
}

/// MediaBox, CropBox and Rotate of every page: the attributes whose values
/// carry no object references, so they compare across renumbering.
fn geometry(doc: &Document) -> Vec<Vec<Option<Object>>> {
    let n = doc.get_pages().len() as u32;
    (1..=n)
        .map(|p| {
            [&b"MediaBox"[..], b"CropBox", b"Rotate"]
                .iter()
                .map(|k| effective(doc, p, k))
                .collect()
        })
        .collect()
}

fn rotation(doc: &Document, n: u32) -> i64 {
    match effective(doc, n, b"Rotate") {
        None => 0,
        Some(Object::Integer(i)) => i.rem_euclid(FULL_TURN),
        Some(other) => panic!("page {n}: rotation {other:?}"),
    }
}

/// The result is self-contained and survives a save and reload.
fn check_file(doc: &mut Document, what: &str) -> Document {
    assert_no_dangling(doc, what);
    let back = load_bytes(&to_bytes(doc).unwrap()).unwrap_or_else(|e| panic!("{what}: {e}"));
    assert_eq!(contents(&back), contents(doc), "{what}: reload");
    back
}

#[test]
fn identity_reorder_changes_nothing() {
    for f in corpus() {
        let mut doc = load_bytes(&f.bytes).unwrap();
        let (before, attrs) = (contents(&doc), page_attrs(&doc));
        reorder(&mut doc, &(1..=f.pages).collect::<Vec<_>>()).unwrap();
        assert_eq!(contents(&doc), before, "{}", f.name);
        assert_eq!(page_attrs(&doc), attrs, "{}", f.name);
        check_file(&mut doc, f.name);
    }
}

#[test]
fn reorder_then_inverse_restores() {
    for f in corpus() {
        let mut doc = load_bytes(&f.bytes).unwrap();
        let (before, attrs) = (contents(&doc), page_attrs(&doc));
        let reversed: Vec<u32> = (1..=f.pages).rev().collect();
        reorder(&mut doc, &reversed).unwrap();
        let mut expected = before.clone();
        expected.reverse();
        assert_eq!(contents(&doc), expected, "{}: reversed", f.name);
        reorder(&mut doc, &reversed).unwrap();
        assert_eq!(contents(&doc), before, "{}", f.name);
        assert_eq!(page_attrs(&doc), attrs, "{}", f.name);
        check_file(&mut doc, f.name);
    }
}

#[test]
fn four_quarter_turns_restore_rotation() {
    for f in corpus() {
        let mut doc = load_bytes(&f.bytes).unwrap();
        let original: Vec<i64> = (1..=f.pages).map(|n| rotation(&doc, n)).collect();
        for turn in 1..=4 {
            rotate(&mut doc, &[], Rotation::Cw90).unwrap();
            let now: Vec<i64> = (1..=f.pages).map(|n| rotation(&doc, n)).collect();
            let expected: Vec<i64> = original
                .iter()
                .map(|r| (r + turn * Rotation::Cw90.degrees()).rem_euclid(FULL_TURN))
                .collect();
            assert_eq!(now, expected, "{}: after {turn} turns", f.name);
        }
        check_file(&mut doc, f.name);
    }
}

#[test]
fn split_per_page_then_merge_restores() {
    for f in corpus().into_iter().filter(|f| f.pages > 1) {
        let doc = load_bytes(&f.bytes).unwrap();
        let parts = split_per_page(&doc, &NoProgress).unwrap();
        assert_eq!(parts.len(), f.pages as usize, "{}", f.name);
        for (i, part) in parts.iter().enumerate() {
            assert_eq!(page_count(part), 1, "{}: part {}", f.name, i + 1);
            assert_eq!(
                contents(part),
                [contents(&doc)[i].clone()],
                "{}: part {}",
                f.name,
                i + 1
            );
        }
        let mut merged = merge(parts, &NoProgress).unwrap();
        assert_eq!(contents(&merged), contents(&doc), "{}", f.name);
        assert_eq!(geometry(&merged), geometry(&doc), "{}", f.name);
        check_file(&mut merged, f.name);
    }
}

#[test]
fn extract_all_pages_restores() {
    for f in corpus() {
        let doc = load_bytes(&f.bytes).unwrap();
        let mut out = extract(&doc, &(1..=f.pages).collect::<Vec<_>>()).unwrap();
        assert_eq!(contents(&out), contents(&doc), "{}", f.name);
        assert_eq!(page_attrs(&out), page_attrs(&doc), "{}", f.name);
        check_file(&mut out, f.name);
    }
}

#[test]
fn remove_each_page_leaves_the_rest() {
    for f in corpus().into_iter().filter(|f| f.pages > 1) {
        let original = load_bytes(&f.bytes).unwrap();
        for k in 1..=f.pages {
            let mut doc = original.clone();
            remove(&mut doc, &[k]).unwrap();
            let mut expected = contents(&original);
            expected.remove(k as usize - 1);
            assert_eq!(contents(&doc), expected, "{}: without page {k}", f.name);
            check_file(&mut doc, f.name);
        }
    }
}

#[test]
fn split_ranges_partition_remerges() {
    for f in corpus().into_iter().filter(|f| f.pages > 1) {
        let doc = load_bytes(&f.bytes).unwrap();
        let ranges = [
            PageRange { start: 1, end: 1 },
            PageRange {
                start: 2,
                end: f.pages,
            },
        ];
        let parts = split_ranges(&doc, &ranges).unwrap();
        let mut merged = merge(parts, &NoProgress).unwrap();
        assert_eq!(contents(&merged), contents(&doc), "{}", f.name);
        assert_eq!(geometry(&merged), geometry(&doc), "{}", f.name);
        check_file(&mut merged, f.name);
    }
}

#[test]
fn shared_image_survives_either_extract() {
    let doc = load_bytes(&shared_image()).unwrap();
    for page in [1, 2] {
        let mut out = extract(&doc, &[page]).unwrap();
        let resources = effective(&out, 1, b"Resources").expect("page 1 has resources");
        let image = resources
            .as_dict()
            .ok()
            .and_then(|r| r.get(b"XObject").ok())
            .and_then(|x| x.as_dict().ok())
            .and_then(|x| x.get(SHARED_IMAGE).ok())
            .and_then(|r| r.as_reference().ok())
            .expect("the image is still named");
        assert!(
            out.get_object(image).unwrap().as_stream().is_ok(),
            "page {page}"
        );
        check_file(&mut out, "shared-image");
    }
}
