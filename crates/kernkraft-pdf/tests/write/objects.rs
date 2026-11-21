// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What is written and how (`write/objects.rs`).

use super::*;
use kernkraft_fs::{Expect, Progress};
use kernkraft_pdf::doc::{Content, ObjectBuilder, Origin, StreamEncoding};
use kernkraft_pdf::file::Location;
use kernkraft_pdf::syntax::{write_indirect_object, Dictionary};
use kernkraft_pdf::write::save_compact;
use kernkraft_pdf::{DiagnosticKind, PdfError};

/// Every object the trailer reaches is written as the bytes it was read
/// from, under its own number; object 50, which nothing reaches, is not.
#[test]
fn untouched_objects_are_copied_byte_for_byte() {
    let objects = pages_document(3);
    let fixture = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R");
    let (bytes, reopened) = rewritten(&fixture.open(), "");
    for (number, _) in &objects {
        if *number == 50 {
            assert!(reopened.object(id(50)).unwrap().is_none());
            continue;
        }
        assert_eq!(
            file_bytes(&reopened, &bytes, id(*number)),
            fixture.object_bytes(*number),
            "object {number}"
        );
    }
    assert_eq!(reopened.page_count().unwrap(), 3);
    assert!(
        reopened.diagnostics().is_empty(),
        "{:?}",
        reopened.diagnostics()
    );
}

/// An edited object is written anew, in canonical form; a new stream is
/// written with its data and a direct `Length`; a removed object is
/// gone, and so is what only it reached.
#[test]
fn edits_are_written() {
    let objects = pages_document(2);
    let document = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open();
    let mut builder = ObjectBuilder::new(&document);
    let mut info = Dictionary::new();
    info.insert(Name::new(b"Title".as_slice()), string(b"Rewritten"));
    let info_object = Object::new(Value::Dictionary(info));
    builder.set(id(3), info_object.clone());
    let added = builder
        .add_stream(Dictionary::new(), b"q Q".to_vec(), StreamEncoding::Flate)
        .unwrap();
    // The second page's content is replaced by the new stream; its old
    // content, object 13, is reached by nothing.
    let mut page = document
        .required(id(12))
        .unwrap()
        .dictionary()
        .unwrap()
        .clone();
    page.insert(
        Name::new(b"Contents".as_slice()),
        Object::new(Value::Reference(added)),
    );
    builder.set(id(12), Object::new(Value::Dictionary(page.clone())));
    let edited = document.apply(builder.finish()).unwrap();
    let (bytes, reopened) = rewritten(&edited, "");

    let mut expected = Vec::new();
    write_indirect_object(id(3), &info_object, &mut expected).unwrap();
    assert_eq!(
        file_bytes(&reopened, &bytes, id(3)),
        expected.trim_ascii_end()
    );
    assert!(reopened.object(id(13)).unwrap().is_none());
    let content = reopened.required(added).unwrap();
    let decoded = reopened
        .decode_stream(added, content.stream().unwrap(), 100)
        .unwrap();
    assert_eq!(&*decoded.data, b"q Q");
    assert_eq!(
        reopened.page(2).unwrap().dictionary.get(b"Contents"),
        Some(&Object::new(Value::Reference(added)))
    );
    // The first page's objects are as the fixture wrote them.
    assert_eq!(
        file_bytes(&reopened, &bytes, id(10)),
        document_bytes(&objects, 10)
    );
}

fn document_bytes(objects: &[(u32, Vec<u8>)], number: u32) -> Vec<u8> {
    let body = &objects.iter().find(|(n, _)| *n == number).unwrap().1;
    let mut bytes = format!("{number} 0 obj\n").into_bytes();
    bytes.extend_from_slice(body);
    bytes.extend_from_slice(b"\nendobj");
    bytes
}

/// An object whose reading tolerated something is written anew, and the
/// file written reads without it.
#[test]
fn damaged_objects_are_written_anew() {
    let fixture = table_file(
        &[
            (1, &b"<< /Type /Catalog /Pages 2 0 R /Damaged 3 0 R >>"[..]),
            (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            (3, b"<< /Length 2 >>\nstream\nabcdef\nendstream"),
        ],
        "/Root 1 0 R",
    );
    let document = fixture.open();
    document.required(id(3)).unwrap();
    assert!(document
        .diagnostics()
        .iter()
        .any(|d| d.kind == DiagnosticKind::StreamLength));
    let (_, reopened) = rewritten(&document, "");
    let stream = reopened.required(id(3)).unwrap();
    assert_eq!(
        stream.dictionary().unwrap().get(b"Length"),
        Some(&Object::new(Value::Integer(6)))
    );
    assert!(
        reopened.diagnostics().is_empty(),
        "{:?}",
        reopened.diagnostics()
    );
}

/// From a file with object streams, the compressed objects go back into
/// one, as their original bytes.
#[test]
fn compressed_objects_are_compressed_again() {
    let fixture = stream_file(
        &[(3, &b"<< /Type /Page /Parent 2 0 R >>"[..])],
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
        ],
        "/Root 1 0 R",
    );
    let (bytes, reopened) = rewritten(&fixture.open(), "");
    assert_eq!(
        file_bytes(&reopened, &bytes, id(3)),
        fixture.object_bytes(3)
    );
    for (number, original) in [
        (1, &b"<< /Type /Catalog /Pages 2 0 R >>"[..]),
        (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
    ] {
        let object = reopened.required(id(number)).unwrap();
        let Origin::File {
            location: Location::ObjectStream { stream, .. },
            verbatim: true,
        } = object.origin
        else {
            panic!("{number}: {:?}", object.origin);
        };
        let span = object.object().unwrap().span().unwrap();
        let data = reopened
            .file()
            .object_stream(stream, &mut kernkraft_pdf::Diagnostics::new())
            .unwrap()
            .data()
            .unwrap()
            .into_owned();
        assert_eq!(&data[span.start as usize..span.end as usize], original);
    }
    assert_eq!(reopened.page_count().unwrap(), 1);
}

/// The same document and seed give the same bytes; another seed gives
/// another second identifier.
#[test]
fn output_is_deterministic() {
    let objects = pages_document(2);
    let document = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    let first = compact(&document, &SEED, &NoProgress).unwrap();
    let second = compact(&document, &SEED, &NoProgress).unwrap();
    assert_eq!(first, second);
    let other = compact(&document, &WriteOptions { seed: [8; 32] }, &NoProgress).unwrap();
    assert_ne!(first, other);
    assert_eq!(first.len(), other.len());
}

struct Cancelled;
impl Progress for Cancelled {
    fn report(&self, _: u64, _: u64, _: &str) {}
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn a_cancelled_write_writes_nothing() {
    let objects = pages_document(1);
    let document = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    assert!(matches!(
        compact(&document, &SEED, &Cancelled),
        Err(PdfError::Cancelled)
    ));
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    assert!(matches!(
        save_compact(&document, &path, Expect::Nothing, &SEED, &Cancelled),
        Err(PdfError::Cancelled)
    ));
    assert!(!path.exists());
}

#[test]
fn save_writes_the_file() {
    let objects = pages_document(1);
    let document = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    save_compact(&document, &path, Expect::Nothing, &SEED, &NoProgress).unwrap();
    assert_eq!(
        std::fs::read(&path).unwrap(),
        compact(&document, &SEED, &NoProgress).unwrap()
    );
    let reopened = Document::open(&path, &OPTIONS, "").unwrap();
    assert_eq!(reopened.page_count().unwrap(), 1);
    assert!(matches!(
        reopened.required(id(11)).unwrap().content,
        Content::Stream(_)
    ));
}
