// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use kernkraft_pdf::{rotate, PdfError, Rotation};

use super::*;

fn rotations(document: &Document) -> Vec<u16> {
    (1..=document.page_count().unwrap())
        .map(|number| document.page(number).unwrap().rotate)
        .collect()
}

#[test]
fn rotates_the_pages_asked_for() {
    let document = open(&pdf_n(3));
    let turned = rotate(&document, &[2], Rotation::Cw90).unwrap();
    assert_eq!(rotations(&turned), [0, 90, 0]);
    assert_eq!(rotations(&document), [0, 0, 0], "the original is untouched");
}

#[test]
fn no_pages_means_all_pages() {
    let turned = rotate(&open(&pdf_n(3)), &[], Rotation::Cw270).unwrap();
    assert_eq!(rotations(&turned), [270, 270, 270]);
}

#[test]
fn turns_add_up_and_wrap() {
    let document = open(&pdf_n(1));
    let once = rotate(&document, &[1], Rotation::Cw270).unwrap();
    let twice = rotate(&once, &[1], Rotation::Cw180).unwrap();
    assert_eq!(rotations(&twice), [90]);
}

/// The rotation page 2 and 3 of the nested document inherit is the base
/// the turn is added to, and it is written on the page itself.
#[test]
fn adds_to_an_inherited_rotation() {
    let document = open(&nested_pdf());
    assert_eq!(rotations(&document), [0, 90, 90]);
    let turned = rotate(&document, &[2], Rotation::Cw90).unwrap();
    assert_eq!(rotations(&turned), [0, 180, 90]);
    let page = turned.page(2).unwrap();
    assert!(page.dictionary.get(b"Rotate").is_some());
    assert_eq!(rotations(&reopen(&turned)), [0, 180, 90]);
}

#[test]
fn page_out_of_range_and_twice_are_refused() {
    let document = open(&pdf_n(2));
    assert!(matches!(
        rotate(&document, &[3], Rotation::Cw90),
        Err(PdfError::PageOutOfRange { page: 3, count: 2 })
    ));
    assert!(matches!(
        rotate(&document, &[0], Rotation::Cw90),
        Err(PdfError::PageOutOfRange { page: 0, .. })
    ));
    assert!(matches!(
        rotate(&document, &[1, 1], Rotation::Cw90),
        Err(PdfError::DuplicatePage(1))
    ));
}

#[test]
fn a_rotation_that_is_not_a_multiple_of_90_changes_nothing() {
    let bytes = flat(&[(100.0, 100.0), (100.0, 100.0)], "", "", "");
    let mut text = bytes.bytes;
    let at = text
        .windows(b"/Contents 11 0 R".len())
        .position(|w| w == b"/Contents 11 0 R")
        .unwrap();
    text.splice(at..at, b"/Rotate 45 ".iter().copied());
    // The patched file has stale offsets; the document is rebuilt.
    let document = Document::from_bytes(text, &OPTIONS, "")
        .and_then(|d| d.rebuilt())
        .unwrap();
    let error = rotate(&document, &[2], Rotation::Cw90);
    assert!(error.is_ok(), "page 2 is fine: {error:?}");
    assert!(matches!(
        rotate(&document, &[1], Rotation::Cw90),
        Err(PdfError::InvalidEntry { key: "Rotate", .. })
    ));
}

#[test]
fn rotation_degrees() {
    assert_eq!(Rotation::Cw90.degrees(), 90);
    assert_eq!(Rotation::Cw180.degrees(), 180);
    assert_eq!(Rotation::Cw270.degrees(), 270);
}
