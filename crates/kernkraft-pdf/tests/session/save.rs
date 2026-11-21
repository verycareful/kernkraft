// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `EditSession::save`: the file it writes and what stops it.

use super::*;
use kernkraft_fs::{Expect, FileStamp, Progress};
use kernkraft_pdf::PdfError;

struct Cancelled;
impl Progress for Cancelled {
    fn report(&self, _: u64, _: u64, _: &str) {}
    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn saving_over_the_file_that_was_opened_keeps_reading_the_opened_bytes() {
    let fixture = one_page();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.pdf");
    std::fs::write(&path, &fixture.bytes).unwrap();
    let document = Document::open(&path, &OPTIONS, "").unwrap();
    let stamp = FileStamp::of(&path).unwrap();
    let mut session = EditSession::new(document).unwrap();
    set_content(&mut session, "one");
    let stamp = session
        .save(&path, Expect::Unchanged(&stamp), &options(1), &NoProgress)
        .unwrap();
    let first = std::fs::read(&path).unwrap();
    assert!(first.starts_with(&fixture.bytes));
    assert_eq!(opened(first.clone()).file().sections().len(), 2);

    // A second save is the original and one update again, not an update
    // of an update.
    set_content(&mut session, "two");
    session
        .save(&path, Expect::Unchanged(&stamp), &options(1), &NoProgress)
        .unwrap();
    let second = std::fs::read(&path).unwrap();
    assert!(second.starts_with(&fixture.bytes));
    let reopened = opened(second);
    assert_eq!(reopened.file().sections().len(), 2);
    assert_eq!(stream_text(&reopened, 11), "two");
}

#[test]
fn a_file_changed_since_it_was_read_is_not_overwritten() {
    let fixture = one_page();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.pdf");
    std::fs::write(&path, &fixture.bytes).unwrap();
    let stamp = FileStamp::of(&path).unwrap();
    let mut session = EditSession::new(Document::open(&path, &OPTIONS, "").unwrap()).unwrap();
    set_content(&mut session, "one");
    std::fs::write(&path, b"someone else wrote this").unwrap();
    let error = session
        .save(&path, Expect::Unchanged(&stamp), &options(1), &NoProgress)
        .unwrap_err();
    assert!(matches!(
        error,
        PdfError::Fs(kernkraft_fs::FsError::ExternalModification { .. })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), b"someone else wrote this");
    assert!(session.is_modified());
}

#[test]
fn a_cancelled_save_writes_nothing() {
    let fixture = one_page();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    // With an update to write, and without.
    let mut changed = session(&fixture);
    set_content(&mut changed, "one");
    assert!(matches!(
        changed.save(&path, Expect::Nothing, &options(1), &Cancelled),
        Err(PdfError::Cancelled)
    ));
    assert!(!path.exists());
    let mut unchanged = session(&fixture);
    assert!(matches!(
        unchanged.save(&path, Expect::Nothing, &options(1), &Cancelled),
        Err(PdfError::Cancelled)
    ));
    assert!(!path.exists());
    assert!(changed.is_modified());
}

#[test]
fn a_file_is_saved_with_nothing_replaced_unless_it_may_be() {
    let fixture = one_page();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("out.pdf");
    std::fs::write(&path, b"in the way").unwrap();
    let mut session = session(&fixture);
    set_content(&mut session, "one");
    assert!(session
        .save(&path, Expect::Nothing, &options(1), &NoProgress)
        .is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"in the way");
    session
        .save(&path, Expect::Anything, &options(1), &NoProgress)
        .unwrap();
    assert!(std::fs::read(&path).unwrap().starts_with(&fixture.bytes));
}

#[test]
fn a_large_file_is_copied_whole() {
    // More than one chunk of the copy.
    let mut objects = pages_document(1);
    let filler = format!("({})", "x".repeat(3 << 20)).into_bytes();
    objects.push((60, filler));
    let fixture = table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R");
    assert!(fixture.bytes.len() > 3 << 20);
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let bytes = save_bytes(&mut session, 1);
    assert!(bytes.starts_with(&fixture.bytes));
    assert_eq!(stream_text(&opened(bytes), 11), "changed");
}
