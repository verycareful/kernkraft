// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of whole files: every truncation, every
//! substitution of one byte by a byte that means something to the reader,
//! and splices of one file's start onto another's end. Each mutated file
//! must open or be refused with a typed error, and each of its objects
//! must load or fail with one; a panic or a hang fails the test.

use kernkraft_pdf::file::{Body, PdfFile, XrefEntry};
use kernkraft_pdf::{Diagnostics, PdfError};

use crate::{deflate, object_stream_data, open, png_up, xref_rows, Builder, Row, HEAD};

/// Bytes that change how the reader takes what follows: delimiters, white
/// space of each kind, digits at each end of the range, the letters of
/// the keywords an entry and a reference end in, and the extreme values.
const REPLACEMENTS: &[u8] = b"()<>[]/% \n\r\x0009Rnf\xFF";

/// A file with a table, a stream whose length is an indirect reference,
/// and an incremental update that frees one object and replaces another.
fn table_fixture() -> Vec<u8> {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog /Pages 2 0 R>>");
    let pages = b.object(2, "<</Type /Pages /Kids [] /Count 0>>");
    let (three, _) = b.stream_with(3, "/Length 4 0 R", b"stream data\nof two lines");
    let four = b.object(4, "24");
    let five = b.object(5, "(freed by the update)");
    let first = b.table(
        &[(
            0,
            &[
                HEAD,
                Row::N(catalog, 0),
                Row::N(pages, 0),
                Row::N(three, 0),
                Row::N(four, 0),
                Row::N(five, 0),
            ],
        )],
        "/Size 6 /Root 1 0 R /ID [<00> <01>]",
    );
    b.startxref(first);
    let pages_again = b.object(2, "<</Type /Pages /Kids [] /Count 0 /New true>>");
    let second = b.table(
        &[
            (0, &[Row::F(5, 65535)]),
            (2, &[Row::N(pages_again, 0)]),
            (5, &[Row::F(0, 1)]),
        ],
        &format!("/Size 6 /Root 1 0 R /Prev {first}"),
    );
    b.startxref(second);
    b.bytes
}

/// A file with an object stream and a cross-reference stream, both Flate
/// encoded, the latter through a predictor.
fn stream_fixture() -> Vec<u8> {
    let mut b = Builder::new();
    let (data, first) = object_stream_data(&[
        (1, "<</Type /Catalog /Pages 2 0 R>>"),
        (2, "<</Type /Pages /Kids [] /Count 0>>"),
    ]);
    let (three, _) = b.stream(
        3,
        &format!("/Type /ObjStm /N 2 /First {first} /Filter /FlateDecode"),
        &deflate(&data),
    );
    let xref = b.here();
    let rows = xref_rows(
        [1, 2, 1],
        &[
            [0, 0, 255],
            [2, 3, 0],
            [2, 3, 1],
            [1, three, 0],
            [1, xref, 0],
        ],
    );
    b.stream(
        4,
        "/Type /XRef /Size 5 /W [1 2 1] /Root 1 0 R /Filter /FlateDecode /DecodeParms <</Predictor 12 /Columns 4>>",
        &deflate(&png_up(&rows, 4)),
    );
    b.startxref(xref);
    b.bytes
}

/// A hybrid-reference file.
fn hybrid_fixture() -> Vec<u8> {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog>>");
    let hidden = b.object(2, "(hidden)");
    let main = b.table(
        &[(0, &[Row::F(2, 65535), Row::N(catalog, 0), Row::F(0, 65535)])],
        "/Size 4 /Root 1 0 R",
    );
    let stream = b.here();
    b.stream(
        3,
        "/Type /XRef /Size 4 /W [1 2 1] /Index [2 2]",
        &xref_rows([1, 2, 1], &[[1, hidden, 0], [1, stream, 0]]),
    );
    let update = b.table(
        &[],
        &format!("/Size 4 /Root 1 0 R /Prev {main} /XRefStm {stream}"),
    );
    b.startxref(update);
    b.bytes
}

fn fixtures() -> Vec<Vec<u8>> {
    vec![table_fixture(), stream_fixture(), hybrid_fixture()]
}

/// The errors the file layer may give for bytes that are not a sound
/// file. A failed read of the source is not among them: the source is
/// memory.
fn is_expected(error: &PdfError) -> bool {
    matches!(
        error,
        PdfError::MissingHeader
            | PdfError::NoCatalog
            | PdfError::BadXref { .. }
            | PdfError::XrefMismatch { .. }
            | PdfError::ObjectStream { .. }
            | PdfError::UnexpectedEnd { .. }
            | PdfError::UnexpectedToken { .. }
            | PdfError::LimitExceeded { .. }
            | PdfError::UnknownFilter { .. }
            | PdfError::FilterParameters { .. }
            | PdfError::DecodedTooLarge { .. }
    )
}

/// How many object numbers past the last one are also loaded.
const PAST_THE_LAST: u32 = 2;

/// Opens `bytes` and loads every object the cross-reference speaks of,
/// under each section, with the data of each stream.
fn exercise(bytes: &[u8]) {
    let context = || String::from_utf8_lossy(bytes).into_owned();
    let file: PdfFile = match open(bytes).result {
        Ok(file) => file,
        Err(error) => {
            assert!(is_expected(&error), "{error:?} opening {:?}", context());
            return;
        }
    };
    for (index, section) in file.sections().iter().enumerate() {
        let highest = section
            .entries()
            .map(|(number, _)| number)
            .max()
            .unwrap_or(0);
        for number in 0..=highest.saturating_add(PAST_THE_LAST) {
            let generation = match file.entry_as_of(index, number) {
                Some(XrefEntry::InUse { generation, .. }) => generation,
                _ => 0,
            };
            let id = kernkraft_pdf::syntax::ObjectId { number, generation };
            match file.load_as_of(index, id, &mut Diagnostics::new()) {
                Ok(Some(object)) => {
                    if let Body::Stream(stream) = &object.body {
                        let data = file.stream_data(stream).unwrap();
                        assert_eq!(data.len() as u64, stream.data.end - stream.data.start);
                    }
                }
                Ok(None) => {}
                Err(error) => {
                    assert!(
                        is_expected(&error),
                        "{error:?} loading {id:?} of {:?}",
                        context()
                    );
                }
            }
        }
    }
}

#[test]
fn the_fixtures_open_clean_and_every_object_loads() {
    for fixture in fixtures() {
        let file = open(&fixture).clean();
        assert!(!file.xref_rebuilt());
        let numbers: Vec<u32> = (0..file.sections()[0].size() as u32)
            .filter(|&number| {
                matches!(
                    file.entry(number),
                    Some(XrefEntry::InUse { .. } | XrefEntry::Compressed { .. })
                )
            })
            .collect();
        assert!(!numbers.is_empty());
        for number in numbers {
            crate::load(&file, number);
        }
    }
}

#[test]
fn every_truncation_opens_or_is_refused() {
    for fixture in fixtures() {
        for length in 0..fixture.len() {
            exercise(&fixture[..length]);
        }
    }
}

#[test]
fn every_single_byte_substitution_opens_or_is_refused() {
    for fixture in fixtures() {
        let mut mutated = fixture.clone();
        for offset in 0..fixture.len() {
            for &replacement in REPLACEMENTS {
                mutated[offset] = replacement;
                exercise(&mutated);
            }
            mutated[offset] = fixture[offset];
        }
    }
}

#[test]
fn every_removal_of_one_byte_opens_or_is_refused() {
    // Every offset after the removed byte is then one too high.
    for fixture in fixtures() {
        for offset in 0..fixture.len() {
            let mut mutated = fixture.clone();
            mutated.remove(offset);
            exercise(&mutated);
        }
    }
}

#[test]
fn every_splice_opens_or_is_refused() {
    // The start of one file up to a cut, then the end of another from
    // the same offset.
    let fixtures = fixtures();
    for first in &fixtures {
        for second in &fixtures {
            for cut in 0..first.len().min(second.len()) {
                let spliced = [&first[..cut], &second[cut..]].concat();
                exercise(&spliced);
            }
        }
    }
}
