// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Cross-reference tables (7.5.4), the trailer and `startxref` (7.5.5).

use kernkraft_pdf::file::{SectionForm, XrefEntry, TRAILER_SEARCH_BYTES};
use kernkraft_pdf::DiagnosticKind;

use crate::{
    id, integer, keys, load, located, open, reference, small_file, Builder, Row, ENTRY_EOLS, HEAD,
};

#[test]
fn a_file_of_one_section_gives_each_entry_as_written() {
    let (b, [catalog, pages, string]) = small_file();
    let file = b.open().clean();
    assert!(!file.xref_rebuilt());
    assert_eq!(file.sections().len(), 1);
    let section = &file.sections()[0];
    assert_eq!(section.form(), SectionForm::Table);
    assert_eq!(section.size(), 4);
    assert_eq!(section.hybrid_offset(), None);
    assert_eq!(keys(file.trailer()), ["Size", "Root"]);
    assert_eq!(integer(file.trailer(), "Size"), 4);
    assert_eq!(reference(file.trailer(), "Root"), id(1));

    let expected = [
        HEAD.entry(),
        Row::N(catalog, 0).entry(),
        Row::N(pages, 0).entry(),
        Row::N(string, 0).entry(),
    ];
    for (number, entry) in (0..).zip(expected) {
        assert_eq!(file.entry(number), Some(entry));
    }
    assert_eq!(file.entry(4), None);
    assert_eq!(
        section.entries().collect::<Vec<_>>(),
        (0..).zip(expected).collect::<Vec<_>>()
    );
    // The section starts at the `xref` keyword.
    let offset = section.offset() as usize;
    assert!(b.bytes[offset..].starts_with(b"xref\n"));
}

#[test]
fn each_end_of_line_of_7_5_4_is_read_without_remark() {
    for eol in ENTRY_EOLS {
        let mut b = Builder::new();
        let catalog = b.object(1, "<</Type /Catalog>>");
        let xref = b.table_with(
            &[(0, &[HEAD, Row::N(catalog, 0)])],
            "/Size 2 /Root 1 0 R",
            eol,
        );
        b.startxref(xref);
        let file = b.open().clean();
        assert_eq!(file.entry(1), Some(Row::N(catalog, 0).entry()), "{eol:?}");
    }
}

/// The table of 7.5.4, Example 3: four subsections holding five entries.
#[test]
fn subsections_in_any_order_are_found_by_object_number() {
    let mut b = Builder::new();
    let o3 = b.object(3, "<</Type /Catalog>>");
    let o23 = b.object_gen(23, 2, "(twenty-three)");
    let o24 = b.object(24, "(twenty-four)");
    let o30 = b.object(30, "(thirty)");
    let xref = b.table(
        &[
            (30, &[Row::N(o30, 0)]),
            (0, &[HEAD]),
            (23, &[Row::N(o23, 2), Row::N(o24, 0)]),
            (3, &[Row::N(o3, 0)]),
        ],
        "/Size 31 /Root 3 0 R",
    );
    b.startxref(xref);
    let file = b.open().clean();
    let in_use = [(3, o3, 0), (23, o23, 2), (24, o24, 0), (30, o30, 0)];
    for (number, offset, generation) in in_use {
        assert_eq!(file.entry(number), Some(Row::N(offset, generation).entry()));
    }
    for number in [1, 2, 4, 22, 25, 29, 31] {
        assert_eq!(file.entry(number), None, "object {number}");
    }
    let numbers: Vec<u32> = file.sections()[0].entries().map(|(n, _)| n).collect();
    assert_eq!(numbers, [0, 3, 23, 24, 30]);
}

#[test]
fn an_entry_at_or_past_size_is_kept_and_reported() {
    // A Size one too low and the catalog itself past it, as writers
    // leave them.
    let mut b = Builder::new();
    let first = b.object(1, "(first)");
    let catalog = b.object(2, "<</Type /Catalog>>");
    let xref = b.table(
        &[(0, &[HEAD, Row::N(first, 0), Row::N(catalog, 0)])],
        "/Size 2 /Root 2 0 R",
    );
    b.startxref(xref);
    let (file, diagnostics) = b.open().with_diagnostics();
    assert_eq!(diagnostics, [(DiagnosticKind::EntryPastSize, xref)]);
    assert!(!file.xref_rebuilt());
    assert_eq!(file.sections()[0].size(), 2);
    assert_eq!(file.entry(2), Some(Row::N(catalog, 0).entry()));
    assert_eq!(
        located(&b.bytes, &load(&file, 2)),
        b"2 0 obj\n<</Type /Catalog>>\nendobj"
    );

    // A Size that is exact, or too high, is not remarked on.
    for size in [3, 4, 1000] {
        let mut b = Builder::new();
        let first = b.object(1, "(first)");
        let catalog = b.object(2, "<</Type /Catalog>>");
        let xref = b.table(
            &[(0, &[HEAD, Row::N(first, 0), Row::N(catalog, 0)])],
            &format!("/Size {size} /Root 2 0 R"),
        );
        b.startxref(xref);
        b.open().clean();
    }
}

#[test]
fn entries_not_laid_out_in_twenty_bytes_are_read_and_reported_once() {
    type RowText = fn(u64) -> String;
    // (the end-of-line of the first entry, the text of the others)
    let cases: [(&str, RowText); 3] = [
        // A first entry of 19 bytes puts the second out of place.
        ("\n", |offset| format!("{offset:010} 00000 n \n")),
        // An offset of nine digits.
        (" \n", |offset| format!("{offset:09} 00000 n \n")),
        // Two spaces before the keyword.
        (" \n", |offset| format!("{offset:010} 00000  n\n")),
    ];
    for (case, (head_eol, row)) in cases.into_iter().enumerate() {
        let mut b = Builder::new();
        let catalog = b.object(1, "<</Type /Catalog>>");
        let other = b.object(2, "(other)");
        let xref = b.text("xref\n0 3\n");
        b.text(&HEAD.text(head_eol));
        let second_entry = b.text(&row(catalog));
        b.text(&row(other));
        b.text("trailer\n<</Size 3 /Root 1 0 R>>\n");
        b.startxref(xref);
        let (file, diagnostics) = b.open().with_diagnostics();
        // The first entry is well formed in itself, so the second is the
        // first out of place or out of shape; the third is not reported.
        assert_eq!(
            diagnostics,
            [(DiagnosticKind::XrefEntryLayout, second_entry)],
            "case {case}"
        );
        assert!(!file.xref_rebuilt());
        assert_eq!(file.entry(0), Some(HEAD.entry()));
        assert_eq!(file.entry(1), Some(Row::N(catalog, 0).entry()));
        assert_eq!(file.entry(2), Some(Row::N(other, 0).entry()));
    }
}

#[test]
fn white_space_at_the_startxref_offset_is_reported() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let before = b.text("\n\n");
    b.table(&[(0, &[HEAD, Row::N(catalog, 0)])], "/Size 2 /Root 1 0 R");
    b.startxref(before);
    let (file, diagnostics) = b.open().with_diagnostics();
    assert_eq!(diagnostics, [(DiagnosticKind::WhitespaceAtOffset, before)]);
    assert_eq!(file.sections()[0].offset(), before);
    assert_eq!(file.entry(1), Some(Row::N(catalog, 0).entry()));
}

#[test]
fn a_missing_end_of_file_marker_is_reported() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let xref = b.table(&[(0, &[HEAD, Row::N(catalog, 0)])], "/Size 2 /Root 1 0 R");
    b.text(&format!("startxref\n{xref}"));
    let after_offset = b.here();
    for tail in ["", "\n", "\n%%EO", "\n%EOF\n"] {
        let mut bytes = b.bytes.clone();
        bytes.extend_from_slice(tail.as_bytes());
        let (file, diagnostics) = open(&bytes).with_diagnostics();
        assert_eq!(
            diagnostics,
            [(DiagnosticKind::MissingEofMarker, after_offset)],
            "{tail:?}"
        );
        assert_eq!(file.entry(1), Some(Row::N(catalog, 0).entry()));
    }
    // The marker with no end-of-line after it is whole.
    let mut bytes = b.bytes.clone();
    bytes.extend_from_slice(b"\n%%EOF");
    open(&bytes).clean();
}

#[test]
fn startxref_is_found_within_the_last_bytes_searched() {
    let (b, _) = small_file();
    let position = b
        .bytes
        .windows(b"startxref".len())
        .rposition(|w| w == b"startxref")
        .unwrap();
    let after_keyword = b.bytes.len() - position;
    // Comment lines after the marker push the keyword away from the end.
    for (padding, found) in [
        (TRAILER_SEARCH_BYTES - after_keyword, true),
        (TRAILER_SEARCH_BYTES - after_keyword + 1, false),
    ] {
        let mut bytes = b.bytes.clone();
        bytes.extend(std::iter::repeat_n(b'\n', padding));
        let opened = open(&bytes);
        if found {
            assert!(!opened.clean().xref_rebuilt());
        } else {
            // No `startxref` where it is looked for: the fault is placed
            // at the end of the file.
            let file = opened.rebuilt(bytes.len() as u64);
            assert_eq!(reference(file.trailer(), "Root"), id(1));
        }
    }
}

#[test]
fn the_last_startxref_is_the_one_used() {
    let (mut b, [catalog, ..]) = small_file();
    // A second, smaller table after the first; only its entries count.
    let xref = b.table(&[(0, &[HEAD, Row::N(catalog, 0)])], "/Size 2 /Root 1 0 R");
    b.startxref(xref);
    let file = b.open().clean();
    assert_eq!(file.sections().len(), 1);
    assert_eq!(file.sections()[0].offset(), xref);
    assert_eq!(file.entry(3), None);
}

/// Each of these tables cannot be read as 7.5.4 and 7.5.5 say, so the
/// file is opened by rebuilding. The fault is placed where the table
/// goes wrong.
#[test]
fn a_table_that_cannot_be_read_gives_way_to_a_rebuilt_one() {
    struct Case {
        name: &'static str,
        /// The table and trailer, with `{c}` for the catalog's offset.
        table: &'static str,
        /// What in `table` the fault is placed at, or `None` for the
        /// start of the table.
        fault_at: Option<&'static str>,
    }
    let cases = [
        Case {
            name: "generation past 65535",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 65536 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("65536"),
        },
        Case {
            name: "next free object past a u32",
            table: "xref\n0 2\n9999999999 65535 f \n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("9999999999"),
        },
        Case {
            name: "a keyword that is neither n nor f",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 x \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("x \n"),
        },
        Case {
            name: "fewer entries than the subsection declares",
            table: "xref\n0 3\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("trailer"),
        },
        Case {
            name: "more entries than the subsection declares",
            table: "xref\n0 1\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("n \n"),
        },
        Case {
            name: "two subsections with one object number",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \n1 1\n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: None,
        },
        Case {
            name: "a subsection that runs past the last object number",
            table: "xref\n4294967295 2\n{c} 00000 n \n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("4294967295"),
        },
        Case {
            name: "a negative first object number",
            table: "xref\n-1 1\n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("-1"),
        },
        Case {
            name: "no trailer keyword",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \n<</Size 2 /Root 1 0 R>>\n",
            fault_at: Some("<</Size"),
        },
        Case {
            name: "a trailer that is not a dictionary",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n[/Size 2]\n",
            fault_at: Some("[/Size"),
        },
        Case {
            name: "no Size",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Root 1 0 R>>\n",
            fault_at: None,
        },
        Case {
            name: "a Size that is not an integer",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2.0 /Root 1 0 R>>\n",
            fault_at: None,
        },
        Case {
            name: "a negative Size",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size -2 /Root 1 0 R>>\n",
            fault_at: None,
        },
        Case {
            name: "a Prev that is not an integer",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R /Prev (9)>>\n",
            fault_at: None,
        },
        Case {
            name: "an XRefStm that is not an integer",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2 /Root 1 0 R /XRefStm /No>>\n",
            fault_at: None,
        },
        Case {
            name: "no Root",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2>>\n",
            fault_at: None,
        },
        Case {
            name: "a Root that is not a reference",
            table: "xref\n0 2\n0000000000 65535 f \n{c} 00000 n \ntrailer\n<</Size 2 /Root <<>> >>\n",
            fault_at: None,
        },
    ];
    for case in cases {
        let mut b = Builder::new();
        let catalog = b.object(1, "<</Type /Catalog>>");
        let table = case.table.replace("{c}", &format!("{catalog:010}"));
        let xref = b.text(&table);
        b.startxref(xref);
        let fault = xref + case.fault_at.map_or(0, |at| table.find(at).unwrap() as u64);
        let file = b.open().rebuilt(fault);
        assert_eq!(
            file.entry(1),
            Some(XrefEntry::InUse {
                offset: catalog,
                generation: 0
            }),
            "{}",
            case.name
        );
        assert_eq!(
            located(&b.bytes, &load(&file, 1)),
            b"1 0 obj\n<</Type /Catalog>>\nendobj",
            "{}",
            case.name
        );
    }
}

#[test]
fn a_startxref_that_names_no_section_gives_way_to_a_rebuilt_table() {
    let (b, _) = small_file();
    let end = b.bytes.len() as u64;
    let position = b
        .bytes
        .windows(b"startxref".len())
        .rposition(|w| w == b"startxref")
        .unwrap();
    let body = &b.bytes[..position];
    // (what follows the body, where the fault is placed)
    let cases: [(String, u64); 5] = [
        // The offset of an object, which is no cross-reference stream:
        // the object's own offset.
        (
            format!("startxref\n{}\n%%EOF\n", crate::HEADER.len()),
            crate::HEADER.len() as u64,
        ),
        // An offset inside an object's dictionary.
        (
            format!("startxref\n{}\n%%EOF\n", crate::HEADER.len() + 10),
            crate::HEADER.len() as u64 + 10,
        ),
        // An offset past the end of the file.
        (format!("startxref\n{}\n%%EOF\n", end + 100), end + 100),
        // No number after the keyword: the keyword's offset.
        ("startxref\n%%EOF\n".to_owned(), position as u64),
        ("startxref\n-5\n%%EOF\n".to_owned(), position as u64),
    ];
    for (tail, fault) in cases {
        let mut bytes = body.to_vec();
        bytes.extend_from_slice(tail.as_bytes());
        let file = open(&bytes).rebuilt(fault);
        assert_eq!(reference(file.trailer(), "Root"), id(1), "{tail:?}");
    }
}
