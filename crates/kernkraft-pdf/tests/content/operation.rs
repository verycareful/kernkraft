// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `ContentParser`: ISO 32000-1, 7.8.2 and 8.9.7.

use kernkraft_pdf::content::{ContentParser, Operation, MAX_OPERANDS};
use kernkraft_pdf::syntax::limits::MAX_NESTING_DEPTH;
use kernkraft_pdf::syntax::{Name, Span, Value};
use kernkraft_pdf::{DiagnosticKind, Diagnostics, Limit, MemorySource, PdfError};

use super::name;

/// Every operation of `text`, and the diagnostics.
fn parse(text: &[u8]) -> (Vec<Operation>, Diagnostics) {
    let source = MemorySource::new(text.to_vec());
    let mut parser = ContentParser::new(&source);
    let mut diagnostics = Diagnostics::new();
    let mut operations = Vec::new();
    while let Some(operation) = parser.next_operation(&mut diagnostics).unwrap() {
        operations.push(operation);
    }
    (operations, diagnostics)
}

fn parse_error(text: &[u8]) -> PdfError {
    let source = MemorySource::new(text.to_vec());
    let mut parser = ContentParser::new(&source);
    let mut diagnostics = Diagnostics::new();
    loop {
        match parser.next_operation(&mut diagnostics) {
            Ok(Some(_)) => {}
            Ok(None) => panic!("no error"),
            Err(error) => return error,
        }
    }
}

#[test]
fn an_operation_is_its_operands_and_its_operator() {
    let (operations, diagnostics) = parse(b"1 0 0 1 10 20 cm");
    assert!(diagnostics.is_empty());
    assert_eq!(operations.len(), 1);
    assert_eq!(operations[0].operator, b"cm");
    let values: Vec<_> = operations[0]
        .operands
        .iter()
        .map(|operand| operand.value().clone())
        .collect();
    assert_eq!(values[..2], [Value::Integer(1), Value::Integer(0)]);
    assert_eq!(values.len(), 6);
    assert_eq!(operations[0].span, Span { start: 0, end: 16 });
}

#[test]
fn operations_have_the_spans_of_their_bytes() {
    //             0123456789
    let (operations, _) = parse(b"q 1 w Q");
    let spans: Vec<_> = operations.iter().map(|op| op.span).collect();
    assert_eq!(
        spans,
        [
            Span { start: 0, end: 1 },
            Span { start: 2, end: 5 },
            Span { start: 6, end: 7 }
        ]
    );
}

#[test]
fn operands_of_every_kind() {
    let (operations, diagnostics) = parse(b"[(a) 1.5] /N <</K 1>> true false null (s) <41> Foo");
    assert!(diagnostics.is_empty());
    let operands = &operations[0].operands;
    assert_eq!(operands.len(), 8);
    assert!(matches!(operands[0].value(), Value::Array(items) if items.len() == 2));
    assert_eq!(operands[1].value(), &Value::Name(name("N")));
    assert!(matches!(operands[2].value(), Value::Dictionary(_)));
    assert_eq!(operands[3].value(), &Value::Boolean(true));
    assert_eq!(operands[4].value(), &Value::Boolean(false));
    assert_eq!(operands[5].value(), &Value::Null);
    assert!(matches!(operands[6].value(), Value::String(s) if s.as_bytes() == b"s"));
    assert!(matches!(operands[7].value(), Value::String(s) if s.as_bytes() == b"A"));
}

#[test]
fn an_operand_remembers_its_span() {
    let (operations, _) = parse(b"  12 (ab) Tj");
    let spans: Vec<_> = operations[0]
        .operands
        .iter()
        .map(|o| o.span().unwrap())
        .collect();
    assert_eq!(
        spans,
        [Span { start: 2, end: 4 }, Span { start: 5, end: 9 }]
    );
    assert_eq!(operations[0].span, Span { start: 2, end: 12 });
}

#[test]
fn a_stray_closing_parenthesis_is_skipped_and_reported() {
    let (operations, diagnostics) = parse(b"q ) Q");
    assert_eq!(operations.len(), 2);
    assert_eq!(diagnostics.as_slice().len(), 1);
    assert_eq!(
        diagnostics.as_slice()[0].kind,
        DiagnosticKind::ContentByteSkipped
    );
    assert_eq!(diagnostics.as_slice()[0].offset, 2);
}

#[test]
fn operands_with_no_operator_at_the_end_are_reported() {
    let (operations, diagnostics) = parse(b"q 1 2");
    assert_eq!(operations.len(), 1);
    assert!(matches!(
        diagnostics.as_slice().last().unwrap().kind,
        DiagnosticKind::OperandsInvalid { .. }
    ));
}

#[test]
fn comments_are_skipped() {
    let (operations, _) = parse(b"q % a comment\nQ");
    assert_eq!(operations.len(), 2);
}

#[test]
fn the_earliest_operands_past_the_stack_are_dropped() {
    let mut text = Vec::new();
    for _ in 0..MAX_OPERANDS + 3 {
        text.extend_from_slice(b"1 ");
    }
    text.extend_from_slice(b"op");
    let (operations, diagnostics) = parse(&text);
    assert_eq!(operations[0].operands.len(), MAX_OPERANDS);
    assert_eq!(
        diagnostics
            .as_slice()
            .iter()
            .filter(|d| d.kind == DiagnosticKind::OperandsDropped)
            .count(),
        3
    );
}

#[test]
fn nesting_past_the_limit_is_an_error() {
    let mut text = vec![b'['; MAX_NESTING_DEPTH + 2];
    text.extend_from_slice(b" op");
    assert!(matches!(
        parse_error(&text),
        PdfError::LimitExceeded {
            limit: Limit::NestingDepth,
            ..
        }
    ));
}

#[test]
fn an_unclosed_array_before_an_operator_ends_there() {
    let (operations, diagnostics) = parse(b"[1 2 Tj Q");
    assert_eq!(operations[0].operator, b"Tj");
    assert_eq!(operations[1].operator, b"Q");
    assert!(!diagnostics.is_empty());
}

// --- inline images (8.9.7) ---

#[test]
fn an_unfiltered_inline_image_takes_the_bytes_its_dictionary_gives() {
    // Width 6, height 1, 8 bits, DeviceGray: six bytes. They hold the
    // letters EI set off by white space, which must not end the data.
    let text = b"BI /W 6 /H 1 /BPC 8 /CS /G ID a EI b EI Q";
    let (operations, diagnostics) = parse(text);
    assert!(diagnostics.is_empty(), "{:?}", diagnostics.as_slice());
    assert_eq!(operations.len(), 2);
    let image = operations[0].inline_image.as_ref().unwrap();
    let data = &text[image.data.start as usize..image.data.end as usize];
    assert_eq!(data, b"a EI b");
    assert_eq!(operations[0].span.end, text.len() as u64 - 2);
    assert_eq!(operations[1].operator, b"Q");
}

#[test]
fn abbreviated_keys_and_names_are_written_out() {
    let (operations, _) =
        parse(b"BI /W 1 /H 1 /BPC 8 /CS /RGB /F [/A85 /Fl] /IM false /D [0 1] ID abc EI");
    let dictionary = &operations[0].inline_image.as_ref().unwrap().dictionary;
    for key in [
        "Width",
        "Height",
        "BitsPerComponent",
        "ColorSpace",
        "Filter",
        "ImageMask",
        "Decode",
    ] {
        assert!(dictionary.get(key.as_bytes()).is_some(), "{key}");
    }
    assert!(dictionary.get(b"W").is_none());
    assert_eq!(
        dictionary.get(b"ColorSpace").unwrap().value(),
        &Value::Name(name("DeviceRGB"))
    );
    let Value::Array(filters) = dictionary.get(b"Filter").unwrap().value() else {
        panic!("not an array");
    };
    assert_eq!(filters[0].value(), &Value::Name(name("ASCII85Decode")));
    assert_eq!(filters[1].value(), &Value::Name(name("FlateDecode")));
}

#[test]
fn an_indexed_colour_space_written_in_the_short_form_is_written_out() {
    let (operations, _) = parse(b"BI /W 1 /H 1 /BPC 8 /CS [/I /RGB 1 <000000FFFFFF>] ID a EI");
    let dictionary = &operations[0].inline_image.as_ref().unwrap().dictionary;
    let Value::Array(space) = dictionary.get(b"ColorSpace").unwrap().value() else {
        panic!("not an array");
    };
    assert_eq!(space[0].value(), &Value::Name(name("Indexed")));
    assert_eq!(space[1].value(), &Value::Name(name("DeviceRGB")));
}

#[test]
fn a_filtered_inline_image_ends_at_ei_set_off_by_white_space() {
    // The data holds "xEIx", where EI is not set off, before the real one.
    let text = b"BI /W 1 /H 1 /F /AHx ID 00xEIx> EI Q";
    let (operations, diagnostics) = parse(text);
    assert!(diagnostics.is_empty());
    let image = operations[0].inline_image.as_ref().unwrap();
    let data = &text[image.data.start as usize..image.data.end as usize];
    assert_eq!(data, b"00xEIx>");
    assert_eq!(operations[1].operator, b"Q");
}

#[test]
fn an_inline_image_with_no_ei_is_damaged() {
    let (operations, diagnostics) = parse(b"BI /W 1 /H 1 /F /AHx ID 00>");
    assert_eq!(operations.len(), 1);
    assert!(operations[0].inline_image.is_none());
    assert!(diagnostics
        .as_slice()
        .iter()
        .any(|d| d.kind == DiagnosticKind::InlineImageDamaged));
}

#[test]
fn an_inline_image_cut_before_id_is_damaged() {
    let (operations, diagnostics) = parse(b"BI /W 1 /H");
    assert!(operations[0].inline_image.is_none());
    assert!(diagnostics
        .as_slice()
        .iter()
        .any(|d| d.kind == DiagnosticKind::InlineImageDamaged));
}

#[test]
fn a_length_entry_gives_the_data_length_of_any_inline_image() {
    let text = b"BI /W 1 /H 1 /F /Fl /L 7 ID a EI b EI Q";
    let (operations, _) = parse(text);
    let image = operations[0].inline_image.as_ref().unwrap();
    let data = &text[image.data.start as usize..image.data.end as usize];
    assert_eq!(data, b"a EI b ");
    assert_eq!(operations[1].operator, b"Q");
}

#[test]
fn a_name_helper_is_a_name() {
    assert_eq!(name("A"), Name::new(b"A".to_vec()));
}
