// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the file layer, one module per source file. [`Builder`] writes
//! the bytes of a PDF file piece by piece and notes the offset each piece
//! lands at, so a test's expectations come from how its file was built.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

mod chain;
mod header;
mod limits;
mod load;
mod object_stream;
mod repair;
mod robustness;
mod xref_stream;
mod xref_table;

use std::fmt::Debug;
use std::io::Write;
use std::sync::Arc;

use kernkraft_pdf::file::{Body, FileObject, Location, PdfFile, ReadLimits, Stream, XrefEntry};
use kernkraft_pdf::syntax::{Dictionary, Object, ObjectId, Value};
use kernkraft_pdf::{Diagnostic, DiagnosticKind, Diagnostics, Limit, MemorySource, PdfError};

/// Limits no test's file comes near, for tests that are not about limits.
pub const ROOMY: ReadLimits = ReadLimits {
    max_object_elements: 1 << 16,
    max_xref_entries: 1 << 16,
    max_object_stream_bytes: 1 << 20,
};

/// The header line and the comment of binary characters that 7.5.2 puts
/// after it.
pub const HEADER: &[u8] = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";

/// The three end-of-line sequences a table entry may end with (7.5.4).
pub const ENTRY_EOLS: [&str; 3] = [" \r", " \n", "\r\n"];

/// One cross-reference table entry as a test states it.
#[derive(Debug, Clone, Copy)]
pub enum Row {
    /// An in-use entry: offset and generation.
    N(u64, u16),
    /// A free entry: next free object and generation.
    F(u32, u16),
}

impl Row {
    /// The 20 bytes of 7.5.4, ending in `eol`.
    pub fn text(self, eol: &str) -> String {
        match self {
            Row::N(offset, generation) => format!("{offset:010} {generation:05} n{eol}"),
            Row::F(next, generation) => format!("{next:010} {generation:05} f{eol}"),
        }
    }

    pub fn entry(self) -> XrefEntry {
        match self {
            Row::N(offset, generation) => XrefEntry::InUse { offset, generation },
            Row::F(next, generation) => XrefEntry::Free { next, generation },
        }
    }
}

/// The free entry that heads the free list of a file with no other free
/// object (7.5.4).
pub const HEAD: Row = Row::F(0, 65535);

/// The bytes of a file under construction.
#[derive(Debug, Clone, Default)]
pub struct Builder {
    pub bytes: Vec<u8>,
}

impl Builder {
    /// A file that so far holds [`HEADER`].
    pub fn new() -> Self {
        Self {
            bytes: HEADER.to_vec(),
        }
    }

    /// The offset the next piece lands at.
    pub fn here(&self) -> u64 {
        self.bytes.len() as u64
    }

    /// Appends `bytes` and returns the offset they start at.
    pub fn raw(&mut self, bytes: &[u8]) -> u64 {
        let offset = self.here();
        self.bytes.extend_from_slice(bytes);
        offset
    }

    pub fn text(&mut self, text: &str) -> u64 {
        self.raw(text.as_bytes())
    }

    /// Appends object `number` of generation 0 holding `body`.
    pub fn object(&mut self, number: u32, body: &str) -> u64 {
        self.object_gen(number, 0, body)
    }

    pub fn object_gen(&mut self, number: u32, generation: u16, body: &str) -> u64 {
        self.text(&format!("{number} {generation} obj\n{body}\nendobj\n"))
    }

    /// Appends stream `number` whose dictionary holds `entries` and a
    /// direct `Length` of `data`'s size. Returns the offset of the
    /// object and the offset of the first byte of data.
    pub fn stream(&mut self, number: u32, entries: &str, data: &[u8]) -> (u64, u64) {
        let length = data.len();
        self.stream_with(number, &format!("{entries} /Length {length}"), data)
    }

    /// As [`Self::stream`], with the dictionary's entries given whole.
    pub fn stream_with(&mut self, number: u32, entries: &str, data: &[u8]) -> (u64, u64) {
        let offset = self.text(&format!("{number} 0 obj\n<<{entries}>>\nstream\n"));
        let data_start = self.raw(data);
        self.text("\nendstream\nendobj\n");
        (offset, data_start)
    }

    /// Appends a cross-reference table of `subsections`, each a first
    /// object number and its rows, then the trailer dictionary holding
    /// `trailer`. Returns the offset of `xref`.
    pub fn table(&mut self, subsections: &[(u32, &[Row])], trailer: &str) -> u64 {
        self.table_with(subsections, trailer, ENTRY_EOLS[1])
    }

    pub fn table_with(&mut self, subsections: &[(u32, &[Row])], trailer: &str, eol: &str) -> u64 {
        let offset = self.text("xref\n");
        for (first, rows) in subsections {
            self.text(&format!("{first} {}\n", rows.len()));
            for row in *rows {
                self.text(&row.text(eol));
            }
        }
        self.text(&format!("trailer\n<<{trailer}>>\n"));
        offset
    }

    /// Appends `startxref`, `offset` and the end-of-file marker.
    pub fn startxref(&mut self, offset: u64) {
        self.text(&format!("startxref\n{offset}\n%%EOF\n"));
    }

    /// The file as built.
    pub fn open(&self) -> Opened {
        open(&self.bytes)
    }
}

/// What opening a file gave.
pub struct Opened {
    pub result: Result<PdfFile, PdfError>,
    pub diagnostics: Vec<Diagnostic>,
}

impl Opened {
    /// The open file, which must have opened with no diagnostic.
    pub fn clean(self) -> PdfFile {
        assert_eq!(self.diagnostics, []);
        self.result.unwrap()
    }

    /// The open file and the kind and offset of each diagnostic.
    pub fn with_diagnostics(self) -> (PdfFile, Vec<(DiagnosticKind, u64)>) {
        let diagnostics = self
            .diagnostics
            .into_iter()
            .map(|d| {
                assert_eq!(d.object, None);
                (d.kind, d.offset)
            })
            .collect();
        (self.result.unwrap(), diagnostics)
    }

    /// The file, which must have been opened by rebuilding its
    /// cross-reference on account of a fault at `fault`.
    pub fn rebuilt(self, fault: u64) -> PdfFile {
        let (file, diagnostics) = self.with_diagnostics();
        assert_eq!(diagnostics, [(DiagnosticKind::XrefRebuilt, fault)]);
        assert!(file.xref_rebuilt());
        file
    }
}

pub fn open(bytes: &[u8]) -> Opened {
    open_with(bytes, ROOMY)
}

pub fn open_with(bytes: &[u8], limits: ReadLimits) -> Opened {
    let mut diagnostics = Diagnostics::new();
    let source = Arc::new(MemorySource::new(bytes.to_vec()));
    let result = PdfFile::open(source, limits, &mut diagnostics);
    Opened {
        result,
        diagnostics: diagnostics.into_vec(),
    }
}

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

/// Loads object `number` of generation 0, which must exist and load with
/// no diagnostic.
pub fn load(file: &PdfFile, number: u32) -> FileObject {
    let mut diagnostics = Diagnostics::new();
    let object = file.load(id(number), &mut diagnostics).unwrap().unwrap();
    assert_eq!(diagnostics.as_slice(), []);
    object
}

/// The kind, object and offset of a diagnostic.
pub type Noted = (DiagnosticKind, Option<ObjectId>, u64);

/// The result of loading `id` and the kind, object and offset of each
/// diagnostic.
pub fn load_noting(
    file: &PdfFile,
    id: ObjectId,
) -> (Result<Option<FileObject>, PdfError>, Vec<Noted>) {
    let mut diagnostics = Diagnostics::new();
    let result = file.load(id, &mut diagnostics);
    let noted = diagnostics
        .into_vec()
        .into_iter()
        .map(|d| (d.kind, d.object, d.offset))
        .collect();
    (result, noted)
}

/// Whether `id` loads as no object.
pub fn is_missing(file: &PdfFile, id: ObjectId) -> bool {
    let (result, diagnostics) = load_noting(file, id);
    assert_eq!(diagnostics, []);
    result.unwrap().is_none()
}

/// The value of an object that is not a stream.
pub fn value(object: &FileObject) -> &Value {
    match &object.body {
        Body::Object(object) => object.value(),
        Body::Stream(stream) => panic!("a stream: {stream:?}"),
    }
}

pub fn stream(object: &FileObject) -> &Stream {
    match &object.body {
        Body::Stream(stream) => stream,
        Body::Object(object) => panic!("not a stream: {object:?}"),
    }
}

/// The bytes of `bytes` that `object` was read from.
pub fn located<'a>(bytes: &'a [u8], object: &FileObject) -> &'a [u8] {
    match object.location {
        Location::File(span) => &bytes[span.start as usize..span.end as usize],
        other => panic!("not in the file: {other:?}"),
    }
}

/// The integer a dictionary holds for `key`.
pub fn integer(dictionary: &Dictionary, key: &str) -> i64 {
    match dictionary.get(key.as_bytes()).map(Object::value) {
        Some(Value::Integer(value)) => *value,
        other => panic!("{key} is {other:?}"),
    }
}

/// The reference a dictionary holds for `key`.
pub fn reference(dictionary: &Dictionary, key: &str) -> ObjectId {
    match dictionary.get(key.as_bytes()).map(Object::value) {
        Some(Value::Reference(id)) => *id,
        other => panic!("{key} is {other:?}"),
    }
}

/// The keys of a dictionary, in its order.
pub fn keys(dictionary: &Dictionary) -> Vec<String> {
    dictionary
        .iter()
        .map(|(key, _)| String::from_utf8(key.as_bytes().to_vec()).unwrap())
        .collect()
}

/// The data of a cross-reference stream (7.5.8.3): each entry's three
/// fields, big-endian, in the given widths.
pub fn xref_rows(widths: [usize; 3], entries: &[[u64; 3]]) -> Vec<u8> {
    let mut out = Vec::new();
    for entry in entries {
        for (field, width) in entry.iter().zip(widths) {
            let bytes = field.to_be_bytes();
            assert!(bytes[..bytes.len() - width].iter().all(|&b| b == 0));
            out.extend_from_slice(&bytes[bytes.len() - width..]);
        }
    }
    out
}

/// `bytes` in hexadecimal with the end-of-data marker of 7.4.2.
pub fn hex(bytes: &[u8]) -> Vec<u8> {
    let mut out: Vec<u8> = bytes
        .iter()
        .flat_map(|byte| format!("{byte:02X}").into_bytes())
        .collect();
    out.push(b'>');
    out
}

/// A zlib stream of `bytes`, from flate2's encoder.
pub fn deflate(bytes: &[u8]) -> Vec<u8> {
    let mut encoder = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
    encoder.write_all(bytes).unwrap();
    encoder.finish().unwrap()
}

/// The PNG tag of the Up filter (RFC 2083, 6.4).
const PNG_UP: u8 = 2;

/// `raw` with the PNG Up filter applied to rows of `row_bytes`: each row
/// is tagged, and each byte becomes its difference from the byte above,
/// the row above the first being zeros.
pub fn png_up(raw: &[u8], row_bytes: usize) -> Vec<u8> {
    let mut out = Vec::new();
    let mut above = vec![0u8; row_bytes];
    for row in raw.chunks(row_bytes) {
        out.push(PNG_UP);
        out.extend(
            row.iter()
                .zip(&above)
                .map(|(byte, up)| byte.wrapping_sub(*up)),
        );
        above = row.to_vec();
    }
    out
}

/// The text of an object stream's data (7.5.7): the table of object
/// numbers and offsets, then the objects. Returns the data and the value
/// of `First`.
pub fn object_stream_data(objects: &[(u32, &str)]) -> (Vec<u8>, usize) {
    let mut table = String::new();
    let mut body = String::new();
    for (number, text) in objects {
        table.push_str(&format!("{number} {} ", body.len()));
        body.push_str(text);
        body.push(' ');
    }
    let first = table.len();
    (format!("{table}{body}").into_bytes(), first)
}

pub fn assert_limit<T: Debug>(result: Result<T, PdfError>, expected_offset: u64, expected: Limit) {
    match result {
        Err(PdfError::LimitExceeded { offset, limit }) => {
            assert_eq!((offset, limit), (expected_offset, expected));
        }
        other => panic!("expected LimitExceeded {expected:?} at {expected_offset}, got {other:?}"),
    }
}

pub fn assert_object_stream<T: Debug>(result: Result<T, PdfError>, expected: u32) {
    match result {
        Err(PdfError::ObjectStream { stream }) => assert_eq!(stream, expected),
        other => panic!("expected ObjectStream {expected}, got {other:?}"),
    }
}

pub fn assert_mismatch<T: Debug>(result: Result<T, PdfError>, id: ObjectId, expected_offset: u64) {
    match result {
        Err(PdfError::XrefMismatch {
            number,
            generation,
            offset,
        }) => assert_eq!(
            (number, generation, offset),
            (id.number, id.generation, expected_offset)
        ),
        other => panic!("expected XrefMismatch of {id:?} at {expected_offset}, got {other:?}"),
    }
}

/// A file of three objects, a table and a trailer, with the offset of
/// each object: the catalog 1, the page tree 2 and the string 3.
pub fn small_file() -> (Builder, [u64; 3]) {
    let mut b = Builder::new();
    let catalog = b.object(1, "<</Type /Catalog /Pages 2 0 R>>");
    let pages = b.object(2, "<</Type /Pages /Kids [] /Count 0>>");
    let string = b.object(3, "(Brillig)");
    let xref = b.table(
        &[(
            0,
            &[
                HEAD,
                Row::N(catalog, 0),
                Row::N(pages, 0),
                Row::N(string, 0),
            ],
        )],
        "/Size 4 /Root 1 0 R",
    );
    b.startxref(xref);
    (b, [catalog, pages, string])
}

#[test]
fn caller_limits_report_the_value_they_were_given() {
    assert_eq!(Limit::ObjectElements { max: 7 }.max(), 7);
    assert_eq!(Limit::XrefEntries { max: 9 }.max(), 9);
    let error = PdfError::LimitExceeded {
        offset: 3,
        limit: Limit::XrefEntries { max: 9 },
    };
    assert_eq!(
        error.to_string(),
        "cross-reference entry count at offset 3 is above the limit of 9"
    );
}
