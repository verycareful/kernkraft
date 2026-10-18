// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::*;
use kernkraft_fs::{FsError, NoProgress, Progress};
use kernkraft_pdf::lopdf::{dictionary, Object, StringFormat};
use kernkraft_pdf::{
    extract, load_bytes, page_count, split_by_bookmarks, split_per_page, split_ranges, to_bytes,
    PageRange, PdfError, START_PART_TITLE,
};

/// `title:P1,P2` for each part, so a whole split compares in one line.
fn summary(parts: &[(String, kernkraft_pdf::lopdf::Document)]) -> Vec<String> {
    parts
        .iter()
        .map(|(t, d)| format!("{t}:{}", markers(d).join(",")))
        .collect()
}

fn range(start: u32, end: u32) -> PageRange {
    PageRange { start, end }
}

#[test]
fn extract_reorders_as_asked() {
    let doc = load_bytes(&pdf_n(4)).unwrap();
    let out = extract(&doc, &[3, 1]).unwrap();
    assert_eq!(markers(&out), ["P3", "P1"]);
    // The source is untouched.
    assert_eq!(page_count(&doc), 4);
}

#[test]
fn extract_rejects_duplicate() {
    let doc = load_bytes(&pdf_n(4)).unwrap();
    assert!(matches!(
        extract(&doc, &[2, 2]),
        Err(PdfError::DuplicatePage(2))
    ));
}

#[test]
fn extract_rejects_out_of_range_and_empty() {
    let doc = load_bytes(&pdf_n(2)).unwrap();
    assert!(matches!(
        extract(&doc, &[3]),
        Err(PdfError::PageOutOfRange { page: 3, count: 2 })
    ));
    assert!(matches!(
        extract(&doc, &[0]),
        Err(PdfError::PageOutOfRange { page: 0, .. })
    ));
    assert!(matches!(extract(&doc, &[]), Err(PdfError::Invalid(_))));
}

#[test]
fn split_ranges_one_doc_per_range() {
    let doc = load_bytes(&pdf_n(5)).unwrap();
    let parts = split_ranges(&doc, &[range(1, 2), range(4, 5)]).unwrap();
    let got: Vec<Vec<String>> = parts.iter().map(markers).collect();
    assert_eq!(got, vec![vec!["P1", "P2"], vec!["P4", "P5"]]);
}

#[test]
fn split_ranges_rejects_bad_range() {
    let doc = load_bytes(&pdf_n(3)).unwrap();
    assert!(matches!(
        split_ranges(&doc, &[range(2, 9)]),
        Err(PdfError::PageOutOfRange { page: 4, count: 3 })
    ));
    assert!(matches!(
        split_ranges(&doc, &[range(3, 1)]),
        Err(PdfError::BadRange(_))
    ));
    assert!(matches!(split_ranges(&doc, &[]), Err(PdfError::Invalid(_))));
}

#[test]
fn split_per_page_output_holds_only_its_page() {
    let doc = load_bytes(&pdf_n(3)).unwrap();
    let mut parts = split_per_page(&doc, &NoProgress).unwrap();
    assert_eq!(parts.len(), 3);
    let bytes = to_bytes(&mut parts[0]).unwrap();
    assert!(contains(&bytes, b"(P1)"));
    assert!(!contains(&bytes, b"(P2)"));
    assert!(!contains(&bytes, b"(P3)"));
}

#[test]
fn link_to_dropped_page_does_not_keep_it_alive() {
    // Page 1 carries a link annotation whose destination is page 2.
    let bytes = edited(&pdf_n(2), |doc, pages| {
        let annot = doc.add_object(dictionary! {
            "Type" => "Annot", "Subtype" => "Link",
            "Rect" => vec![0.into(), 0.into(), 10.into(), 10.into()],
            "Dest" => vec![pages[1].into(), "Fit".into()],
        });
        doc.get_dictionary_mut(pages[0])
            .unwrap()
            .set("Annots", vec![annot.into()]);
        doc.catalog_mut()
            .unwrap()
            .set("OpenAction", vec![pages[1].into(), "Fit".into()]);
    });
    let doc = load_bytes(&bytes).unwrap();
    let mut out = extract(&doc, &[1]).unwrap();
    let saved = to_bytes(&mut out).unwrap();
    assert!(contains(&saved, b"(P1)"));
    assert!(!contains(&saved, b"(P2)"));
    assert_eq!(page_count(&load_bytes(&saved).unwrap()), 1);
}

#[test]
fn split_keeps_inherited_mediabox() {
    let doc = load_bytes(&nested_pdf()).unwrap();
    let out = extract(&doc, &[2]).unwrap();
    assert_eq!(markers(&out), ["P2"]);
    assert_eq!(
        page_attr(&out, 1, b"MediaBox"),
        Some(rect_of(NESTED_MEDIABOX))
    );
    assert!(page_attr(&out, 1, b"Resources").is_some());
    assert_eq!(
        page_attr(&out, 1, b"Rotate"),
        Some(Object::Integer(NESTED_ROTATE))
    );
}

#[test]
fn extract_drops_outline() {
    let doc = load_bytes(&with_outline(3, &[(1, "A")])).unwrap();
    let out = extract(&doc, &[1]).unwrap();
    assert!(out.catalog().unwrap().get(b"Outlines").is_err());
    let outline_items = out
        .objects
        .values()
        .filter(|o| o.as_dict().is_ok_and(|d| d.has(b"Title")))
        .count();
    assert_eq!(outline_items, 0);
}

#[test]
fn split_by_bookmarks_cuts_at_marks() {
    let doc = load_bytes(&with_outline(5, &[(1, "A"), (3, "B")])).unwrap();
    let parts = split_by_bookmarks(&doc).unwrap();
    let got = summary(&parts);
    assert_eq!(got, ["A:P1,P2", "B:P3,P4,P5"]);
}

#[test]
fn split_by_bookmarks_adds_start_part() {
    let doc = load_bytes(&with_outline(4, &[(3, "Later")])).unwrap();
    let parts = split_by_bookmarks(&doc).unwrap();
    assert_eq!(parts[0].0, START_PART_TITLE);
    assert_eq!(markers(&parts[0].1), ["P1", "P2"]);
    assert_eq!(parts[1].0, "Later");
    assert_eq!(markers(&parts[1].1), ["P3", "P4"]);
}

#[test]
fn split_by_bookmarks_sorts_and_merges_same_page() {
    let doc = load_bytes(&with_outline(4, &[(3, "C"), (1, "A"), (3, "C2")])).unwrap();
    let titles: Vec<String> = split_by_bookmarks(&doc)
        .unwrap()
        .into_iter()
        .map(|(t, _)| t)
        .collect();
    assert_eq!(titles, ["A", "C"]);
}

#[test]
fn split_by_bookmarks_reads_utf16_titles_and_goto_actions() {
    let bytes = edited(&with_outline(3, &[(1, "A")]), |doc, pages| {
        let outlines = doc
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .unwrap()
            .as_reference()
            .unwrap();
        let first = doc
            .get_dictionary(outlines)
            .unwrap()
            .get(b"First")
            .unwrap()
            .as_reference()
            .unwrap();
        // "Ü2" in UTF-16BE with a byte order mark.
        let title = vec![0xFE, 0xFF, 0x00, 0xDC, 0x00, 0x32];
        let second = doc.add_object(dictionary! {
            "Title" => Object::String(title, StringFormat::Hexadecimal),
            "Parent" => outlines,
            "Prev" => first,
            "A" => dictionary! { "S" => "GoTo", "D" => vec![pages[2].into(), "Fit".into()] },
        });
        doc.get_dictionary_mut(first).unwrap().set("Next", second);
    });
    let doc = load_bytes(&bytes).unwrap();
    let parts = split_by_bookmarks(&doc).unwrap();
    assert_eq!(parts[1].0, "\u{dc}2");
    assert_eq!(markers(&parts[1].1), ["P3"]);
}

#[test]
fn split_by_bookmarks_follows_named_destinations() {
    let bytes = edited(&with_outline(3, &[(1, "A")]), |doc, pages| {
        let outlines = doc
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .unwrap()
            .as_reference()
            .unwrap();
        let first = doc
            .get_dictionary(outlines)
            .unwrap()
            .get(b"First")
            .unwrap()
            .as_reference()
            .unwrap();
        // One string-named destination through the /Names tree, one
        // name-named destination through /Dests.
        let tree = doc.add_object(dictionary! {
            "Names" => vec![
                Object::String(b"sec.two".to_vec(), StringFormat::Literal),
                dictionary! { "D" => vec![pages[1].into(), "Fit".into()] }.into(),
            ],
        });
        let dests = doc.add_object(dictionary! {
            "three" => vec![Object::Reference(pages[2]), "Fit".into()],
        });
        let second = doc.add_object(dictionary! {
            "Title" => Object::String(b"Two".to_vec(), StringFormat::Literal),
            "Parent" => outlines,
            "Dest" => Object::String(b"sec.two".to_vec(), StringFormat::Literal),
        });
        let third = doc.add_object(dictionary! {
            "Title" => Object::String(b"Three".to_vec(), StringFormat::Literal),
            "Parent" => outlines,
            "Dest" => Object::Name(b"three".to_vec()),
        });
        doc.get_dictionary_mut(first).unwrap().set("Next", second);
        doc.get_dictionary_mut(second).unwrap().set("Next", third);
        let catalog = doc.catalog_mut().unwrap();
        catalog.set("Names", dictionary! { "Dests" => tree });
        catalog.set("Dests", dests);
    });
    let doc = load_bytes(&bytes).unwrap();
    let parts = split_by_bookmarks(&doc).unwrap();
    let got = summary(&parts);
    assert_eq!(got, ["A:P1", "Two:P2", "Three:P3"]);
}

#[test]
fn split_by_bookmarks_survives_outline_cycle() {
    let bytes = edited(&with_outline(2, &[(1, "A"), (2, "B")]), |doc, _| {
        let outlines = doc
            .catalog()
            .unwrap()
            .get(b"Outlines")
            .unwrap()
            .as_reference()
            .unwrap();
        let o = doc.get_dictionary(outlines).unwrap();
        let first = o.get(b"First").unwrap().as_reference().unwrap();
        let last = o.get(b"Last").unwrap().as_reference().unwrap();
        doc.get_dictionary_mut(last).unwrap().set("Next", first);
    });
    let doc = load_bytes(&bytes).unwrap();
    assert_eq!(split_by_bookmarks(&doc).unwrap().len(), 2);
}

#[test]
fn split_by_bookmarks_without_outline_errors() {
    let doc = load_bytes(&pdf_n(3)).unwrap();
    assert!(matches!(
        split_by_bookmarks(&doc),
        Err(PdfError::Invalid(_))
    ));
}

struct CancelAt(u64);

impl Progress for CancelAt {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {}
    fn is_cancelled(&self) -> bool {
        self.0 == 0
    }
}

#[test]
fn split_per_page_cancel_is_reported() {
    let doc = load_bytes(&pdf_n(2)).unwrap();
    assert!(matches!(
        split_per_page(&doc, &CancelAt(0)),
        Err(PdfError::Fs(FsError::Cancelled))
    ));
}
