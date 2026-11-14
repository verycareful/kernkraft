// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Cross-reference streams (7.5.8): the dictionary of Table 17 and the
//! entries of Table 18.

use kernkraft_pdf::file::{SectionForm, XrefEntry};
use kernkraft_pdf::syntax::{PdfString, Value};

use crate::{
    deflate, hex, id, integer, is_missing, keys, load, png_up, reference, value, xref_rows, Builder,
};

/// A file whose only cross-reference is a stream, object 3: the catalog
/// 1 and the string 2 before it.
struct Streamed {
    b: Builder,
    catalog: u64,
    string: u64,
    stream: u64,
}

/// Builds the file. `entries` gives the stream dictionary's entries and
/// the stream's data from the three offsets.
fn streamed(entries: impl Fn(u64, u64, u64) -> (String, Vec<u8>)) -> Streamed {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let string = b.object(2, "(two)");
    let stream = b.here();
    let (dictionary, data) = entries(catalog, string, stream);
    b.stream(3, &dictionary, &data);
    b.startxref(stream);
    Streamed {
        b,
        catalog,
        string,
        stream,
    }
}

/// The four entries of the file as `[type, field 2, field 3]`.
fn rows(catalog: u64, string: u64, stream: u64) -> [[u64; 3]; 4] {
    [
        [0, 0, 65535],
        [1, catalog, 0],
        [1, string, 0],
        [1, stream, 0],
    ]
}

fn in_use(offset: u64) -> Option<XrefEntry> {
    Some(XrefEntry::InUse {
        offset,
        generation: 0,
    })
}

fn assert_entries(s: &Streamed, file: &kernkraft_pdf::file::PdfFile) {
    assert_eq!(
        file.entry(0),
        Some(XrefEntry::Free {
            next: 0,
            generation: 65535
        })
    );
    assert_eq!(file.entry(1), in_use(s.catalog));
    assert_eq!(file.entry(2), in_use(s.string));
    assert_eq!(file.entry(3), in_use(s.stream));
    assert_eq!(file.entry(4), None);
    assert_eq!(value(&load(file, 2)), &Value::String(PdfString::new("two")));
}

#[test]
fn a_stream_with_no_filter_and_the_default_index() {
    let s = streamed(|c, t, x| {
        (
            "/Type /XRef /Size 4 /W [1 2 2] /Root 1 0 R".to_owned(),
            xref_rows([1, 2, 2], &rows(c, t, x)),
        )
    });
    let file = s.b.open().clean();
    assert!(!file.xref_rebuilt());
    assert_eq!(file.sections().len(), 1);
    let section = &file.sections()[0];
    assert_eq!(section.form(), SectionForm::Stream);
    assert_eq!(section.offset(), s.stream);
    assert_eq!(section.size(), 4);
    // The stream's dictionary is the trailer.
    assert_eq!(
        keys(file.trailer()),
        ["Type", "Size", "W", "Root", "Length"]
    );
    assert_eq!(reference(file.trailer(), "Root"), id(1));
    assert_entries(&s, &file);
}

#[test]
fn the_filters_a_writer_would_use() {
    type Encode = fn(&[u8]) -> Vec<u8>;
    let cases: [(&str, Encode); 3] = [
        ("/Filter /ASCIIHexDecode", |data| hex(data)),
        ("/Filter /FlateDecode", |data| deflate(data)),
        (
            "/Filter [/FlateDecode] /DecodeParms [<</Predictor 12 /Columns 5>>]",
            |data| deflate(&png_up(data, 5)),
        ),
    ];
    for (filter, encode) in cases {
        let s = streamed(|c, t, x| {
            (
                format!("/Type /XRef /Size 4 /W [1 2 2] /Root 1 0 R {filter}"),
                encode(&xref_rows([1, 2, 2], &rows(c, t, x))),
            )
        });
        let file = s.b.open().clean();
        assert!(!file.xref_rebuilt(), "{filter}");
        assert_entries(&s, &file);
    }
}

#[test]
fn an_absent_type_field_means_an_object_in_use() {
    // W [0 2 1]: no type field, so every entry is of type 1.
    let s = streamed(|c, t, x| {
        (
            "/Type /XRef /Size 4 /W [0 2 1] /Index [1 3] /Root 1 0 R".to_owned(),
            xref_rows([0, 2, 1], &[[0, c, 0], [0, t, 0], [0, x, 0]]),
        )
    });
    let file = s.b.open().clean();
    assert_eq!(file.entry(0), None);
    assert_eq!(file.entry(1), in_use(s.catalog));
    assert_eq!(file.entry(2), in_use(s.string));
    assert_eq!(file.entry(3), in_use(s.stream));
}

#[test]
fn an_absent_third_field_is_generation_0() {
    let s = streamed(|c, t, x| {
        (
            "/Type /XRef /Size 4 /W [1 2 0] /Root 1 0 R".to_owned(),
            xref_rows([1, 2, 0], &[[0, 0, 0], [1, c, 0], [1, t, 0], [1, x, 0]]),
        )
    });
    let file = s.b.open().clean();
    assert_eq!(
        file.entry(0),
        Some(XrefEntry::Free {
            next: 0,
            generation: 0
        })
    );
    assert_eq!(file.entry(1), in_use(s.catalog));
    assert_eq!(file.entry(3), in_use(s.stream));
}

#[test]
fn fields_of_every_width_up_to_eight_bytes() {
    for width in 1..=8usize {
        // The widest value the offset field can hold that is still an
        // offset of this small file is the offset itself; the field is
        // padded with zeros on the left.
        let s = streamed(|c, t, x| {
            (
                format!("/Type /XRef /Size 4 /W [{width} {width} {width}] /Root 1 0 R"),
                xref_rows(
                    [width, width, width],
                    &[[0, 0, 0], [1, c, 0], [1, t, 0], [1, x, 0]],
                ),
            )
        });
        // Offsets of this file fit one byte only if the file is small
        // enough; the builder asserts a field fits its width.
        let file = s.b.open().clean();
        assert_eq!(file.entry(2), in_use(s.string), "width {width}");
    }
}

#[test]
fn elements_of_w_past_the_third_widen_the_entry_and_are_not_read() {
    let s = streamed(|c, t, x| {
        let mut data = Vec::new();
        for row in rows(c, t, x) {
            data.extend_from_slice(&xref_rows([1, 2, 2], &[row]));
            // The fourth field, two bytes that mean nothing here.
            data.extend_from_slice(&[0xAB, 0xCD]);
        }
        (
            "/Type /XRef /Size 4 /W [1 2 2 2] /Root 1 0 R".to_owned(),
            data,
        )
    });
    let file = s.b.open().clean();
    assert_entries(&s, &file);
}

#[test]
fn an_index_of_several_subsections() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let ten = b.object(10, "(ten)");
    let eleven = b.object(11, "(eleven)");
    let stream = b.here();
    let data = xref_rows(
        [1, 2, 1],
        &[
            [1, ten, 0],
            [1, eleven, 0],
            [1, stream, 0],
            [0, 0, 255],
            [1, catalog, 0],
        ],
    );
    b.stream(
        20,
        "/Type /XRef /Size 21 /W [1 2 1] /Index [10 2 20 1 0 2] /Root 1 0 R",
        &data,
    );
    b.startxref(stream);
    let file = b.open().clean();
    assert_eq!(file.entry(10), in_use(ten));
    assert_eq!(file.entry(11), in_use(eleven));
    assert_eq!(file.entry(20), in_use(stream));
    assert_eq!(file.entry(1), in_use(catalog));
    assert_eq!(
        file.entry(0),
        Some(XrefEntry::Free {
            next: 0,
            generation: 255
        })
    );
    for number in [2, 9, 12, 19] {
        assert_eq!(file.entry(number), None);
    }
    let numbers: Vec<u32> = file.sections()[0].entries().map(|(n, _)| n).collect();
    assert_eq!(numbers, [0, 1, 10, 11, 20]);
}

#[test]
fn compressed_entries_and_entries_of_an_undefined_type() {
    let s = streamed(|c, _, x| {
        (
            "/Type /XRef /Size 6 /W [1 2 2] /Root 1 0 R".to_owned(),
            xref_rows(
                [1, 2, 2],
                &[
                    [0, 0, 65535],
                    [1, c, 0],
                    [2, 9, 300],
                    [1, x, 0],
                    [3, 7, 7],
                    [255, 0, 0],
                ],
            ),
        )
    });
    let file = s.b.open().clean();
    assert_eq!(
        file.entry(2),
        Some(XrefEntry::Compressed {
            stream: 9,
            index: 300
        })
    );
    // 7.5.8.3: any other type is a reference to the null object.
    assert_eq!(file.entry(4), Some(XrefEntry::Null));
    assert_eq!(file.entry(5), Some(XrefEntry::Null));
    assert!(is_missing(&file, id(4)));
}

#[test]
fn a_chain_of_streams_follows_prev() {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let two = b.object(2, "(two)");
    let first = b.here();
    b.stream(
        3,
        "/Type /XRef /Size 4 /W [1 2 1] /Root 1 0 R",
        &xref_rows(
            [1, 2, 1],
            &[[0, 0, 255], [1, catalog, 0], [1, two, 0], [1, first, 0]],
        ),
    );
    b.startxref(first);
    let two_again = b.object(2, "(deux)");
    let second = b.here();
    b.stream(
        4,
        &format!("/Type /XRef /Size 5 /W [1 2 1] /Index [2 1 4 1] /Root 1 0 R /Prev {first}"),
        &xref_rows([1, 2, 1], &[[1, two_again, 0], [1, second, 0]]),
    );
    b.startxref(second);
    let file = b.open().clean();
    let offsets: Vec<u64> = file.sections().iter().map(|s| s.offset()).collect();
    assert_eq!(offsets, [second, first]);
    assert_eq!(integer(file.trailer(), "Prev"), first as i64);
    assert_eq!(file.entry(2), in_use(two_again));
    assert_eq!(file.entry_as_of(1, 2), in_use(two));
    assert_eq!(
        value(&load(&file, 2)),
        &Value::String(PdfString::new("deux"))
    );
}

/// Each of these streams cannot be read as 7.5.8 says, so the file is
/// opened by rebuilding, the fault placed at the stream's object.
#[test]
fn a_stream_that_cannot_be_read_gives_way_to_a_rebuilt_table() {
    type Data = fn(&[u8]) -> Vec<u8>;
    let whole: Data = |data| data.to_vec();
    let cases: [(&str, &str, Data); 16] = [
        ("no Type", "/Size 4 /W [1 2 2] /Root 1 0 R", whole),
        (
            "another Type",
            "/Type /Other /Size 4 /W [1 2 2] /Root 1 0 R",
            whole,
        ),
        ("no Size", "/Type /XRef /W [1 2 2] /Root 1 0 R", whole),
        ("no W", "/Type /XRef /Size 4 /Root 1 0 R", whole),
        (
            "a W of two elements",
            "/Type /XRef /Size 4 /W [1 4] /Root 1 0 R",
            whole,
        ),
        (
            "a W that is no array",
            "/Type /XRef /Size 4 /W 5 /Root 1 0 R",
            whole,
        ),
        (
            "a negative width",
            "/Type /XRef /Size 4 /W [1 -2 2] /Root 1 0 R",
            whole,
        ),
        (
            "a width that is no integer",
            "/Type /XRef /Size 4 /W [1 2.0 2] /Root 1 0 R",
            whole,
        ),
        (
            "a field wider than eight bytes",
            "/Type /XRef /Size 4 /W [1 9 2] /Root 1 0 R",
            whole,
        ),
        (
            "entries of no bytes",
            "/Type /XRef /Size 4 /W [0 0 0] /Root 1 0 R",
            whole,
        ),
        (
            "an Index of odd length",
            "/Type /XRef /Size 4 /W [1 2 2] /Index [0 4 9] /Root 1 0 R",
            whole,
        ),
        (
            "an Index past the last object number",
            "/Type /XRef /Size 4 /W [1 2 2] /Index [4294967293 4] /Root 1 0 R",
            whole,
        ),
        (
            "two subsections with one object number",
            "/Type /XRef /Size 4 /W [1 2 2] /Index [0 2 1 2] /Root 1 0 R",
            whole,
        ),
        (
            "data one byte short",
            "/Type /XRef /Size 4 /W [1 2 2] /Root 1 0 R",
            |data| data[..data.len() - 1].to_vec(),
        ),
        (
            "data one byte long",
            "/Type /XRef /Size 4 /W [1 2 2] /Root 1 0 R",
            |data| [data, &[0]].concat(),
        ),
        (
            "a filter that is not known",
            "/Type /XRef /Size 4 /W [1 2 2] /Root 1 0 R /Filter /Crypt",
            whole,
        ),
    ];
    for (name, dictionary, data) in cases {
        let s = streamed(|c, t, x| {
            (
                dictionary.to_owned(),
                data(&xref_rows([1, 2, 2], &rows(c, t, x))),
            )
        });
        let file = s.b.open().rebuilt(s.stream);
        assert_eq!(file.entry(1), in_use(s.catalog), "{name}");
        assert_eq!(file.entry(2), in_use(s.string), "{name}");
        assert_eq!(reference(file.trailer(), "Root"), id(1), "{name}");
    }
}

#[test]
fn an_entry_whose_field_has_no_room_gives_way_to_a_rebuilt_table() {
    // A generation past 65535, a next free object past a u32, an object
    // stream number past a u32 and an index past a u32.
    let wide = [1usize, 8, 8];
    let past_u32 = u64::from(u32::MAX) + 1;
    for bad in [
        [1, 0, 65536],
        [0, past_u32, 0],
        [2, past_u32, 0],
        [2, 9, past_u32],
    ] {
        let s = streamed(|c, t, x| {
            (
                "/Type /XRef /Size 5 /W [1 8 8] /Root 1 0 R".to_owned(),
                xref_rows(wide, &[[0, 0, 65535], [1, c, 0], [1, t, 0], [1, x, 0], bad]),
            )
        });
        let file = s.b.open().rebuilt(s.stream);
        assert_eq!(file.entry(2), in_use(s.string), "{bad:?}");
    }
}

#[test]
fn damaged_stream_data_gives_way_to_a_rebuilt_table() {
    // Flate data cut short: what was decoded is not all of the entries.
    let s = streamed(|c, t, x| {
        let encoded = deflate(&xref_rows([1, 2, 2], &rows(c, t, x)));
        (
            "/Type /XRef /Size 4 /W [1 2 2] /Root 1 0 R /Filter /FlateDecode".to_owned(),
            encoded[..encoded.len() / 2].to_vec(),
        )
    });
    let file = s.b.open().rebuilt(s.stream);
    assert_eq!(file.entry(2), in_use(s.string));
}

#[test]
fn a_length_that_is_a_reference_cannot_be_followed_and_the_data_is_found() {
    // No cross-reference is known while the stream is read, so the end
    // of its data is found by `endstream`.
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let stream = b.here();
    let data = xref_rows([1, 2, 1], &[[0, 0, 255], [1, catalog, 0], [1, stream, 0]]);
    let (_, data_start) = b.stream_with(
        2,
        "/Type /XRef /Size 3 /W [1 2 1] /Root 1 0 R /Length 9 0 R",
        &data,
    );
    b.startxref(stream);
    let opened = b.open();
    let noted: Vec<_> = opened
        .diagnostics
        .iter()
        .map(|d| (d.kind.clone(), d.object, d.offset))
        .collect();
    assert_eq!(
        noted,
        [(
            kernkraft_pdf::DiagnosticKind::StreamLength,
            Some(id(2)),
            data_start
        )]
    );
    let file = opened.result.unwrap();
    assert!(!file.xref_rebuilt());
    assert_eq!(file.entry(1), in_use(catalog));
}
