// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Applying, undoing and redoing edits, and whether the session is
//! modified (`session.rs`).

use super::*;

#[test]
fn a_new_session_is_unmodified_and_has_nothing_to_undo() {
    let session = session(&one_page());
    assert!(!session.is_modified());
    assert!(!session.can_undo());
    assert!(!session.can_redo());
}

#[test]
fn a_document_that_carries_edits_starts_modified() {
    let fixture = one_page();
    let document = fixture.open();
    let mut builder = kernkraft_pdf::doc::ObjectBuilder::new(&document);
    builder.set(id(50), string(b"edited"));
    let edited = document.apply(builder.finish()).unwrap();
    let session = EditSession::new(edited).unwrap();
    assert!(session.is_modified());
    assert!(!session.can_undo());
}

#[test]
fn undo_and_redo_move_between_the_documents() {
    let mut session = session(&one_page());
    assert_eq!(
        stream_text(session.document(), 11),
        "BT /F1 12 Tf 72 700 Td (Page 1) Tj ET"
    );
    set_content(&mut session, "one");
    set_content(&mut session, "two");
    assert_eq!(stream_text(session.document(), 11), "two");
    assert!(session.undo());
    assert_eq!(stream_text(session.document(), 11), "one");
    assert!(session.undo());
    assert!(stream_text(session.document(), 11).contains("Page 1"));
    assert!(!session.undo());
    assert!(session.redo());
    assert!(session.redo());
    assert_eq!(stream_text(session.document(), 11), "two");
    assert!(!session.redo());
}

#[test]
fn an_edit_after_an_undo_drops_what_could_be_redone() {
    let mut session = session(&one_page());
    set_content(&mut session, "one");
    set_content(&mut session, "two");
    session.undo();
    assert!(session.can_redo());
    set_content(&mut session, "three");
    assert!(!session.can_redo());
    assert_eq!(stream_text(session.document(), 11), "three");
    assert!(session.undo());
    assert_eq!(stream_text(session.document(), 11), "one");
}

#[test]
fn changes_that_change_nothing_add_no_step() {
    let mut session = session(&one_page());
    let builder = session.builder();
    assert!(!session.apply(builder.finish()).unwrap());
    assert!(!session.can_undo());
    assert!(!session.is_modified());
}

#[test]
fn modified_follows_the_document_back_to_the_file() {
    let mut session = session(&one_page());
    set_content(&mut session, "one");
    assert!(session.is_modified());
    session.undo();
    assert!(!session.is_modified());
    session.redo();
    assert!(session.is_modified());
}

#[test]
fn modified_is_measured_against_the_last_save() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    let mut session = session(&one_page());
    set_content(&mut session, "one");
    session
        .save(
            &path,
            kernkraft_fs::Expect::Nothing,
            &options(1),
            &NoProgress,
        )
        .unwrap();
    assert!(!session.is_modified());
    set_content(&mut session, "two");
    assert!(session.is_modified());
    // Back to what was saved.
    session.undo();
    assert!(!session.is_modified());
    // Back to what was opened: that is not what the file holds now.
    session.undo();
    assert!(session.is_modified());
}

#[test]
fn a_page_operation_is_a_step() {
    let fixture = one_page();
    let mut session = session(&fixture);
    let rotated =
        kernkraft_pdf::rotate(session.document(), &[1], kernkraft_pdf::Rotation::Cw90).unwrap();
    assert!(session.push(rotated).unwrap());
    assert_eq!(session.document().page(1).unwrap().rotate, 90);
    assert!(session.is_modified());
    assert!(session.undo());
    assert_eq!(session.document().page(1).unwrap().rotate, 0);
    assert!(session.redo());

    // Saved, the rotation is in the update, and the file is the file.
    let bytes = save_bytes(&mut session, 1);
    assert!(bytes.starts_with(&fixture.bytes));
    assert_eq!(opened(bytes).page(1).unwrap().rotate, 90);
}

#[test]
fn a_document_of_another_file_is_not_a_step() {
    let mut session = session(&one_page());
    let other = one_page().open();
    assert!(session.push(other).is_err());
    assert!(!session.can_undo());
    // Nor is the document as it stands one.
    let same = session.document().clone();
    assert!(!session.push(same).unwrap());
    assert!(!session.can_undo());
}
