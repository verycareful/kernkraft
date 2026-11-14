// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening a document and reading its objects (`doc/mod.rs`).

use super::*;
use kernkraft_pdf::doc::{Content, Document, OpenOptions, Origin, StreamData};
use kernkraft_pdf::file::Location;
use kernkraft_pdf::{DiagnosticKind, PdfError};

fn assert_shared<T: Send + Sync>() {}

/// A snapshot is shared between threads (E10).
#[test]
fn a_document_is_send_and_sync() {
    assert_shared::<Document>();
}

#[test]
fn objects_are_read_where_the_file_has_them() {
    let objects = pages_document(2);
    let fixture = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R");
    let document = fixture.open();
    let catalog = document.catalog().unwrap();
    assert_eq!(catalog.id, id(1));
    let (start, end) = fixture.spans[&1];
    assert_eq!(
        catalog.origin,
        Origin::File {
            location: Location::File(kernkraft_pdf::syntax::Span {
                start: start as u64,
                end: end as u64
            }),
            verbatim: true,
        }
    );
    let info = document.info().unwrap().unwrap();
    assert_eq!(info.get(b"Title"), Some(&string(b"Fixture")));
    // An object number the file does not have is null (7.3.10).
    assert!(document.object(id(99)).unwrap().is_none());
    assert!(matches!(
        document.required(id(99)),
        Err(PdfError::MissingObject { .. })
    ));
}

/// Objects of an object stream are read through one decoding, and say
/// where in it they were.
#[test]
fn compressed_objects() {
    let fixture = stream_file(
        &[(3, &b"(in the file)"[..])],
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
        ],
        "/Root 1 0 R",
    );
    let document = fixture.open();
    let catalog = document.catalog().unwrap();
    assert_eq!(
        catalog.origin,
        Origin::File {
            location: Location::ObjectStream {
                stream: 4,
                index: 0
            },
            verbatim: true,
        }
    );
    let pages = document.required(id(2)).unwrap();
    assert!(matches!(
        pages.origin,
        Origin::File {
            location: Location::ObjectStream { index: 1, .. },
            ..
        }
    ));
    assert_eq!(document.page_count().unwrap(), 0);
}

/// With nothing cached, every object is read again, and what reading it
/// found is recorded once.
#[test]
fn diagnostics_are_recorded_once_per_object() {
    // `Length` 3 is wrong: the data is 5 bytes (StreamLength).
    let mut damaged = b"<< /Length 3 >>\nstream\nabcde\nendstream".to_vec();
    damaged.shrink_to_fit();
    let fixture = table_file(
        &[
            (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
            (2, &damaged),
        ],
        "/Root 1 0 R",
    );
    let options = OpenOptions {
        cached_objects: 0,
        ..OPTIONS
    };
    let document = Document::from_bytes(fixture.bytes.clone(), &options, "").unwrap();
    for _ in 0..3 {
        let object = document.required(id(2)).unwrap();
        assert!(matches!(
            object.origin,
            Origin::File {
                verbatim: false,
                ..
            }
        ));
    }
    let lengths: Vec<_> = document
        .diagnostics()
        .into_iter()
        .filter(|d| d.kind == DiagnosticKind::StreamLength)
        .collect();
    assert_eq!(lengths.len(), 1);
    assert_eq!(lengths[0].object, Some(id(2)));
}

#[test]
fn diagnostics_past_the_bound_are_counted() {
    let bodies: Vec<(u32, Vec<u8>)> = (2..7)
        .map(|n| (n, b"<< /Length 1 >>\nstream\nabc\nendstream".to_vec()))
        .chain([(1, b"<< /Type /Catalog >>".to_vec())])
        .collect();
    let fixture = table_file(&borrowed(&bodies), "/Root 1 0 R");
    let options = OpenOptions {
        max_diagnostics: 2,
        ..OPTIONS
    };
    let document = Document::from_bytes(fixture.bytes.clone(), &options, "").unwrap();
    for number in 2..7 {
        document.required(id(number)).unwrap();
    }
    assert_eq!(document.diagnostics().len(), 2);
    assert_eq!(document.dropped_diagnostics(), 3);
}

#[test]
fn encrypted_files_open_with_their_passwords() {
    for name in ["rc4-40", "rc4-128-object-streams", "aes-128", "aes-256"] {
        let bytes = encrypted_fixture(name);
        assert!(matches!(
            Document::from_bytes(bytes.clone(), &OPTIONS, "wrong"),
            Err(PdfError::WrongPassword)
        ));
        for password in ["user", "owner"] {
            let document = Document::from_bytes(bytes.clone(), &OPTIONS, password).unwrap();
            assert!(document.decryptor().is_some());
            let info = document.info().unwrap().unwrap();
            assert_eq!(
                info.get(b"Title"),
                Some(&string(b"Kern encrypted fixture")),
                "{name}"
            );
            let page = document.page(1).unwrap();
            let contents = page.dictionary.get(b"Contents").unwrap().clone();
            let Value::Reference(contents) = contents.into_value() else {
                panic!("{name}: contents");
            };
            let stream = document.required(contents).unwrap();
            let decoded = document
                .decode_stream(contents, stream.stream().unwrap(), 1 << 16)
                .unwrap();
            assert_eq!(
                &*decoded.data, b"BT /F1 12 Tf 72 720 Td (Hello, encrypted world) Tj ET\n",
                "{name}"
            );
        }
    }
}

/// A file that is not encrypted ignores the password.
#[test]
fn plain_file_ignores_the_password() {
    let document = Document::from_bytes(encrypted_fixture("plain"), &OPTIONS, "anything").unwrap();
    assert!(document.decryptor().is_none());
}

/// `Filter` and `DecodeParms` may be indirect, and so may their items.
#[test]
fn indirect_filter_entries_are_followed() {
    let data = b"48656C6C6F>";
    let objects = [
        (1, &b"<< /Type /Catalog >>"[..]),
        (2, &b"[3 0 R]"[..]),
        (3, &b"/ASCIIHexDecode"[..]),
        (4, &stream("/Filter 2 0 R", data)[..]),
    ];
    let fixture = table_file(&objects, "/Root 1 0 R");
    let document = fixture.open();
    let stream = document.required(id(4)).unwrap();
    let decoded = document
        .decode_stream(id(4), stream.stream().unwrap(), 100)
        .unwrap();
    assert_eq!(&*decoded.data, b"Hello");
    assert!(decoded.complete);
    assert!(matches!(
        document.decode_stream(id(4), stream.stream().unwrap(), 4),
        Err(PdfError::DecodedTooLarge { cap: 4 })
    ));
}

/// The catalog must be a dictionary the trailer names indirectly.
#[test]
fn a_catalog_that_is_not_a_dictionary_is_refused() {
    let fixture = table_file(&[(1, &b"42"[..])], "/Root 1 0 R");
    let document = fixture.open();
    assert!(matches!(
        document.catalog(),
        Err(PdfError::InvalidEntry { key: "Root", .. })
    ));
}

/// A document reopened with a rebuilt cross-reference reads the same
/// objects.
#[test]
fn rebuilt_reads_the_objects_again() {
    let objects = pages_document(1);
    let document = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    let rebuilt = document.rebuilt().unwrap();
    assert!(rebuilt.file().xref_rebuilt());
    assert_eq!(rebuilt.page_count().unwrap(), 1);
    let stream = rebuilt.required(id(11)).unwrap();
    assert!(matches!(
        stream.content,
        Content::Stream(ref s) if matches!(s.data, StreamData::File(_))
    ));
}
