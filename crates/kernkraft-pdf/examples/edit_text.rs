// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Edits a block of text on a one-page PDF, for scripts/check-edit-text.sh.
//!
//!     edit_text OUT_DIR [FONT]
//!
//! Writes OUT_DIR/before.pdf (a page with a rectangle, a line "Hello World"
//! and a footer line, set in Helvetica) and OUT_DIR/after-reuse.pdf (the
//! first line replaced, set in Helvetica again). With FONT, a TrueType or
//! OpenType file, also OUT_DIR/after-font.pdf (the line replaced, set in
//! FONT's first family, embedded). Prints `band TOP BOTTOM`, the vertical
//! extent of the replaced line as fractions of the page's height, for the
//! script's comparison of what lies outside it. Exits 0 when the files are
//! written and 1 on any failure.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use kernkraft_font::FontList;
use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::{Date, Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::write::{save_compact, WriteOptions};
use kernkraft_pdf::{edit_text, EditSession, PdfError, SaveOptions, TextEdit, TextStyle};

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

const CONTENT: &str = "0.8 0.1 0.1 rg 20 20 100 30 re f 0 g\n\
    BT /F1 14 Tf 50 150 Td (Hello World) Tj ET\n\
    BT /F1 14 Tf 50 60 Td (Footer text stays) Tj ET\n";

fn input() -> Result<Document, PdfError> {
    let objects: [String; 5] = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 400 200] /Contents 4 0 R \
         /Resources << /Font << /F1 5 0 R >> >> >>"
            .to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{CONTENT}\nendstream",
            CONTENT.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /Encoding /WinAnsiEncoding >>"
            .to_owned(),
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

fn save(document: &Document, path: &Path) -> Result<(), PdfError> {
    save_compact(document, path, Expect::Anything, &SEED, &NoProgress).map(drop)
}

fn run(out: &Path, font: Option<&str>) -> Result<(), PdfError> {
    let before = input()?;
    save(&before, &out.join("before.pdf"))?;
    let page = before.text_page(1, &NoProgress)?;
    let Some(block) = page
        .blocks
        .iter()
        .min_by(|a, b| a.rect.top.total_cmp(&b.rect.top))
    else {
        return Err(PdfError::Invalid("the page has no text".to_owned()));
    };
    println!("band {} {}", block.rect.top, block.rect.bottom);

    let reused = edit_text(&before, 1, block, &TextEdit::new("Howdy all"), &NoProgress)?;
    save(&reused.document, &out.join("after-reuse.pdf"))?;

    // The same edit through a session on the saved file: the update is
    // appended to the bytes of before.pdf.
    let saved = out.join("before.pdf");
    let bytes = std::fs::read(&saved).map_err(|source| {
        PdfError::Fs(kernkraft_fs::FsError::Io {
            path: saved.clone(),
            source,
        })
    })?;
    let opened = Document::from_bytes(bytes, &OPTIONS, "")?;
    let mut session = EditSession::new(opened)?;
    let appended = edit_text(
        session.document(),
        1,
        block,
        &TextEdit::new("Howdy all"),
        &NoProgress,
    )?;
    session.push(appended.document)?;
    let options = SaveOptions {
        write: SEED,
        modified: Date {
            year: 2026,
            month: 11,
            day: 20,
            hour: 10,
            minute: 30,
            second: 0,
            offset_minutes: Some(0),
        },
    };
    session.save(
        &out.join("after-session.pdf"),
        Expect::Anything,
        &options,
        &NoProgress,
    )?;

    if let Some(font) = font {
        let list = FontList::index(&[PathBuf::from(font)], &NoProgress)?;
        let family = list
            .faces()
            .first()
            .and_then(|face| face.families.first().cloned())
            .ok_or_else(|| PdfError::Invalid(format!("{font} has no family name")))?;
        let edit = TextEdit::new("Howdy all")
            .with_style(TextStyle::default().with_family(family))
            .with_fonts(&list);
        let embedded = edit_text(&before, 1, block, &edit, &NoProgress)?;
        save(&embedded.document, &out.join("after-font.pdf"))?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(out) = args.get(1) else {
        eprintln!("usage: edit_text OUT_DIR [FONT]");
        return ExitCode::FAILURE;
    };
    match run(Path::new(out), args.get(2).map(String::as_str)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
