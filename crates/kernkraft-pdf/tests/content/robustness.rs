// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Content that is cut, flipped or spliced is a typed error or a run, never
//! a panic or a hang.

use kernkraft_fs::{NoProgress, Progress};
use kernkraft_pdf::content::{InterpretOptions, Interpreter};
use kernkraft_pdf::PdfError;

use super::{content_fixture, Recorder, FONTS, FONT_OBJECTS};

/// Content that uses most of what the interpreter does.
const RICH: &str = "q 2 0 0 2 10 10 cm 3 w [3 2] 0 d /G1 gs 0.1 0.2 0.3 rg 1 0 0 0 K \
     0 0 100 100 re W n 10 10 m 20 20 30 30 40 40 c h B* \
     BT /F1 12 Tf 100 700 Td (AB) Tj [(A) -120 (B)] TJ T* (C) ' 1 2 (C) \" ET \
     /Span <</MCID 4>> BDC /Fm1 Do /Im1 Do EMC \
     BI /W 2 /H 2 /BPC 8 /CS /G ID abcd EI Q";

fn resources() -> String {
    format!(
        "{FONTS} /XObject << /Fm1 40 0 R /Im1 41 0 R >> \
         /ExtGState << /G1 << /LW 2 /CA 0.5 /BM /Multiply >> >>"
    )
}

fn objects() -> Vec<(u32, Vec<u8>)> {
    let mut objects: Vec<(u32, Vec<u8>)> =
        FONT_OBJECTS.iter().map(|(n, b)| (*n, b.to_vec())).collect();
    objects.push((
        40,
        super::stream(
            "/Type /XObject /Subtype /Form /BBox [0 0 50 50] /Matrix [1 0 0 1 5 5]",
            b"0 0 m 10 10 l S q BT /F1 8 Tf (A) Tj ET",
        ),
    ));
    objects.push((
        41,
        super::stream(
            "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray",
            b"\x00",
        ),
    ));
    objects
}

fn run_bytes(content: &[u8]) -> Result<usize, PdfError> {
    let objects = objects();
    let extra: Vec<(u32, &[u8])> = objects.iter().map(|(n, b)| (*n, b.as_slice())).collect();
    let document = content_fixture(content, &resources(), &extra).open();
    let page = document.page(1)?;
    let mut interpreter = Interpreter::new(&document, InterpretOptions::default())?;
    let mut recorder = Recorder::default();
    interpreter.run_page(&page, &mut recorder, &NoProgress)?;
    Ok(recorder.events.len())
}

#[test]
fn the_rich_content_runs_clean() {
    let events = run_bytes(RICH.as_bytes()).unwrap();
    assert!(events > 20, "{events}");
}

#[test]
fn every_truncation_runs_or_fails_with_an_error() {
    let bytes = RICH.as_bytes();
    for end in 0..=bytes.len() {
        let _ = run_bytes(&bytes[..end]);
    }
}

#[test]
fn every_byte_replaced_runs_or_fails_with_an_error() {
    let bytes = RICH.as_bytes();
    for position in 0..bytes.len() {
        for replacement in [0x00, 0xFF, b'(', b')', b'[', b']', b'<', b'>', b'/', b' '] {
            let mut mutated = bytes.to_vec();
            mutated[position] = replacement;
            let _ = run_bytes(&mutated);
        }
    }
}

#[test]
fn every_byte_deleted_runs_or_fails_with_an_error() {
    let bytes = RICH.as_bytes();
    for position in 0..bytes.len() {
        let mut mutated = bytes.to_vec();
        mutated.remove(position);
        let _ = run_bytes(&mutated);
    }
}

#[test]
fn a_cancelled_run_stops_with_cancelled() {
    struct Cancelled;
    impl Progress for Cancelled {
        fn report(&self, _: u64, _: u64, _: &str) {}
        fn is_cancelled(&self) -> bool {
            true
        }
    }
    let content = "q Q ".repeat(400);
    let document = content_fixture(content.as_bytes(), "", &[]).open();
    let page = document.page(1).unwrap();
    let mut interpreter = Interpreter::new(&document, InterpretOptions::default()).unwrap();
    let mut recorder = Recorder::default();
    let error = interpreter
        .run_page(&page, &mut recorder, &Cancelled)
        .unwrap_err();
    assert!(matches!(error, PdfError::Cancelled));
}

#[test]
fn a_large_stream_of_operations_runs() {
    let content = "0 0 m 1 1 l S ".repeat(50_000);
    let count = run_bytes(content.as_bytes()).unwrap();
    assert_eq!(count, 50_000);
}
