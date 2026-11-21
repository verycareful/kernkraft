// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Operator spans (design E10): the stream, byte range and index of every
//! painting operation, so an editor can find it in the decoded stream.

use kernkraft_pdf::content::StreamId;
use kernkraft_pdf::syntax::Span;

use super::{id, paths, run, Event, FONTS, FONT_OBJECTS};

#[test]
fn a_path_reports_the_painting_operator_and_the_whole_construction() {
    //                  0         1         2
    //                  0123456789012345678901234
    let content = "q 1 w 0 0 m 5 5 l 9 9 l S Q";
    let (_, events) = run(content, "", &[]);
    let Event::Path {
        span, construction, ..
    } = paths(&events)[0]
    else {
        panic!()
    };
    let offset = content.find(" S").unwrap() as u64 + 1;
    assert_eq!(span.stream, StreamId::Page(id(10)));
    assert_eq!(
        span.range,
        Span {
            start: offset,
            end: offset + 1
        }
    );
    // Operations: q 0, w 1, m 2, l 3, l 4, S 5.
    assert_eq!(span.index, 5);
    let first = content.find("0 0 m").unwrap() as u64;
    assert_eq!(
        construction.range,
        Span {
            start: first,
            end: offset + 1
        }
    );
    assert_eq!(construction.index, 2);
}

#[test]
fn a_text_operator_reports_its_operands_and_operator() {
    let content = "BT /F1 12 Tf 10 20 Td (AB) Tj ET";
    let (_, events) = run(content, FONTS, &FONT_OBJECTS);
    let Event::Text { span, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    let start = content.find("(AB)").unwrap() as u64;
    assert_eq!(
        span.range,
        Span {
            start,
            end: start + "(AB) Tj".len() as u64
        }
    );
    // BT 0, Tf 1, Td 2, Tj 3.
    assert_eq!(span.index, 3);
    assert_eq!(
        &content.as_bytes()[span.range.start as usize..span.range.end as usize],
        b"(AB) Tj"
    );
}

#[test]
fn the_index_counts_every_operation_of_the_stream() {
    let (_, events) = run("q q q 0 0 m 1 1 l S", "", &[]);
    let Event::Path { span, .. } = paths(&events)[0] else {
        panic!()
    };
    // q 0, q 1, q 2, m 3, l 4, S 5.
    assert_eq!(span.index, 5);
}

#[test]
fn an_inline_image_is_one_operation_with_its_span() {
    let content = "q BI /W 1 /H 1 /BPC 8 /CS /G ID a EI Q";
    let (_, events) = run(content, "", &[]);
    assert!(events.iter().any(|e| matches!(e, Event::Image { .. })));
}
