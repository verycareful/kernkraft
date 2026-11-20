// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Shows text with an embedded font on a one-page PDF, for
//! scripts/check-font.sh.
//!
//!     embed_font FONT INDEX OUTPUT TEXT
//!
//! FONT is a TrueType or OpenType file (INDEX picks a font out of a
//! collection). The page is 400 by 100 points and shows TEXT at 14 points
//! with the font embedded as a subset. Exits with 0 when the file is written,
//! 3 when the font has no glyph for a character of TEXT, 4 when the font is
//! one that cannot be reduced to a subset (a `CFF2` font, a font of bitmaps),
//! and 1 on any other failure. The seed is fixed, so the same input gives the same output.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::build::{ContentBuilder, ObjectBuilder, StreamEncoding};
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::font::FontEmbedder;
use kernkraft_pdf::syntax::{Dictionary, Name, Object, ObjectId, Value};
use kernkraft_pdf::write::{save_compact, WriteOptions};
use kernkraft_pdf::PdfError;

const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 1 << 16,
        max_xref_entries: 1 << 16,
        max_object_stream_bytes: 1 << 20,
    },
    cached_objects: 64,
    cached_object_streams: 4,
    max_diagnostics: 1000,
};
const SEED: WriteOptions = WriteOptions { seed: [0x5A; 32] };
const MISSING_GLYPH: u8 = 3;
const NOT_REDUCIBLE: u8 = 4;
const PAGE_WIDTH: i64 = 400;
const PAGE_HEIGHT: i64 = 100;
const SIZE: f64 = 14.0;
const LEFT: f64 = 10.0;
const BASELINE: f64 = 40.0;

/// A page of no content, written out: the objects are 1 the catalog, 2 the
/// page tree, 3 the page and 4 its (empty) contents.
fn blank_page() -> Result<Document, PdfError> {
    let objects: [&str; 4] = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 100] /Contents 4 0 R >>",
        "<< /Length 0 >>\nstream\n\nendstream",
    ];
    let mut bytes = b"%PDF-1.7\n".to_vec();
    let mut offsets = Vec::new();
    for (i, body) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend(format!("{} 0 obj\n{body}\nendobj\n", i + 1).bytes());
    }
    let xref = bytes.len();
    bytes.extend(format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).bytes());
    for offset in offsets {
        bytes.extend(format!("{offset:010} 00000 n \n").bytes());
    }
    bytes.extend(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .bytes(),
    );
    Document::from_bytes(bytes, &OPTIONS, "")
}

fn entry(dictionary: &mut Dictionary, key: &str, value: Value) {
    dictionary.insert(Name::new(key.as_bytes()), Object::new(value));
}

fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

fn run(program: Vec<u8>, index: u32, output: &Path, text: &str) -> Result<(), PdfError> {
    let mut embedder = FontEmbedder::new(program, index)?;
    let codes = embedder.encode(text)?;
    let mut content = ContentBuilder::new();
    content
        .begin_text()?
        .font(b"F1", SIZE)?
        .text_position(LEFT, BASELINE)?
        .show_text(&codes)?
        .end_text()?;

    let base = blank_page()?;
    let mut builder = ObjectBuilder::new(&base);
    let font_object = embedder.write(&mut builder)?;
    let mut fonts = Dictionary::new();
    entry(&mut fonts, "F1", Value::Reference(font_object));
    let mut resources = Dictionary::new();
    entry(&mut resources, "Font", Value::Dictionary(fonts));
    let mut page = Dictionary::new();
    entry(&mut page, "Type", Value::Name(Name::new(&b"Page"[..])));
    entry(&mut page, "Parent", Value::Reference(id(2)));
    entry(
        &mut page,
        "MediaBox",
        Value::Array(
            [0, 0, PAGE_WIDTH, PAGE_HEIGHT]
                .map(|v| Object::new(Value::Integer(v)))
                .to_vec(),
        ),
    );
    entry(&mut page, "Contents", Value::Reference(id(4)));
    entry(&mut page, "Resources", Value::Dictionary(resources));
    builder.set(id(3), Object::new(Value::Dictionary(page)));
    builder.set_stream(
        id(4),
        Dictionary::new(),
        content.finish(),
        StreamEncoding::Flate,
    )?;
    let document = base.apply(builder.finish())?;
    save_compact(&document, output, Expect::Anything, &SEED, &NoProgress).map(drop)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(font), Some(index), Some(output), Some(text)) =
        (args.get(1), args.get(2), args.get(3), args.get(4))
    else {
        eprintln!("usage: embed_font FONT INDEX OUTPUT TEXT");
        return ExitCode::FAILURE;
    };
    let Ok(index) = index.parse::<u32>() else {
        eprintln!("INDEX is not a number: {index}");
        return ExitCode::FAILURE;
    };
    let program = match std::fs::read(font) {
        Ok(program) => program,
        Err(error) => {
            eprintln!("{font}: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(program, index, Path::new(output), text) {
        Ok(()) => ExitCode::SUCCESS,
        Err(PdfError::NoGlyphForCharacter { character }) => {
            eprintln!("{font}: no glyph for U+{:04X}", u32::from(character));
            ExitCode::from(MISSING_GLYPH)
        }
        Err(error @ PdfError::Font(kernkraft_font::FontError::Subset(_))) => {
            eprintln!("{font}: {error}");
            ExitCode::from(NOT_REDUCIBLE)
        }
        Err(error) => {
            eprintln!("{font}: {error}");
            ExitCode::FAILURE
        }
    }
}
