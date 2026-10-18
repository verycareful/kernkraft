// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::sync::atomic::{AtomicU32, Ordering};

use common::*;
use kernkraft_fs::{FsError, NoProgress, Progress};
use kernkraft_pdf::lopdf::Object;
use kernkraft_pdf::{load_bytes, merge, page_count, to_bytes, PdfError};

#[test]
fn merge_concatenates_in_order() {
    let docs = vec![
        load_bytes(&pdf_n(1)).unwrap(),
        load_bytes(&pdf_n(2)).unwrap(),
    ];
    let out = merge(docs, &NoProgress).unwrap();
    assert_eq!(markers(&out), ["P1", "P1", "P2"]);
}

#[test]
fn merge_keeps_page_order_not_object_order() {
    // Reorder a document's pages so page order differs from object ids.
    let bytes = edited(&pdf_n(3), |doc, pages| {
        let root = doc
            .catalog()
            .unwrap()
            .get(b"Pages")
            .unwrap()
            .as_reference()
            .unwrap();
        let kids: Vec<Object> = [pages[2], pages[0], pages[1]]
            .iter()
            .map(|&p| Object::Reference(p))
            .collect();
        doc.get_dictionary_mut(root).unwrap().set("Kids", kids);
    });
    let docs = vec![load_bytes(&bytes).unwrap(), load_bytes(&pdf_n(1)).unwrap()];
    let out = merge(docs, &NoProgress).unwrap();
    assert_eq!(markers(&out), ["P3", "P1", "P2", "P1"]);
}

#[test]
fn merge_sums_three_documents() {
    let docs = vec![
        load_bytes(&pdf_n(1)).unwrap(),
        load_bytes(&pdf_n(1)).unwrap(),
        load_bytes(&pdf_n(5)).unwrap(),
    ];
    assert_eq!(page_count(&merge(docs, &NoProgress).unwrap()), 1 + 1 + 5);
}

#[test]
fn merge_keeps_inherited_attributes() {
    let docs = vec![
        load_bytes(&pdf_n(1)).unwrap(),
        load_bytes(&nested_pdf()).unwrap(),
    ];
    let out = merge(docs, &NoProgress).unwrap();
    assert_eq!(markers(&out), ["P1", "P1", "P2", "P3"]);
    for n in [3, 4] {
        assert_eq!(
            page_attr(&out, n, b"MediaBox"),
            Some(rect_of(NESTED_MEDIABOX))
        );
        assert!(page_attr(&out, n, b"Resources").is_some());
        assert_eq!(
            page_attr(&out, n, b"Rotate"),
            Some(Object::Integer(NESTED_ROTATE))
        );
        assert_eq!(
            page_attr(&out, n, b"CropBox"),
            Some(rect_of(NESTED_CROPBOX))
        );
    }
}

#[test]
fn merge_drops_outlines_and_old_catalogs() {
    let docs = vec![
        load_bytes(&with_outline(2, &[(1, "A")])).unwrap(),
        load_bytes(&pdf_n(1)).unwrap(),
    ];
    let out = merge(docs, &NoProgress).unwrap();
    let catalogs = out
        .objects
        .values()
        .filter(|o| o.type_name().ok() == Some(b"Catalog".as_slice()))
        .count();
    assert_eq!(catalogs, 1);
    assert!(out.catalog().unwrap().get(b"Outlines").is_err());
}

#[test]
fn merge_takes_highest_version() {
    let mut old = load_bytes(&pdf_n(1)).unwrap();
    old.version = "1.3".to_string();
    let mut new = load_bytes(&pdf_n(1)).unwrap();
    new.version = "2.0".to_string();
    assert_eq!(merge(vec![old, new], &NoProgress).unwrap().version, "2.0");
}

#[test]
fn merge_needs_two() {
    let one = vec![load_bytes(&pdf_n(1)).unwrap()];
    assert!(matches!(merge(one, &NoProgress), Err(PdfError::Invalid(_))));
    assert!(matches!(
        merge(Vec::new(), &NoProgress),
        Err(PdfError::Invalid(_))
    ));
}

#[test]
fn merge_result_survives_save_and_reload() {
    let docs = vec![
        load_bytes(&nested_pdf()).unwrap(),
        load_bytes(&pdf_n(2)).unwrap(),
    ];
    let mut out = merge(docs, &NoProgress).unwrap();
    let back = load_bytes(&to_bytes(&mut out).unwrap()).unwrap();
    assert_eq!(page_count(&back), 5);
    assert_eq!(markers(&back), ["P1", "P2", "P3", "P1", "P2"]);
}

struct CancelAfter(AtomicU32);

impl Progress for CancelAfter {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
    fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst) >= 1
    }
}

#[test]
fn merge_cancel_is_reported() {
    let docs = vec![
        load_bytes(&pdf_n(1)).unwrap(),
        load_bytes(&pdf_n(1)).unwrap(),
    ];
    assert!(matches!(
        merge(docs, &CancelAfter(AtomicU32::new(0))),
        Err(PdfError::Fs(FsError::Cancelled))
    ));
}

#[test]
fn merge_same_document_twice() {
    let doc = load_bytes(&pdf_n(2)).unwrap();
    let mut out = merge(vec![doc.clone(), doc], &NoProgress).unwrap();
    assert_eq!(markers(&out), ["P1", "P2", "P1", "P2"]);
    common::corpus::assert_no_dangling(&out, "merged");
    assert_eq!(
        page_count(&load_bytes(&to_bytes(&mut out).unwrap()).unwrap()),
        4
    );
}

#[test]
fn merge_version_handles_unparsable() {
    let with_version = |v: &str| {
        let mut doc = load_bytes(&pdf_n(1)).unwrap();
        doc.version = v.to_string();
        doc
    };
    let out = merge(vec![with_version("2.0"), with_version("abc")], &NoProgress).unwrap();
    assert_eq!(out.version, "2.0");
    let out = merge(vec![with_version("abc"), with_version("1.3")], &NoProgress).unwrap();
    assert_eq!(out.version, "1.4", "the floor when every input is older");
}
