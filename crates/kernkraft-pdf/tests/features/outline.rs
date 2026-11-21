// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The document outline (`outline.rs`, 12.3.3).

use super::*;
use kernkraft_pdf::syntax::Name;
use kernkraft_pdf::{ActionKind, Destination, DestinationPage, View};

/// Two chapters of four pages, the first open with two sections, the
/// second closed with one; destinations explicit, by name (catalog
/// `Dests`), by string (the `Dests` name tree) and through a go-to
/// action.
pub fn chapters() -> Fixture {
    document_with(
        4,
        "/Outlines 70 0 R /Dests << /chap2 [16 0 R /FitR 1 2 3 4] >> \
         /Names << /Dests << /Names [(sec21) << /D [16 0 R /FitB] >>] >> >>",
        &[
            (
                70,
                b"<< /Type /Outlines /First 71 0 R /Last 74 0 R /Count 3 >>",
            ),
            (
                71,
                b"<< /Title (Chapter 1) /Parent 70 0 R /Next 74 0 R /First 72 0 R \
                  /Last 73 0 R /Count 2 /Dest [10 0 R /XYZ 0 792 0] >>",
            ),
            (
                72,
                b"<< /Title (Section 1.1) /Parent 71 0 R /Next 73 0 R /Dest [12 0 R /Fit] >>",
            ),
            (
                73,
                b"<< /Title (Section 1.2) /Parent 71 0 R /Prev 72 0 R \
                  /A << /S /GoTo /D [14 0 R /FitH 500] >> >>",
            ),
            (
                74,
                b"<< /Title (Chapter 2) /Parent 70 0 R /Prev 71 0 R /First 75 0 R \
                  /Last 75 0 R /Count -1 /Dest /chap2 /C [1 0 0.5] /F 3 >>",
            ),
            (
                75,
                b"<< /Title <FEFF00DC006E00EF> /Parent 74 0 R /Dest (sec21) >>",
            ),
        ],
    )
}

#[test]
fn items_in_reading_order_with_levels_and_parents() {
    let document = chapters().open();
    let items = document.outline().unwrap();
    let shape: Vec<(u32, String, u32, Option<usize>, bool)> = items
        .iter()
        .map(|item| {
            (
                item.id.number,
                item.title.text.clone(),
                item.level,
                item.parent,
                item.open,
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            (71, "Chapter 1".to_string(), 0, None, true),
            (72, "Section 1.1".to_string(), 1, Some(0), false),
            (73, "Section 1.2".to_string(), 1, Some(0), false),
            (74, "Chapter 2".to_string(), 0, None, false),
            (75, "\u{dc}n\u{ef}".to_string(), 1, Some(3), false),
        ]
    );
    assert!(
        document.diagnostics().is_empty(),
        "{:?}",
        document.diagnostics()
    );
}

#[test]
fn destinations_of_every_form() {
    let items = chapters().open().outline().unwrap();
    let destinations: Vec<Option<Destination>> = items.iter().map(|i| i.destination).collect();
    assert_eq!(
        destinations,
        [
            // A zoom of 0 keeps the viewer's (Table 151).
            Some(Destination {
                page: DestinationPage::Page(1),
                view: View::Xyz {
                    left: Some(0.0),
                    top: Some(792.0),
                    zoom: None
                },
            }),
            Some(Destination {
                page: DestinationPage::Page(2),
                view: View::Fit,
            }),
            Some(Destination {
                page: DestinationPage::Page(3),
                view: View::FitH { top: Some(500.0) },
            }),
            Some(Destination {
                page: DestinationPage::Page(4),
                view: View::FitR {
                    left: 1.0,
                    bottom: 2.0,
                    right: 3.0,
                    top: 4.0
                },
            }),
            Some(Destination {
                page: DestinationPage::Page(4),
                view: View::FitB,
            }),
        ]
    );
    assert_eq!(items[2].actions.len(), 1);
    assert_eq!(items[2].actions[0].kind, ActionKind::GoTo);
}

#[test]
fn style_and_colour() {
    let items = chapters().open().outline().unwrap();
    let chapter = &items[3];
    assert_eq!(chapter.colour, Some([1.0, 0.0, 0.5]));
    assert!(chapter.italic && chapter.bold);
    assert_eq!(items[0].colour, None);
    assert!(!items[0].italic && !items[0].bold);
}

#[test]
fn a_document_without_an_outline() {
    assert!(document_with(1, "", &[])
        .open()
        .outline()
        .unwrap()
        .is_empty());
}

/// An item reached again ends the chain it was reached along.
#[test]
fn a_loop_ends_its_chain() {
    let document = document_with(
        1,
        "/Outlines 70 0 R",
        &[
            (70, b"<< /First 71 0 R >>"),
            (71, b"<< /Title (one) /Next 72 0 R /First 71 0 R >>"),
            (72, b"<< /Title (two) /Next 71 0 R >>"),
        ],
    )
    .open();
    let titles: Vec<String> = document
        .outline()
        .unwrap()
        .into_iter()
        .map(|item| item.title.text)
        .collect();
    assert_eq!(titles, ["one", "two"]);
    let loops = of_kind(&document, &DiagnosticKind::OutlineLoop);
    assert_eq!(loops.len(), 2);
}

fn ignored(key: &str) -> DiagnosticKind {
    DiagnosticKind::IgnoredEntry {
        key: Name::new(key.as_bytes()),
    }
}

/// Entries that are not what Table 153 defines are left out and reported.
#[test]
fn entries_of_the_wrong_kind_are_left_out() {
    let document = document_with(
        1,
        "/Outlines 70 0 R",
        &[
            (70, b"<< /First 71 0 R >>"),
            (
                71,
                b"<< /Next 99 0 R /First << /Title (direct) >> /C [2 0 0] /F 7 /Count /x >>",
            ),
        ],
    )
    .open();
    let items = document.outline().unwrap();
    assert_eq!(items.len(), 1);
    let item = &items[0];
    assert_eq!(item.title.text, "");
    assert_eq!(item.colour, None);
    // Of `F` 7 the first two bits are read.
    assert!(item.italic && item.bold);
    assert!(!item.open);
    for key in ["Title", "C", "F", "First", "Next", "Count"] {
        assert_eq!(
            of_kind(&document, &ignored(key)).len(),
            1,
            "{key}: {:?}",
            document.diagnostics()
        );
    }
}
