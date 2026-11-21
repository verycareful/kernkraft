// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The page tree (`doc/pages.rs`).

use super::*;
use kernkraft_pdf::doc::{Document, Rectangle};
use kernkraft_pdf::{DiagnosticKind, PdfError};

fn open(objects: &[(u32, &[u8])]) -> Document {
    table_file(objects, "/Root 1 0 R").open()
}

/// Pages are numbered in document order, depth first, and take what they
/// lack from their nearest ancestor (7.7.3.4).
#[test]
fn order_and_inheritance() {
    let document = open(&[
        (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
        (
            2,
            b"<< /Type /Pages /Kids [3 0 R 6 0 R] /Count 3 /MediaBox [0 0 612 792] /Rotate 90 /Resources << /Font << >> >> >>",
        ),
        (3, b"<< /Type /Pages /Parent 2 0 R /Kids [4 0 R 5 0 R] /Count 2 /Rotate 180 /CropBox [10 10 600 780] >>"),
        (4, b"<< /Type /Page /Parent 3 0 R >>"),
        (5, b"<< /Type /Page /Parent 3 0 R /MediaBox [200 300 0 0] /Rotate -90 >>"),
        (6, b"<< /Type /Page /Parent 2 0 R /Rotate 450 /Resources 7 0 R >>"),
        (7, b"<< >>"),
    ]);
    assert_eq!(document.page_count().unwrap(), 3);
    let ids: Vec<_> = (1..=3).map(|n| document.page_id(n).unwrap()).collect();
    assert_eq!(ids, [id(4), id(5), id(6)]);

    let first = document.page(1).unwrap();
    assert_eq!(
        first.media_box,
        Some(Rectangle {
            llx: 0.0,
            lly: 0.0,
            urx: 612.0,
            ury: 792.0
        })
    );
    assert_eq!(
        first.crop_box,
        Some(Rectangle {
            llx: 10.0,
            lly: 10.0,
            urx: 600.0,
            ury: 780.0
        })
    );
    assert_eq!(first.rotate, 180);
    assert!(first.resources.is_some());

    // Its own MediaBox, corners in the other order (7.9.5), and -90.
    let second = document.page(2).unwrap();
    assert_eq!(
        second.media_box,
        Some(Rectangle {
            llx: 0.0,
            lly: 0.0,
            urx: 200.0,
            ury: 300.0
        })
    );
    assert_eq!(second.rotate, 270);

    let third = document.page(3).unwrap();
    assert_eq!(third.rotate, 90);
    assert_eq!(third.crop_box, None);
    assert_eq!(third.resources, Some(reference(7)));

    assert!(matches!(
        document.page(4),
        Err(PdfError::PageOutOfRange { page: 4, count: 3 })
    ));
    assert!(matches!(
        document.page(0),
        Err(PdfError::PageOutOfRange { page: 0, count: 3 })
    ));
}

#[test]
fn a_node_reached_twice_is_refused() {
    // 3 is a kid of 2 twice; then a tree whose node is its own kid.
    for kids in [&b"[3 0 R 3 0 R]"[..], b"[2 0 R]"] {
        let mut pages = b"<< /Type /Pages /Kids ".to_vec();
        pages.extend_from_slice(kids);
        pages.extend_from_slice(b" /Count 1 >>");
        let document = open(&[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, &pages),
            (3, b"<< /Type /Page >>"),
        ]);
        assert!(matches!(
            document.page_count(),
            Err(PdfError::PageTree { .. })
        ));
    }
}

#[test]
fn kids_that_are_not_references_to_dictionaries_are_refused() {
    for kids in [&b"[<< /Type /Page >>]"[..], b"[3 0 R]", b"[9 0 R]", b"5"] {
        let mut pages = b"<< /Type /Pages /Kids ".to_vec();
        pages.extend_from_slice(kids);
        pages.extend_from_slice(b" /Count 1 >>");
        let document = open(&[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, &pages),
            (3, b"42"),
        ]);
        assert!(
            matches!(document.page_count(), Err(PdfError::PageTree { .. })),
            "{}",
            String::from_utf8_lossy(kids)
        );
    }
}

/// A node is what its `Kids` make it; a `Type` that says otherwise is
/// reported, with the node and its offset.
#[test]
fn a_wrong_type_is_reported() {
    let fixture = table_file(
        &[
            (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
            (2, b"<< /Kids [3 0 R] /Count 1 >>"),
            (3, b"<< /Type /Pages >>"),
        ],
        "/Root 1 0 R",
    );
    let document = fixture.open();
    assert_eq!(document.page_count().unwrap(), 1);
    let found: Vec<_> = document
        .diagnostics()
        .into_iter()
        .filter(|d| matches!(d.kind, DiagnosticKind::PageTreeType { .. }))
        .map(|d| (d.kind, d.object, d.offset))
        .collect();
    let expected = |name: &str| DiagnosticKind::PageTreeType {
        expected: kernkraft_pdf::syntax::Name::new(name.as_bytes()),
    };
    assert_eq!(
        found,
        [
            (expected("Pages"), Some(id(2)), fixture.spans[&2].0 as u64),
            (expected("Page"), Some(id(3)), fixture.spans[&3].0 as u64),
        ]
    );
}

#[test]
fn bad_attributes_are_refused() {
    for (entry, key) in [
        (&b"/MediaBox [0 0 612]"[..], "MediaBox"),
        (b"/CropBox (box)", "CropBox"),
        (b"/Rotate 45", "Rotate"),
        (b"/Rotate 90.5", "Rotate"),
    ] {
        let mut page = b"<< /Type /Page ".to_vec();
        page.extend_from_slice(entry);
        page.extend_from_slice(b" >>");
        let document = open(&[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, &page),
        ]);
        match document.page(1) {
            Err(PdfError::InvalidEntry { object, key: found }) => {
                assert_eq!((object, found), (Some(id(3)), key));
            }
            other => panic!("{key}: {other:?}"),
        }
    }
}

/// A `Pages` that is not a reference is refused (Table 28).
#[test]
fn the_catalog_must_name_the_root_node() {
    let document = open(&[(1, b"<< /Type /Catalog /Pages << /Kids [] >> >>")]);
    assert!(matches!(
        document.page_count(),
        Err(PdfError::InvalidEntry { key: "Pages", .. })
    ));
}
