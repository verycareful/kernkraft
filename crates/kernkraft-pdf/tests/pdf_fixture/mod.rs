// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Files for the tests of the document and write layers, written byte by
//! byte here so that neither is tested only against the library's own
//! writer. Each file notes where every object lies, so a test's
//! expectations come from how its file was built.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::Write;
use std::path::PathBuf;

use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;

/// Limits and caches no test's file comes near.
pub const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 1 << 16,
        max_xref_entries: 1 << 16,
        max_object_stream_bytes: 1 << 20,
    },
    cached_objects: 64,
    cached_object_streams: 4,
    max_diagnostics: 1000,
};

pub const HEADER: &[u8] = b"%PDF-1.7\n%\xE2\xE3\xCF\xD3\n";

/// A file, and the bytes of each indirect object in it: from its number
/// to the end of `endobj`.
#[derive(Debug, Clone)]
pub struct Fixture {
    pub bytes: Vec<u8>,
    pub spans: BTreeMap<u32, (usize, usize)>,
}

impl Fixture {
    /// The bytes object `number` was written as.
    pub fn object_bytes(&self, number: u32) -> &[u8] {
        let (start, end) = self.spans[&number];
        &self.bytes[start..end]
    }

    pub fn open(&self) -> Document {
        Document::from_bytes(self.bytes.clone(), &OPTIONS, "").expect("fixture opens")
    }
}

/// The body of a stream object: its dictionary entries with a direct
/// `Length`, and its data.
pub fn stream(entries: &str, data: &[u8]) -> Vec<u8> {
    let mut body = format!("<<{entries} /Length {}>>\nstream\n", data.len()).into_bytes();
    body.extend_from_slice(data);
    body.extend_from_slice(b"\nendstream");
    body
}

fn write_object(
    out: &mut Vec<u8>,
    spans: &mut BTreeMap<u32, (usize, usize)>,
    number: u32,
    body: &[u8],
) {
    let start = out.len();
    let _ = writeln!(out, "{number} 0 obj");
    out.extend_from_slice(body);
    out.extend_from_slice(b"\nendobj");
    spans.insert(number, (start, out.len()));
    out.push(b'\n');
}

/// A file of `objects`, all generation 0, with a cross-reference table
/// and a trailer of `trailer`'s entries and `Size`.
pub fn table_file(objects: &[(u32, &[u8])], trailer: &str) -> Fixture {
    let mut out = HEADER.to_vec();
    let mut spans = BTreeMap::new();
    for (number, body) in objects {
        write_object(&mut out, &mut spans, *number, body);
    }
    let size = objects.iter().map(|(n, _)| n + 1).max().unwrap_or(1);
    let xref = out.len();
    let _ = write!(out, "xref\n0 {size}\n");
    for number in 0..size {
        match spans.get(&number) {
            Some((start, _)) => {
                let _ = write!(out, "{start:010} 00000 n\r\n");
            }
            None if number == 0 => out.extend_from_slice(b"0000000000 65535 f\r\n"),
            None => out.extend_from_slice(b"0000000000 00000 f\r\n"),
        }
    }
    let _ = write!(
        out,
        "trailer\n<<{trailer} /Size {size}>>\nstartxref\n{xref}\n%%EOF\n"
    );
    Fixture { bytes: out, spans }
}

/// A file whose `compressed` objects lie in one object stream, written
/// without a filter, and whose `plain` objects lie in the file, with a
/// cross-reference stream also without a filter (`W` [1 4 2]). The
/// object stream takes the number after every object's and the
/// cross-reference stream the one after that.
pub fn stream_file(plain: &[(u32, &[u8])], compressed: &[(u32, &[u8])], trailer: &str) -> Fixture {
    let mut out = HEADER.to_vec();
    let mut spans = BTreeMap::new();
    for (number, body) in plain {
        write_object(&mut out, &mut spans, *number, body);
    }
    let highest = plain
        .iter()
        .chain(compressed)
        .map(|(n, _)| *n)
        .max()
        .unwrap_or(0);
    let object_stream = highest + 1;
    let xref_stream = highest + 2;
    let mut header = Vec::new();
    let mut body = Vec::new();
    for (index, (number, object)) in compressed.iter().enumerate() {
        if index > 0 {
            header.push(b' ');
        }
        let _ = write!(header, "{number} {}", body.len());
        body.extend_from_slice(object);
        body.push(b'\n');
    }
    header.push(b'\n');
    let first = header.len();
    header.extend_from_slice(&body);
    write_object(
        &mut out,
        &mut spans,
        object_stream,
        &stream(
            &format!("/Type /ObjStm /N {} /First {first}", compressed.len()),
            &header,
        ),
    );
    let size = xref_stream + 1;
    let mut entries = BTreeMap::new();
    for (number, (start, _)) in &spans {
        entries.insert(*number, (1u8, *start as u32, 0u16));
    }
    for (index, (number, _)) in compressed.iter().enumerate() {
        entries.insert(*number, (2, object_stream, index as u16));
    }
    let xref_offset = out.len();
    entries.insert(xref_stream, (1, xref_offset as u32, 0));
    let mut data = Vec::new();
    for number in 0..size {
        let (kind, second, third) =
            entries
                .get(&number)
                .copied()
                .unwrap_or((0, 0, if number == 0 { 65535 } else { 0 }));
        data.push(kind);
        data.extend_from_slice(&second.to_be_bytes());
        data.extend_from_slice(&third.to_be_bytes());
    }
    write_object(
        &mut out,
        &mut spans,
        xref_stream,
        &stream(
            &format!("/Type /XRef /Size {size} /W [1 4 2] {trailer}"),
            &data,
        ),
    );
    let _ = write!(out, "startxref\n{xref_offset}\n%%EOF\n");
    Fixture { bytes: out, spans }
}

/// A document of `pages` pages, each with a content stream: objects 1
/// (catalog), 2 (page tree), 3 (information), then a page and its
/// content for each page, and object 50, which nothing refers to.
pub fn pages_document(pages: u32) -> Vec<(u32, Vec<u8>)> {
    let kids: Vec<String> = (0..pages).map(|i| format!("{} 0 R", 10 + 2 * i)).collect();
    let mut objects = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (
            2,
            format!(
                "<< /Type /Pages /Kids [{}] /Count {pages} /MediaBox [0 0 612 792] >>",
                kids.join(" ")
            )
            .into_bytes(),
        ),
        (
            3,
            b"<< /Title (Fixture) /Producer (kernkraft tests) >>".to_vec(),
        ),
    ];
    for i in 0..pages {
        let page = 10 + 2 * i;
        objects.push((
            page,
            format!("<< /Type /Page /Parent 2 0 R /Contents {} 0 R >>", page + 1).into_bytes(),
        ));
        objects.push((
            page + 1,
            stream(
                "",
                format!("BT /F1 12 Tf 72 700 Td (Page {}) Tj ET", i + 1).as_bytes(),
            ),
        ));
    }
    objects.push((50, b"(nothing refers to this)".to_vec()));
    objects
}

/// `objects` borrowed, as the file builders take them.
pub fn borrowed(objects: &[(u32, Vec<u8>)]) -> Vec<(u32, &[u8])> {
    objects.iter().map(|(n, b)| (*n, b.as_slice())).collect()
}

pub fn encrypted_fixture(name: &str) -> Vec<u8> {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/encrypted")
        .join(format!("{name}.pdf"));
    std::fs::read(path).expect("fixture")
}

/// The offset the last `startxref` of `bytes` gives.
pub fn last_startxref(bytes: &[u8]) -> usize {
    let keyword = b"startxref";
    let at = bytes
        .windows(keyword.len())
        .rposition(|window| window == keyword)
        .expect("a startxref");
    let digits: String = bytes[at + keyword.len()..]
        .iter()
        .skip_while(|byte| byte.is_ascii_whitespace())
        .take_while(|byte| byte.is_ascii_digit())
        .map(|byte| char::from(*byte))
        .collect();
    digits.parse().expect("an offset")
}

/// `base` with an incremental update appended (7.5.6): `objects`, then a
/// cross-reference table of their entries alone, and a trailer of
/// `trailer`'s entries, `Size` and `Prev`. The update's section starts at
/// the returned offset.
pub fn append_update(base: &Fixture, objects: &[(u32, &[u8])], trailer: &str) -> (Fixture, usize) {
    let mut out = base.bytes.clone();
    let mut spans = base.spans.clone();
    let prev = last_startxref(&base.bytes);
    for (number, body) in objects {
        write_object(&mut out, &mut spans, *number, body);
    }
    let size = spans.keys().map(|n| n + 1).max().unwrap_or(1);
    let xref = out.len();
    out.extend_from_slice(b"xref\n");
    for (number, _) in objects {
        let (start, _) = spans[number];
        let _ = write!(out, "{number} 1\n{start:010} 00000 n\r\n");
    }
    let _ = write!(
        out,
        "trailer\n<<{trailer} /Size {size} /Prev {prev}>>\nstartxref\n{xref}\n%%EOF\n"
    );
    (Fixture { bytes: out, spans }, xref)
}

/// A file laid out as Annex F lays out a linearized one, as far as its
/// cross-reference goes: object `first` (the linearization parameter
/// dictionary when `linearized` holds, a plain dictionary otherwise) and
/// then the first-page section, with the trailer of `trailer`'s entries
/// and a `Prev` naming the main section; then `rest`, and the main
/// section, whose trailer has `Size` alone. The final `startxref` gives
/// the first-page section (F.3.11). Returns the file and the offsets of
/// the two sections.
pub fn linearized_file(
    first: u32,
    linearized: bool,
    early: &[(u32, &[u8])],
    rest: &[(u32, &[u8])],
    trailer: &str,
) -> (Fixture, usize, usize) {
    // The first-page section's offsets are written with a fixed width, so
    // the main section's offset can be filled in once it is known.
    const WIDTH: usize = 10;
    let mut out = HEADER.to_vec();
    let mut spans = BTreeMap::new();
    let parameters: &[u8] = if linearized {
        b"<< /Linearized 1 /L 0 /H [0 0] /O 0 /E 0 /N 1 /T 0 >>"
    } else {
        b"<< /Producer (not linearized) >>"
    };
    write_object(&mut out, &mut spans, first, parameters);
    let first_page = out.len();
    let mut numbers: Vec<u32> = std::iter::once(first)
        .chain(early.iter().map(|(n, _)| *n))
        .collect();
    numbers.sort_unstable();
    let size = numbers
        .iter()
        .chain(rest.iter().map(|(n, _)| n))
        .map(|n| n + 1)
        .max()
        .unwrap_or(1);
    // The table is written with placeholders, objects after it.
    out.extend_from_slice(b"xref\n");
    let mut placeholders = Vec::new();
    for number in &numbers {
        let _ = writeln!(out, "{number} 1");
        placeholders.push((*number, out.len()));
        out.extend_from_slice(b"0000000000 00000 n\r\n");
    }
    let _ = write!(out, "trailer\n<<{trailer} /Size {size} /Prev ");
    let prev_at = out.len();
    out.extend_from_slice(&[b'0'; WIDTH]);
    out.extend_from_slice(b">>\nstartxref\n0\n%%EOF\n");
    for (number, body) in early {
        write_object(&mut out, &mut spans, *number, body);
    }
    for (number, body) in rest {
        write_object(&mut out, &mut spans, *number, body);
    }
    let main = out.len();
    let _ = write!(out, "xref\n0 1\n0000000000 65535 f\r\n");
    for (number, _) in rest {
        let (start, _) = spans[number];
        let _ = write!(out, "{number} 1\n{start:010} 00000 n\r\n");
    }
    let _ = write!(
        out,
        "trailer\n<< /Size {size} >>\nstartxref\n{first_page}\n%%EOF\n"
    );
    for (number, at) in placeholders {
        let (start, _) = spans[&number];
        out[at..at + WIDTH].copy_from_slice(format!("{start:010}").as_bytes());
    }
    out[prev_at..prev_at + WIDTH].copy_from_slice(format!("{main:010}").as_bytes());
    (Fixture { bytes: out, spans }, first_page, main)
}

// Fonts and pages for the tests of font embedding.

#[path = "../../../kernkraft-font/tests/common/mod.rs"]
pub mod fontkit;

use fontkit::cff::{self, CffGlyph, CffSpec, Cs2};
use fontkit::sfnt::{self, Glyph, SfntSpec};
use kernkraft_pdf::build::{ContentBuilder, ObjectBuilder, StreamEncoding};
use kernkraft_pdf::font::FontEmbedder;
use kernkraft_pdf::syntax::{Dictionary, Name, Object, ObjectId, Value};

/// The test fonts' glyphs on an em of 1000 units: `A` is a rectangle 500 by
/// 700 with advance 700, `B` a square of 200 with advance 450, the space has
/// no marks and advance 250, and `l` is a stem 80 wide and 700 high.
pub const EMBED_A_ADVANCE: u16 = 700;
pub const EMBED_B_ADVANCE: u16 = 450;
pub const EMBED_SPACE_ADVANCE: u16 = 250;
pub const EMBED_STEM: i16 = 80;

fn rectangle(width: i16, height: i16) -> Vec<Vec<(i16, i16, bool)>> {
    vec![vec![
        (0, 0, true),
        (width, 0, true),
        (width, height, true),
        (0, height, true),
    ]]
}

/// A TrueType font of `.notdef`, `A` (glyph 1, also reached from `a`), `B`
/// (2), the space (3) and `l` (4). `stem` leaves `l` out when false.
pub fn embed_truetype(stem: bool) -> Vec<u8> {
    let mut glyphs = vec![
        Glyph {
            name: ".notdef",
            advance: 500,
            contours: vec![],
        },
        Glyph {
            name: "A",
            advance: EMBED_A_ADVANCE,
            contours: rectangle(500, 700),
        },
        Glyph {
            name: "B",
            advance: EMBED_B_ADVANCE,
            contours: rectangle(200, 200),
        },
        Glyph {
            name: "space",
            advance: EMBED_SPACE_ADVANCE,
            contours: vec![],
        },
    ];
    let mut unicode = vec![(0x41, 1), (0x61, 1), (0x42, 2), (0x20, 3)];
    if stem {
        glyphs.push(Glyph {
            name: "l",
            advance: 300,
            contours: rectangle(EMBED_STEM, 700),
        });
        unicode.push((0x6C, 4));
    }
    let mut spec = SfntSpec::new(glyphs);
    spec.unicode = unicode;
    sfnt::build(&spec)
}

/// The same `A`, `B` and space as OpenType with CFF outlines.
pub fn embed_opentype() -> Vec<u8> {
    let glyph = |id: u16, code: Cs2| CffGlyph { id, code };
    let box_of = |width: i32, height: i32| {
        Cs2::new()
            .with(&[0, 0], "rmoveto")
            .with(&[width], "hlineto")
            .with(&[height], "vlineto")
            .with(&[-width], "hlineto")
            .op("endchar")
    };
    // The standard strings 34, 35 and 1 are A, B and space.
    let program = cff::build(&CffSpec {
        glyphs: vec![
            glyph(0, Cs2::new().op("endchar")),
            glyph(34, box_of(500, 700)),
            glyph(35, box_of(200, 200)),
            glyph(1, Cs2::new().op("endchar")),
        ],
        default_width: 600,
        nominal_width: 500,
        matrix: None,
        cid_keyed: false,
        encoding: None,
    });
    sfnt::open_type_with_cff(
        program,
        &[500, EMBED_A_ADVANCE, EMBED_B_ADVANCE, EMBED_SPACE_ADVANCE],
        &[(0x41, 1), (0x42, 2), (0x20, 3)],
    )
}

fn entry(dictionary: &mut Dictionary, key: &str, value: Value) {
    dictionary.insert(Name::new(key.as_bytes()), Object::new(value));
}

fn object_id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

/// The content that shows `codes` with the font `/F1` at `size` points, its
/// origin at `(x, y)`.
pub fn show_codes(codes: &[u8], size: f64, x: f64, y: f64) -> Vec<u8> {
    let mut content = ContentBuilder::new();
    content
        .begin_text()
        .unwrap()
        .font(b"F1", size)
        .unwrap()
        .text_position(x, y)
        .unwrap()
        .show_text(codes)
        .unwrap()
        .end_text()
        .unwrap();
    content.finish()
}

/// A 20 by 20 point one-page file (page 10, contents 11) whose page shows
/// `content` with `embedder`'s font as `/F1`, and the font's object.
pub fn embedded_page(embedder: &FontEmbedder, content: Vec<u8>) -> (Document, ObjectId) {
    let objects: Vec<(u32, Vec<u8>)> = vec![
        (1, b"<< /Type /Catalog /Pages 2 0 R >>".to_vec()),
        (2, b"<< /Type /Pages /Kids [10 0 R] /Count 1 >>".to_vec()),
        (
            10,
            b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 20 20] /Contents 11 0 R >>".to_vec(),
        ),
        (11, stream("", b"")),
    ];
    let base = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    let mut builder = ObjectBuilder::new(&base);
    let font = embedder.write(&mut builder).expect("the font is written");

    let mut fonts = Dictionary::new();
    entry(&mut fonts, "F1", Value::Reference(font));
    let mut resources = Dictionary::new();
    entry(&mut resources, "Font", Value::Dictionary(fonts));
    let mut page = Dictionary::new();
    entry(&mut page, "Type", Value::Name(Name::new(&b"Page"[..])));
    entry(&mut page, "Parent", Value::Reference(object_id(2)));
    entry(
        &mut page,
        "MediaBox",
        Value::Array(
            [0, 0, 20, 20]
                .map(|v| Object::new(Value::Integer(v)))
                .to_vec(),
        ),
    );
    entry(&mut page, "Contents", Value::Reference(object_id(11)));
    entry(&mut page, "Resources", Value::Dictionary(resources));
    builder.set(object_id(10), Object::new(Value::Dictionary(page)));
    builder
        .set_stream(
            object_id(11),
            Dictionary::new(),
            content,
            StreamEncoding::AsGiven,
        )
        .expect("the contents are set");
    let doc = base.apply(builder.finish()).expect("the changes apply");
    (doc, font)
}

// A CMYK profile written by hand (ICC.1:2010, 6 and 10.8).

/// An `s15Fixed16Number` (ICC.1:2010, 4.6).
pub fn s15(value: f64) -> [u8; 4] {
    ((value * 65536.0).round() as i32).to_be_bytes()
}

pub fn xyz_tag(x: f64, y: f64, z: f64) -> Vec<u8> {
    let mut tag = b"XYZ \0\0\0\0".to_vec();
    for value in [x, y, z] {
        tag.extend_from_slice(&s15(value));
    }
    tag
}

/// The D50 illuminant of the profile connection space (ICC.1:2010, 7.2.16).
pub const D50: [f64; 3] = [0.9642, 1.0, 0.8249];

/// A `lut8Type` (10.8) from four channels to Lab with two grid points a
/// channel: a CLUT of 16 entries, which `lightness` gives the L* byte of
/// from the four channels (0 or 1 each). The matrix is the identity and
/// the tables are ramps, so the CLUT alone decides.
pub fn lut8(lightness: impl Fn([u8; 4]) -> u8) -> Vec<u8> {
    let mut tag = b"mft1\0\0\0\0".to_vec();
    // Input channels, output channels, grid points, padding.
    tag.extend_from_slice(&[4, 3, 2, 0]);
    for value in [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0] {
        tag.extend_from_slice(&s15(value));
    }
    let ramp: Vec<u8> = (0..=255u8).collect();
    for _ in 0..4 {
        tag.extend_from_slice(&ramp);
    }
    // The first channel varies slowest (10.8).
    for index in 0..16u8 {
        let corner = [index >> 3 & 1, index >> 2 & 1, index >> 1 & 1, index & 1];
        // a* and b* are zero: the 8-bit encoding of Lab puts them at 128.
        tag.extend_from_slice(&[lightness(corner), 128, 128]);
    }
    for _ in 0..3 {
        tag.extend_from_slice(&ramp);
    }
    tag
}

/// A CMYK profile whose lightness is `100 * (1 - k)`: white without black
/// ink, black with it, and neutral whatever the cyan, magenta and yellow.
pub fn grey_by_black_profile() -> Vec<u8> {
    let table = lut8(|[_, _, _, k]| 255 * (1 - k));
    let white = xyz_tag(D50[0], D50[1], D50[2]);
    // The tags: the table is both the perceptual and the relative
    // colorimetric one, and shared.
    let tags: [(&[u8; 4], &Vec<u8>, bool); 3] = [
        (b"A2B0", &table, false),
        (b"A2B1", &table, true),
        (b"wtpt", &white, false),
    ];
    let mut offset = 128 + 4 + 12 * tags.len();
    let mut table_of_tags = (tags.len() as u32).to_be_bytes().to_vec();
    let mut data = Vec::new();
    let mut shared_at = None;
    for (signature, bytes, shares) in tags {
        let at = match (shares, shared_at) {
            (true, Some(at)) => at,
            _ => {
                let at = offset + data.len();
                data.extend_from_slice(bytes);
                while data.len() % 4 != 0 {
                    data.push(0);
                }
                at
            }
        };
        if signature == b"A2B0" {
            shared_at = Some(at);
        }
        table_of_tags.extend_from_slice(signature);
        table_of_tags.extend_from_slice(&(at as u32).to_be_bytes());
        table_of_tags.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
    }
    offset += data.len();
    let mut header = vec![0u8; 128];
    header[0..4].copy_from_slice(&(offset as u32).to_be_bytes());
    // Version 2.1.0.
    header[8..12].copy_from_slice(&[2, 0x10, 0, 0]);
    header[12..16].copy_from_slice(b"prtr");
    header[16..20].copy_from_slice(b"CMYK");
    header[20..24].copy_from_slice(b"Lab ");
    header[36..40].copy_from_slice(b"acsp");
    for (at, value) in D50.iter().enumerate() {
        header[68 + 4 * at..72 + 4 * at].copy_from_slice(&s15(*value));
    }
    [header, table_of_tags, data].concat()
}
