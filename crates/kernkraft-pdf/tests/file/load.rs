// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Loading one object where the cross-reference places it (7.3.10) and
//! finding the extent of a stream's data (7.3.8.2).

use kernkraft_pdf::file::Location;
use kernkraft_pdf::syntax::{ObjectId, PdfString, Span, Value};
use kernkraft_pdf::{DiagnosticKind, PdfError};

use crate::{
    assert_mismatch, id, integer, is_missing, load, load_noting, located, object_stream_data, open,
    small_file, stream, value, xref_rows, Builder, Row, HEAD,
};

/// Stream data that holds every kind of end-of-line and ends in none.
const DATA: &[u8] = b"line one\nline two\r\nline three\rend";

/// A file of the catalog 1 and whatever `body` writes as object 2, with
/// a table of both. Returns the file and what `body` returned.
fn with_object_2<T>(body: impl FnOnce(&mut Builder) -> T) -> (Builder, T) {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let two = b.here();
    let made = body(&mut b);
    let xref = b.table(
        &[(0, &[HEAD, Row::N(catalog, 0), Row::N(two, 0)])],
        "/Size 3 /Root 1 0 R",
    );
    b.startxref(xref);
    (b, made)
}

fn data_span(start: u64) -> Span {
    Span {
        start,
        end: start + DATA.len() as u64,
    }
}

#[test]
fn an_object_is_read_with_the_bytes_it_came_from() {
    let (b, [catalog, _, string]) = small_file();
    let file = b.open().clean();
    let object = load(&file, 3);
    assert_eq!(object.id, id(3));
    assert_eq!(value(&object), &Value::String(PdfString::new("Brillig")));
    assert_eq!(located(&b.bytes, &object), b"3 0 obj\n(Brillig)\nendobj");
    assert_eq!(
        object.location,
        Location::File(Span {
            start: string,
            end: string + b"3 0 obj\n(Brillig)\nendobj".len() as u64
        })
    );
    let catalog_object = load(&file, 1);
    match value(&catalog_object) {
        Value::Dictionary(dictionary) => {
            assert_eq!(crate::reference(dictionary, "Pages"), id(2));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(located(&b.bytes, &catalog_object).len() as u64 + catalog, {
        match catalog_object.location {
            Location::File(span) => span.end,
            other => panic!("{other:?}"),
        }
    });
}

#[test]
fn an_object_the_cross_reference_does_not_have_in_use_is_missing() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let reused = b.object_gen(2, 3, "(third generation)");
    let xref = b.table(
        &[
            (
                0,
                &[Row::F(4, 65535), Row::N(catalog, 0), Row::N(reused, 3)],
            ),
            (4, &[Row::F(0, 7)]),
        ],
        "/Size 5 /Root 1 0 R",
    );
    b.startxref(xref);
    let file = b.open().clean();
    // Free entries and numbers with no entry.
    for number in [0, 4, 3, 5, u32::MAX] {
        assert!(is_missing(&file, id(number)), "object {number}");
    }
    // The generation is part of the identifier.
    for generation in [0, 2, 4] {
        let other = ObjectId {
            number: 2,
            generation,
        };
        assert!(is_missing(&file, other), "generation {generation}");
    }
    let (result, diagnostics) = load_noting(
        &file,
        ObjectId {
            number: 2,
            generation: 3,
        },
    );
    assert_eq!(diagnostics, []);
    assert_eq!(
        value(&result.unwrap().unwrap()),
        &Value::String(PdfString::new("third generation"))
    );
    // A free entry's generation does not make a free object loadable.
    assert!(is_missing(
        &file,
        ObjectId {
            number: 4,
            generation: 7
        }
    ));
}

#[test]
fn a_stream_is_read_as_its_dictionary_and_the_extent_of_its_data() {
    let (b, (offset, data_start)) = with_object_2(|b| b.stream(2, "/Kind /Test", DATA));
    let file = b.open().clean();
    let object = load(&file, 2);
    let stream = stream(&object);
    assert_eq!(stream.data, data_span(data_start));
    assert_eq!(integer(&stream.dictionary, "Length"), DATA.len() as i64);
    assert_eq!(crate::keys(&stream.dictionary), ["Kind", "Length"]);
    assert_eq!(&*file.stream_data(stream).unwrap(), DATA);
    let whole = located(&b.bytes, &object);
    assert!(whole.starts_with(b"2 0 obj\n<</Kind /Test"));
    assert!(whole.ends_with(b"\nendstream\nendobj"));
    assert_eq!(
        object.location,
        Location::File(Span {
            start: offset,
            end: offset + whole.len() as u64
        })
    );
}

#[test]
fn the_end_of_line_after_the_stream_keyword_is_not_data() {
    // 7.3.8.1: CR LF or LF. A lone CR is taken and reported by the syntax
    // layer.
    for (eol, reported) in [("\r\n", false), ("\n", false), ("\r", true)] {
        let (b, (keyword, data_start)) = with_object_2(|b| {
            b.text(&format!("2 0 obj\n<</Length {}>>\n", DATA.len()));
            let keyword = b.text(&format!("stream{eol}"));
            let data_start = b.raw(DATA);
            b.text("\nendstream\nendobj\n");
            (keyword, data_start)
        });
        let file = b.open().clean();
        let (result, diagnostics) = load_noting(&file, id(2));
        let expected = if reported {
            vec![(DiagnosticKind::StreamKeywordEol, Some(id(2)), keyword)]
        } else {
            Vec::new()
        };
        assert_eq!(diagnostics, expected, "{eol:?}");
        let object = result.unwrap().unwrap();
        assert_eq!(stream(&object).data, data_span(data_start), "{eol:?}");
    }
}

#[test]
fn the_end_of_line_before_endstream_is_optional() {
    for eol in ["", "\n", "\r\n", "\r", " ", "\n\n"] {
        let (b, data_start) = with_object_2(|b| {
            b.text(&format!("2 0 obj\n<</Length {}>>\nstream\n", DATA.len()));
            let data_start = b.raw(DATA);
            b.text(&format!("{eol}endstream\nendobj\n"));
            data_start
        });
        let file = b.open().clean();
        assert_eq!(
            stream(&load(&file, 2)).data,
            data_span(data_start),
            "{eol:?}"
        );
    }
}

#[test]
fn an_empty_stream() {
    let (b, (_, data_start)) = with_object_2(|b| b.stream(2, "", b""));
    let file = b.open().clean();
    let object = load(&file, 2);
    assert_eq!(
        stream(&object).data,
        Span {
            start: data_start,
            end: data_start
        }
    );
    assert_eq!(&*file.stream_data(stream(&object)).unwrap(), b"");
}

#[test]
fn a_length_that_is_a_reference_to_an_object_of_the_file() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let (two, data_start) = b.stream_with(2, "/Length 3 0 R", DATA);
    // The length comes after the stream, as a writer that does not know
    // it beforehand leaves it.
    let three = b.object(3, &DATA.len().to_string());
    let xref = b.table(
        &[(
            0,
            &[HEAD, Row::N(catalog, 0), Row::N(two, 0), Row::N(three, 0)],
        )],
        "/Size 4 /Root 1 0 R",
    );
    b.startxref(xref);
    let file = b.open().clean();
    assert_eq!(stream(&load(&file, 2)).data, data_span(data_start));
}

#[test]
fn a_length_that_is_a_reference_to_a_compressed_object() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let (two, data_start) = b.stream_with(2, "/Length 3 0 R", DATA);
    let length = DATA.len().to_string();
    let (data, first) = object_stream_data(&[(3, &length)]);
    let (four, _) = b.stream(4, &format!("/Type /ObjStm /N 1 /First {first}"), &data);
    let xref = b.here();
    b.stream(
        5,
        "/Type /XRef /Size 6 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[
                [0, 0, 255],
                [1, catalog, 0],
                [1, two, 0],
                [2, 4, 0],
                [1, four, 0],
                [1, xref, 0],
            ],
        ),
    );
    b.startxref(xref);
    let file = b.open().clean();
    assert_eq!(stream(&load(&file, 2)).data, data_span(data_start));
}

/// Data with no end-of-line in it, so that where a search for
/// `endstream` puts its end does not depend on how the data ends.
const PLAIN: &[u8] = b"0123456789";

#[test]
fn a_length_that_does_not_lead_to_endstream_is_set_aside_and_reported() {
    let exact = PLAIN.len();
    let cases: [(&str, String); 9] = [
        ("too short", format!("/Length {}", exact - 3)),
        ("too long", format!("/Length {}", exact + 3)),
        ("past the end of the file", "/Length 100000".to_owned()),
        ("negative", "/Length -1".to_owned()),
        ("absent", String::new()),
        ("a real", format!("/Length {exact}.0")),
        ("null", "/Length null".to_owned()),
        ("a reference to no object", "/Length 9 0 R".to_owned()),
        (
            "a reference to an object that is no integer",
            "/Length 1 0 R".to_owned(),
        ),
    ];
    for (name, entries) in cases {
        let (b, (_, data_start)) = with_object_2(|b| b.stream_with(2, &entries, PLAIN));
        let file = b.open().clean();
        let (result, diagnostics) = load_noting(&file, id(2));
        assert_eq!(
            diagnostics,
            [(DiagnosticKind::StreamLength, Some(id(2)), data_start)],
            "{name}"
        );
        let object = result.unwrap().unwrap();
        // The builder writes one line feed between data and `endstream`.
        assert_eq!(
            stream(&object).data,
            Span {
                start: data_start,
                end: data_start + exact as u64
            },
            "{name}"
        );
        assert!(located(&b.bytes, &object).ends_with(b"endobj"), "{name}");
    }
}

#[test]
fn a_search_for_endstream_leaves_out_one_end_of_line() {
    // (what lies between the data and `endstream`, how much of it is
    // counted as data)
    for (gap, kept) in [
        ("", 0),
        ("\n", 0),
        ("\r\n", 0),
        ("\r", 0),
        ("\n\n", 1),
        ("\n\r\n", 1),
        (" ", 1),
    ] {
        let (b, data_start) = with_object_2(|b| {
            b.text("2 0 obj\n<<>>\nstream\n");
            let data_start = b.raw(PLAIN);
            b.text(&format!("{gap}endstream\nendobj\n"));
            data_start
        });
        let file = b.open().clean();
        let (result, diagnostics) = load_noting(&file, id(2));
        assert_eq!(diagnostics.len(), 1, "{gap:?}");
        let object = result.unwrap().unwrap();
        assert_eq!(
            stream(&object).data,
            Span {
                start: data_start,
                end: data_start + PLAIN.len() as u64 + kept
            },
            "{gap:?}"
        );
    }
}

#[test]
fn a_stream_with_no_endstream_ends_with_the_file() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let xref = b.here();
    // The table places object 2 after `%%EOF`, where the file is cut.
    let table_len = {
        let mut probe = Builder::new();
        probe.table(
            &[(0, &[HEAD, Row::N(0, 0), Row::N(0, 0)])],
            "/Size 3 /Root 1 0 R",
        );
        probe.startxref(xref);
        probe.bytes.len() as u64 - crate::HEADER.len() as u64
    };
    let two = xref + table_len;
    b.table(
        &[(0, &[HEAD, Row::N(catalog, 0), Row::N(two, 0)])],
        "/Size 3 /Root 1 0 R",
    );
    b.startxref(xref);
    assert_eq!(b.here(), two);
    b.text("2 0 obj\n<</Length 400>>\nstream\ncut short");
    // `startxref` is still within reach of the end.
    let file = b.open().clean();
    let (result, _) = load_noting(&file, id(2));
    assert!(matches!(result, Err(PdfError::UnexpectedEnd { offset }) if offset == two));
}

#[test]
fn a_missing_endobj_is_reported() {
    // After a stream, and after a plain object.
    let (b, after) = with_object_2(|b| {
        b.text(&format!("2 0 obj\n<</Length {}>>\nstream\n", PLAIN.len()));
        b.raw(PLAIN);
        b.text("\nendstream");
        let after = b.here();
        b.text("\n");
        after
    });
    let file = b.open().clean();
    let (result, diagnostics) = load_noting(&file, id(2));
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::MissingEndobj, Some(id(2)), after)]
    );
    let object = result.unwrap().unwrap();
    assert!(located(&b.bytes, &object).ends_with(b"endstream"));

    let (b, after) = with_object_2(|b| {
        b.text("2 0 obj\n(no end)");
        let after = b.here();
        b.text("\n");
        after
    });
    let file = b.open().clean();
    let (result, diagnostics) = load_noting(&file, id(2));
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::MissingEndobj, Some(id(2)), after)]
    );
    assert!(located(&b.bytes, &result.unwrap().unwrap()).ends_with(b"(no end)"));
}

#[test]
fn white_space_at_an_object_s_offset_is_reported() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let before = b.text(" \n");
    let two = b.object(2, "(two)");
    let xref = b.table(
        &[(0, &[HEAD, Row::N(catalog, 0), Row::N(before, 0)])],
        "/Size 3 /Root 1 0 R",
    );
    b.startxref(xref);
    let file = b.open().clean();
    let (result, diagnostics) = load_noting(&file, id(2));
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::WhitespaceAtOffset, Some(id(2)), before)]
    );
    let object = result.unwrap().unwrap();
    assert_eq!(located(&b.bytes, &object), b"2 0 obj\n(two)\nendobj");
    assert_eq!(
        object.location,
        Location::File(Span {
            start: two,
            end: two + b"2 0 obj\n(two)\nendobj".len() as u64
        })
    );
}

#[test]
fn an_object_that_is_not_where_the_cross_reference_places_it() {
    let (b, [catalog, pages, string]) = small_file();
    let end = b.bytes.len() as u64;
    // Where object 3 is said to be: another object, the middle of an
    // object, the header comment, the end of the file and past it, and
    // an object of the same number and another generation.
    for offset in [catalog, pages + 3, 9, end, end + 50, string + 2] {
        let mut b = Builder::new();
        let c = b.object(1, "<</Type /Catalog /Pages 2 0 R>>");
        let p = b.object(2, "<</Type /Pages /Kids [] /Count 0>>");
        b.object(3, "(Brillig)");
        assert_eq!((c, p), (catalog, pages));
        let xref = b.table(
            &[(0, &[HEAD, Row::N(c, 0), Row::N(p, 0), Row::N(offset, 0)])],
            "/Size 4 /Root 1 0 R",
        );
        b.startxref(xref);
        let file = b.open().clean();
        let (result, diagnostics) = load_noting(&file, id(3));
        assert_eq!(diagnostics, []);
        assert_mismatch(result, id(3), offset);
        // The other objects are not affected.
        load(&file, 1);
    }

    // The object is there under another generation.
    let mut b = Builder::new();
    let c = b.object(1, "<</Type /Catalog>>");
    let two = b.object_gen(2, 1, "(generation one)");
    let xref = b.table(
        &[(0, &[HEAD, Row::N(c, 0), Row::N(two, 0)])],
        "/Size 3 /Root 1 0 R",
    );
    b.startxref(xref);
    let file = b.open().clean();
    assert_mismatch(load_noting(&file, id(2)).0, id(2), two);
}

#[test]
fn an_object_that_does_not_read_as_one_is_a_syntax_error() {
    let (b, two) = with_object_2(|b| {
        let two = b.here();
        b.text("2 0 obj\n<</Key >>\nendobj\n");
        two
    });
    let file = b.open().clean();
    let (result, _) = load_noting(&file, id(2));
    let closer = two + b"2 0 obj\n<</Key ".len() as u64;
    assert!(
        matches!(result, Err(PdfError::UnexpectedToken { offset, ref found }) if offset == closer && found == ">>"),
        "{result:?}"
    );
}

#[test]
fn rebuild_reads_a_sound_file_to_the_same_objects() {
    let (b, offsets) = small_file();
    let opened = b.open().clean();
    let mut diagnostics = kernkraft_pdf::Diagnostics::new();
    let source = std::sync::Arc::new(kernkraft_pdf::MemorySource::new(b.bytes.clone()));
    let rebuilt =
        kernkraft_pdf::file::PdfFile::rebuild(source, crate::ROOMY, &mut diagnostics).unwrap();
    assert!(rebuilt.xref_rebuilt());
    assert!(!opened.xref_rebuilt());
    for (number, offset) in (1..).zip(offsets) {
        assert_eq!(rebuilt.entry(number), Some(Row::N(offset, 0).entry()));
        assert_eq!(load(&rebuilt, number), load(&opened, number));
    }
    assert_eq!(open(&b.bytes).clean().limits(), crate::ROOMY);
}

#[test]
fn a_file_read_from_disk_gives_what_memory_gives() {
    let (b, _) = small_file();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("small.pdf");
    std::fs::write(&path, &b.bytes).unwrap();
    let source = std::sync::Arc::new(kernkraft_pdf::FileSource::open(&path).unwrap());
    let mut diagnostics = kernkraft_pdf::Diagnostics::new();
    let from_disk =
        kernkraft_pdf::file::PdfFile::open(source, crate::ROOMY, &mut diagnostics).unwrap();
    assert!(diagnostics.is_empty());
    let from_memory = b.open().clean();
    assert_eq!(from_disk.sections(), from_memory.sections());
    for number in 1..=3 {
        assert_eq!(load(&from_disk, number), load(&from_memory, number));
    }
    assert_eq!(from_disk.source().len(), b.bytes.len() as u64);
}

#[test]
fn an_open_file_is_shared_between_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<kernkraft_pdf::file::PdfFile>();

    let (b, _) = small_file();
    let file = b.open().clean();
    let expected = load(&file, 3);
    std::thread::scope(|scope| {
        for _ in 0..4 {
            scope.spawn(|| assert_eq!(load(&file, 3), expected));
        }
    });
}
