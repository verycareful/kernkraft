// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The limits a caller sets ([`ReadLimits`]), each tested at its value
//! and one past it.

use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::Limit;

use crate::{
    assert_limit, id, load_noting, object_stream_data, open_with, xref_rows, Builder, Row, HEAD,
    ROOMY,
};

fn entry_limit(max_xref_entries: usize) -> ReadLimits {
    ReadLimits {
        max_xref_entries,
        ..ROOMY
    }
}

fn element_limit(max_object_elements: usize) -> ReadLimits {
    ReadLimits {
        max_object_elements,
        ..ROOMY
    }
}

const ENTRIES: Limit = Limit::XrefEntries { max: 4 };

#[test]
fn the_entries_of_a_table_are_counted() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let rows = [
        HEAD,
        Row::N(catalog, 0),
        Row::N(catalog, 0),
        Row::N(catalog, 0),
    ];
    let xref = b.table(&[(0, &rows)], "/Size 4 /Root 1 0 R");
    b.startxref(xref);
    open_with(&b.bytes, entry_limit(4)).clean();
    assert_limit(
        open_with(&b.bytes, entry_limit(3)).result,
        xref,
        Limit::XrefEntries { max: 3 },
    );
}

#[test]
fn the_entries_of_every_section_count_together() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let first = b.table(&[(0, &[HEAD, Row::N(catalog, 0)])], "/Size 2 /Root 1 0 R");
    b.startxref(first);
    let two = b.object(2, "(two)");
    let second = b.table(
        &[(1, &[Row::N(catalog, 0), Row::N(two, 0)])],
        &format!("/Size 3 /Root 1 0 R /Prev {first}"),
    );
    b.startxref(second);
    open_with(&b.bytes, entry_limit(4)).clean();
    // The newest section is read first, so the limit is passed in the
    // older one.
    assert_limit(
        open_with(&b.bytes, entry_limit(3)).result,
        first,
        Limit::XrefEntries { max: 3 },
    );
}

#[test]
fn the_entries_a_stream_declares_are_counted_before_its_data_is_decoded() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let xref = b.here();
    b.stream(
        2,
        "/Type /XRef /Size 4 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[[0, 0, 255], [1, catalog, 0], [1, xref, 0], [0, 0, 0]],
        ),
    );
    b.startxref(xref);
    open_with(&b.bytes, entry_limit(4)).clean();
    assert_limit(
        open_with(&b.bytes, entry_limit(3)).result,
        xref,
        Limit::XrefEntries { max: 3 },
    );

    // A stream that declares far more entries than it has data for is
    // refused for the count, not read and then found short.
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    let xref = b.here();
    b.stream(
        2,
        "/Type /XRef /Size 4000000000 /W [1 2 1] /Root 1 0 R",
        b"",
    );
    b.startxref(xref);
    assert_limit(open_with(&b.bytes, entry_limit(4)).result, xref, ENTRIES);
}

#[test]
fn the_entries_of_a_hybrid_stream_are_counted() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let stream = b.here();
    b.stream(
        2,
        "/Type /XRef /Size 3 /W [1 2 1] /Index [2 1]",
        &xref_rows([1, 2, 1], &[[1, stream, 0]]),
    );
    let xref = b.table(
        &[(0, &[HEAD, Row::N(catalog, 0), Row::F(0, 65535)])],
        &format!("/Size 3 /Root 1 0 R /XRefStm {stream}"),
    );
    b.startxref(xref);
    open_with(&b.bytes, entry_limit(4)).clean();
    assert_limit(
        open_with(&b.bytes, entry_limit(3)).result,
        stream,
        Limit::XrefEntries { max: 3 },
    );
}

#[test]
fn the_objects_found_while_rebuilding_are_counted() {
    // No cross-reference at all: three objects in the file and two more
    // in an object stream.
    let mut b = Builder::new();
    b.object(1, "<</Type /Catalog>>");
    b.object(2, "(two)");
    let (data, first) = object_stream_data(&[(4, "(four)"), (5, "(five)")]);
    let (three, _) = b.stream(3, &format!("/Type /ObjStm /N 2 /First {first}"), &data);
    let file = open_with(&b.bytes, entry_limit(5)).result.unwrap();
    assert!(file.xref_rebuilt());
    assert_limit(
        open_with(&b.bytes, entry_limit(4)).result,
        three,
        Limit::XrefEntries { max: 4 },
    );
    // The third object found is the one past a limit of two.
    assert_limit(
        open_with(&b.bytes, entry_limit(2)).result,
        three,
        Limit::XrefEntries { max: 2 },
    );
}

/// A file whose object 2 is an array of two arrays of two integers: six
/// elements. Returns the file and the offset of the outer array.
fn nested_array() -> (Builder, u64) {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let two = b.object(2, "[[1 2] [3 4]]");
    let xref = b.table(
        &[(0, &[HEAD, Row::N(catalog, 0), Row::N(two, 0)])],
        "/Size 3 /Root 1 0 R",
    );
    b.startxref(xref);
    (b, two + b"2 0 obj\n".len() as u64)
}

#[test]
fn the_elements_of_an_object_are_counted_through_its_nesting() {
    let (b, array) = nested_array();
    let file = open_with(&b.bytes, element_limit(6)).clean();
    assert!(load_noting(&file, id(2)).0.unwrap().is_some());

    let file = open_with(&b.bytes, element_limit(5)).clean();
    // The sixth element is the second item of the second inner array.
    let second_inner = array + b"[[1 2] ".len() as u64;
    assert_limit(
        load_noting(&file, id(2)).0,
        second_inner,
        Limit::ObjectElements { max: 5 },
    );
    // The count starts again with each object: the catalog has one.
    assert!(load_noting(&file, id(1)).0.unwrap().is_some());
}

#[test]
fn the_elements_of_a_trailer_and_of_a_compressed_object_are_counted() {
    // The trailer of `nested_array` has two entries.
    let (b, _) = nested_array();
    let trailer = b.bytes.windows(2).rposition(|w| w == b"<<").unwrap() as u64;
    assert_limit(
        open_with(&b.bytes, element_limit(1)).result,
        trailer,
        Limit::ObjectElements { max: 1 },
    );

    let mut b = Builder::new();
    let (data, first) =
        object_stream_data(&[(1, "<</Type /Catalog>>"), (2, "[1 2 3 4 5 6 7 8 9]")]);
    let (three, _) = b.stream(3, &format!("/Type /ObjStm /N 2 /First {first}"), &data);
    let xref = b.here();
    b.stream(
        4,
        "/Type /XRef /Size 5 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[
                [0, 0, 255],
                [2, 3, 0],
                [2, 3, 1],
                [1, three, 0],
                [1, xref, 0],
            ],
        ),
    );
    b.startxref(xref);
    // The cross-reference stream's dictionary has eight elements: five
    // entries and the three items of W.
    let file = open_with(&b.bytes, element_limit(8)).clean();
    assert!(load_noting(&file, id(1)).0.unwrap().is_some());
    // The offset is that of the array in the decoded data.
    let array = first as u64 + "<</Type /Catalog>> ".len() as u64;
    assert_limit(
        load_noting(&file, id(2)).0,
        array,
        Limit::ObjectElements { max: 8 },
    );
}
