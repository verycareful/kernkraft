// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A file written with object numbers far above its length is written
//! with dense numbers (`write/renumber.rs`).

use super::*;
use kernkraft_pdf::doc::{ObjectBuilder, StreamEncoding};
use kernkraft_pdf::syntax::Dictionary;

/// A document whose page content is object 9000.
fn sparse() -> Fixture {
    table_file(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (2, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>"),
            (
                3,
                b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 10 10] /Contents 9000 0 R >>",
            ),
            (9000, &stream("", b"BT (sparse) Tj ET")),
        ],
        "/Root 1 0 R",
    )
}

fn highest(document: &Document) -> u32 {
    document.next_object_number() - 1
}

fn content(document: &Document) -> Vec<u8> {
    let page = document.page(1).unwrap();
    let Value::Reference(contents) = page.dictionary.get(b"Contents").unwrap().value().clone()
    else {
        panic!("contents is a reference");
    };
    let object = document.required(contents).unwrap();
    document
        .stream_data(contents, object.stream().unwrap())
        .unwrap()
        .into_owned()
}

#[test]
fn a_small_file_with_high_numbers_is_written_with_dense_ones() {
    let source = sparse().open();
    assert_eq!(highest(&source), 9000);
    let (bytes, written) = rewritten(&source, "");
    assert!(bytes.len() < 1000);
    // Four objects, and the numbers are 1 to 4.
    assert!(highest(&written) <= 4, "{}", highest(&written));
    assert_eq!(content(&written), b"BT (sparse) Tj ET");
    assert_eq!(written.page_count().unwrap(), 1);
    assert!(
        rewritten(&written, "").0 == bytes,
        "writing again changes nothing"
    );
}

#[test]
fn a_dense_file_keeps_its_numbers() {
    let source = open_pages(3);
    let (_, written) = rewritten(&source, "");
    assert_eq!(written.page_id(3).unwrap(), source.page_id(3).unwrap());
}

fn open_pages(pages: u32) -> Document {
    let objects = pages_document(pages);
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open()
}

#[test]
fn an_encrypted_file_is_encrypted_under_the_new_numbers() {
    for (name, password) in [
        ("rc4-128", "user"),
        ("aes-128", "user"),
        ("aes-256", "user"),
        ("aes-256-object-streams", "user"),
    ] {
        let source = Document::from_bytes(encrypted_fixture(name), &OPTIONS, password).unwrap();
        // A string and a stream far above the file's numbers, referred to
        // from the trailer.
        let mut builder = ObjectBuilder::new(&source);
        builder.reserve_many(60_000).unwrap();
        let note = builder
            .add_stream(
                Dictionary::new(),
                b"A secret stream".to_vec(),
                StreamEncoding::AsGiven,
            )
            .unwrap();
        let text = builder.add(string(b"A secret string")).unwrap();
        for (key, target) in [("Extra", note), ("Text", text)] {
            builder.set_trailer(
                Name::new(key.as_bytes()),
                Some(Object::new(Value::Reference(target))),
            );
        }
        let edited = source.apply(builder.finish()).unwrap();
        assert!(highest(&edited) > 60_000);
        let (bytes, written) = rewritten(&edited, password);
        assert!(highest(&written) < 1000, "{name}: {}", highest(&written));
        assert!(written.decryptor().is_some(), "{name}");
        let extra = written.trailer().get(b"Extra").unwrap().clone();
        let Value::Reference(extra) = extra.value() else {
            panic!("{name}");
        };
        let object = written.required(*extra).unwrap();
        assert_eq!(
            written
                .stream_data(*extra, object.stream().unwrap())
                .unwrap()
                .as_ref(),
            b"A secret stream",
            "{name}"
        );
        let text = written.trailer().get(b"Text").unwrap().clone();
        assert_eq!(
            written.resolve(&text).unwrap().object(),
            &string(b"A secret string"),
            "{name}"
        );
        for secret in [&b"A secret stream"[..], b"A secret string"] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{name}");
        }
        // Everything the plain document held is still there.
        let page = written.page(1).unwrap();
        assert!(page.dictionary.get(b"Contents").is_some(), "{name}");
        assert!(written.info().unwrap().is_some(), "{name}");
    }
}
