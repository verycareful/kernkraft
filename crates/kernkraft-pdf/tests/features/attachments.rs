// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Attachments (`attachments.rs`, 7.11.4, 12.5.6.15).

use std::io::Write as _;

use super::*;
use kernkraft_fs::{Expect, FsError};
use kernkraft_pdf::doc::Date;
use kernkraft_pdf::syntax::Name;
use kernkraft_pdf::{AttachmentPlace, Check, PdfError};
use md5::{Digest, Md5};

const HELLO: &[u8] = b"hello\n";
const ON_PAGE: &[u8] = b"on page";
/// How many bytes `b.bin` decodes to, and the byte it repeats.
const REPEATED: usize = 1000;
const REPEATED_BYTE: u8 = b'x';

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02X}")).collect()
}

fn zlib(data: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(data).unwrap();
    encoder.finish().unwrap()
}

/// Three files attached to the document (one of them not embedded), one
/// to page 1 by an annotation whose checksum is wrong, and one with
/// parameters of the wrong kinds.
pub fn attached() -> Fixture {
    let mut objects = pages_document(2);
    objects[0].1 = b"<< /Type /Catalog /Pages 2 0 R /Names << /EmbeddedFiles << /Names \
        [(a.txt) 90 0 R (b.bin) 92 0 R (c.txt) 94 0 R (d.txt) 98 0 R] >> >> >>"
        .to_vec();
    objects[3].1 = b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R /Annots [95 0 R] >>".to_vec();
    let hello_sum = hex(&Md5::digest(HELLO));
    let extra: Vec<(u32, Vec<u8>)> = vec![
        (
            90,
            b"<< /Type /Filespec /F (a.txt) /UF <FEFF0061002E007400780074> /Desc (first) \
              /EF << /F 91 0 R >> >>"
                .to_vec(),
        ),
        (
            91,
            stream(
                &format!(
                    "/Type /EmbeddedFile /Subtype /text#2Fplain /Params << /Size {} \
                     /CreationDate (D:20260102030405Z) /ModDate (D:2026) /CheckSum <{hello_sum}> >>",
                    HELLO.len()
                ),
                HELLO,
            ),
        ),
        (92, b"<< /Type /Filespec /F (b.bin) /EF << /F 93 0 R >> >>".to_vec()),
        (
            93,
            stream(
                "/Type /EmbeddedFile /Filter /FlateDecode",
                &zlib(&[REPEATED_BYTE; REPEATED]),
            ),
        ),
        (94, b"<< /Type /Filespec /F (c.txt) >>".to_vec()),
        (
            95,
            b"<< /Type /Annot /Subtype /FileAttachment /Rect [0 0 10 10] /FS 96 0 R >>".to_vec(),
        ),
        (96, b"<< /Type /Filespec /F (page.txt) /EF << /F 97 0 R >> >>".to_vec()),
        (
            97,
            stream(
                &format!("/Type /EmbeddedFile /Params << /CheckSum <{}> >>", hex(&[0; 16])),
                ON_PAGE,
            ),
        ),
        (98, b"<< /Type /Filespec /F (d.txt) /EF << /F 99 0 R >> >>".to_vec()),
        (
            99,
            stream(
                "/Type /EmbeddedFile /Params << /Size -1 /ModDate (yesterday) /CheckSum <00> >>",
                HELLO,
            ),
        ),
    ];
    objects.extend(extra);
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

#[test]
fn attachments_of_the_document_then_of_its_pages() {
    let document = attached().open();
    let attachments = document.attachments().unwrap();
    let names: Vec<(String, AttachmentPlace, u32)> = attachments
        .iter()
        .map(|a| {
            (
                a.file.name.as_ref().unwrap().text.clone(),
                a.place,
                a.stream.number,
            )
        })
        .collect();
    assert_eq!(
        names,
        [
            ("a.txt".to_string(), AttachmentPlace::Document, 91),
            ("b.bin".to_string(), AttachmentPlace::Document, 93),
            ("d.txt".to_string(), AttachmentPlace::Document, 99),
            (
                "page.txt".to_string(),
                AttachmentPlace::Page {
                    page: 1,
                    annotation: Some(id(95))
                },
                97
            ),
        ]
    );
    let first = &attachments[0];
    assert_eq!(first.key.as_ref().unwrap().text, "a.txt");
    assert_eq!(first.file.description.as_ref().unwrap().text, "first");
    assert_eq!(first.media_type, Some(Name::new(&b"text/plain"[..])));
    assert_eq!(first.size, Some(HELLO.len() as u64));
    assert_eq!(
        first.created,
        Some(Date {
            year: 2026,
            month: 1,
            day: 2,
            hour: 3,
            minute: 4,
            second: 5,
            offset_minutes: Some(0),
        })
    );
    assert_eq!(first.modified.map(|date| date.year), Some(2026));
    let not_embedded = of_kind(&document, &DiagnosticKind::AttachmentNotEmbedded);
    assert_eq!(not_embedded.len(), 1);
    assert_eq!(not_embedded[0].object, Some(id(94)));
}

#[test]
fn data_checked_against_its_parameters() {
    let document = attached().open();
    let attachments = document.attachments().unwrap();
    let hello = document.attachment_data(&attachments[0], 1 << 16).unwrap();
    assert_eq!(hello.data, HELLO);
    assert!(hello.complete);
    assert_eq!(
        (hello.size, hello.checksum),
        (Check::Matches, Check::Matches)
    );
    assert!(hello.is_sound());

    let repeated = document.attachment_data(&attachments[1], 1 << 16).unwrap();
    assert_eq!(repeated.data, vec![REPEATED_BYTE; REPEATED]);
    assert_eq!(
        (repeated.size, repeated.checksum),
        (Check::Absent, Check::Absent)
    );
    assert!(repeated.is_sound());

    let on_page = document.attachment_data(&attachments[3], 1 << 16).unwrap();
    assert_eq!(on_page.data, ON_PAGE);
    assert_eq!(on_page.checksum, Check::Differs);
    assert!(!on_page.is_sound());
}

#[test]
fn the_cap_on_decoded_bytes() {
    let document = attached().open();
    let attachments = document.attachments().unwrap();
    assert!(matches!(
        document.attachment_data(&attachments[1], REPEATED - 1),
        Err(PdfError::DecodedTooLarge { .. })
    ));
    assert!(document.attachment_data(&attachments[1], REPEATED).is_ok());
}

#[test]
fn parameters_of_the_wrong_kind_are_left_out() {
    let document = attached().open();
    let attachments = document.attachments().unwrap();
    let odd = &attachments[2];
    assert_eq!((odd.size, odd.modified, odd.checksum), (None, None, None));
    for key in ["Size", "ModDate", "CheckSum"] {
        let kind = DiagnosticKind::IgnoredEntry {
            key: Name::new(key.as_bytes()),
        };
        let found = of_kind(&document, &kind);
        assert_eq!(found.len(), 1, "{key}");
        assert_eq!(found[0].object, Some(id(99)));
    }
}

#[test]
fn saving_writes_only_what_is_sound() {
    let document = attached().open();
    let attachments = document.attachments().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("a.txt");
    document
        .save_attachment(&attachments[0], 1 << 16, &path, Expect::Nothing)
        .unwrap();
    assert_eq!(std::fs::read(&path).unwrap(), HELLO);
    // A file already there is not replaced when nothing is expected.
    assert!(matches!(
        document.save_attachment(&attachments[0], 1 << 16, &path, Expect::Nothing),
        Err(PdfError::Fs(FsError::Exists { .. }))
    ));

    let damaged = dir.path().join("page.txt");
    assert!(matches!(
        document.save_attachment(&attachments[3], 1 << 16, &damaged, Expect::Nothing),
        Err(PdfError::AttachmentDamaged { stream }) if stream == id(97)
    ));
    assert!(!damaged.exists());
}
