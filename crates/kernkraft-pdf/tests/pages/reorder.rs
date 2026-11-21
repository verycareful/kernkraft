// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use kernkraft_pdf::{reorder, PdfError};

use super::*;

#[test]
fn puts_the_pages_in_the_new_order() {
    let document = open(&pdf_n(3));
    let moved = reorder(&document, &[3, 1, 2]).unwrap();
    assert_eq!(markers(&moved), ["P3", "P1", "P2"]);
    assert_eq!(markers(&reopen(&moved)), ["P3", "P1", "P2"]);
    assert_eq!(markers(&document), ["P1", "P2", "P3"]);
}

#[test]
fn the_order_must_name_every_page_once() {
    let document = open(&pdf_n(3));
    assert!(matches!(
        reorder(&document, &[1, 2]),
        Err(PdfError::Invalid(_))
    ));
    assert!(matches!(
        reorder(&document, &[1, 2, 2]),
        Err(PdfError::DuplicatePage(2))
    ));
    assert!(matches!(
        reorder(&document, &[1, 2, 4]),
        Err(PdfError::PageOutOfRange { page: 4, count: 3 })
    ));
}

/// Moving a page out from under a node that gave it its attributes must
/// not change the page.
#[test]
fn pages_keep_what_they_inherited() {
    let document = open(&nested_pdf());
    let moved = reorder(&document, &[3, 2, 1]).unwrap();
    for (new, old) in [(1, 3), (2, 2), (3, 1)] {
        let before = document.page(old).unwrap();
        let after = moved.page(new).unwrap();
        assert_eq!(after.media_box, before.media_box, "page {old} media box");
        assert_eq!(after.crop_box, before.crop_box, "page {old} crop box");
        assert_eq!(after.rotate, before.rotate, "page {old} rotation");
        assert_eq!(after.resources.is_some(), before.resources.is_some());
    }
    let again = reopen(&moved);
    assert_eq!(again.page(1).unwrap().rotate, 90);
    assert_eq!(markers(&again), ["P3", "P2", "P1"]);
}

/// A page keeps its label when it moves.
#[test]
fn labels_follow_the_pages() {
    let document = open(&features("").bytes);
    assert_eq!(labels(&document).unwrap(), ["i", "A-1", "A-2"]);
    let moved = reorder(&document, &[3, 1, 2]).unwrap();
    assert_eq!(labels(&moved).unwrap(), ["A-2", "i", "A-1"]);
    assert_eq!(labels(&reopen(&moved)).unwrap(), ["A-2", "i", "A-1"]);
}

/// Nothing leaves a document that is only reordered.
#[test]
fn reordering_keeps_the_outline_the_form_and_the_tags() {
    let document = open(&features("").bytes);
    let moved = reorder(&document, &[3, 2, 1]).unwrap();
    assert_eq!(
        outline(&moved),
        outline(&document)
            .iter()
            .map(|(level, title, page)| { (*level, title.clone(), page.map(|page| 4 - page)) })
            .collect::<Vec<_>>()
    );
    assert_eq!(field_names(&moved), ["_f2", "_f3"]);
    assert_eq!(structure_pages(&moved), [Some(3), Some(2), Some(1)]);
    assert_eq!(dest_names(&moved), ["-p1", "-p2", "-p3"]);
}
