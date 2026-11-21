// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A save of a file whose `Info` is not a dictionary (14.3.3): the entry is
//! kept as it is, no `ModDate` is set, and a diagnostic says so.

use super::*;
use kernkraft_pdf::DiagnosticKind;

/// `pages_document(1)` with object 3, the information dictionary, replaced
/// by `body`.
fn with_info_object(body: &[u8]) -> Fixture {
    let mut objects = pages_document(1);
    for (number, bytes) in &mut objects {
        if *number == 3 {
            *bytes = body.to_vec();
        }
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

fn count(document: &Document) -> usize {
    document
        .diagnostics()
        .iter()
        .filter(|d| d.kind == DiagnosticKind::InfoNotDictionary)
        .count()
}

#[test]
fn an_info_that_is_an_integer_is_kept_and_the_edit_is_saved() {
    let fixture = with_info_object(b"42");
    let mut session = session(&fixture);
    set_content(&mut session, "changed");

    let saved = opened(save_bytes(&mut session, 1));

    assert_eq!(stream_text(&saved, 11), "changed");
    assert!(matches!(
        saved.get(saved.trailer(), b"Info").unwrap().value(),
        Value::Integer(42)
    ));
    assert!(saved.info().is_err());
    assert_eq!(count(session.document()), 1);
}

#[test]
fn an_info_written_in_the_trailer_that_is_not_a_dictionary_is_kept() {
    let objects = pages_document(1);
    let fixture = table_file(&borrowed(&objects), "/Root 1 0 R /Info 7");
    let mut session = session(&fixture);
    set_content(&mut session, "changed");

    let saved = opened(save_bytes(&mut session, 1));

    assert_eq!(stream_text(&saved, 11), "changed");
    assert_eq!(integer(saved.trailer(), b"Info"), 7);
    assert_eq!(count(session.document()), 1);
}

#[test]
fn an_append_of_such_a_file_keeps_the_info_entry() {
    let fixture = with_info_object(b"42");
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    set_content(&mut session, "changed");

    let appended = append(&mut session, base, 1);

    let mut bytes = fixture.bytes.clone();
    bytes.extend_from_slice(&appended.bytes);
    let reopened = opened(bytes);
    assert_eq!(stream_text(&reopened, 11), "changed");
    assert!(matches!(
        reopened.get(reopened.trailer(), b"Info").unwrap().value(),
        Value::Integer(42)
    ));
    assert_eq!(count(session.document()), 1);
}

#[test]
fn an_info_dictionary_still_takes_the_modification_date() {
    let fixture = one_page();
    let mut session = session(&fixture);
    set_content(&mut session, "changed");

    let saved = opened(save_bytes(&mut session, 1));

    assert_eq!(info_string(&saved, b"ModDate").as_deref(), Some(DATE_TEXT));
    assert_eq!(count(session.document()), 0);
}
