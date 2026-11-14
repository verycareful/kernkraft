// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Encrypted files written (`write/objects.rs`, with the encryption of
//! `crypt/`). The fixtures are those of the crypt tests, written by
//! `scripts/make-encrypted-fixtures.py`; what they hold is restated here
//! from that script.

use super::*;
use kernkraft_pdf::doc::{ObjectBuilder, StreamEncoding};
use kernkraft_pdf::syntax::Dictionary;
use kernkraft_pdf::DiagnosticKind;

const TITLE: &[u8] = b"Kern encrypted fixture";
const CONTENT: &[u8] = b"BT /F1 12 Tf 72 720 Td (Hello, encrypted world) Tj ET\n";
const ATTACHMENT: &[u8] = b"An attached note.\n";

const FIXTURES: [(&str, &str); 18] = [
    ("rc4-40", "user"),
    ("rc4-40-restricted", "user"),
    ("rc4-128", "user"),
    ("rc4-128-restricted", "user"),
    ("rc4-128-accented", "p\u{e4}ssw\u{f6}rd\u{20ac}"),
    ("rc4-128-utf8-bytes", "p\u{e4}ssw\u{f6}rd\u{20ac}"),
    ("rc4-128-object-streams", "user"),
    ("rc4-128-v4", "user"),
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

fn decoded(document: &Document, value: &Object) -> Vec<u8> {
    let Value::Reference(id) = value.value() else {
        panic!("not a reference: {value:?}");
    };
    let object = document.required(*id).unwrap();
    document
        .decode_stream(*id, object.stream().unwrap(), 1 << 16)
        .unwrap()
        .data
        .into_owned()
}

/// What the plain document holds, read from `document`.
fn check(document: &Document, name: &str) {
    let info = document.info().unwrap().unwrap();
    assert_eq!(info.get(b"Title"), Some(&string(TITLE)), "{name}");
    let page = document.page(1).unwrap();
    assert_eq!(
        decoded(document, page.dictionary.get(b"Contents").unwrap()),
        CONTENT,
        "{name}"
    );
    let catalog = document.catalog().unwrap();
    let names = document
        .get(catalog.dictionary().unwrap(), b"Names")
        .unwrap();
    let files = document
        .get(names.dictionary().unwrap(), b"EmbeddedFiles")
        .unwrap();
    let tree = kernkraft_pdf::doc::NameTree::new(document, files.object().clone());
    let spec = tree.get(b"note.txt").unwrap().unwrap();
    let spec = document.resolve(&spec).unwrap();
    let ef = document.get(spec.dictionary().unwrap(), b"EF").unwrap();
    let file = ef.dictionary().unwrap().get(b"F").unwrap().clone();
    assert_eq!(decoded(document, &file), ATTACHMENT, "{name}");
}

/// Every form of the standard security handler is written encrypted with
/// its key, opens with its password, and holds what it held.
#[test]
fn encrypted_files_stay_encrypted() {
    for (name, password) in FIXTURES {
        let source = Document::from_bytes(encrypted_fixture(name), &OPTIONS, password).unwrap();
        let (bytes, reopened) = rewritten(&source, password);
        assert!(reopened.decryptor().is_some(), "{name}");
        assert_eq!(
            reopened.trailer().get(b"Encrypt").is_some(),
            source.trailer().get(b"Encrypt").is_some()
        );
        let first_id = |document: &Document| match document.trailer().get(b"ID").map(Object::value)
        {
            Some(Value::Array(items)) => items[0].clone(),
            other => panic!("{name}: {other:?}"),
        };
        assert_eq!(first_id(&reopened), first_id(&source), "{name}");
        check(&reopened, name);
        // The owner password still opens it, and a wrong one does not.
        assert!(
            Document::from_bytes(bytes.clone(), &OPTIONS, "owner").is_ok(),
            "{name}"
        );
        if !password.is_empty() {
            assert!(
                Document::from_bytes(bytes.clone(), &OPTIONS, "wrong").is_err(),
                "{name}"
            );
        }
        // Nothing of the content is in the file in the clear.
        assert!(
            !bytes.windows(CONTENT.len()).any(|window| window == CONTENT),
            "{name}"
        );
        // The one fixture whose password is its UTF-8 bytes is opened so,
        // as the source was.
        let found: Vec<_> = reopened
            .diagnostics()
            .into_iter()
            .filter(|d| {
                !(name == "rc4-128-utf8-bytes" && d.kind == DiagnosticKind::PasswordEncoding)
            })
            .collect();
        assert!(found.is_empty(), "{name}: {found:?}");
    }
}

/// New strings and streams of an encrypted document are encrypted when
/// written.
#[test]
fn edits_to_an_encrypted_file_are_encrypted() {
    for (name, password) in [
        ("rc4-128", "user"),
        ("aes-128", "user"),
        ("aes-256-object-streams", "user"),
    ] {
        let source = Document::from_bytes(encrypted_fixture(name), &OPTIONS, password).unwrap();
        let mut builder = ObjectBuilder::new(&source);
        let mut info = source.info().unwrap().unwrap();
        info.insert(
            Name::new(b"Subject".as_slice()),
            string(b"A secret subject"),
        );
        let info_id = match source.trailer().get(b"Info").map(Object::value) {
            Some(Value::Reference(id)) => *id,
            other => panic!("{other:?}"),
        };
        builder.set(info_id, Object::new(Value::Dictionary(info)));
        let note = builder
            .add_stream(
                Dictionary::new(),
                b"A secret stream".to_vec(),
                StreamEncoding::AsGiven,
            )
            .unwrap();
        builder.set_trailer(
            Name::new(b"Extra".as_slice()),
            Some(Object::new(Value::Reference(note))),
        );
        let edited = source.apply(builder.finish()).unwrap();
        let (bytes, reopened) = rewritten(&edited, password);
        assert_eq!(
            reopened.info().unwrap().unwrap().get(b"Subject"),
            Some(&string(b"A secret subject")),
            "{name}"
        );
        let extra = reopened.trailer().get(b"Extra").unwrap().clone();
        assert_eq!(decoded(&reopened, &extra), b"A secret stream", "{name}");
        for secret in [&b"A secret subject"[..], b"A secret stream"] {
            assert!(!bytes.windows(secret.len()).any(|w| w == secret), "{name}");
        }
        check(&reopened, name);
    }
}
