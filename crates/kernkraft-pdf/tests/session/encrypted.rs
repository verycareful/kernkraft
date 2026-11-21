// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! An update to an encrypted file (7.6, 14.4): what it writes is
//! encrypted with the key the file has, and the file keeps its first
//! identifier. The fixtures are those of the crypt tests.

use super::*;
use kernkraft_pdf::doc::{ObjectBuilder, StreamEncoding};
use kernkraft_pdf::PdfError;

const FIXTURES: [(&str, &str); 14] = [
    ("rc4-40", "user"),
    ("rc4-128", "user"),
    ("rc4-128-v4", "user"),
    ("rc4-128-object-streams", "user"),
    ("aes-128", "user"),
    ("aes-128-cleartext-metadata", "user"),
    ("aes-128-object-streams", "user"),
    ("aes-256-r5", "user"),
    ("aes-256", "user"),
    ("aes-256-restricted", "user"),
    ("aes-256-cleartext-metadata", "user"),
    ("aes-256-object-streams", "user"),
    ("aes-256-no-user-password", ""),
    ("aes-256-accented", "p\u{e4}ssw\u{f6}rd\u{20ac}"),
];

const NEW_TITLE: &[u8] = b"Edited under encryption";
const NEW_FILE: &[u8] = b"new embedded bytes";

fn open_with(bytes: Vec<u8>, password: &str) -> Document {
    Document::from_bytes(bytes, &OPTIONS, password).unwrap()
}

fn first_identifier(document: &Document) -> Object {
    match document.trailer().get(b"ID").map(Object::value) {
        Some(Value::Array(items)) => items[0].clone(),
        other => panic!("{other:?}"),
    }
}

/// Edits made to every fixture: a new title in the information
/// dictionary, and a new stream the trailer reaches.
fn edit(session: &mut EditSession) -> ObjectId {
    let document = session.document().clone();
    let Some(Value::Reference(info_id)) = document.trailer().get(b"Info").map(Object::value) else {
        panic!("no information dictionary");
    };
    let mut info = document.info().unwrap().unwrap();
    info.insert(name("Title"), string(NEW_TITLE));
    let mut builder = ObjectBuilder::new(&document);
    builder.set(*info_id, Object::new(Value::Dictionary(info)));
    let added = builder
        .add_stream(Dictionary::new(), NEW_FILE.to_vec(), StreamEncoding::Flate)
        .unwrap();
    builder.set_trailer(name("Extra"), Some(Object::new(Value::Reference(added))));
    session.apply(builder.finish()).unwrap();
    added
}

#[test]
fn what_an_update_writes_is_encrypted_with_the_key_of_the_file() {
    for (fixture, password) in FIXTURES {
        let original = encrypted_fixture(fixture);
        let mut session = EditSession::new(open_with(original.clone(), password)).unwrap();
        let added = edit(&mut session);
        let bytes = save_bytes(&mut session, 1);
        assert!(bytes.starts_with(&original), "{fixture}");
        let update = &bytes[original.len()..];

        let reopened = open_with(bytes.clone(), password);
        let source = open_with(original.clone(), password);
        assert_eq!(
            first_identifier(&reopened),
            first_identifier(&source),
            "{fixture}"
        );
        assert!(reopened.trailer().get(b"Encrypt").is_some(), "{fixture}");
        assert_eq!(
            info_string(&reopened, b"Title").as_deref(),
            Some(NEW_TITLE),
            "{fixture}"
        );
        assert_eq!(
            info_string(&reopened, b"ModDate").as_deref(),
            Some(DATE_TEXT),
            "{fixture}"
        );
        assert_eq!(
            stream_text(&reopened, added.number),
            "new embedded bytes",
            "{fixture}"
        );
        // Nothing the update wrote is in the clear. The title is a string;
        // the new stream's data is Flate coded, so its plain text is not
        // there either way, and its coded bytes are encrypted.
        let clear = |needle: &[u8]| update.windows(needle.len()).any(|w| w == needle);
        assert!(!clear(NEW_TITLE), "{fixture}");
        assert!(!clear(DATE_TEXT), "{fixture}");
        // The rest of the file is as it was.
        assert_eq!(
            reopened.page_count().unwrap(),
            source.page_count().unwrap(),
            "{fixture}"
        );
        assert!(
            reopened
                .diagnostics()
                .iter()
                .all(|d| d.kind == kernkraft_pdf::DiagnosticKind::PasswordEncoding),
            "{fixture}: {:?}",
            reopened.diagnostics()
        );
    }
}

#[test]
fn an_encrypted_file_without_a_save_is_copied() {
    for (fixture, password) in [("aes-256", "user"), ("rc4-128", "user")] {
        let original = encrypted_fixture(fixture);
        let mut session = EditSession::new(open_with(original.clone(), password)).unwrap();
        assert_eq!(save_bytes(&mut session, 1), original, "{fixture}");
    }
}

#[test]
fn a_locked_document_is_updated_with_its_embedded_file_as_stored() {
    let original = encrypted_fixture("attachments-only");
    let document = open_with(original.clone(), "");
    assert!(document.attachments_locked());
    // The embedded file's bytes in the file, which a locked document
    // cannot decrypt.
    let stored = |document: &Document| {
        let object = document.required(id(9)).unwrap();
        let kernkraft_pdf::doc::StreamData::File(span) = object.stream().unwrap().data else {
            panic!("the embedded file is not in the file");
        };
        document
            .file()
            .source()
            .read(span.start, (span.end - span.start) as usize)
            .unwrap()
            .into_owned()
    };
    let mut session = EditSession::new(document.clone()).unwrap();
    let mut info = document.info().unwrap().unwrap();
    info.insert(name("Title"), string(NEW_TITLE));
    let Some(Value::Reference(info_id)) = document.trailer().get(b"Info").map(Object::value) else {
        panic!("no information dictionary");
    };
    let mut builder = session.builder();
    builder.set(*info_id, Object::new(Value::Dictionary(info)));
    session.apply(builder.finish()).unwrap();
    let bytes = save_bytes(&mut session, 1);

    let reopened = open_with(bytes, "");
    assert!(reopened.attachments_locked());
    assert_eq!(info_string(&reopened, b"Title").as_deref(), Some(NEW_TITLE));
    assert_eq!(stored(&reopened), stored(&document));
    // The password opens the embedded file as it did.
    let unlocked = reopened.unlock_attachments("user").unwrap();
    let object = unlocked.required(id(9)).unwrap();
    let data = unlocked
        .decode_stream(id(9), object.stream().unwrap(), 1 << 16)
        .unwrap();
    assert_eq!(&*data.data, b"An attached note.\n");
}

#[test]
fn a_new_embedded_file_in_a_locked_document_is_refused_and_nothing_is_written() {
    let document = open_with(encrypted_fixture("attachments-only"), "");
    let mut session = EditSession::new(document).unwrap();
    let mut builder = session.builder();
    let mut dictionary = Dictionary::new();
    dictionary.insert(name("Type"), Object::new(Value::Name(name("EmbeddedFile"))));
    let added = builder
        .add_stream(dictionary, b"new file".to_vec(), StreamEncoding::AsGiven)
        .unwrap();
    builder.set_trailer(name("Extra"), Some(Object::new(Value::Reference(added))));
    session.apply(builder.finish()).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    assert!(matches!(
        session.save(
            &path,
            kernkraft_fs::Expect::Nothing,
            &options(1),
            &NoProgress
        ),
        Err(PdfError::AttachmentsLocked)
    ));
    assert!(!path.exists());
    assert!(session.is_modified());
}
