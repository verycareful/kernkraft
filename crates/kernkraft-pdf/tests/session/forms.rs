// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! An update to files of each form: a cross-reference stream with object
//! streams (7.5.8, 7.5.7) and a hybrid-reference file (7.5.8.4).

use super::*;
use kernkraft_pdf::file::{SectionForm, XrefEntry};

/// Objects 1, 2, 10 and 11 in the file; 3 (the information dictionary)
/// and 12 in object stream 13; the cross-reference stream is 14, `Size`
/// 15.
pub fn stream_form() -> Fixture {
    stream_file(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Kids [10 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            ),
            (10, b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R >>"),
        ],
        &[
            (3, b"<< /Title (Fixture) /Producer (kernkraft tests) >>"),
            (12, b"(twelve)"),
        ],
        "/Root 1 0 R /Info 3 0 R",
    )
}

fn stream_form_with_content() -> Fixture {
    // `stream_file` writes plain objects as given, and a stream is one.
    let content = stream("", b"BT (Page 1) Tj ET");
    stream_file(
        &[
            (1, b"<< /Type /Catalog /Pages 2 0 R >>"),
            (
                2,
                b"<< /Type /Pages /Kids [10 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
            ),
            (10, b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R >>"),
            (11, &content),
        ],
        &[
            (3, b"<< /Title (Fixture) /Producer (kernkraft tests) >>"),
            (12, b"(twelve)"),
        ],
        "/Root 1 0 R /Info 3 0 R",
    )
}

#[test]
fn a_cross_reference_stream_is_followed_by_one() {
    let fixture = stream_form_with_content();
    let mut session = session(&fixture);
    set_content(&mut session, "changed");
    let bytes = save_bytes(&mut session, 1);
    assert!(bytes.starts_with(&fixture.bytes));
    let reopened = opened(bytes);
    let file = reopened.file();
    assert_eq!(file.sections().len(), 2);
    assert_eq!(file.sections()[0].form(), SectionForm::Stream);
    // The content, the information dictionary (read from an object stream
    // and written whole with its ModDate), and the cross-reference stream
    // itself, which takes the number after Size.
    assert_eq!(numbers(&reopened, 0), [3, 11, 15]);
    let trailer = file.sections()[0].trailer();
    assert_eq!(integer(trailer, b"Size"), 16);
    assert_eq!(
        integer(trailer, b"Prev"),
        last_startxref(&fixture.bytes) as i64
    );
    assert_eq!(stream_text(&reopened, 11), "changed");
    assert_eq!(
        info_string(&reopened, b"ModDate").as_deref(),
        Some(DATE_TEXT)
    );
    assert_eq!(
        info_string(&reopened, b"Title").as_deref(),
        Some(&b"Fixture"[..])
    );
    // An object left in the object stream is read from it.
    let twelve = reopened.required(id(12)).unwrap();
    assert_eq!(twelve.object(), Some(&string(b"twelve")));
}

#[test]
fn an_object_back_in_its_object_stream_keeps_its_entry() {
    let fixture = stream_form();
    let base = fixture.bytes.len() as u64;
    let mut session = session(&fixture);
    let mut builder = session.builder();
    builder.set(id(12), string(b"edited"));
    session.apply(builder.finish()).unwrap();
    let first = append(&mut session, base, 1);
    assert!(session.undo());
    let second = append(&mut session, first.length, 2);
    let reopened = opened([fixture.bytes.clone(), first.bytes, second.bytes.clone()].concat());
    // Object stream 13, second object in it (3 is the first).
    assert_eq!(
        reopened.file().entry(12),
        Some(XrefEntry::Compressed {
            stream: 13,
            index: 1
        })
    );
    assert_eq!(
        reopened.required(id(12)).unwrap().object(),
        Some(&string(b"twelve"))
    );
    assert!(!second.bytes.windows(9).any(|w| w == b"12 0 obj\n"));
}

#[test]
fn a_removed_object_of_an_object_stream_is_free() {
    let fixture = stream_form();
    let mut session = session(&fixture);
    remove(&mut session, &[12]);
    let reopened = opened(save_bytes(&mut session, 1));
    assert_eq!(free_list(&reopened), [(12, 1)]);
    assert!(reopened.object(id(12)).unwrap().is_none());
}

/// A hybrid-reference file: the table lists objects 0 to 3, and its
/// `XRefStm` stream object 4, which lies in object stream 2 (7.5.8.4).
fn hybrid() -> (Vec<u8>, usize) {
    let mut out = HEADER.to_vec();
    out.extend_from_slice(b"1 0 obj\n<< /Type /Catalog >>\nendobj\n");
    let pairs = b"4 0\n(four)\n";
    let objstm = format!(
        "2 0 obj\n<< /Type /ObjStm /N 1 /First 4 /Length {} >>\nstream\n{}\nendstream\nendobj\n",
        pairs.len(),
        std::str::from_utf8(pairs).unwrap()
    );
    let objstm_at = out.len();
    out.extend_from_slice(objstm.as_bytes());
    // Entry for 4: type 2, object stream 2, index 0.
    let rows = [2u8, 0, 2, 0];
    let xref_stream_at = out.len();
    out.extend_from_slice(
        format!(
            "3 0 obj\n<< /Type /XRef /Size 5 /W [1 2 1] /Index [4 1] /Length {} >>\nstream\n",
            rows.len()
        )
        .as_bytes(),
    );
    out.extend_from_slice(&rows);
    out.extend_from_slice(b"\nendstream\nendobj\n");
    let table_at = out.len();
    out.extend_from_slice(b"xref\n0 4\n0000000000 65535 f\r\n");
    out.extend_from_slice(format!("{:010} 00000 n\r\n", HEADER.len()).as_bytes());
    out.extend_from_slice(format!("{objstm_at:010} 00000 n\r\n").as_bytes());
    out.extend_from_slice(format!("{xref_stream_at:010} 00000 n\r\n").as_bytes());
    out.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R /XRefStm {xref_stream_at} >>\nstartxref\n{table_at}\n%%EOF\n")
            .as_bytes(),
    );
    (out, table_at)
}

#[test]
fn a_hybrid_file_is_followed_by_a_table_that_reaches_its_table() {
    let (bytes, table_at) = hybrid();
    let document = opened(bytes.clone());
    assert_eq!(
        document.file().entry(4),
        Some(XrefEntry::Compressed {
            stream: 2,
            index: 0
        })
    );
    let base = bytes.len() as u64;
    let mut session = EditSession::new(document).unwrap();
    let mut builder = session.builder();
    builder.set(id(4), string(b"edited"));
    session.apply(builder.finish()).unwrap();
    let first = append(&mut session, base, 1);
    // Undone: object 4 is the one in the object stream again. A table
    // cannot name that (Table 18 is the stream's), so it is written.
    assert!(session.undo());
    let second = append(&mut session, first.length, 2);

    let after_first = [bytes.clone(), first.bytes].concat();
    let reopened = opened([after_first, second.bytes.clone()].concat());
    let sections = reopened.file().sections();
    assert_eq!(sections.len(), 3);
    assert!(sections
        .iter()
        .take(2)
        .all(|s| s.form() == SectionForm::Table));
    // The first update reaches the table, not the stream the table names.
    assert_eq!(integer(sections[1].trailer(), b"Prev"), table_at as i64);
    assert!(second.bytes.windows(9).any(|w| w == b"4 0 obj\n("));
    assert!(matches!(
        reopened.file().entry(4),
        Some(XrefEntry::InUse { generation: 0, .. })
    ));
    assert_eq!(
        reopened.required(id(4)).unwrap().object(),
        Some(&string(b"four"))
    );
}
