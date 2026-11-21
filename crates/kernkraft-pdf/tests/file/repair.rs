// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The cross-reference rebuilt by reading a file through. Every file
//! here ends without `startxref`, so the fault is placed at its end.

use kernkraft_pdf::file::{PdfFile, SectionForm, XrefEntry};
use kernkraft_pdf::syntax::{PdfString, Value};
use kernkraft_pdf::{DiagnosticKind, PdfError};

use crate::{
    deflate, id, integer, is_missing, keys, load, located, object_stream_data, open, reference,
    small_file, value, xref_rows, Builder, HEADER,
};

fn in_use(offset: u64) -> Option<XrefEntry> {
    Some(XrefEntry::InUse {
        offset,
        generation: 0,
    })
}

fn rebuilt(b: &Builder) -> PdfFile {
    b.open().rebuilt(b.here())
}

fn string(file: &PdfFile, number: u32) -> Vec<u8> {
    match value(&load(file, number)) {
        Value::String(string) => string.as_bytes().to_vec(),
        other => panic!("not a string: {other:?}"),
    }
}

#[test]
fn objects_are_found_and_the_catalog_names_the_root() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog /Pages 2 0 R>>");
    let pages = b.object(2, "<</Type /Pages /Kids [] /Count 0>>");
    let seven = b.object_gen(7, 2, "(seven)");
    let file = rebuilt(&b);
    assert_eq!(file.sections().len(), 1);
    let section = &file.sections()[0];
    assert_eq!(section.form(), SectionForm::Rebuilt);
    assert_eq!(section.offset(), 0);
    assert_eq!(section.size(), 8);
    assert_eq!(keys(file.trailer()), ["Root", "Size"]);
    assert_eq!(reference(file.trailer(), "Root"), id(1));
    assert_eq!(integer(file.trailer(), "Size"), 8);

    assert_eq!(file.entry(1), in_use(catalog));
    assert_eq!(file.entry(2), in_use(pages));
    assert_eq!(
        file.entry(7),
        Some(XrefEntry::InUse {
            offset: seven,
            generation: 2
        })
    );
    for number in [0, 3, 6, 8] {
        assert_eq!(file.entry(number), None);
    }
    let numbers: Vec<u32> = section.entries().map(|(n, _)| n).collect();
    assert_eq!(numbers, [1, 2, 7]);
    assert_eq!(
        located(&b.bytes, &load(&file, 2)),
        b"2 0 obj\n<</Type /Pages /Kids [] /Count 0>>\nendobj"
    );
}

#[test]
fn a_file_cut_off_before_its_cross_reference_is_read() {
    let (b, [catalog, pages, string_offset]) = small_file();
    let table = b.bytes.windows(6).rposition(|w| w == b"\nxref\n").unwrap() + 1;
    // Cut at the table and inside it.
    for cut in [table, table + 30] {
        let bytes = &b.bytes[..cut];
        let file = open(bytes).rebuilt(cut as u64);
        assert_eq!(file.entry(1), in_use(catalog));
        assert_eq!(file.entry(2), in_use(pages));
        assert_eq!(file.entry(3), in_use(string_offset), "cut at {cut}");
        assert_eq!(string(&file, 3), b"Brillig");
        assert_eq!(reference(file.trailer(), "Root"), id(1));
    }
    // Cut inside the `endobj` of the last object, which is then an
    // object with no `endobj`: found, and reported when it is loaded.
    let cut = table - b"obj\n".len();
    let file = open(&b.bytes[..cut]).rebuilt(cut as u64);
    assert_eq!(file.entry(3), in_use(string_offset));
    let (result, diagnostics) = crate::load_noting(&file, id(3));
    let after_value = string_offset + b"3 0 obj\n(Brillig)".len() as u64;
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::MissingEndobj, Some(id(3)), after_value)]
    );
    assert_eq!(
        value(&result.unwrap().unwrap()),
        &Value::String(PdfString::new("Brillig"))
    );
    // Cut inside the string of the last object, which is then no object.
    let cut = string_offset as usize + b"3 0 obj\n(Bril".len();
    let file = open(&b.bytes[..cut]).rebuilt(cut as u64);
    assert_eq!(file.entry(3), None);
    assert_eq!(file.entry(2), in_use(pages));
}

#[test]
fn the_later_of_two_objects_with_one_number_counts() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    b.object(2, "(first)");
    let later = b.object(2, "(second)");
    let newer_generation = b.object_gen(3, 1, "(three)");
    let older_generation = b.object(3, "(three again)");
    let file = rebuilt(&b);
    assert_eq!(file.entry(2), in_use(later));
    assert_eq!(string(&file, 2), b"second");
    // File order decides, whatever the generations are.
    assert_eq!(file.entry(3), in_use(older_generation));
    assert_ne!(newer_generation, older_generation);
}

#[test]
fn the_last_catalog_standing_names_the_root() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog /Old true>>");
    b.object(4, "<</Type /Catalog>>");
    // Object 4 is replaced by something that is no catalog, so the
    // catalog that still stands is object 1.
    b.object(4, "(no catalog)");
    let file = rebuilt(&b);
    assert_eq!(reference(file.trailer(), "Root"), id(1));

    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    b.object_gen(4, 3, "<</Type /Catalog>>");
    let file = rebuilt(&b);
    assert_eq!(
        reference(file.trailer(), "Root"),
        kernkraft_pdf::syntax::ObjectId {
            number: 4,
            generation: 3
        }
    );
}

#[test]
fn a_file_with_no_catalog_is_refused() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Pages>>");
    b.object(2, "(two)");
    assert!(matches!(b.open().result, Err(PdfError::NoCatalog)));
    // A header and nothing else.
    assert!(matches!(open(HEADER).result, Err(PdfError::NoCatalog)));
    // A stream whose dictionary says it is a catalog is not one.
    let mut b = Builder::new();
    b.stream(1, "/Type /Catalog", b"data");
    assert!(matches!(b.open().result, Err(PdfError::NoCatalog)));
}

#[test]
fn the_entries_of_the_last_trailer_with_a_root_are_kept() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    b.object(2, "<</Title (T)>>");
    b.object(5, "<</Type /Catalog>>");
    b.text("trailer\n<</Size 3 /Root 1 0 R /Info 2 0 R /Prev 77 /XRefStm 5>>\n");
    b.text("trailer\n<</Size 6 /Root 5 0 R /Info 2 0 R /ID [<01> <02>] /Encrypt 9 0 R /Prev 99 /Other 1>>\n");
    // A trailer with no Root is passed over.
    b.text("trailer\n<</Size 9>>\n");
    let file = rebuilt(&b);
    assert_eq!(
        keys(file.trailer()),
        ["Root", "Info", "ID", "Encrypt", "Size"]
    );
    assert_eq!(reference(file.trailer(), "Root"), id(5));
    assert_eq!(reference(file.trailer(), "Info"), id(2));
    assert_eq!(reference(file.trailer(), "Encrypt"), id(9));
    // Size is counted from what was found, not copied.
    assert_eq!(integer(file.trailer(), "Size"), 6);
    match file.trailer().get(b"ID").unwrap().value() {
        Value::Array(items) => assert_eq!(
            items.iter().map(|i| i.value().clone()).collect::<Vec<_>>(),
            [
                Value::String(PdfString::new([1u8])),
                Value::String(PdfString::new([2u8]))
            ]
        ),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_trailer_takes_precedence_over_a_catalog_found() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    b.object(2, "<</NoType true>>");
    b.text("trailer\n<</Root 2 0 R>>\n");
    assert_eq!(reference(rebuilt(&b).trailer(), "Root"), id(2));
}

#[test]
fn what_looks_like_an_object_inside_another_is_not_one() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog /Note (7 0 obj (in a string) endobj)>>");
    // In the data of a stream with a sound Length, and of one whose end
    // is found by `endstream`.
    let (two, _) = b.stream(
        2,
        "",
        b"8 0 obj (in stream data) endobj\ntrailer <</Root 8 0 R>>",
    );
    let (three, _) = b.stream_with(3, "", b"9 0 obj (in stream data) endobj");
    // In a comment.
    b.text("% 10 0 obj (in a comment) endobj\n");
    let four = b.object(4, "(four)");
    let file = rebuilt(&b);
    assert_eq!(file.entry(1), in_use(catalog));
    assert_eq!(file.entry(2), in_use(two));
    assert_eq!(file.entry(3), in_use(three));
    assert_eq!(file.entry(4), in_use(four));
    for number in [7, 8, 9, 10] {
        assert_eq!(file.entry(number), None, "object {number}");
    }
    assert_eq!(file.sections()[0].size(), 5);
    assert_eq!(reference(file.trailer(), "Root"), id(1));
}

#[test]
fn a_header_is_two_numbers_and_obj_with_white_space_between() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    // Each of these is not a header.
    for not_a_header in [
        "x2 0 obj (a) endobj\n",
        "2 0 objx (a) endobj\n",
        "2 0x obj (a) endobj\n",
        "2 0 endobj\n",
        "2 obj (a) endobj\n",
        "2 /N 0 obj (a) endobj\n",
        "2 0 % comment\nobj (a) endobj\n",
        "-2 0 obj (a) endobj\n",
        "2.0 0 obj (a) endobj\n",
        "4294967296 0 obj (a) endobj\n",
        "2 65536 obj (a) endobj\n",
        "12345678901 0 obj (a) endobj\n",
        "2 0 obj >> endobj\n",
    ] {
        b.text(not_a_header);
    }
    // These are: any white space between the parts, a delimiter straight
    // after `obj`, a number before the header, and no white space before
    // the header but a delimiter.
    let three = b.text("3\r\n0\t\x00obj(three)endobj\n");
    b.text("99 ");
    let four = b.text("4 0 obj<</K 1>>endobj\n");
    b.text("]");
    let five = b.text("5 0 obj[5]endobj\n");
    let file = rebuilt(&b);
    assert_eq!(file.entry(2), None);
    assert_eq!(file.entry(3), in_use(three));
    assert_eq!(file.entry(4), in_use(four));
    assert_eq!(file.entry(5), in_use(five));
    let numbers: Vec<u32> = file.sections()[0].entries().map(|(n, _)| n).collect();
    assert_eq!(numbers, [1, 3, 4, 5]);
}

/// A file with objects in an object stream and a cross-reference stream
/// that cannot be read (it has no `W`). Returns the file, the offset of
/// the object stream 4, and the offset of the cross-reference stream 5.
fn compressed_file(objects: &[(u32, &str)], xref_entries: &str) -> (Builder, u64, u64) {
    let mut b = Builder::new();
    let (data, first) = object_stream_data(objects);
    let count = objects.len();
    let (four, _) = b.stream(
        4,
        &format!("/Type /ObjStm /N {count} /First {first} /Filter /FlateDecode"),
        &deflate(&data),
    );
    let xref = b.here();
    b.stream(5, xref_entries, b"");
    b.startxref(xref);
    (b, four, xref)
}

#[test]
fn the_objects_of_an_object_stream_are_found() {
    let objects = [(1, "<</Type /Catalog>>"), (2, "(two)"), (3, "(three)")];
    let (b, four, xref) = compressed_file(&objects, "/Type /XRef /Size 6 /Root 1 0 R /Info 2 0 R");
    let file = b.open().rebuilt(xref);
    for (index, number) in (0..).zip([1, 2, 3]) {
        assert_eq!(
            file.entry(number),
            Some(XrefEntry::Compressed { stream: 4, index })
        );
    }
    assert_eq!(file.entry(4), in_use(four));
    assert_eq!(file.entry(5), in_use(xref));
    assert_eq!(string(&file, 3), b"three");
    // The dictionary of the cross-reference stream serves as a trailer.
    assert_eq!(keys(file.trailer()), ["Root", "Info", "Size"]);
    assert_eq!(reference(file.trailer(), "Root"), id(1));
}

#[test]
fn a_catalog_in_an_object_stream_names_the_root() {
    let objects = [(2, "(two)"), (1, "<</Type /Catalog>>")];
    // The cross-reference stream names no Root.
    let (b, _, xref) = compressed_file(&objects, "/Type /XRef /Size 6");
    let file = b.open().rebuilt(xref);
    assert_eq!(reference(file.trailer(), "Root"), id(1));
    assert_eq!(
        file.entry(1),
        Some(XrefEntry::Compressed {
            stream: 4,
            index: 1
        })
    );
}

#[test]
fn file_order_decides_between_an_object_and_a_compressed_one() {
    // Object 2 in the file, then in an object stream: the stream's
    // counts. Object 3 in the stream, then in the file: the file's.
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    b.object(2, "(two in the file)");
    let (data, first) = object_stream_data(&[(2, "(two compressed)"), (3, "(three compressed)")]);
    b.stream(4, &format!("/Type /ObjStm /N 2 /First {first}"), &data);
    let three = b.object(3, "(three in the file)");
    let file = rebuilt(&b);
    assert_eq!(
        file.entry(2),
        Some(XrefEntry::Compressed {
            stream: 4,
            index: 0
        })
    );
    assert_eq!(string(&file, 2), b"two compressed");
    assert_eq!(file.entry(3), in_use(three));
    assert_eq!(string(&file, 3), b"three in the file");
}

#[test]
fn the_objects_of_a_replaced_object_stream_do_not_count() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    let (data, first) = object_stream_data(&[(2, "(two)"), (3, "(three)")]);
    b.stream(4, &format!("/Type /ObjStm /N 2 /First {first}"), &data);
    // Object 4 is replaced by a stream that holds only object 3.
    let (data, first) = object_stream_data(&[(3, "(three again)")]);
    let (four, _) = b.stream(4, &format!("/Type /ObjStm /N 1 /First {first}"), &data);
    let file = rebuilt(&b);
    assert_eq!(file.entry(4), in_use(four));
    assert_eq!(file.entry(2), None);
    assert_eq!(
        file.entry(3),
        Some(XrefEntry::Compressed {
            stream: 4,
            index: 0
        })
    );
    assert_eq!(string(&file, 3), b"three again");
}

#[test]
fn an_object_stream_that_cannot_be_read_is_reported_and_left_out() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    // The table is cut short by First.
    let (four, _) = b.stream(4, "/Type /ObjStm /N 2 /First 2", b"2 0 3 6 (two) (three)");
    // A generation other than 0 is no object stream (7.5.7).
    let five = b.text("5 1 obj\n<</Type /ObjStm /N 1 /First 4 /Length 9>>\nstream\n6 0 (six)\nendstream\nendobj\n");
    let (file, diagnostics) = b.open().with_diagnostics();
    assert_eq!(
        diagnostics,
        [
            (DiagnosticKind::XrefRebuilt, b.here()),
            (DiagnosticKind::ObjectStreamUnreadable { stream: 4 }, four),
        ]
    );
    assert_eq!(file.entry(1), in_use(catalog));
    assert_eq!(file.entry(4), in_use(four));
    assert_eq!(
        file.entry(5),
        Some(XrefEntry::InUse {
            offset: five,
            generation: 1
        })
    );
    for number in [2, 3, 6] {
        assert_eq!(file.entry(number), None);
    }
}

#[test]
fn the_length_of_an_object_stream_is_followed_to_an_object_of_the_file() {
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    let (data, first) = object_stream_data(&[(2, "(two)")]);
    b.stream_with(
        4,
        &format!("/Type /ObjStm /N 1 /First {first} /Length 6 0 R"),
        &data,
    );
    b.object(6, &data.len().to_string());
    let file = rebuilt(&b);
    assert_eq!(string(&file, 2), b"two");
}

#[test]
fn offsets_counted_from_a_header_that_is_not_at_the_start() {
    // The file of `small_file` behind a line of other bytes: every
    // offset it holds is short by the length of that line.
    let (b, [catalog, _, string_offset]) = small_file();
    let junk = b"Content-Type: application/pdf\r\n\r\n";
    let shift = junk.len() as u64;
    let bytes = [&junk[..], &b.bytes].concat();
    let (file, diagnostics) = open(&bytes).with_diagnostics();
    let startxref_value = b.bytes.windows(6).rposition(|w| w == b"\nxref\n").unwrap() as u64 + 1;
    // What lies at the offset `startxref` gives is `ref\n0 4...`: the
    // table's keyword less its first bytes, in the shifted file. The
    // fault is placed there.
    assert_eq!(
        diagnostics,
        [
            (DiagnosticKind::HeaderNotAtStart, shift),
            (DiagnosticKind::XrefRebuilt, startxref_value),
        ]
    );
    assert!(file.xref_rebuilt());
    assert_eq!(file.entry(1), in_use(catalog + shift));
    assert_eq!(file.entry(3), in_use(string_offset + shift));
    assert_eq!(string(&file, 3), b"Brillig");
    assert!(!is_missing(&file, id(2)));
    // The trailer the file wrote is found and used.
    assert_eq!(keys(file.trailer()), ["Root", "Size"]);
    let _ = xref_rows;
}
