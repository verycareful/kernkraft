// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The active-content report (`active.rs`).

use super::*;
use kernkraft_fs::{NoProgress, Progress};
use kernkraft_pdf::{ActionKind, PdfError};

/// A script run on opening, a document-level script, a page that
/// launches an application when opened, a page that submits its form when
/// closed, a link to a URI, a link whose go-to imports data next, XFA, an
/// attachment, and a script nothing reaches.
pub fn active() -> Fixture {
    let mut objects = pages_document(2);
    objects[0].1 = b"<< /Type /Catalog /Pages 2 0 R /OpenAction << /S /JavaScript /JS (start) >> \
        /Names << /JavaScript << /Names [(init) 84 0 R] >> /EmbeddedFiles << /Names [(a) 90 0 R] >> >> \
        /AcroForm << /Fields [] /XFA 85 0 R >> >>"
        .to_vec();
    objects[3].1 = b"<< /Type /Page /Parent 2 0 R /Contents 11 0 R \
        /AA << /O << /S /Launch /F (calc.exe) >> >> /Annots [86 0 R 87 0 R] >>"
        .to_vec();
    objects[5].1 = b"<< /Type /Page /Parent 2 0 R /Contents 13 0 R \
        /AA << /C << /S /SubmitForm /F (https://example.invalid) >> >> >>"
        .to_vec();
    let extra: Vec<(u32, Vec<u8>)> =
        vec![
        (84, b"<< /S /JavaScript /JS (lib) >>".to_vec()),
        (85, stream("", b"<xdp:xdp/>")),
        (
            86,
            b"<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /A << /S /URI /URI (http://x) >> >>"
                .to_vec(),
        ),
        (
            87,
            b"<< /Type /Annot /Subtype /Link /Rect [0 0 1 1] /A << /S /GoTo /D [12 0 R /Fit] \
              /Next << /S /ImportData /F (d.fdf) >> >> >>"
                .to_vec(),
        ),
        (89, b"<< /S /JavaScript /JS (dead) >>".to_vec()),
        (90, b"<< /Type /Filespec /F (a) /EF << /F 91 0 R >> >>".to_vec()),
        (91, stream("/Type /EmbeddedFile", b"a")),
    ];
    objects.extend(extra);
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

#[test]
fn what_could_run_or_reach_out() {
    let document = active().open();
    let report = document.active_content(&NoProgress).unwrap();
    let found: Vec<(u32, ActionKind, Option<u32>, Option<u32>)> = report
        .actions
        .iter()
        .map(|active| {
            (
                active.holder.number,
                active.action.kind.clone(),
                active.page,
                active.action.object.map(|id| id.number),
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            (1, ActionKind::JavaScript, None, None),
            (10, ActionKind::Launch, Some(1), None),
            (12, ActionKind::SubmitForm, Some(2), None),
            (84, ActionKind::JavaScript, None, Some(84)),
            (86, ActionKind::Uri, Some(1), None),
            (87, ActionKind::ImportData, Some(1), None),
        ]
    );
    assert!(report.xfa);
    assert_eq!(report.attachments, 1);
    assert!(!report.is_empty());
}

#[test]
fn a_plain_document_reports_nothing() {
    let document = document_with(2, "/OpenAction [10 0 R /Fit]", &[]).open();
    let report = document.active_content(&NoProgress).unwrap();
    assert!(report.is_empty(), "{report:?}");
}

struct Cancelling;

impl Progress for Cancelling {
    fn report(&self, _done: u64, _total: u64, _stage: &str) {}

    fn is_cancelled(&self) -> bool {
        true
    }
}

#[test]
fn a_cancelled_report() {
    assert!(matches!(
        active().open().active_content(&Cancelling),
        Err(PdfError::Cancelled)
    ));
}
