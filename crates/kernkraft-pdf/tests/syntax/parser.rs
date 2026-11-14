// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Parser tests. Inputs and expected values are the examples of ISO 32000
//! 7.3 unless a test builds its own input; every expected offset is counted
//! from the input.

use kernkraft_pdf::syntax::limits::{MAX_NESTING_DEPTH, PORTABLE_OBJECT_COUNT};
use kernkraft_pdf::syntax::{IndirectObject, Name, Object, ObjectId, Parser, Span, Value};
use kernkraft_pdf::{
    Diagnostic, DiagnosticKind, Diagnostics, FileSource, Limit, PdfError, PortableLimit,
};

use crate::{
    array, assert_limit, assert_unexpected_end, assert_unexpected_token, boolean, dict, int,
    kinds_and_offsets, memory, name, null, real, reference, string,
};

/// Enough repeats that handling each in time proportional to the number
/// already seen would not finish.
const MANY_REPEATS: usize = 200_000;
/// Far more than any stack could hold as nested calls.
const A_MILLION: usize = 1_000_000;

/// The dictionary example of 7.3.7, its numbers written without the spaces
/// the printed text puts around the period.
const DICTIONARY_EXAMPLE: &[u8] = b"<< /Type /Example
   /Subtype /DictionaryExample
   /Version 0.01
   /IntegerItem 12
   /StringItem (a string)
   /Subdictionary << /Item1 0.4
                     /Item2 true
                     /LastItem (not!)
                     /VeryLastItem (OK)
                  >>
>>";

fn dictionary_example() -> Object {
    dict(vec![
        ("Type", name("Example")),
        ("Subtype", name("DictionaryExample")),
        ("Version", real(0.01)),
        ("IntegerItem", int(12)),
        ("StringItem", string(b"a string")),
        (
            "Subdictionary",
            dict(vec![
                ("Item1", real(0.4)),
                ("Item2", boolean(true)),
                ("LastItem", string(b"not!")),
                ("VeryLastItem", string(b"OK")),
            ]),
        ),
    ])
}

fn parse(input: &[u8]) -> (Result<Object, PdfError>, Diagnostics) {
    let source = memory(input);
    let mut diagnostics = Diagnostics::new();
    let result = Parser::new(&source).parse_object(&mut diagnostics);
    (result, diagnostics)
}

/// The object `input` holds, for input that raises no diagnostic.
fn object(input: &[u8]) -> Object {
    let (result, diagnostics) = parse(input);
    assert!(diagnostics.is_empty(), "{:?}", diagnostics.as_slice());
    result.unwrap()
}

fn parse_indirect(input: &[u8]) -> (Result<IndirectObject, PdfError>, Diagnostics) {
    let source = memory(input);
    let mut diagnostics = Diagnostics::new();
    let result = Parser::new(&source).parse_indirect_object(&mut diagnostics);
    (result, diagnostics)
}

fn offset_of(input: &[u8], needle: &[u8]) -> u64 {
    input
        .windows(needle.len())
        .position(|window| window == needle)
        .unwrap() as u64
}

fn span(object: &Object) -> (u64, u64) {
    let span = object.span().unwrap();
    (span.start, span.end)
}

fn id(number: u32, generation: u16) -> ObjectId {
    ObjectId { number, generation }
}

#[test]
fn scalars() {
    assert_eq!(object(b"true"), boolean(true));
    assert_eq!(object(b"false"), boolean(false));
    assert_eq!(object(b"null"), null());
    assert_eq!(object(b"12"), int(12));
    assert_eq!(object(b"-3.5"), real(-3.5));
    assert_eq!(object(b"(s)"), string(b"s"));
    assert_eq!(object(b"/N"), name("N"));
}

// 3.14 is the number the example of 7.3.6 holds, not an approximation of pi.
#[allow(clippy::approx_constant)]
#[test]
fn array_of_7_3_6() {
    assert_eq!(
        object(b"[549 3.14 false (Ralph) /SomeName]"),
        array(vec![
            int(549),
            real(3.14),
            boolean(false),
            string(b"Ralph"),
            name("SomeName")
        ])
    );
}

#[test]
fn dictionary_of_7_3_7() {
    assert_eq!(object(DICTIONARY_EXAMPLE), dictionary_example());
}

#[test]
fn empty_containers() {
    assert_eq!(object(b"[]"), array(vec![]));
    assert_eq!(object(b"<<>>"), dict(vec![]));
    assert_eq!(
        object(b"[[]<<>>]"),
        array(vec![array(vec![]), dict(vec![])])
    );
}

#[test]
fn reference_and_plain_integers() {
    assert_eq!(
        object(b"[1 0 R 2 3 4 0 R 5]"),
        array(vec![
            reference(1, 0),
            int(2),
            int(3),
            reference(4, 0),
            int(5)
        ])
    );
    assert_eq!(object(b"12 0 R"), reference(12, 0));
    assert_eq!(object(b"[1 2]"), array(vec![int(1), int(2)]));
    assert_eq!(object(b"[1 2 /R]"), array(vec![int(1), int(2), name("R")]));
}

#[test]
fn a_parse_stops_after_its_object() {
    let source = memory(b"5 (unterminated");
    let mut parser = Parser::new(&source);
    let mut diagnostics = Diagnostics::new();
    assert_eq!(parser.parse_object(&mut diagnostics).unwrap(), int(5));
    assert_eq!(parser.position(), 1);
    assert_unexpected_end(parser.parse_object(&mut diagnostics), 2);
}

#[test]
fn reference_parts_are_not_negative() {
    let input = b"[-1 0 R]";
    assert_unexpected_token(parse(input).0, offset_of(input, b"R"), "R");
    assert_eq!(object(b"[+1 0 R]"), array(vec![reference(1, 0)]));
}

#[test]
fn reference_out_of_range_is_null() {
    let above_u32 = u64::from(u32::MAX) + 1;
    let above_u16 = u32::from(u16::MAX) + 1;
    let input = format!("[{above_u32} 0 R 1 {above_u16} R]");
    let (result, diagnostics) = parse(input.as_bytes());
    assert_eq!(result.unwrap(), array(vec![null(), null()]));
    let second = offset_of(input.as_bytes(), b"R 1") + 2;
    assert_eq!(
        kinds_and_offsets(&diagnostics),
        [
            (DiagnosticKind::ReferenceOutOfRange, 1),
            (DiagnosticKind::ReferenceOutOfRange, second)
        ]
    );
}

#[test]
fn object_number_above_the_portable_count() {
    assert_eq!(
        object(format!("{PORTABLE_OBJECT_COUNT} 0 R").as_bytes()),
        reference(PORTABLE_OBJECT_COUNT, 0)
    );
    let above = PORTABLE_OBJECT_COUNT + 1;
    let (result, diagnostics) = parse(format!("{above} 0 R").as_bytes());
    assert_eq!(result.unwrap(), reference(above, 0));
    assert_eq!(
        kinds_and_offsets(&diagnostics),
        [(
            DiagnosticKind::BeyondPortableLimit(PortableLimit::ObjectNumber),
            0
        )]
    );
}

#[test]
fn duplicate_key_keeps_the_last_in_first_place() {
    let input = b"<</A 1 /B 2 /A 3>>";
    let (result, diagnostics) = parse(input);
    assert_eq!(result.unwrap(), dict(vec![("A", int(3)), ("B", int(2))]));
    assert_eq!(
        kinds_and_offsets(&diagnostics),
        [(
            DiagnosticKind::DuplicateKey {
                key: Name::new("A")
            },
            offset_of(input, b"/A 3")
        )]
    );
}

#[test]
fn key_must_be_a_name() {
    let input = b"<</A 1 2 3>>";
    assert_unexpected_token(parse(input).0, offset_of(input, b"2"), "2");
}

#[test]
fn key_without_value() {
    let input = b"<</A>>";
    assert_unexpected_token(parse(input).0, offset_of(input, b">>"), ">>");
}

#[test]
fn unknown_keyword_is_refused() {
    assert_unexpected_token(parse(b"[foo]").0, 1, "foo");
    assert_unexpected_token(parse(b"obj").0, 0, "obj");
    assert_unexpected_token(parse(b"R").0, 0, "R");
}

#[test]
fn mismatched_closer() {
    assert_unexpected_token(parse(b"[1 >>").0, 3, ">>");
    assert_unexpected_token(parse(b"<</A ]").0, 5, "]");
    assert_unexpected_token(parse(b"]").0, 0, "]");
}

#[test]
fn ends_inside_a_container() {
    assert_unexpected_end(parse(b"[1 2").0, 0);
    assert_unexpected_end(parse(b"<</A 1").0, 0);
    assert_unexpected_end(parse(b"<</A").0, 0);
    let nested = b"[<</A [";
    assert_unexpected_end(parse(nested).0, nested.len() as u64 - 1);
    assert_unexpected_end(parse(b"  ").0, 2);
    assert_unexpected_end(parse(b"").0, 0);
}

#[test]
fn spans_nest() {
    let input = b"<< /A [1 (x)] >>";
    let parsed = object(input);
    assert_eq!(span(&parsed), (0, input.len() as u64));
    let Value::Dictionary(dictionary) = parsed.value() else {
        panic!("a dictionary");
    };
    let inner = dictionary.get(b"A").unwrap();
    assert_eq!(
        span(inner),
        (offset_of(input, b"["), offset_of(input, b"]") + 1)
    );
    let Value::Array(items) = inner.value() else {
        panic!("an array");
    };
    assert_eq!(
        span(&items[0]),
        (offset_of(input, b"1"), offset_of(input, b"1") + 1)
    );
    assert_eq!(
        span(&items[1]),
        (offset_of(input, b"("), offset_of(input, b")") + 1)
    );

    let input = b" 12 0 R ";
    assert_eq!(span(&object(input)), (1, offset_of(input, b"R") + 1));
}

#[test]
fn editing_clears_spans_up_the_tree() {
    let mut parsed = object(b"[<</A 1>> 2]");
    let Value::Array(items) = parsed.value_mut() else {
        panic!("an array");
    };
    let Value::Dictionary(dictionary) = items[0].value_mut() else {
        panic!("a dictionary");
    };
    *dictionary.get_mut(b"A").unwrap().value_mut() = Value::Integer(9);

    assert_eq!(parsed.span(), None);
    let Value::Array(items) = parsed.value() else {
        panic!("an array");
    };
    assert_eq!(items[0].span(), None);
    let Value::Dictionary(dictionary) = items[0].value() else {
        panic!("a dictionary");
    };
    assert_eq!(dictionary.get(b"A").unwrap().span(), None);
    assert_eq!(dictionary.get(b"A"), Some(&int(9)));
    // The sibling was not on the path to the edit.
    assert_eq!(span(&items[1]), (10, 11));
}

#[test]
fn nesting_at_the_cap() {
    let mut at_cap = vec![b'['; MAX_NESTING_DEPTH];
    at_cap.extend(vec![b']'; MAX_NESTING_DEPTH]);
    let mut depth = 0;
    let mut current = &object(&at_cap);
    while let Value::Array(items) = current.value() {
        depth += 1;
        match items.first() {
            Some(inner) => current = inner,
            None => break,
        }
    }
    assert_eq!(depth, MAX_NESTING_DEPTH);

    let mut above = vec![b'['; MAX_NESTING_DEPTH + 1];
    above.extend(vec![b']'; MAX_NESTING_DEPTH + 1]);
    assert_limit(
        parse(&above).0,
        MAX_NESTING_DEPTH as u64,
        Limit::NestingDepth,
    );
}

/// `levels` containers, arrays and dictionaries in turn, around the
/// integer 1. Returns the text and the offset of the innermost opener.
fn alternating(levels: usize) -> (Vec<u8>, u64) {
    let mut text = Vec::new();
    let mut innermost = 0;
    for level in 0..levels {
        innermost = text.len() as u64;
        text.extend_from_slice(if level % 2 == 0 { b"[" } else { b"<</K " });
    }
    text.push(b'1');
    for level in (0..levels).rev() {
        text.extend_from_slice(if level % 2 == 0 { b"]" } else { b">>" });
    }
    (text, innermost)
}

#[test]
fn nesting_of_mixed_containers_at_the_cap() {
    let (at_cap, _) = alternating(MAX_NESTING_DEPTH);
    assert!(parse(&at_cap).0.is_ok());
    let (above, innermost) = alternating(MAX_NESTING_DEPTH + 1);
    assert_limit(parse(&above).0, innermost, Limit::NestingDepth);
}

#[test]
fn a_million_open_brackets() {
    let input = vec![b'['; A_MILLION];
    assert_limit(
        parse(&input).0,
        MAX_NESTING_DEPTH as u64,
        Limit::NestingDepth,
    );
    let input = b"<</K ".repeat(A_MILLION);
    assert_limit(
        parse(&input).0,
        (MAX_NESTING_DEPTH * b"<</K ".len()) as u64,
        Limit::NestingDepth,
    );
}

#[test]
fn many_repeats_of_one_key() {
    let mut input = b"<<".to_vec();
    input.extend(b"/A 1 ".repeat(MANY_REPEATS));
    input.extend_from_slice(b">>");
    let (result, diagnostics) = parse(&input);
    assert_eq!(result.unwrap(), dict(vec![("A", int(1))]));
    assert_eq!(diagnostics.as_slice().len(), MANY_REPEATS - 1);
    assert!(diagnostics.as_slice().iter().all(|d| d.kind
        == DiagnosticKind::DuplicateKey {
            key: Name::new("A")
        }));
}

#[test]
fn many_distinct_keys() {
    let mut input = b"<<".to_vec();
    for index in 0..MANY_REPEATS {
        input.extend_from_slice(format!("/K{index} {index} ").as_bytes());
    }
    input.extend_from_slice(b">>");
    let parsed = object(&input);
    let Value::Dictionary(dictionary) = parsed.value() else {
        panic!("a dictionary");
    };
    assert_eq!(dictionary.len(), MANY_REPEATS);
    let last = MANY_REPEATS - 1;
    assert_eq!(
        dictionary.get(format!("K{last}").as_bytes()),
        Some(&int(last as i64))
    );
}

#[test]
fn indirect_object_of_7_3_10() {
    let input = b"12 0 obj\n(Brillig)\nendobj";
    let (result, diagnostics) = parse_indirect(input);
    assert!(diagnostics.is_empty());
    let parsed = result.unwrap();
    assert_eq!(parsed.id, id(12, 0));
    assert_eq!(parsed.object, string(b"Brillig"));
    assert_eq!(parsed.stream_data_start, None);
    assert_eq!(
        parsed.span,
        Span {
            start: 0,
            end: input.len() as u64
        }
    );
}

#[test]
fn indirect_object_without_spaces() {
    let input = b"  1 0 obj<</A 2>>endobj ";
    let (result, diagnostics) = parse_indirect(input);
    assert!(diagnostics.is_empty());
    let parsed = result.unwrap();
    assert_eq!(parsed.id, id(1, 0));
    assert_eq!(parsed.object, dict(vec![("A", int(2))]));
    assert_eq!(
        parsed.span,
        Span {
            start: 2,
            end: input.len() as u64 - 1
        }
    );
}

#[test]
fn indirect_objects_one_after_another() {
    let source = memory(b"1 0 obj 5 endobj\n2 3 obj [1 0 R] endobj");
    let mut parser = Parser::new(&source);
    let mut diagnostics = Diagnostics::new();
    let first = parser.parse_indirect_object(&mut diagnostics).unwrap();
    assert_eq!((first.id, first.object), (id(1, 0), int(5)));
    let second = parser.parse_indirect_object(&mut diagnostics).unwrap();
    assert_eq!(
        (second.id, second.object),
        (id(2, 3), array(vec![reference(1, 0)]))
    );
    assert!(diagnostics.is_empty());
}

#[test]
fn stream_header_with_lf_and_crlf() {
    for end_of_line in [&b"\n"[..], b"\r\n"] {
        let mut input = b"3 0 obj\n<</Length 4>>\nstream".to_vec();
        input.extend_from_slice(end_of_line);
        input.extend_from_slice(b"DATA\nendstream\nendobj");
        let (result, diagnostics) = parse_indirect(&input);
        assert!(diagnostics.is_empty());
        let parsed = result.unwrap();
        let data = offset_of(&input, b"DATA");
        assert_eq!(parsed.id, id(3, 0));
        assert_eq!(parsed.object, dict(vec![("Length", int(4))]));
        assert_eq!(parsed.stream_data_start, Some(data));
        assert_eq!(
            parsed.span,
            Span {
                start: 0,
                end: data
            }
        );
    }
}

#[test]
fn the_parser_is_left_at_the_stream_data() {
    let input = b"3 0 obj <</Length 4>> stream\n\x00\xFF()\nendstream endobj";
    let source = memory(input);
    let mut parser = Parser::new(&source);
    let parsed = parser
        .parse_indirect_object(&mut Diagnostics::new())
        .unwrap();
    let start = parsed.stream_data_start.unwrap();
    assert_eq!(parser.position(), start);
    assert_eq!(
        &parser.lexer().cursor().read_exact(4).unwrap()[..],
        [0x00, 0xFF, b'(', b')']
    );
}

#[test]
fn stream_keyword_with_bare_cr_or_nothing() {
    // Data that starts with a delimiter, so the keyword ends where it does
    // when no end-of-line follows it.
    for after_keyword in [&b"\r"[..], b""] {
        let mut input = b"3 0 obj <</Length 4>> stream".to_vec();
        input.extend_from_slice(after_keyword);
        input.extend_from_slice(b"(ATA");
        let (result, diagnostics) = parse_indirect(&input);
        assert_eq!(
            result.unwrap().stream_data_start,
            Some(offset_of(&input, b"(ATA"))
        );
        assert_eq!(
            diagnostics.as_slice(),
            [Diagnostic {
                kind: DiagnosticKind::StreamKeywordEol,
                object: Some(id(3, 0)),
                offset: offset_of(&input, b"stream"),
            }]
        );
    }
}

#[test]
fn stream_needs_a_dictionary() {
    let input = b"1 0 obj 5 stream\n";
    assert_unexpected_token(
        parse_indirect(input).0,
        offset_of(input, b"stream"),
        "stream",
    );
}

#[test]
fn missing_endobj_is_reported() {
    let input = b"1 0 obj 5\n2 0 obj 6 endobj";
    let source = memory(input);
    let mut parser = Parser::new(&source);
    let mut diagnostics = Diagnostics::new();

    let first = parser.parse_indirect_object(&mut diagnostics).unwrap();
    let value_end = offset_of(input, b"5") + 1;
    assert_eq!((first.id, &first.object), (id(1, 0), &int(5)));
    assert_eq!(
        first.span,
        Span {
            start: 0,
            end: value_end
        }
    );
    assert_eq!(
        diagnostics.as_slice(),
        [Diagnostic {
            kind: DiagnosticKind::MissingEndobj,
            object: Some(id(1, 0)),
            offset: value_end,
        }]
    );

    let second = parser.parse_indirect_object(&mut diagnostics).unwrap();
    assert_eq!((second.id, second.object), (id(2, 0), int(6)));
    assert_eq!(diagnostics.as_slice().len(), 1);
}

#[test]
fn missing_endobj_at_the_end_of_input() {
    let (result, diagnostics) = parse_indirect(b"1 0 obj [1]");
    assert_eq!(result.unwrap().object, array(vec![int(1)]));
    assert_eq!(
        kinds_and_offsets(&diagnostics),
        [(DiagnosticKind::MissingEndobj, 11)]
    );
}

#[test]
fn diagnostics_carry_the_object_id() {
    let source = memory(b"7 0 obj <</A 1 /A 2>> endobj <</B 1 /B 2>>");
    let mut parser = Parser::new(&source);
    let mut diagnostics = Diagnostics::new();
    parser.parse_indirect_object(&mut diagnostics).unwrap();
    parser.parse_object(&mut diagnostics).unwrap();
    let objects: Vec<Option<ObjectId>> = diagnostics.as_slice().iter().map(|d| d.object).collect();
    assert_eq!(objects, [Some(id(7, 0)), None]);
}

#[test]
fn a_failed_indirect_object_stops_stamping() {
    let source = memory(b"7 0 obj [ foo ] <</B 1 /B 2>>");
    let mut parser = Parser::new(&source);
    let mut diagnostics = Diagnostics::new();
    assert!(parser.parse_indirect_object(&mut diagnostics).is_err());
    parser.seek(16).unwrap();
    parser.parse_object(&mut diagnostics).unwrap();
    assert_eq!(diagnostics.as_slice().len(), 1);
    assert_eq!(diagnostics.as_slice()[0].object, None);
}

#[test]
fn object_header_above_the_portable_count() {
    let above = PORTABLE_OBJECT_COUNT + 1;
    let (result, diagnostics) = parse_indirect(format!("{above} 0 obj null endobj").as_bytes());
    assert_eq!(result.unwrap().id, id(above, 0));
    assert_eq!(
        diagnostics.as_slice(),
        [Diagnostic {
            kind: DiagnosticKind::BeyondPortableLimit(PortableLimit::ObjectNumber),
            object: Some(id(above, 0)),
            offset: 0,
        }]
    );

    // The largest object number and generation the identifier holds.
    let input = format!("{} {} obj null endobj", u32::MAX, u16::MAX);
    let (result, _) = parse_indirect(input.as_bytes());
    assert_eq!(result.unwrap().id, id(u32::MAX, u16::MAX));
}

#[test]
fn bad_object_header() {
    assert_unexpected_token(parse_indirect(b"x 0 obj").0, 0, "x");
    assert_unexpected_token(parse_indirect(b"1 x obj").0, 2, "x");
    assert_unexpected_token(parse_indirect(b"1 0 foo").0, 4, "foo");
    assert_unexpected_token(parse_indirect(b"-1 0 obj").0, 0, "-1");
    let above_u32 = (u64::from(u32::MAX) + 1).to_string();
    assert_unexpected_token(
        parse_indirect(format!("{above_u32} 0 obj").as_bytes()).0,
        0,
        &above_u32,
    );
    let above_u16 = (u32::from(u16::MAX) + 1).to_string();
    assert_unexpected_token(
        parse_indirect(format!("1 {above_u16} obj").as_bytes()).0,
        2,
        &above_u16,
    );
    assert_unexpected_end(parse_indirect(b"1 0").0, 0);
    assert_unexpected_end(parse_indirect(b" 1 0 obj").0, 1);
    assert_unexpected_end(parse_indirect(b"").0, 0);
}

#[test]
fn parses_through_a_file_source() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("dictionary.txt");
    std::fs::write(&path, DICTIONARY_EXAMPLE).unwrap();
    let file = FileSource::open(&path).unwrap();
    let from_file = Parser::new(&file)
        .parse_object(&mut Diagnostics::new())
        .unwrap();
    let from_memory = object(DICTIONARY_EXAMPLE);
    assert_eq!(from_file, from_memory);
    assert_eq!(from_file, dictionary_example());

    fn spans(object: &Object, out: &mut Vec<Option<Span>>) {
        out.push(object.span());
        match object.value() {
            Value::Array(items) => items.iter().for_each(|item| spans(item, out)),
            Value::Dictionary(dictionary) => {
                dictionary.iter().for_each(|(_, value)| spans(value, out));
            }
            _ => {}
        }
    }
    let (mut file_spans, mut memory_spans) = (Vec::new(), Vec::new());
    spans(&from_file, &mut file_spans);
    spans(&from_memory, &mut memory_spans);
    assert_eq!(file_spans, memory_spans);
    assert!(file_spans.iter().all(Option::is_some));
}

/// The elements of `[[1 2] <</A 3 /B [4]>>]`: two items of the outer
/// array, two of the first inner array, two entries of the dictionary
/// and one item of its array.
const SEVEN_ELEMENTS: &[u8] = b"[[1 2] <</A 3 /B [4]>>]";

#[test]
fn elements_are_counted_through_arrays_and_dictionaries() {
    let source = memory(SEVEN_ELEMENTS);
    let limited = |max| {
        Parser::new(&source)
            .with_max_elements(max)
            .parse_object(&mut Diagnostics::new())
    };
    assert!(limited(7).is_ok());
    // The seventh element is the item of the innermost array.
    let innermost = SEVEN_ELEMENTS.iter().rposition(|&b| b == b'[').unwrap() as u64;
    assert_limit(limited(6), innermost, Limit::ObjectElements { max: 6 });
    // The first element of all belongs to the outer array.
    assert_limit(limited(0), 0, Limit::ObjectElements { max: 0 });
    // An object that is no array or dictionary has no elements.
    let scalar = memory(b"(a string)");
    let result = Parser::new(&scalar)
        .with_max_elements(0)
        .parse_object(&mut Diagnostics::new());
    assert!(result.is_ok());
}

#[test]
fn the_element_count_starts_again_with_each_object() {
    let source = memory(b"[1 2] [3 4] 5 0 obj [6 7] endobj");
    let mut parser = Parser::new(&source).with_max_elements(2);
    let mut diagnostics = Diagnostics::new();
    assert!(parser.parse_object(&mut diagnostics).is_ok());
    assert!(parser.parse_object(&mut diagnostics).is_ok());
    assert!(parser.parse_indirect_object(&mut diagnostics).is_ok());
}

#[test]
fn a_parser_given_no_limit_counts_none() {
    let source = memory(SEVEN_ELEMENTS);
    assert!(Parser::new(&source)
        .parse_object(&mut Diagnostics::new())
        .is_ok());
}
