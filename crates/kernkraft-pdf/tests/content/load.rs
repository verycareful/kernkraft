// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Loading content streams: ISO 32000-1, 7.8.2 and 7.7.3.3.

use kernkraft_pdf::content::{ContentPart, StreamId};
use kernkraft_pdf::{DiagnosticKind, PdfError};

use super::{
    assert_matrix, borrowed, has, id, paths, run_document, run_document_with, stream, table_file,
    Event, Fixture,
};

/// A one-page document whose page (object 10) has `contents` as its
/// `Contents` entry, with the extra objects.
fn page_with(contents: &str, extra: &[(u32, Vec<u8>)]) -> Fixture {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            2,
            b"<< /Type /Pages /Kids [10 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
        ),
        (3, b"<< >>".to_vec()),
        (
            10,
            format!("<< /Type /Page /Parent 2 0 R /Contents {contents} >>").into_bytes(),
        ),
    ];
    objects.extend(extra.iter().cloned());
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

#[test]
fn the_streams_of_a_page_are_joined_by_a_line_feed() {
    let fixture = page_with(
        "[11 0 R 12 0 R]",
        &[(11, stream("", b"q 1 w")), (12, stream("", b"Q"))],
    );
    let document = fixture.open();
    let content = document
        .content_stream(StreamId::Page(id(10)), 1 << 20)
        .unwrap();
    assert_eq!(content.data, b"q 1 w\nQ");
    assert_eq!(
        content.parts,
        [
            ContentPart {
                object: id(11),
                start: 0,
                len: 5
            },
            ContentPart {
                object: id(12),
                start: 6,
                len: 1
            },
        ]
    );
}

#[test]
fn locate_names_the_stream_and_offset_of_a_byte() {
    let fixture = page_with(
        "[11 0 R 12 0 R]",
        &[(11, stream("", b"q 1 w")), (12, stream("", b"Q"))],
    );
    let document = fixture.open();
    let content = document
        .content_stream(StreamId::Page(id(10)), 1 << 20)
        .unwrap();
    assert_eq!(content.locate(2), Some((id(11), 2)));
    assert_eq!(content.locate(6), Some((id(12), 0)));
    // The joining line feed belongs to neither.
    assert_eq!(content.locate(5), None);
    assert_eq!(content.locate(7), None);
}

#[test]
fn a_single_stream_and_an_indirect_array_are_read_too() {
    let single = page_with("11 0 R", &[(11, stream("", b"q Q"))]).open();
    assert_eq!(
        single
            .content_stream(StreamId::Page(id(10)), 1 << 20)
            .unwrap()
            .data,
        b"q Q"
    );
    let indirect = page_with(
        "13 0 R",
        &[
            (11, stream("", b"q")),
            (12, stream("", b"Q")),
            (13, b"[11 0 R 12 0 R]".to_vec()),
        ],
    )
    .open();
    assert_eq!(
        indirect
            .content_stream(StreamId::Page(id(10)), 1 << 20)
            .unwrap()
            .data,
        b"q\nQ"
    );
}

#[test]
fn a_page_with_no_contents_runs_nothing() {
    let document = page_with("null", &[]).open();
    assert!(run_document(&document).unwrap().is_empty());
    let document = page_with("[]", &[]).open();
    assert!(run_document(&document).unwrap().is_empty());
}

#[test]
fn an_operation_may_be_split_between_streams() {
    // 7.8.2: the division falls between tokens; the operands are in one
    // stream and the operator in the next.
    let document = page_with(
        "[11 0 R 12 0 R]",
        &[
            (11, stream("", b"2 0 0 2 10 10")),
            (12, stream("", b"cm 0 0 m 1 1 l S")),
        ],
    )
    .open();
    let events = run_document(&document).unwrap();
    let Event::Path { state, span, .. } = paths(&events)[0] else {
        panic!()
    };
    assert_matrix(state.ctm, [2.0, 0.0, 0.0, 2.0, 10.0, 10.0]);
    // The spans are offsets into the joined bytes.
    assert!(span.range.start > 13);
}

#[test]
fn a_filtered_stream_is_decoded() {
    let document = page_with(
        "11 0 R",
        &[(
            11,
            stream("/Filter /ASCIIHexDecode", b"302030206D20312031206C2053>"),
        )],
    )
    .open();
    let content = document
        .content_stream(StreamId::Page(id(10)), 1 << 20)
        .unwrap();
    assert_eq!(content.data, b"0 0 m 1 1 l S");
    assert_eq!(paths(&run_document(&document).unwrap()).len(), 1);
}

#[test]
fn a_missing_stream_in_the_array_is_skipped() {
    let document = page_with(
        "[11 0 R 99 0 R 12 0 R]",
        &[(11, stream("", b"0 0 m")), (12, stream("", b"1 1 l S"))],
    )
    .open();
    let events = run_document(&document).unwrap();
    assert_eq!(paths(&events).len(), 1);
}

#[test]
fn a_stream_that_cannot_be_decoded_is_skipped_and_reported() {
    let document = page_with(
        "[11 0 R 12 0 R]",
        &[
            (11, stream("/Filter /NoSuchFilter", b"junk")),
            (12, stream("", b"0 0 m 1 1 l S")),
        ],
    )
    .open();
    let events = run_document(&document).unwrap();
    assert_eq!(paths(&events).len(), 1);
    let found = document
        .diagnostics()
        .into_iter()
        .find(|d| d.kind == DiagnosticKind::ContentStreamUnreadable)
        .expect("reported");
    assert_eq!(found.object, Some(id(11)));
}

#[test]
fn a_contents_entry_that_is_not_a_stream_or_an_array_is_an_error() {
    let document = page_with("(text)", &[]).open();
    assert!(matches!(
        run_document(&document).unwrap_err(),
        PdfError::InvalidEntry {
            key: "Contents",
            ..
        }
    ));
}

#[test]
fn an_item_of_the_array_that_is_not_a_reference_is_an_error() {
    let document = page_with("[11 0 R 5]", &[(11, stream("", b"q"))]).open();
    assert!(matches!(
        run_document(&document).unwrap_err(),
        PdfError::InvalidEntry {
            key: "Contents",
            ..
        }
    ));
}

#[test]
fn a_stream_decoding_past_the_cap_is_an_error() {
    // The cap is on what a filter produces: ten bytes of "q" in hex.
    let document = page_with(
        "11 0 R",
        &[(
            11,
            stream("/Filter /ASCIIHexDecode", b"71717171717171717171>"),
        )],
    )
    .open();
    let options = kernkraft_pdf::content::InterpretOptions {
        max_stream_bytes: 5,
        ..Default::default()
    };
    assert!(matches!(
        run_document_with(&document, options).unwrap_err(),
        PdfError::DecodedTooLarge { .. }
    ));
}

#[test]
fn a_form_stream_is_loaded_by_its_object() {
    let document = page_with(
        "11 0 R",
        &[
            (11, stream("", b"q")),
            (40, stream("/Subtype /Form", b"0 0 m")),
        ],
    )
    .open();
    let content = document
        .content_stream(StreamId::Object(id(40)), 1 << 20)
        .unwrap();
    assert_eq!(content.data, b"0 0 m");
    assert_eq!(content.parts.len(), 1);
}

#[test]
fn has_is_usable() {
    let document = page_with("11 0 R", &[(11, stream("", b"q Q"))]).open();
    run_document(&document).unwrap();
    assert!(!has(&document, |_| true));
}
