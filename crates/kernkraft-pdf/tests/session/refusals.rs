// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! What is not edited in place (`EditSession::new`).

use super::*;
use kernkraft_pdf::PdfError;

#[test]
fn a_document_opened_with_a_rebuilt_cross_reference_is_refused() {
    let fixture = one_page();
    let document = fixture.open();
    let rebuilt = document.rebuilt().unwrap();
    assert!(rebuilt.file().xref_rebuilt());
    assert!(matches!(
        EditSession::new(rebuilt),
        Err(PdfError::RebuiltNotEditable)
    ));
}

#[test]
fn a_file_that_opens_after_a_rebuild_is_refused() {
    // `startxref` names a place with no table.
    let fixture = one_page();
    let mut bytes = fixture.bytes.clone();
    let at = bytes.windows(9).rposition(|w| w == b"startxref").unwrap();
    bytes.truncate(at);
    bytes.extend_from_slice(b"startxref\n9\n%%EOF\n");
    let document = Document::from_bytes(bytes, &OPTIONS, "").unwrap();
    assert!(document.file().xref_rebuilt());
    assert!(matches!(
        EditSession::new(document),
        Err(PdfError::RebuiltNotEditable)
    ));
}

#[test]
fn an_earlier_revision_is_refused() {
    let base = one_page();
    let (updated, _) = append_update(
        &base,
        &[(3, b"<< /Title (Second) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let document = updated.open();
    let earlier = document.open_revision(0).unwrap();
    assert!(earlier.is_revision_view());
    assert!(matches!(
        EditSession::new(earlier),
        Err(PdfError::RevisionNotEditable)
    ));
    // The newest revision of the same file is a document like any other.
    assert!(!document.is_revision_view());
    assert!(EditSession::new(document).is_ok());
}

#[test]
fn a_file_of_several_revisions_is_updated_after_the_newest() {
    let base = one_page();
    let (updated, section) = append_update(
        &base,
        &[(3, b"<< /Title (Second) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let mut session = session(&updated);
    set_content(&mut session, "changed");
    let bytes = save_bytes(&mut session, 1);
    let reopened = opened(bytes.clone());
    assert_eq!(reopened.file().sections().len(), 3);
    assert_eq!(
        integer(reopened.file().sections()[0].trailer(), b"Prev"),
        section as i64
    );
    assert_eq!(reopened.revisions().unwrap().len(), 3);
    // Title was set by the second revision and ModDate by the third.
    assert_eq!(
        info_string(&reopened, b"Title").as_deref(),
        Some(&b"Second"[..])
    );
}
