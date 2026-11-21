// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Object streams (7.5.7) and the compressed objects in them.

use kernkraft_pdf::file::{Location, ObjectStream, PdfFile, ReadLimits, XrefEntry};
use kernkraft_pdf::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Value};
use kernkraft_pdf::{DiagnosticKind, Diagnostics, PdfError};

use crate::{
    assert_object_stream, deflate, hex, id, is_missing, load, load_noting, object_stream_data,
    open_with, value, xref_rows, Builder, ROOMY,
};

/// The objects every file here keeps in object stream 4.
const OBJECTS: [(u32, &str); 3] = [
    (1, "<</Type /Catalog /Pages 2 0 R>>"),
    (2, "<</Type /Pages /Kids [] /Count 0>>"),
    (3, "(Brillig)"),
];

/// A file whose objects 1 to 3 lie in object stream 4, with the
/// cross-reference stream 5. `entries` gives the object stream's
/// dictionary entries (without `Length`) and its data, from the plain
/// data and the offset of the first object.
fn compressed(entries: impl Fn(&[u8], usize) -> (String, Vec<u8>)) -> Builder {
    compressed_with(entries, [[2, 4, 0], [2, 4, 1], [2, 4, 2]])
}

/// As [`compressed`], with the cross-reference rows of objects 1 to 3.
fn compressed_with(
    entries: impl Fn(&[u8], usize) -> (String, Vec<u8>),
    rows: [[u64; 3]; 3],
) -> Builder {
    let mut b = Builder::new();
    let (plain, first) = object_stream_data(&OBJECTS);
    let (dictionary, data) = entries(&plain, first);
    let (four, _) = b.stream(4, &dictionary, &data);
    let xref = b.here();
    let [one, two, three] = rows;
    b.stream(
        5,
        "/Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[[0, 0, 255], one, two, three, [1, four, 0], [1, xref, 0]],
        ),
    );
    b.startxref(xref);
    b
}

fn plain_entries(first: usize) -> String {
    format!("/Type /ObjStm /N 3 /First {first}")
}

fn plain() -> Builder {
    compressed(|data, first| (plain_entries(first), data.to_vec()))
}

#[test]
fn compressed_objects_are_read_from_their_stream() {
    let file = plain().open().clean();
    assert_eq!(
        file.entry(3),
        Some(XrefEntry::Compressed {
            stream: 4,
            index: 2
        })
    );
    let three = load(&file, 3);
    assert_eq!(value(&three), &Value::String(PdfString::new("Brillig")));
    assert_eq!(
        three.location,
        Location::ObjectStream {
            stream: 4,
            index: 2
        }
    );
    match value(&load(&file, 1)) {
        Value::Dictionary(dictionary) => assert_eq!(crate::reference(dictionary, "Pages"), id(2)),
        other => panic!("{other:?}"),
    }
    match value(&load(&file, 2)) {
        Value::Dictionary(dictionary) => assert_eq!(crate::integer(dictionary, "Count"), 0),
        other => panic!("{other:?}"),
    }
}

#[test]
fn the_filters_a_writer_would_use() {
    type Encode = fn(&[u8]) -> Vec<u8>;
    let cases: [(&str, Encode); 2] = [
        ("/Filter /FlateDecode", |data| deflate(data)),
        ("/Filter [/ASCIIHexDecode /FlateDecode]", |data| {
            hex(&deflate(data))
        }),
    ];
    for (filter, encode) in cases {
        let b =
            compressed(|data, first| (format!("{} {filter}", plain_entries(first)), encode(data)));
        let file = b.open().clean();
        assert_eq!(
            value(&load(&file, 3)),
            &Value::String(PdfString::new("Brillig"))
        );
    }
}

#[test]
fn a_span_in_a_compressed_object_is_an_offset_in_the_decoded_data() {
    let file = plain().open().clean();
    let mut diagnostics = Diagnostics::new();
    let stream = file.object_stream(4, &mut diagnostics).unwrap();
    assert!(diagnostics.is_empty());
    assert_eq!(stream.number(), 4);
    assert_eq!(stream.len(), 3);
    assert!(!stream.is_empty());
    assert_eq!(stream.object_numbers().collect::<Vec<_>>(), [1, 2, 3]);
    let (plain_data, _) = object_stream_data(&OBJECTS);
    let data = stream.data().unwrap();
    assert_eq!(&*data, &plain_data[..]);

    for (index, (number, text)) in (0..).zip(OBJECTS) {
        let object = stream
            .object(index, number, ROOMY.max_object_elements, &mut diagnostics)
            .unwrap();
        let span = object.span().unwrap();
        assert_eq!(
            &data[span.start as usize..span.end as usize],
            text.as_bytes()
        );
    }
    assert!(diagnostics.is_empty());
}

#[test]
fn a_compressed_object_has_generation_0() {
    let file = plain().open().clean();
    assert!(is_missing(
        &file,
        ObjectId {
            number: 3,
            generation: 1
        }
    ));
}

#[test]
fn a_diagnostic_of_a_compressed_object_names_the_object() {
    let mut b = Builder::new();
    let (data, first) = object_stream_data(&[(1, "<</Type /Catalog /Type /Catalog>>")]);
    let (two, _) = b.stream(2, &format!("/Type /ObjStm /N 1 /First {first}"), &data);
    let xref = b.here();
    b.stream(
        3,
        "/Type /XRef /Size 4 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[[0, 0, 255], [2, 2, 0], [1, two, 0], [1, xref, 0]],
        ),
    );
    b.startxref(xref);
    let file = b.open().clean();
    let (result, diagnostics) = load_noting(&file, id(1));
    result.unwrap().unwrap();
    // The second `/Type` key, as an offset in the decoded data.
    let second_key = first as u64 + "<</Type /Catalog ".len() as u64;
    assert_eq!(
        diagnostics,
        [(
            DiagnosticKind::DuplicateKey {
                key: Name::new("Type")
            },
            Some(id(1)),
            second_key
        )]
    );
}

#[test]
fn a_cross_reference_entry_the_stream_does_not_bear_out() {
    // An index the stream does not have; an index that holds another
    // object.
    for row in [[2, 4, 3], [2, 4, 0]] {
        let b = compressed_with(
            |data, first| (plain_entries(first), data.to_vec()),
            [[2, 4, 0], [2, 4, 1], row],
        );
        let file = b.open().clean();
        assert_object_stream(load_noting(&file, id(3)).0, 4);
        load(&file, 1);
    }
    // A stream number that is no object, a free one, and one that is
    // itself compressed.
    for row in [[2, 9, 0], [2, 0, 0], [2, 3, 0]] {
        let b = compressed_with(
            |data, first| (plain_entries(first), data.to_vec()),
            [[2, 4, 0], [2, 4, 1], row],
        );
        let file = b.open().clean();
        assert_object_stream(load_noting(&file, id(3)).0, row[1] as u32);
    }
}

#[test]
fn an_object_stream_that_is_not_a_stream_or_has_another_generation() {
    let mut b = Builder::new();
    let four = b.object(4, "<</Type /ObjStm /N 1 /First 4>>");
    let five = b.object_gen(5, 1, "(five)");
    let xref = b.here();
    b.stream(
        6,
        "/Type /XRef /Size 7 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[
                [0, 0, 255],
                [2, 4, 0],
                [2, 5, 0],
                [0, 0, 0],
                [1, four, 0],
                [1, five, 1],
                [1, xref, 0],
            ],
        ),
    );
    b.startxref(xref);
    let file = b.open().clean();
    assert_object_stream(load_noting(&file, id(1)).0, 4);
    assert_object_stream(load_noting(&file, id(2)).0, 5);
}

/// Parses `data` as object stream 4 under a dictionary of `entries`.
fn parse(entries: &[(&str, Value)], data: &[u8]) -> Result<ObjectStream, PdfError> {
    let mut dictionary = Dictionary::new();
    for (key, value) in entries {
        dictionary.insert(Name::new(*key), Object::new(value.clone()));
    }
    ObjectStream::parse(4, &dictionary, data.to_vec())
}

fn object_stream_type() -> Value {
    Value::Name(Name::new("ObjStm"))
}

/// The entries of an object stream's dictionary.
type Entries = Vec<(&'static str, Value)>;

fn entries(count: i64, first: i64) -> Entries {
    vec![
        ("Type", object_stream_type()),
        ("N", Value::Integer(count)),
        ("First", Value::Integer(first)),
    ]
}

#[test]
fn the_table_of_an_object_stream() {
    // Two objects: `true` at First and `null` five bytes on.
    let data = b"7 0 8 5 true null";
    let first = 8;
    let stream = parse(&entries(2, first), data).unwrap();
    assert_eq!(stream.object_numbers().collect::<Vec<_>>(), [7, 8]);
    let mut diagnostics = Diagnostics::new();
    let object = |index, number| stream.object(index, number, 10, &mut Diagnostics::new());
    assert_eq!(object(0, 7).unwrap().value(), &Value::Boolean(true));
    assert_eq!(object(1, 8).unwrap().value(), &Value::Null);
    assert_object_stream(object(2, 8), 4);
    assert_object_stream(object(1, 7), 4);
    assert_object_stream(object(u32::MAX, 7), 4);
    assert!(stream.object(0, 7, 10, &mut diagnostics).is_ok());
    assert!(diagnostics.is_empty());

    // An empty stream holds nothing and is valid.
    let empty = parse(&entries(0, 0), b"").unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.len(), 0);
}

#[test]
fn a_table_that_cannot_be_read() {
    let data = b"7 0 8 5 true null";
    let cases: [(&str, Entries, &[u8]); 16] = [
        ("no Type", entries(2, 8)[1..].to_vec(), data),
        (
            "another Type",
            vec![
                ("Type", Value::Name(Name::new("XRef"))),
                ("N", Value::Integer(2)),
                ("First", Value::Integer(8)),
            ],
            data,
        ),
        (
            "no N",
            vec![("Type", object_stream_type()), ("First", Value::Integer(8))],
            data,
        ),
        (
            "no First",
            vec![("Type", object_stream_type()), ("N", Value::Integer(2))],
            data,
        ),
        ("a negative N", entries(-1, 8), data),
        ("a negative First", entries(2, -8), data),
        (
            "an N that is no integer",
            vec![
                ("Type", object_stream_type()),
                ("N", Value::Null),
                ("First", Value::Integer(8)),
            ],
            data,
        ),
        (
            "a First past the data",
            entries(2, data.len() as i64 + 1),
            data,
        ),
        (
            "more pairs declared than the table holds",
            entries(3, 8),
            data,
        ),
        ("a table that runs past First", entries(2, 6), data),
        (
            "offsets that do not increase",
            entries(2, 8),
            b"7 5 8 5 true null",
        ),
        ("offsets that decrease", entries(2, 8), b"7 5 8 0 true null"),
        (
            "an offset past the data",
            entries(2, 8),
            b"7 0 8 9 true null",
        ),
        (
            "a negative object number",
            entries(2, 9),
            b"-7 0 8 5 true null",
        ),
        (
            "an object number past a u32",
            entries(1, 13),
            b"4294967296 0 true",
        ),
        (
            "a table entry that is no number",
            entries(2, 8),
            b"7 0 /8 5 true null",
        ),
    ];
    for (name, entries, data) in cases {
        let result = parse(&entries, data);
        assert!(
            matches!(result, Err(PdfError::ObjectStream { stream: 4 })),
            "{name}: {result:?}"
        );
    }
}

#[test]
fn a_compressed_object_that_cannot_be_read() {
    // 7.5.7: an object is not solely a reference. The others are no
    // object at all.
    for (text, first) in [("3 0 R", 4), (">>", 4), ("<</A", 4), ("endobj", 4)] {
        let data = format!("7 0 {text}");
        let stream = parse(&entries(1, first), data.as_bytes()).unwrap();
        let result = stream.object(0, 7, 10, &mut Diagnostics::new());
        assert_object_stream(result, 4);
    }
}

#[test]
fn damaged_stream_data_is_refused() {
    let b = compressed(|data, first| {
        let encoded = deflate(data);
        (
            format!("{} /Filter /FlateDecode", plain_entries(first)),
            encoded[..encoded.len() - 8].to_vec(),
        )
    });
    let file = b.open().clean();
    let (result, diagnostics) = load_noting(&file, id(3));
    assert_object_stream(result, 4);
    // The filter layer says what it met, against the object stream.
    assert_eq!(diagnostics.len(), 1);
    assert_eq!(diagnostics[0].1, Some(id(4)));
    assert!(matches!(
        diagnostics[0].0,
        DiagnosticKind::FilterDataDamaged { .. }
    ));
}

#[test]
fn the_decoded_size_of_an_object_stream_is_capped() {
    let b = plain();
    let (data, _) = object_stream_data(&OBJECTS);
    let limits = |max_object_stream_bytes| ReadLimits {
        max_object_stream_bytes,
        ..ROOMY
    };
    let load_three = |file: &PdfFile| load_noting(file, id(3)).0;

    let file = open_with(&b.bytes, limits(data.len())).clean();
    assert!(load_three(&file).unwrap().is_some());
    let file = open_with(&b.bytes, limits(data.len() - 1)).clean();
    let result = load_three(&file);
    assert!(
        matches!(result, Err(PdfError::DecodedTooLarge { cap }) if cap == data.len() as u64 - 1),
        "{result:?}"
    );
}
