// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The free entries of an update (7.5.4): a removed object is free one
//! generation on and linked into the list entry 0 heads.

use super::*;
use kernkraft_pdf::file::XrefEntry;

#[test]
fn removed_objects_are_free_and_linked_in_order() {
    let fixture = with_spare_objects();
    let mut session = session(&fixture);
    remove(&mut session, &[52, 50]);
    let reopened = opened(save_bytes(&mut session, 1));
    let file = reopened.file();
    // The list starts at the lowest number and ends at 0; the entries are
    // one generation on (7.5.4).
    assert_eq!(
        file.entry(0),
        Some(XrefEntry::Free {
            next: 50,
            generation: 65535
        })
    );
    assert_eq!(
        file.entry(50),
        Some(XrefEntry::Free {
            next: 52,
            generation: 1
        })
    );
    assert_eq!(
        file.entry(52),
        Some(XrefEntry::Free {
            next: 0,
            generation: 1
        })
    );
    assert_eq!(free_list(&reopened), [(50, 1), (52, 1)]);
    assert_eq!(numbers(&reopened, 0), [0, 3, 50, 52]);
    assert!(reopened.object(id(50)).unwrap().is_none());
    assert!(reopened.object(id(52)).unwrap().is_none());
    // The object that was not removed is still there.
    assert!(reopened.object(id(51)).unwrap().is_some());
    assert_eq!(integer(reopened.trailer(), b"Size"), 53);
}

#[test]
fn a_removal_joins_the_list_the_file_has() {
    let fixture = with_spare_objects();
    let mut session = session(&fixture);
    let base = fixture.bytes.len() as u64;
    remove(&mut session, &[52]);
    let first = append(&mut session, base, 1);
    remove(&mut session, &[50]);
    let second = append(&mut session, first.length, 2);

    let all = [fixture.bytes.clone(), first.bytes, second.bytes.clone()].concat();
    let reopened = opened(all);
    assert_eq!(reopened.file().sections().len(), 3);
    assert_eq!(free_list(&reopened), [(50, 1), (52, 1)]);
    // Only the entries that changed are in the second section: entry 0 and
    // the new free entry. The entry of 52 is the first section's.
    assert_eq!(numbers(&reopened, 0), [0, 50]);
    assert_eq!(
        reopened.file().sections()[0].entry(50),
        Some(XrefEntry::Free {
            next: 52,
            generation: 1
        })
    );
}

#[test]
fn an_object_that_is_back_in_use_leaves_the_list() {
    let fixture = with_spare_objects();
    let mut session = session(&fixture);
    let base = fixture.bytes.len() as u64;
    remove(&mut session, &[52]);
    let first = append(&mut session, base, 1);
    remove(&mut session, &[50]);
    let second = append(&mut session, first.length, 2);
    // Removing 50 is undone, and the file is brought to that.
    assert!(session.undo());
    let third = append(&mut session, second.length, 3);

    let all = [
        fixture.bytes.clone(),
        first.bytes,
        second.bytes,
        third.bytes.clone(),
    ]
    .concat();
    let reopened = opened(all);
    assert_eq!(free_list(&reopened), [(52, 1)]);
    // 50 is the object it was, where it was: its bytes are not written
    // again, only its entry.
    assert_eq!(
        reopened.file().entry(50),
        Some(XrefEntry::InUse {
            offset: fixture.spans[&50].0 as u64,
            generation: 0
        })
    );
    assert!(!third.bytes.windows(8).any(|w| w == b"50 0 obj"));
    assert_eq!(numbers(&reopened, 0), [0, 50]);
    assert_eq!(
        reopened.file().sections()[0].entry(0),
        Some(XrefEntry::Free {
            next: 52,
            generation: 65535
        })
    );
    assert!(reopened.object(id(50)).unwrap().is_some());
    assert!(reopened.object(id(52)).unwrap().is_none());
}

#[test]
fn an_object_added_and_undone_after_an_append_is_freed() {
    let fixture = one_page();
    let mut session = session(&fixture);
    let base = fixture.bytes.len() as u64;
    let mut builder = session.builder();
    let added = builder.add(string(b"added")).unwrap();
    session.apply(builder.finish()).unwrap();
    assert_eq!(added, id(51));
    let first = append(&mut session, base, 1);
    assert!(session.undo());
    let second = append(&mut session, first.length, 2);

    let all = [fixture.bytes.clone(), first.bytes, second.bytes].concat();
    let reopened = opened(all);
    assert_eq!(
        reopened.file().entry(51),
        Some(XrefEntry::Free {
            next: 0,
            generation: 1
        })
    );
    assert_eq!(free_list(&reopened), [(51, 1)]);
    assert!(reopened.object(id(51)).unwrap().is_none());
}

/// Where `needle` first lies in `bytes`.
fn find(bytes: &[u8], needle: &[u8]) -> usize {
    bytes
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap()
}

#[test]
fn a_cross_reference_stream_without_entry_0_gets_a_head() {
    // The fixture's stream has a row of 7 bytes for each of the 15
    // numbers from 0; its first row is taken out and `Index` made to
    // start at 1.
    const ROW: usize = 7;
    const ROWS: usize = 15;
    let fixture = super::forms::stream_form();
    let mut bytes = fixture.bytes.clone();
    let marker = b">>\nstream\n";
    let last = bytes
        .windows(marker.len())
        .rposition(|window| window == marker)
        .unwrap();
    let data_at = last + marker.len();
    bytes.drain(data_at..data_at + ROW);
    let old = format!("/W [1 4 2] /Root 1 0 R /Info 3 0 R /Length {}", ROW * ROWS);
    let new = format!(
        "/W [1 4 2] /Index [1 14] /Root 1 0 R /Info 3 0 R /Length {}",
        ROW * (ROWS - 1)
    );
    let at = find(&bytes, old.as_bytes());
    bytes.splice(at..at + old.len(), new.bytes());
    let document = opened(bytes.clone());
    assert!(
        !document.file().xref_rebuilt(),
        "{:?}",
        document.diagnostics()
    );
    assert_eq!(document.file().entry(0), None);

    let mut session = EditSession::new(document).unwrap();
    remove(&mut session, &[12]);
    let appended = append(&mut session, bytes.len() as u64, 1);
    let reopened = opened([bytes, appended.bytes].concat());
    assert_eq!(free_list(&reopened), [(12, 1)]);
}
