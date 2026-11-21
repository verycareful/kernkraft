// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page labels (`labels.rs`, 12.4.2).

use super::*;
use kernkraft_pdf::{LabelStyle, PageLabel, PdfError};

fn shown(document: &Document) -> Vec<Option<String>> {
    document
        .page_labels()
        .unwrap()
        .unwrap()
        .iter()
        .map(|label| label.as_ref().map(PageLabel::to_string))
        .collect()
}

/// The example of 12.4.2: i, ii, iii, iv, 1, 2, 3, A-8, A-9.
#[test]
fn the_example_of_the_clause() {
    let fixture = document_with(
        9,
        "/PageLabels << /Nums [0 << /S /r >> 4 << /S /D >> 7 << /S /D /P (A-) /St 8 >>] >>",
        &[],
    );
    let document = fixture.open();
    assert_eq!(
        shown(&document),
        ["i", "ii", "iii", "iv", "1", "2", "3", "A-8", "A-9"].map(|s| Some(s.to_string()))
    );
    let labels = document.page_labels().unwrap().unwrap();
    assert_eq!(
        labels[7],
        Some(PageLabel {
            prefix: "A-".to_string(),
            style: Some(LabelStyle::Decimal),
            number: 8,
        })
    );
    assert!(document.diagnostics().is_empty());
}

#[test]
fn a_document_without_labels_has_none() {
    let document = document_with(2, "", &[]).open();
    assert_eq!(document.page_labels().unwrap(), None);
}

/// Label dictionaries and tree nodes may be indirect; a range with no
/// style is its prefix alone.
#[test]
fn indirect_nodes_and_a_prefix_alone() {
    let fixture = document_with(
        3,
        "/PageLabels 60 0 R",
        &[
            (60, b"<< /Kids [61 0 R] >>"),
            (
                61,
                b"<< /Limits [0 1] /Nums [0 62 0 R 1 << /S /A /St 27 >>] >>",
            ),
            (62, b"<< /Type /PageLabel /P (Contents) >>"),
        ],
    );
    let document = fixture.open();
    assert_eq!(
        shown(&document),
        [
            Some("Contents".to_string()),
            Some("AA".to_string()),
            Some("BB".to_string())
        ]
    );
}

/// 12.4.2 requires a range at page index 0. Without one, the pages before
/// the first range have no label, and that is reported.
#[test]
fn a_tree_that_starts_late() {
    let fixture = document_with(3, "/PageLabels << /Nums [2 << /S /R >>] >>", &[]);
    let document = fixture.open();
    assert_eq!(shown(&document), [None, None, Some("I".to_string())]);
    let late = of_kind(&document, &DiagnosticKind::PageLabelsStartLate);
    assert_eq!(late.len(), 1);
    assert_eq!(late[0].object, Some(id(1)));
    // Reading the labels again does not report it again.
    document.page_labels().unwrap();
    assert_eq!(
        of_kind(&document, &DiagnosticKind::PageLabelsStartLate).len(),
        1
    );
}

#[test]
fn entries_of_the_wrong_kind_are_refused() {
    for (dictionary, key) in [
        ("<< /S /X >>", "S"),
        ("<< /S (D) >>", "S"),
        ("<< /S /D /St 0 >>", "St"),
        ("<< /S /D /St 1.5 >>", "St"),
        ("<< /P /Name >>", "P"),
    ] {
        let fixture = document_with(1, &format!("/PageLabels << /Nums [0 {dictionary}] >>"), &[]);
        let error = fixture.open().page_labels().unwrap_err();
        assert!(
            matches!(error, PdfError::InvalidEntry { key: found, .. } if found == key),
            "{dictionary}: {error:?}"
        );
    }
}

/// A negative page index is no key of a page label tree.
#[test]
fn a_negative_key_is_refused() {
    let fixture = document_with(1, "/PageLabels << /Nums [-1 << /S /D >>] >>", &[]);
    assert!(matches!(
        fixture.open().page_labels(),
        Err(PdfError::InvalidTree { .. })
    ));
}

/// A first number past what a page count can reach is kept as it is.
#[test]
fn a_large_first_number() {
    let fixture = document_with(
        2,
        "/PageLabels << /Nums [0 << /S /D /St 9000000000 >>] >>",
        &[],
    );
    assert_eq!(
        shown(&fixture.open()),
        [
            Some("9000000000".to_string()),
            Some("9000000001".to_string())
        ]
    );
}
