// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use kernkraft_pdf::lopdf::Object;
use kernkraft_pdf::{
    load_bytes, page_count, remove, reorder, rotate, split_by_bookmarks, to_bytes, PdfError,
    Rotation,
};

fn rotation(doc: &kernkraft_pdf::lopdf::Document, n: u32) -> Option<Object> {
    page_attr(doc, n, b"Rotate")
}

fn with_rotate(value: Object) -> Vec<u8> {
    edited(&pdf_n(1), |doc, pages| {
        doc.get_dictionary_mut(pages[0])
            .unwrap()
            .set("Rotate", value);
    })
}

#[test]
fn rotate_all_when_empty() {
    let mut doc = load_bytes(&pdf_n(2)).unwrap();
    rotate(&mut doc, &[], Rotation::Cw90).unwrap();
    assert_eq!(rotation(&doc, 1), Some(Object::Integer(90)));
    assert_eq!(rotation(&doc, 2), Some(Object::Integer(90)));
}

#[test]
fn rotate_only_named_pages() {
    let mut doc = load_bytes(&pdf_n(3)).unwrap();
    rotate(&mut doc, &[2], Rotation::Cw270).unwrap();
    assert_eq!(rotation(&doc, 1), None);
    assert_eq!(rotation(&doc, 2), Some(Object::Integer(270)));
    assert_eq!(rotation(&doc, 3), None);
}

#[test]
fn rotate_normalises_odd_stored_value() {
    let mut doc = load_bytes(&with_rotate(Object::Integer(-90))).unwrap();
    rotate(&mut doc, &[1], Rotation::Cw180).unwrap();
    assert_eq!(rotation(&doc, 1), Some(Object::Integer(90)));

    let mut doc = load_bytes(&with_rotate(Object::Integer(450))).unwrap();
    rotate(&mut doc, &[1], Rotation::Cw270).unwrap();
    assert_eq!(rotation(&doc, 1), Some(Object::Integer(0)));
}

#[test]
fn rotate_refuses_non_quarter_turn() {
    let mut doc = load_bytes(&with_rotate(Object::Integer(45))).unwrap();
    assert!(matches!(
        rotate(&mut doc, &[1], Rotation::Cw90),
        Err(PdfError::Invalid(_))
    ));
    assert_eq!(rotation(&doc, 1), Some(Object::Integer(45)));
}

#[test]
fn rotate_respects_inherited_rotate() {
    let mut doc = load_bytes(&nested_pdf()).unwrap();
    rotate(&mut doc, &[2], Rotation::Cw90).unwrap();
    assert_eq!(rotation(&doc, 2), Some(Object::Integer(NESTED_ROTATE + 90)));
    assert_eq!(rotation(&doc, 3), Some(Object::Integer(NESTED_ROTATE)));
    assert_eq!(rotation(&doc, 1), None);
}

#[test]
fn rotate_rejects_bad_pages() {
    let mut doc = load_bytes(&pdf_n(2)).unwrap();
    assert!(matches!(
        rotate(&mut doc, &[3], Rotation::Cw90),
        Err(PdfError::PageOutOfRange { page: 3, count: 2 })
    ));
    assert!(matches!(
        rotate(&mut doc, &[1, 1], Rotation::Cw90),
        Err(PdfError::DuplicatePage(1))
    ));
}

#[test]
fn reorder_applies_permutation() {
    let mut doc = load_bytes(&pdf_n(3)).unwrap();
    reorder(&mut doc, &[3, 1, 2]).unwrap();
    assert_eq!(markers(&doc), ["P3", "P1", "P2"]);
    let back = load_bytes(&to_bytes(&mut doc).unwrap()).unwrap();
    assert_eq!(markers(&back), ["P3", "P1", "P2"]);
}

#[test]
fn reorder_keeps_inherited_attributes() {
    let mut doc = load_bytes(&nested_pdf()).unwrap();
    reorder(&mut doc, &[3, 2, 1]).unwrap();
    assert_eq!(markers(&doc), ["P3", "P2", "P1"]);
    assert_eq!(
        page_attr(&doc, 1, b"MediaBox"),
        Some(rect_of(NESTED_MEDIABOX))
    );
    assert_eq!(
        page_attr(&doc, 3, b"MediaBox"),
        Some(rect_of(PAGE1_MEDIABOX))
    );
}

#[test]
fn reorder_rejects_non_permutation() {
    let mut doc = load_bytes(&pdf_n(3)).unwrap();
    assert!(matches!(
        reorder(&mut doc, &[1, 1, 2]),
        Err(PdfError::DuplicatePage(1))
    ));
    assert!(matches!(
        reorder(&mut doc, &[1, 2]),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(
        reorder(&mut doc, &[1, 2, 4]),
        Err(PdfError::PageOutOfRange { page: 4, count: 3 })
    ));
    assert_eq!(markers(&doc), ["P1", "P2", "P3"]);
}

#[test]
fn remove_drops_pages_and_their_content() {
    let mut doc = load_bytes(&pdf_n(3)).unwrap();
    remove(&mut doc, &[2]).unwrap();
    assert_eq!(markers(&doc), ["P1", "P3"]);
    let bytes = to_bytes(&mut doc).unwrap();
    assert!(!contains(&bytes, b"(P2)"));
    assert_eq!(page_count(&load_bytes(&bytes).unwrap()), 2);
}

#[test]
fn remove_keeps_outline() {
    let mut doc = load_bytes(&with_outline(4, &[(1, "A"), (3, "C")])).unwrap();
    remove(&mut doc, &[2]).unwrap();
    let parts = split_by_bookmarks(&doc).unwrap();
    let titles: Vec<&str> = parts.iter().map(|(t, _)| t.as_str()).collect();
    assert_eq!(titles, ["A", "C"]);
    assert_eq!(markers(&parts[1].1), ["P3", "P4"]);
}

#[test]
fn remove_refuses_all_pages() {
    let mut doc = load_bytes(&pdf_n(2)).unwrap();
    assert!(matches!(
        remove(&mut doc, &[1, 2]),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(remove(&mut doc, &[]), Err(PdfError::Invalid(_))));
    assert!(matches!(
        remove(&mut doc, &[2, 2]),
        Err(PdfError::DuplicatePage(2))
    ));
    assert_eq!(page_count(&doc), 2);
}

#[test]
fn rotate_reads_referenced_real_and_negative_values() {
    let referenced = edited(&pdf_n(1), |doc, pages| {
        let value = doc.add_object(Object::Integer(90));
        doc.get_dictionary_mut(pages[0])
            .unwrap()
            .set("Rotate", Object::Reference(value));
    });
    for (bytes, expected) in [
        (referenced, 180),
        (with_rotate(Object::Real(90.0)), 180),
        (with_rotate(Object::Integer(-90)), 0),
    ] {
        let mut doc = load_bytes(&bytes).unwrap();
        rotate(&mut doc, &[], Rotation::Cw90).unwrap();
        assert_eq!(rotation(&doc, 1), Some(Object::Integer(expected)));
    }
}
