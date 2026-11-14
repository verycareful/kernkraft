// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The cross-reference and trailer written (`write/xref.rs`,
//! `write/mod.rs`).

use super::*;
use kernkraft_pdf::file::{SectionForm, XrefEntry};

/// A table file is written with a table; a number nothing reaches is a
/// free entry one generation on, linked into the free list (7.5.4).
#[test]
fn table_with_free_list() {
    // 4 is reached by nothing; 5 and 6 never existed.
    let fixture = table_file(
        &[
            (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (3, b"<< /Type /Page /Parent 2 0 R >>"),
            (4, b"(unreached)"),
            (7, b"<< /Title (t) >>"),
        ],
        "/Root 1 0 R /Info 7 0 R",
    );
    let (bytes, reopened) = rewritten(&fixture.open(), "");
    let file = reopened.file();
    assert_eq!(file.sections().len(), 1);
    assert_eq!(file.sections()[0].form(), SectionForm::Table);
    assert_eq!(
        file.entry(0),
        Some(XrefEntry::Free {
            next: 4,
            generation: 65535
        })
    );
    assert_eq!(
        file.entry(4),
        Some(XrefEntry::Free {
            next: 5,
            generation: 1
        })
    );
    assert_eq!(
        file.entry(5),
        Some(XrefEntry::Free {
            next: 6,
            generation: 0
        })
    );
    assert_eq!(
        file.entry(6),
        Some(XrefEntry::Free {
            next: 0,
            generation: 0
        })
    );
    assert!(matches!(
        file.entry(7),
        Some(XrefEntry::InUse { generation: 0, .. })
    ));
    let trailer = reopened.trailer();
    assert_eq!(trailer.get(b"Size"), Some(&Object::new(Value::Integer(8))));
    assert!(bytes.starts_with(b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n"));
    assert!(bytes.ends_with(b"%%EOF\n"));
}

/// The trailer keeps what the document's had but `Prev` and the like,
/// and gains a file identifier: both halves new when there was none, the
/// first kept when there was one (14.4).
#[test]
fn trailer_and_identifier() {
    let objects = pages_document(1);
    let fixture = table_file(
        &borrowed(&objects),
        "/Root 1 0 R /Info 3 0 R /Custom (kept)",
    );
    let (_, reopened) = rewritten(&fixture.open(), "");
    let trailer = reopened.trailer();
    assert_eq!(trailer.get(b"Custom"), Some(&string(b"kept")));
    let Some(Value::Array(id)) = trailer.get(b"ID").map(Object::value) else {
        panic!("no ID");
    };
    assert_eq!(id.len(), 2);
    assert_eq!(id[0], id[1]);
    let Value::String(first) = id[0].value() else {
        panic!();
    };
    assert_eq!(first.as_bytes().len(), 16);

    let fixture = table_file(
        &borrowed(&objects),
        "/Root 1 0 R /ID [<00112233445566778899AABBCCDDEEFF> <00112233445566778899AABBCCDDEEFF>]",
    );
    let (_, reopened) = rewritten(&fixture.open(), "");
    let Some(Value::Array(id)) = reopened.trailer().get(b"ID").map(Object::value) else {
        panic!("no ID");
    };
    let permanent: Vec<u8> = (0..16).map(|i| i * 0x11).collect();
    assert_eq!(id[0], string(&permanent));
    assert_ne!(id[1], string(&permanent));
}

/// A file with a cross-reference stream is written with one, which
/// carries the trailer's entries.
#[test]
fn cross_reference_stream() {
    let fixture = stream_file(
        &[],
        &[
            (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
            (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            (3, b"<< /Title (t) >>"),
        ],
        "/Root 1 0 R /Info 3 0 R",
    );
    let (_, reopened) = rewritten(&fixture.open(), "");
    let file = reopened.file();
    assert_eq!(file.sections()[0].form(), SectionForm::Stream);
    for number in 1..=3 {
        assert!(
            matches!(file.entry(number), Some(XrefEntry::Compressed { .. })),
            "{number}"
        );
    }
    assert_eq!(
        reopened.info().unwrap().unwrap().get(b"Title"),
        Some(&string(b"t"))
    );
    assert!(reopened.trailer().get(b"Prev").is_none());
}
