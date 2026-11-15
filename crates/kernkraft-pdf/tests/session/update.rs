// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What a save appends (7.5.6): the file kept as it was, one
//! cross-reference section with the objects that changed, a trailer that
//! reaches the one before.

use super::*;
use kernkraft_pdf::file::{SectionForm, XrefEntry};

#[test]
fn the_file_is_kept_and_one_section_with_the_changed_objects_follows() {
    let fixture = one_page();
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let bytes = save_bytes(&mut session, 1);

    assert!(bytes.starts_with(&fixture.bytes));
    assert!(bytes.len() > fixture.bytes.len());
    let reopened = opened(bytes.clone());
    let file = reopened.file();
    assert_eq!(file.sections().len(), 2);
    assert_eq!(file.sections()[0].form(), SectionForm::Table);
    // 7.5.6: the section has entries for the changed objects alone: the
    // content stream, and the information dictionary for its ModDate.
    assert_eq!(numbers(&reopened, 0), [3, 11]);
    let start = file.sections()[0].offset();
    assert!(start >= fixture.bytes.len() as u64);
    assert_eq!(last_startxref(&bytes) as u64, start);
    let trailer = file.sections()[0].trailer();
    assert_eq!(
        integer(trailer, b"Prev"),
        last_startxref(&fixture.bytes) as i64
    );
    // Table 15: one more than the highest object number.
    assert_eq!(integer(trailer, b"Size"), 51);
    let Some(XrefEntry::InUse {
        offset,
        generation: 0,
    }) = file.entry(11)
    else {
        panic!("{:?}", file.entry(11));
    };
    assert!(offset >= fixture.bytes.len() as u64);
    assert!(bytes[offset as usize..].starts_with(b"11 0 obj\n"));
    // An object that did not change is where it was.
    assert_eq!(
        file.entry(1),
        Some(XrefEntry::InUse {
            offset: fixture.spans[&1].0 as u64,
            generation: 0
        })
    );
    assert_eq!(stream_text(&reopened, 11), "changed");
    assert_eq!(
        info_string(&reopened, b"Title").as_deref(),
        Some(&b"Fixture"[..])
    );
}

#[test]
fn the_modification_date_is_set_and_the_rest_of_the_information_kept() {
    let mut session = session(&one_page());
    set_content(&mut session, "changed");
    let reopened = opened(save_bytes(&mut session, 1));
    assert_eq!(
        info_string(&reopened, b"ModDate").as_deref(),
        Some(DATE_TEXT)
    );
    assert_eq!(
        info_string(&reopened, b"Producer").as_deref(),
        Some(&b"kernkraft tests"[..])
    );
}

#[test]
fn a_file_without_information_gets_a_dictionary_for_the_date() {
    let objects = pages_document(1);
    let fixture = table_file(&borrowed(&objects), "/Root 1 0 R");
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let reopened = opened(save_bytes(&mut session, 1));
    // Object 50 is the highest, so the new object is 51.
    assert_eq!(
        reopened.trailer().get(b"Info").map(Object::value),
        Some(&Value::Reference(id(51)))
    );
    assert_eq!(integer(reopened.trailer(), b"Size"), 52);
    assert_eq!(numbers(&reopened, 0), [11, 51]);
    assert_eq!(
        info_string(&reopened, b"ModDate").as_deref(),
        Some(DATE_TEXT)
    );
}

#[test]
fn the_trailer_keeps_its_entries_and_the_second_identifier_changes() {
    let objects = pages_document(1);
    let fixture = table_file(
        &borrowed(&objects),
        &format!("/Root 1 0 R /Info 3 0 R /Custom (kept) {IDENTIFIER}"),
    );
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let reopened = opened(save_bytes(&mut session, 1));
    let trailer = reopened.trailer();
    assert_eq!(trailer.get(b"Custom"), Some(&string(b"kept")));
    let Some(Value::Array(identifier)) = trailer.get(b"ID").map(Object::value) else {
        panic!("no identifier");
    };
    // 14.4: the first part is permanent, the second says the update.
    let first: Vec<u8> = (0..16).map(|i| i * 0x11).collect();
    let second: Vec<u8> = (0..16).map(|i| 0xff - i * 0x11).collect();
    assert_eq!(identifier[0], string(&first));
    assert_ne!(identifier[1], string(&second));
    let Value::String(changed) = identifier[1].value() else {
        panic!("not a string");
    };
    assert_eq!(changed.as_bytes().len(), 16);
}

#[test]
fn the_same_edits_and_options_give_the_same_bytes() {
    let fixture = one_page();
    let save = |seed| {
        let mut session = session(&fixture);
        set_content(&mut session, "changed");
        save_bytes(&mut session, seed)
    };
    assert_eq!(save(1), save(1));
    // The second identifier comes from the seed.
    assert_ne!(save(1), save(2));
}

#[test]
fn a_document_that_does_not_differ_is_written_as_the_file_is() {
    let fixture = one_page();
    let mut session = session(&fixture);
    assert_eq!(save_bytes(&mut session, 1), fixture.bytes);
    // An edit undone differs no more.
    set_content(&mut session, "changed");
    session.undo();
    assert_eq!(save_bytes(&mut session, 1), fixture.bytes);
}

#[test]
fn an_update_starts_on_a_line_of_its_own() {
    let fixture = one_page();
    let mut without_end = fixture.bytes.clone();
    assert_eq!(without_end.pop(), Some(b'\n'));
    let mut session = EditSession::new(opened(without_end.clone())).unwrap();
    set_content(&mut session, "changed");
    let bytes = save_bytes(&mut session, 1);
    assert!(bytes.starts_with(&without_end));
    assert_eq!(bytes[without_end.len()], b'\n');
    let reopened = opened(bytes);
    assert_eq!(reopened.file().sections().len(), 2);
    assert_eq!(stream_text(&reopened, 11), "changed");
}

#[test]
fn a_new_object_takes_the_next_number() {
    let mut session = session(&one_page());
    let mut builder = session.builder();
    let added = builder.add(string(b"added")).unwrap();
    builder.set_trailer(name("Extra"), Some(Object::new(Value::Reference(added))));
    session.apply(builder.finish()).unwrap();
    assert_eq!(added, id(51));
    let reopened = opened(save_bytes(&mut session, 1));
    assert_eq!(integer(reopened.trailer(), b"Size"), 52);
    assert_eq!(numbers(&reopened, 0), [3, 51]);
    let extra = reopened.get(reopened.trailer(), b"Extra").unwrap();
    assert_eq!(extra.object(), &string(b"added"));
}
