// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Revisions (`revisions.rs`, 7.5.6, Annex F).

use super::*;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{write_object, Value};
use kernkraft_pdf::{PdfError, Revision};

fn title(document: &Document) -> Vec<u8> {
    match document
        .info()
        .unwrap()
        .unwrap()
        .get(b"Title")
        .map(|o| o.value())
    {
        Some(Value::String(title)) => title.as_bytes().to_vec(),
        other => panic!("no title: {other:?}"),
    }
}

fn base() -> Fixture {
    table_file(&borrowed(&pages_document(1)), "/Root 1 0 R /Info 3 0 R")
}

#[test]
fn a_file_never_updated_has_one_revision() {
    let fixture = base();
    let revisions = fixture.open().revisions().unwrap();
    assert_eq!(
        revisions,
        [Revision {
            index: 0,
            sections: 0..1,
            length: Some(fixture.bytes.len() as u64),
            linearized: false,
        }]
    );
}

/// Each update is a revision, oldest first, ending at its `%%EOF`.
#[test]
fn each_update_is_a_revision() {
    let first = base();
    let (second, _) = append_update(
        &first,
        &[(3, b"<< /Title (Second) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let (third, _) = append_update(
        &second,
        &[(3, b"<< /Title (Third) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let document = third.open();
    let revisions = document.revisions().unwrap();
    let lengths: Vec<Option<u64>> = revisions.iter().map(|r| r.length).collect();
    assert_eq!(
        lengths,
        [first.bytes.len(), second.bytes.len(), third.bytes.len()].map(|n| Some(n as u64))
    );
    let sections: Vec<_> = revisions.iter().map(|r| r.sections.clone()).collect();
    assert_eq!(sections, [2..3, 1..2, 0..1]);
    assert_eq!(title(&document), b"Third");
    let titles: Vec<Vec<u8>> = (0..3)
        .map(|index| title(&document.open_revision(index).unwrap()))
        .collect();
    assert_eq!(
        titles,
        [b"Fixture".to_vec(), b"Second".to_vec(), b"Third".to_vec()]
    );
    assert!(matches!(
        document.open_revision(3),
        Err(PdfError::Invalid(_))
    ));
}

/// An earlier revision is opened without the edits of the snapshot.
#[test]
fn an_earlier_revision_has_no_edits() {
    let (updated, _) = append_update(
        &base(),
        &[(3, b"<< /Title (Second) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let document = updated.open();
    let mut builder = kernkraft_pdf::doc::ObjectBuilder::new(&document);
    builder.remove(id(50));
    let edited = document.apply(builder.finish()).unwrap();
    assert!(edited.is_edited());
    let revision = edited.open_revision(1).unwrap();
    assert!(!revision.is_edited());
    assert!(revision.object(id(50)).unwrap().is_some());
}

fn linearized(parameters: bool) -> (Fixture, usize, usize) {
    let objects = pages_document(1);
    let get = |number: u32| {
        objects
            .iter()
            .find(|(n, _)| *n == number)
            .unwrap()
            .1
            .clone()
    };
    let (catalog, pages, page, info, content) = (get(1), get(2), get(10), get(3), get(11));
    linearized_file(
        4,
        parameters,
        &[(1, &catalog), (2, &pages), (10, &page)],
        &[(3, &info), (11, &content)],
        "/Root 1 0 R /Info 3 0 R",
    )
}

/// The first-page and main sections of a linearized file are one
/// revision, which ends at the `%%EOF` after the main section.
#[test]
fn a_linearized_file_is_one_revision() {
    let (fixture, _, _) = linearized(true);
    let document = fixture.open();
    assert_eq!(document.file().sections().len(), 2);
    assert_eq!(
        document.revisions().unwrap(),
        [Revision {
            index: 0,
            sections: 0..2,
            length: Some(fixture.bytes.len() as u64),
            linearized: true,
        }]
    );
    assert_eq!(document.open_revision(0).unwrap().page_count().unwrap(), 1);

    let (updated, _) = append_update(
        &fixture,
        &[(3, b"<< /Title (Later) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let document = updated.open();
    let revisions = document.revisions().unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0].sections, 1..3);
    assert!(revisions[0].linearized);
    assert_eq!(revisions[0].length, Some(fixture.bytes.len() as u64));
    assert_eq!(revisions[1].length, Some(updated.bytes.len() as u64));
    let original = document.open_revision(0).unwrap();
    assert_eq!(title(&original), b"Fixture");
    assert_eq!(title(&document), b"Later");
}

/// Sections in that order without a linearization dictionary are two
/// revisions; the older one ends at no `startxref` of its own.
#[test]
fn reversed_sections_without_parameters() {
    let (fixture, _, _) = linearized(false);
    let revisions = fixture.open().revisions().unwrap();
    assert_eq!(revisions.len(), 2);
    assert!(revisions.iter().all(|revision| !revision.linearized));
    assert_eq!(revisions[0].length, None);
    assert_eq!(revisions[1].length, Some(fixture.bytes.len() as u64));
}

#[test]
fn a_rebuilt_file_has_one_revision_of_no_known_length() {
    let mut bytes = base().bytes;
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(at);
    let document = Document::from_bytes(bytes, &OPTIONS, "").unwrap();
    assert!(document.file().xref_rebuilt());
    assert_eq!(
        document.revisions().unwrap(),
        [Revision {
            index: 0,
            sections: 0..1,
            length: None,
            linearized: false,
        }]
    );
}

/// The end-of-line after `%%EOF` is part of the revision, whether it is a
/// line feed, a carriage return and line feed, or absent; with no marker
/// the revision ends after the offset's line.
#[test]
fn the_end_of_line_after_the_marker() {
    let plain = base().bytes;
    assert!(plain.ends_with(b"%%EOF\n"));
    let stem = &plain[..plain.len() - 1];
    for (tail, extra) in [(&b"\r\n"[..], 2), (b"\r", 1), (b"", 0), (b"\n\n", 1)] {
        let mut bytes = stem.to_vec();
        bytes.extend_from_slice(tail);
        let document = Document::from_bytes(bytes, &OPTIONS, "").unwrap();
        let length = document.revisions().unwrap()[0].length;
        assert_eq!(length, Some((stem.len() + extra) as u64), "{tail:?}");
    }
    let unmarked = &plain[..plain.len() - b"%%EOF\n".len()];
    let document = Document::from_bytes(unmarked.to_vec(), &OPTIONS, "").unwrap();
    assert_eq!(
        document.revisions().unwrap()[0].length,
        Some(unmarked.len() as u64)
    );
}

fn serialised(document: &Document, key: &[u8]) -> String {
    let mut out = Vec::new();
    write_object(document.trailer().get(key).unwrap(), &mut out).unwrap();
    String::from_utf8(out).unwrap()
}

/// The trailer's `ID` as an array of hexadecimal strings, which a text
/// trailer can hold whatever the identifier's bytes.
fn file_id(document: &Document) -> String {
    let Some(Value::Array(items)) = document.trailer().get(b"ID").map(|o| o.value()) else {
        panic!("no ID");
    };
    let strings: Vec<String> = items
        .iter()
        .map(|item| match item.value() {
            Value::String(id) => {
                let hex: String = id.as_bytes().iter().map(|b| format!("{b:02X}")).collect();
                format!("<{hex}>")
            }
            other => panic!("{other:?}"),
        })
        .collect();
    format!("[{}]", strings.join(" "))
}

/// An earlier revision of an encrypted file is read with the key the file
/// was opened with; one whose trailer names other encryption is refused.
#[test]
fn revisions_of_an_encrypted_file() {
    let bytes = encrypted_fixture("rc4-128");
    let opened = Document::from_bytes(bytes.clone(), &OPTIONS, "user").unwrap();
    let encrypt = serialised(&opened, b"Encrypt");
    let file_id = file_id(&opened);
    let base = Fixture {
        bytes,
        spans: Default::default(),
    };
    let pages = b"<</Type /Pages /Kids [3 0 R] /Count 1>>";
    let (updated, _) = append_update(
        &base,
        &[(2, pages)],
        &format!("/Root 1 0 R /Info 7 0 R /Encrypt {encrypt} /ID {file_id}"),
    );
    let document = Document::from_bytes(updated.bytes.clone(), &OPTIONS, "user").unwrap();
    assert_eq!(document.revisions().unwrap().len(), 2);
    let original = document.open_revision(0).unwrap();
    assert_eq!(title(&original), b"Kern encrypted fixture");
    assert!(original.decryptor().is_some());

    let (unencrypted, _) = append_update(&base, &[(2, pages)], "/Root 1 0 R /Info 7 0 R");
    let document = Document::from_bytes(unencrypted.bytes, &OPTIONS, "").unwrap();
    assert!(matches!(
        document.open_revision(0),
        Err(PdfError::RevisionEncryption { revision: 0 })
    ));
}
