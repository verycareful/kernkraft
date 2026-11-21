// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the edit session (`session.rs`) and the incremental writer
//! (`write/incremental.rs`). The files are written by `pdf_fixture`, byte
//! by byte; the expectations come from ISO 32000-1:2008, 7.5.4 to 7.5.6
//! and from how each file is built.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod append;
mod encrypted;
mod forms;
mod free_list;
mod history;
mod info;
mod refusals;
mod save;
mod update;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::{Date, Document, StreamEncoding};
use kernkraft_pdf::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Value};
use kernkraft_pdf::write::WriteOptions;
use kernkraft_pdf::{EditSession, SaveOptions};

pub use pdf_fixture::*;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

pub const DATE: Date = Date {
    year: 2026,
    month: 11,
    day: 15,
    hour: 12,
    minute: 30,
    second: 5,
    offset_minutes: Some(0),
};

/// How 7.9.4 writes `DATE`.
pub const DATE_TEXT: &[u8] = b"D:20261115123005Z";

pub fn options(seed: u8) -> SaveOptions {
    SaveOptions {
        write: WriteOptions { seed: [seed; 32] },
        modified: DATE,
    }
}

/// The file of `pages_document(1)`: objects 1, 2, 3, 10, 11 and 50, with
/// a table and the information dictionary in object 3.
pub fn one_page() -> Fixture {
    let objects = pages_document(1);
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

pub fn session(fixture: &Fixture) -> EditSession {
    EditSession::new(fixture.open()).unwrap()
}

/// Makes `text` the content of page 1 of `pages_document`, object 11.
pub fn set_content(session: &mut EditSession, text: &str) {
    let mut builder = session.builder();
    builder
        .set_stream(
            id(11),
            Dictionary::new(),
            text.as_bytes().to_vec(),
            StreamEncoding::AsGiven,
        )
        .unwrap();
    assert!(session.apply(builder.finish()).unwrap());
}

/// The data of stream `number` as the document holds it.
pub fn stream_text(document: &Document, number: u32) -> String {
    let object = document.required(id(number)).unwrap();
    let data = document
        .decode_stream(id(number), object.stream().unwrap(), 1 << 16)
        .unwrap();
    String::from_utf8(data.data.into_owned()).unwrap()
}

/// The text of the string `key` in the information dictionary.
pub fn info_string(document: &Document, key: &[u8]) -> Option<Vec<u8>> {
    match document.info().unwrap()?.get(key).map(Object::value) {
        Some(Value::String(text)) => Some(text.as_bytes().to_vec()),
        _ => None,
    }
}

pub fn opened(bytes: Vec<u8>) -> Document {
    Document::from_bytes(bytes, &OPTIONS, "").unwrap()
}

pub fn integer(trailer: &Dictionary, key: &[u8]) -> i64 {
    match trailer.get(key).map(Object::value) {
        Some(Value::Integer(value)) => *value,
        other => panic!("{key:?}: {other:?}"),
    }
}

pub fn string(bytes: &[u8]) -> Object {
    Object::new(Value::String(PdfString::new(bytes)))
}

pub fn name(text: &str) -> Name {
    Name::new(text.as_bytes())
}

/// The numbers the section `index` of `document`'s file has entries for.
pub fn numbers(document: &Document, index: usize) -> Vec<u32> {
    document.file().sections()[index]
        .entries()
        .map(|(number, _)| number)
        .collect()
}

/// What saving `session` with `seed` writes to a new file.
pub fn save_bytes(session: &mut EditSession, seed: u8) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    session
        .save(
            &path,
            kernkraft_fs::Expect::Nothing,
            &options(seed),
            &NoProgress,
        )
        .unwrap();
    std::fs::read(path).unwrap()
}

/// The same hexadecimal string twice over, as 14.4 gives the two parts of
/// a file identifier.
pub const IDENTIFIER: &str =
    "/ID [<00112233445566778899aabbccddeeff> <ffeeddccbbaa99887766554433221100>]";

/// What `session.append_since(length)` returns, with `seed`.
pub fn append(session: &mut EditSession, length: u64, seed: u8) -> kernkraft_pdf::Appended {
    session
        .append_since(length, &options(seed), &NoProgress)
        .unwrap()
}

/// `pages_document(1)` and two objects nothing refers to, 51 and 52, in a
/// file whose free entries are the numbers it does not use.
pub fn with_spare_objects() -> Fixture {
    let mut objects = pages_document(1);
    objects.push((51, b"(fifty-one)".to_vec()));
    objects.push((52, b"(fifty-two)".to_vec()));
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

/// Removes the objects `numbers` in one step.
pub fn remove(session: &mut EditSession, numbers: &[u32]) {
    let mut builder = session.builder();
    for number in numbers {
        builder.remove(id(*number));
    }
    assert!(session.apply(builder.finish()).unwrap());
}

/// The free entries `document` reaches from entry 0, in order, as
/// (number, generation) pairs.
pub fn free_list(document: &Document) -> Vec<(u32, u16)> {
    use kernkraft_pdf::file::XrefEntry;
    let file = document.file();
    let mut list = Vec::new();
    let Some(XrefEntry::Free { next, .. }) = file.entry(0) else {
        panic!("entry 0 is not free");
    };
    let mut at = next;
    while at != 0 {
        let Some(XrefEntry::Free { next, generation }) = file.entry(at) else {
            panic!("entry {at} is not free");
        };
        list.push((at, generation));
        at = next;
        assert!(list.len() < 1000, "the free list loops");
    }
    list
}

/// `options(seed)` with the time of the save changed to `date`.
pub fn options_on(seed: u8, date: Date) -> SaveOptions {
    SaveOptions {
        modified: date,
        ..options(seed)
    }
}
