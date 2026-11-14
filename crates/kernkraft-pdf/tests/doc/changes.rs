// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Edits and snapshots (`doc/changes.rs`).

use super::*;
use kernkraft_pdf::doc::{Content, ObjectBuilder, Origin, StreamData, StreamEncoding};
use kernkraft_pdf::syntax::{Dictionary, Value};
use kernkraft_pdf::PdfError;

/// An edit gives a new snapshot; the old one reads as it did.
#[test]
fn snapshots_do_not_see_each_others_edits() {
    let objects = pages_document(1);
    let original = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open();
    assert!(!original.is_edited());
    let first_new = original.next_object_number();
    assert_eq!(first_new, 51);

    let mut builder = ObjectBuilder::new(&original);
    let mut info = original.info().unwrap().unwrap();
    info.insert(
        kernkraft_pdf::syntax::Name::new(b"Title".as_slice()),
        string(b"Edited"),
    );
    builder.set(id(3), Object::new(Value::Dictionary(info)));
    let added = builder.add(string(b"new")).unwrap();
    let content = builder
        .add_stream(
            Dictionary::new(),
            b"0 0 m 10 10 l S".to_vec(),
            StreamEncoding::Flate,
        )
        .unwrap();
    builder.remove(id(50));
    let edited = original.apply(builder.finish()).unwrap();

    assert_eq!((added, content), (id(51), id(52)));
    assert_eq!(edited.next_object_number(), 53);
    assert!(edited.is_edited());
    assert_eq!(
        edited.info().unwrap().unwrap().get(b"Title"),
        Some(&string(b"Edited"))
    );
    assert_eq!(
        original.info().unwrap().unwrap().get(b"Title"),
        Some(&string(b"Fixture"))
    );
    assert!(edited.object(id(50)).unwrap().is_none());
    assert!(original.object(id(50)).unwrap().is_some());
    assert!(original.object(added).unwrap().is_none());

    let stream = edited.required(content).unwrap();
    assert_eq!(stream.origin, Origin::Edited);
    let Content::Stream(stream_body) = &stream.content else {
        panic!("not a stream");
    };
    assert!(matches!(stream_body.data, StreamData::Memory(_)));
    assert_eq!(
        stream_body.dictionary.get(b"Filter"),
        Some(&name("FlateDecode"))
    );
    let decoded = edited.decode_stream(content, stream_body, 1000).unwrap();
    assert_eq!(&*decoded.data, b"0 0 m 10 10 l S");
}

/// Changes that take new numbers apply to the snapshot they were built
/// for; others apply anywhere.
#[test]
fn new_numbers_belong_to_one_snapshot() {
    let objects = pages_document(1);
    let original = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    let mut first = ObjectBuilder::new(&original);
    first.add(string(b"a")).unwrap();
    let mut second = ObjectBuilder::new(&original);
    second.add(string(b"b")).unwrap();
    let edited = original.apply(first.finish()).unwrap();
    assert!(matches!(
        edited.apply(second.finish()),
        Err(PdfError::Invalid(_))
    ));
    let mut set_only = ObjectBuilder::new(&original);
    set_only.set(id(50), string(b"changed"));
    assert!(edited.apply(set_only.finish()).is_ok());
}

/// A trailer entry can be set and removed; the page list follows edits.
#[test]
fn trailer_and_pages_follow_edits() {
    let objects = pages_document(2);
    let original = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R").open();
    assert_eq!(original.page_count().unwrap(), 2);
    let mut builder = ObjectBuilder::new(&original);
    builder.set_trailer(kernkraft_pdf::syntax::Name::new(b"Info".as_slice()), None);
    builder.set(
        id(2),
        Object::new(Value::Dictionary({
            let mut pages = original
                .required(id(2))
                .unwrap()
                .dictionary()
                .unwrap()
                .clone();
            pages.insert(
                kernkraft_pdf::syntax::Name::new(b"Kids".as_slice()),
                Object::new(Value::Array(vec![reference(12)])),
            );
            pages
        })),
    );
    let edited = original.apply(builder.finish()).unwrap();
    assert_eq!(edited.info().unwrap(), None);
    assert_eq!(edited.page_count().unwrap(), 1);
    assert_eq!(edited.page_id(1).unwrap(), id(12));
    assert_eq!(original.page_count().unwrap(), 2);
}
