// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Edited text through an edit session: the file as opened is kept and the
//! edit is appended to it (ISO 32000-1, 7.5.6), and in an encrypted file
//! what the edit writes is encrypted with the file's key (7.6).

use super::*;
use kernkraft_fs::Expect;
use kernkraft_pdf::doc::{Date, Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::write::WriteOptions;
use kernkraft_pdf::{edit_text, EditSession, SaveOptions, TextEdit};

const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 1 << 16,
        max_xref_entries: 1 << 16,
        max_object_stream_bytes: 1 << 20,
    },
    cached_objects: 64,
    cached_object_streams: 4,
    max_diagnostics: 1000,
};

fn save_options() -> SaveOptions {
    SaveOptions {
        write: WriteOptions { seed: [3; 32] },
        modified: Date {
            year: 2026,
            month: 11,
            day: 20,
            hour: 10,
            minute: 30,
            second: 0,
            offset_minutes: Some(0),
        },
    }
}

/// Edits the one block of page 1 of `original` in a session, saves, and
/// gives the saved bytes.
fn edit_and_save(original: &[u8], password: &str, text: &str) -> Vec<u8> {
    let document = Document::from_bytes(original.to_vec(), &OPTIONS, password).unwrap();
    let mut session = EditSession::new(document).unwrap();
    let block = session
        .document()
        .text_page(1, &NoProgress)
        .unwrap()
        .blocks
        .remove(0);
    let edited = edit_text(
        session.document(),
        1,
        &block,
        &TextEdit::new(text),
        &NoProgress,
    )
    .unwrap();
    assert!(session.push(edited.document).unwrap());
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("saved.pdf");
    session
        .save(&path, Expect::Anything, &save_options(), &NoProgress)
        .unwrap();
    std::fs::read(path).unwrap()
}

#[test]
fn a_saved_edit_is_appended_to_the_file_as_it_was_opened() {
    let fixture = one_page("BT /F1 10 Tf 100 700 Td (Hello) Tj ET");
    let saved = edit_and_save(&fixture.bytes, "", "Howdy");
    assert!(saved.starts_with(&fixture.bytes));
    assert!(saved.len() > fixture.bytes.len());
    let reopened = Document::from_bytes(saved, &OPTIONS, "").unwrap();
    assert_eq!(
        reopened.text_page(1, &NoProgress).unwrap().text().trim(),
        "Howdy"
    );
}

#[test]
fn an_edit_of_an_encrypted_file_is_encrypted_with_its_key() {
    for (name, password) in [
        ("rc4-40", "user"),
        ("rc4-128", "user"),
        ("aes-128", "user"),
        ("aes-256", "user"),
        ("aes-256-object-streams", "user"),
    ] {
        let original = encrypted_fixture(name);
        let saved = edit_and_save(&original, password, "Hi there");
        assert!(saved.starts_with(&original), "{name}");
        // The new words appear nowhere in the update as plain bytes: the
        // content stream holding them is compressed and then encrypted.
        let update = &saved[original.len()..];
        assert!(
            !update.windows(8).any(|window| window == b"Hi there"),
            "{name}"
        );
        let reopened = Document::from_bytes(saved, &OPTIONS, password).unwrap();
        assert_eq!(
            reopened.text_page(1, &NoProgress).unwrap().text().trim(),
            "Hi there",
            "{name}"
        );
    }
}
