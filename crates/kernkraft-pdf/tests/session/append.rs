// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The update returned for an application that can only append to the
//! file (`EditSession::append_since`).

use super::*;
use kernkraft_pdf::file::XrefEntry;
use kernkraft_pdf::PdfError;

#[test]
fn the_bytes_appended_are_what_a_save_adds() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let appended = append(&mut session, base, 1);
    assert_eq!(appended.length, base + appended.bytes.len() as u64);
    let saved = save_bytes(&mut session, 1);
    assert_eq!(saved, [fixture.bytes.clone(), appended.bytes].concat());
}

#[test]
fn nothing_to_append_is_no_bytes() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    let appended = append(&mut session, base, 1);
    assert!(appended.bytes.is_empty());
    assert_eq!(appended.length, base);
    set_content(&mut session, "changed");
    session.undo();
    assert!(append(&mut session, base, 1).bytes.is_empty());
}

#[test]
fn a_length_the_session_did_not_leave_the_file_at_is_refused() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    for length in [0, base - 1, base + 1] {
        assert!(matches!(
            session.append_since(length, &options(1), &NoProgress),
            Err(PdfError::UnknownAppendPoint { length: found }) if found == length
        ));
    }
    assert!(matches!(
        session.confirm_appended(base + 1),
        Err(PdfError::UnknownAppendPoint { .. })
    ));
}

#[test]
fn the_session_is_modified_until_the_append_is_confirmed() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let appended = append(&mut session, base, 1);
    assert!(session.is_modified());
    session.confirm_appended(appended.length).unwrap();
    assert!(!session.is_modified());
    set_content(&mut session, "again");
    assert!(session.is_modified());
    session.undo();
    assert!(!session.is_modified());
    // The file the append left is not the file as opened.
    session.undo();
    assert!(session.is_modified());
}

#[test]
fn each_append_reaches_the_section_before_it() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "one");
    let first = append(&mut session, base, 1);
    set_content(&mut session, "two");
    let second = append(&mut session, first.length, 2);
    let after_first = [fixture.bytes.clone(), first.bytes].concat();
    let all = [after_first.clone(), second.bytes].concat();
    let reopened = opened(all);
    let sections = reopened.file().sections();
    assert_eq!(sections.len(), 3);
    assert_eq!(
        integer(sections[0].trailer(), b"Prev"),
        last_startxref(&after_first) as i64
    );
    assert_eq!(
        integer(sections[1].trailer(), b"Prev"),
        last_startxref(&fixture.bytes) as i64
    );
    assert_eq!(stream_text(&reopened, 11), "two");
    // Only what changed since the first append: the content.
    assert_eq!(numbers(&reopened, 0), [11]);
}

#[test]
fn a_later_save_sets_a_later_modification_date() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "one");
    let first = append(&mut session, base, 1);
    set_content(&mut session, "two");
    let later = Date { minute: 31, ..DATE };
    let second = session
        .append_since(first.length, &options_on(2, later), &NoProgress)
        .unwrap();
    let reopened = opened([fixture.bytes.clone(), first.bytes, second.bytes].concat());
    assert_eq!(
        info_string(&reopened, b"ModDate").as_deref(),
        Some(&b"D:20261115123105Z"[..])
    );
    assert_eq!(numbers(&reopened, 0), [3, 11]);
}

#[test]
fn an_edit_that_was_appended_and_undone_is_undone_by_the_next_append() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "one");
    let first = append(&mut session, base, 1);
    assert!(session.undo());
    let second = append(&mut session, first.length, 2);
    let reopened = opened([fixture.bytes.clone(), first.bytes, second.bytes.clone()].concat());
    // The content stream is the file's own again, at its own offset, and
    // is not written a second time.
    assert_eq!(
        reopened.file().entry(11),
        Some(XrefEntry::InUse {
            offset: fixture.spans[&11].0 as u64,
            generation: 0
        })
    );
    assert!(stream_text(&reopened, 11).contains("Page 1"));
    assert!(!second.bytes.windows(9).any(|w| w == b"11 0 obj\n"));
}

#[test]
fn the_file_as_opened_stays_a_point_after_an_append() {
    let fixture = one_page();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "one");
    let first = append(&mut session, base, 1);
    let again = append(&mut session, base, 1);
    assert_eq!(first, again);
}
