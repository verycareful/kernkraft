// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A file that encrypts only its embedded files and asks for their
//! password when one is opened (Table 20 `EFF`, Table 25 `AuthEvent`
//! `EFOpen`): `attachments-only.pdf`, which the fixture script writes from
//! the plain document with its embedded file, object 9, encrypted under
//! RC4 128 for the user password `user` and the owner password `owner`.

use super::*;
use kernkraft_fs::NoProgress;
use kernkraft_pdf::crypt::Access;
use kernkraft_pdf::doc::{Document, ObjectBuilder, OpenOptions, StreamEncoding};
use kernkraft_pdf::syntax::Name;
use kernkraft_pdf::write::{compact, WriteOptions};

const NAME: &str = "attachments-only";
const EMBEDDED_FILE: ObjectId = ObjectId {
    number: 9,
    generation: 0,
};
const OPTIONS: OpenOptions = OpenOptions {
    limits: ROOMY,
    cached_objects: 64,
    cached_object_streams: 4,
    max_diagnostics: 1000,
};
const SEED: WriteOptions = WriteOptions { seed: [3; 32] };

fn open(bytes: Vec<u8>, password: &str) -> Result<Document, PdfError> {
    Document::from_bytes(bytes, &OPTIONS, password)
}

fn title(document: &Document) -> Vec<u8> {
    match document
        .info()
        .unwrap()
        .unwrap()
        .get(b"Title")
        .map(Object::value)
    {
        Some(Value::String(title)) => title.as_bytes().to_vec(),
        other => panic!("no title: {other:?}"),
    }
}

fn attachment_data(document: &Document) -> Result<Vec<u8>, PdfError> {
    let attachments = document.attachments()?;
    assert_eq!(attachments.len(), 1);
    assert_eq!(attachments[0].stream, EMBEDDED_FILE);
    Ok(document.attachment_data(&attachments[0], 1 << 16)?.data)
}

#[test]
fn the_dictionary_says_only_attachments_are_encrypted() {
    let mut diagnostics = Diagnostics::new();
    let file = open_bytes(fixture(NAME), &mut diagnostics).unwrap();
    let encryption = Encryption::read(&file, &mut diagnostics).unwrap().unwrap();
    assert!(encryption.encrypts_only_attachments());
    for other in ["rc4-128-v4", "aes-128", "aes-256"] {
        let file = open_bytes(fixture(other), &mut diagnostics).unwrap();
        let encryption = Encryption::read(&file, &mut diagnostics).unwrap().unwrap();
        assert!(!encryption.encrypts_only_attachments(), "{other}");
    }
}

/// Without a password everything but the embedded file reads, and the
/// embedded file is refused, never handed back encrypted.
#[test]
fn opened_without_a_password() {
    let document = open(fixture(NAME), "").unwrap();
    assert!(document.attachments_locked());
    assert_eq!(document.decryptor().unwrap().access(), Access::Locked);
    assert_eq!(title(&document), TITLE);
    assert_eq!(document.page_count().unwrap(), 1);
    let page = document.page(1).unwrap();
    let Some(Value::Reference(content)) = page.dictionary.get(b"Contents").map(Object::value)
    else {
        panic!("no contents");
    };
    let content = document.required(*content).unwrap();
    let data = document
        .decode_stream(content.id, content.stream().unwrap(), 1 << 16)
        .unwrap();
    assert_eq!(&data.data[..], CONTENT);
    assert!(matches!(
        attachment_data(&document),
        Err(PdfError::AttachmentsLocked)
    ));
}

#[test]
fn unlocking_the_attachments() {
    let document = open(fixture(NAME), "").unwrap();
    assert!(matches!(
        document.unlock_attachments("wrong"),
        Err(PdfError::WrongPassword)
    ));
    for password in [USER, OWNER] {
        let unlocked = document.unlock_attachments(password).unwrap();
        assert!(!unlocked.attachments_locked());
        assert_eq!(
            attachment_data(&unlocked).unwrap(),
            ATTACHMENT,
            "{password}"
        );
    }
    // The locked snapshot stays as it was.
    assert!(document.attachments_locked());
}

#[test]
fn edits_carry_over_when_unlocking() {
    let document = open(fixture(NAME), "").unwrap();
    let mut builder = ObjectBuilder::new(&document);
    let added = builder
        .add(Object::new(Value::Name(Name::new(&b"Added"[..]))))
        .unwrap();
    let edited = document.apply(builder.finish()).unwrap();
    let unlocked = edited.unlock_attachments(USER).unwrap();
    assert!(unlocked.object(added).unwrap().is_some());
    assert_eq!(attachment_data(&unlocked).unwrap(), ATTACHMENT);
}

/// A password given at open opens the embedded files too; a wrong one is
/// refused as for any encrypted file.
#[test]
fn opened_with_a_password() {
    let document = open(fixture(NAME), USER).unwrap();
    assert!(!document.attachments_locked());
    assert_eq!(attachment_data(&document).unwrap(), ATTACHMENT);
    assert!(matches!(
        open(fixture(NAME), "wrong"),
        Err(PdfError::WrongPassword)
    ));
}

/// `fixture` with `from` replaced by `to`, which is as long.
fn replaced(from: &[u8], to: &[u8]) -> Vec<u8> {
    assert_eq!(from.len(), to.len());
    let mut bytes = fixture(NAME);
    let at = bytes.windows(from.len()).position(|w| w == from).unwrap();
    bytes[at..at + to.len()].copy_from_slice(to);
    bytes
}

/// With `AuthEvent` `DocOpen` the password is asked for at open (Table
/// 25); an `AuthEvent` the table does not define is refused.
#[test]
fn other_auth_events() {
    let doc_open = replaced(b"/AuthEvent /EFOpen", b"/AuthEvent/DocOpen");
    assert!(matches!(
        open(doc_open.clone(), ""),
        Err(PdfError::WrongPassword)
    ));
    assert_eq!(
        attachment_data(&open(doc_open, USER).unwrap()).unwrap(),
        ATTACHMENT
    );
    let unknown = replaced(b"/EFOpen", b"/XXOpen");
    assert!(matches!(
        open(unknown, ""),
        Err(PdfError::EncryptionDictionary { key: "AuthEvent" })
    ));
}

/// A locked document is written with its embedded file as stored, which
/// the password then opens; a new embedded file, which `EFF` would
/// encrypt, is refused.
#[test]
fn writing_a_locked_document() {
    let document = open(fixture(NAME), "").unwrap();
    let copy = compact(&document, &SEED, &NoProgress).unwrap();
    let reopened = open(copy.clone(), "").unwrap();
    assert!(reopened.attachments_locked());
    let reopened = open(copy, USER).unwrap();
    assert_eq!(attachment_data(&reopened).unwrap(), ATTACHMENT);

    let mut builder = ObjectBuilder::new(&document);
    let mut dictionary = Dictionary::new();
    dictionary.insert(
        Name::new(&b"Type"[..]),
        Object::new(Value::Name(Name::new(&b"EmbeddedFile"[..]))),
    );
    let added = builder
        .add_stream(dictionary, b"new file".to_vec(), StreamEncoding::AsGiven)
        .unwrap();
    // Reached from the trailer, so that the writer writes it.
    builder.set_trailer(
        Name::new(&b"Extra"[..]),
        Some(Object::new(Value::Reference(added))),
    );
    let edited = document.apply(builder.finish()).unwrap();
    assert!(matches!(
        compact(&edited, &SEED, &NoProgress),
        Err(PdfError::AttachmentsLocked)
    ));
}
