// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The chain of sections that incremental updates leave (7.5.6) and the
//! `XRefStm` of a hybrid-reference file (7.5.8.4).

use kernkraft_pdf::file::{PdfFile, SectionForm, XrefEntry};
use kernkraft_pdf::syntax::{PdfString, Value};
use kernkraft_pdf::Diagnostics;

use crate::{id, integer, is_missing, load, small_file, value, xref_rows, Builder, Row, HEAD};

/// [`small_file`] updated twice. The first update replaces the string 3
/// and adds the string 4; the second frees 3 and replaces 4.
struct Updated {
    b: Builder,
    /// The offset of each section, the newest first.
    sections: [u64; 3],
    original: [u64; 3],
    first_update: [u64; 2],
    second_update: u64,
}

fn updated() -> Updated {
    let (mut b, original) = small_file();
    // The table's keyword starts a line; the one inside `startxref` does
    // not.
    let base_xref = b.bytes.windows(6).rposition(|w| w == b"\nxref\n").unwrap() as u64 + 1;

    let three = b.object(3, "(Slithy)");
    let four = b.object(4, "(Toves)");
    let first_xref = b.table(
        &[(3, &[Row::N(three, 0), Row::N(four, 0)])],
        &format!("/Size 5 /Root 1 0 R /Prev {base_xref}"),
    );
    b.startxref(first_xref);

    let four_again = b.object(4, "(Gyre)");
    let second_xref = b.table(
        &[
            (0, &[Row::F(3, 65535)]),
            (3, &[Row::F(0, 1), Row::N(four_again, 0)]),
        ],
        &format!("/Size 5 /Root 1 0 R /Prev {first_xref}"),
    );
    b.startxref(second_xref);
    Updated {
        b,
        sections: [second_xref, first_xref, base_xref],
        original,
        first_update: [three, four],
        second_update: four_again,
    }
}

fn string_as_of(file: &PdfFile, section: usize, number: u32) -> Option<Vec<u8>> {
    let mut diagnostics = Diagnostics::new();
    let object = file
        .load_as_of(section, id(number), &mut diagnostics)
        .unwrap()?;
    assert!(diagnostics.is_empty());
    match value(&object) {
        Value::String(string) => Some(string.as_bytes().to_vec()),
        other => panic!("not a string: {other:?}"),
    }
}

#[test]
fn sections_are_listed_newest_first() {
    let u = updated();
    let file = u.b.open().clean();
    let offsets: Vec<u64> = file.sections().iter().map(|s| s.offset()).collect();
    assert_eq!(offsets, u.sections);
    assert!(file
        .sections()
        .iter()
        .all(|s| s.form() == SectionForm::Table));
    let sizes: Vec<u64> = file.sections().iter().map(|s| s.size()).collect();
    assert_eq!(sizes, [5, 5, 4]);
    assert_eq!(integer(file.trailer(), "Prev"), u.sections[1] as i64);
}

#[test]
fn the_newest_entry_of_an_object_is_the_one_found() {
    let u = updated();
    let file = u.b.open().clean();
    assert_eq!(file.entry(0), Some(Row::F(3, 65535).entry()));
    assert_eq!(file.entry(1), Some(Row::N(u.original[0], 0).entry()));
    assert_eq!(file.entry(2), Some(Row::N(u.original[1], 0).entry()));
    assert_eq!(file.entry(3), Some(Row::F(0, 1).entry()));
    assert_eq!(file.entry(4), Some(Row::N(u.second_update, 0).entry()));
    assert_eq!(file.entry(5), None);

    assert!(is_missing(&file, id(3)));
    assert_eq!(string_as_of(&file, 0, 4).unwrap(), b"Gyre");
    assert_eq!(
        value(&load(&file, 4)),
        &Value::String(PdfString::new("Gyre"))
    );
}

#[test]
fn an_earlier_section_shows_the_file_as_it_was() {
    let u = updated();
    let file = u.b.open().clean();
    // After the first update.
    assert_eq!(
        file.entry_as_of(1, 3),
        Some(Row::N(u.first_update[0], 0).entry())
    );
    assert_eq!(
        file.entry_as_of(1, 4),
        Some(Row::N(u.first_update[1], 0).entry())
    );
    assert_eq!(file.entry_as_of(1, 0), Some(HEAD.entry()));
    assert_eq!(string_as_of(&file, 1, 3).unwrap(), b"Slithy");
    assert_eq!(string_as_of(&file, 1, 4).unwrap(), b"Toves");
    // As first written: no section has object 4 yet.
    assert_eq!(
        file.entry_as_of(2, 3),
        Some(Row::N(u.original[2], 0).entry())
    );
    assert_eq!(file.entry_as_of(2, 4), None);
    assert_eq!(string_as_of(&file, 2, 3).unwrap(), b"Brillig");
    assert_eq!(string_as_of(&file, 2, 4), None);
    // A section the file does not have.
    assert_eq!(file.entry_as_of(3, 1), None);
    assert_eq!(string_as_of(&file, 3, 3), None);
}

#[test]
fn a_chain_that_runs_in_a_circle_gives_way_to_a_rebuilt_table() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    // The offset of the second table is known before it is written: the
    // first table's text has the same length whatever ten digits it holds.
    let first_len = {
        let mut probe = Builder::new();
        probe.table(
            &[(0, &[HEAD, Row::N(catalog, 0)])],
            &format!("/Size 2 /Root 1 0 R /Prev {:010}", 0),
        );
        probe.bytes.len() as u64 - crate::HEADER.len() as u64
    };
    let first = b.here();
    let second = first + first_len;
    b.table(
        &[(0, &[HEAD, Row::N(catalog, 0)])],
        &format!("/Size 2 /Root 1 0 R /Prev {second:010}"),
    );
    assert_eq!(b.here(), second);
    b.table(
        &[(1, &[Row::N(catalog, 0)])],
        &format!("/Size 2 /Root 1 0 R /Prev {first}"),
    );
    b.startxref(second);
    // The second section is read, then the first, whose Prev leads back
    // to the second: that is where the fault is placed.
    let file = b.open().rebuilt(second);
    assert_eq!(
        file.entry(1),
        Some(XrefEntry::InUse {
            offset: catalog,
            generation: 0
        })
    );
}

#[test]
fn a_prev_that_leads_nowhere_gives_way_to_a_rebuilt_table() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let xref = b.table(
        &[(0, &[HEAD, Row::N(catalog, 0)])],
        &format!("/Size 2 /Root 1 0 R /Prev {catalog}"),
    );
    b.startxref(xref);
    // The catalog is an object but no cross-reference stream.
    b.open().rebuilt(catalog);
}

/// A hybrid-reference file in the shape of 7.5.8.4, Example 2: a main
/// table, then an update whose trailer names a cross-reference stream.
struct Hybrid {
    b: Builder,
    catalog: u64,
    hidden: u64,
    shown: u64,
    stream: u64,
    update: u64,
}

fn hybrid() -> Hybrid {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    // Object 2 is free in the main table and present in the stream.
    let hidden = b.object(2, "(hidden)");
    // Object 3 has an entry in the update's table and another, to the
    // hidden object's offset, in the stream.
    let shown = b.object(3, "(shown)");
    let main = b.table(
        &[(0, &[Row::F(2, 65535), Row::N(catalog, 0), Row::F(0, 65535)])],
        "/Size 5 /Root 1 0 R",
    );
    let stream = b.here();
    let data = xref_rows([1, 2, 1], &[[1, hidden, 0], [1, hidden, 0], [1, stream, 0]]);
    b.stream(
        4,
        "/Type /XRef /Size 5 /W [1 2 1] /Index [2 3] /Prev 0",
        &data,
    );
    let update = b.table(
        &[(3, &[Row::N(shown, 0)])],
        &format!("/Size 5 /Root 1 0 R /Prev {main} /XRefStm {stream}"),
    );
    b.startxref(update);
    Hybrid {
        b,
        catalog,
        hidden,
        shown,
        stream,
        update,
    }
}

#[test]
fn a_hybrid_file_shows_the_objects_of_its_cross_reference_stream() {
    let h = hybrid();
    let file = h.b.open().clean();
    assert_eq!(file.sections().len(), 2);
    let update = &file.sections()[0];
    assert_eq!(update.offset(), h.update);
    assert_eq!(update.form(), SectionForm::Table);
    assert_eq!(update.hybrid_offset(), Some(h.stream));
    assert_eq!(file.sections()[1].hybrid_offset(), None);

    // Found in the stream before the free entry of the main table.
    assert_eq!(file.entry(2), Some(Row::N(h.hidden, 0).entry()));
    assert_eq!(
        value(&load(&file, 2)),
        &Value::String(PdfString::new("hidden"))
    );
    // The update's own table is searched before its stream.
    assert_eq!(file.entry(3), Some(Row::N(h.shown, 0).entry()));
    assert_eq!(
        value(&load(&file, 3)),
        &Value::String(PdfString::new("shown"))
    );
    assert_eq!(file.entry(4), Some(Row::N(h.stream, 0).entry()));
    assert_eq!(file.entry(1), Some(Row::N(h.catalog, 0).entry()));

    // Before the update, the hidden object is free.
    assert_eq!(file.entry_as_of(1, 2), Some(Row::F(0, 65535).entry()));
    // The section lists its table's entries, then its stream's.
    let numbers: Vec<u32> = update.entries().map(|(n, _)| n).collect();
    assert_eq!(numbers, [3, 2, 3, 4]);
}

#[test]
fn the_prev_of_a_hybrid_stream_is_not_followed() {
    // The stream of `hybrid` has a Prev of 0, where no section is; the
    // file opens clean, so it was not followed.
    let h = hybrid();
    assert_eq!(h.b.open().clean().sections().len(), 2);
}

#[test]
fn an_xrefstm_that_names_no_stream_gives_way_to_a_rebuilt_table() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let xref = b.table(
        &[(0, &[HEAD, Row::N(catalog, 0)])],
        &format!("/Size 2 /Root 1 0 R /XRefStm {catalog}"),
    );
    b.startxref(xref);
    b.open().rebuilt(catalog);
}
