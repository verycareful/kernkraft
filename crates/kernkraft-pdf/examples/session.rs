// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Edits a PDF in an edit session and saves it as an incremental update,
//! for scripts/check-session.sh.
//!
//!     session title  INPUT OUTPUT TEXT
//!     session rotate INPUT OUTPUT PAGES
//!     session remove INPUT OUTPUT PAGES
//!     session twice  INPUT OUTPUT PAGES TEXT
//!
//! `title` sets the document's title, `rotate` turns pages 90 degrees
//! clockwise, `remove` removes pages (PAGES is comma-separated page
//! numbers). `twice` turns the pages and appends the update, then sets the
//! title and appends a second update, as an application does that can only
//! append to its file. The input is opened with the empty password. The
//! seed and the time are fixed, so the same input gives the same output.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::doc::{Date, Document, ObjectBuilder, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::syntax::{Dictionary, Name, Object, PdfString, Value};
use kernkraft_pdf::write::WriteOptions;
use kernkraft_pdf::{remove, rotate, EditSession, PdfError, Rotation, SaveOptions};

const OPTIONS: OpenOptions = OpenOptions {
    limits: ReadLimits {
        max_object_elements: 4 << 20,
        max_xref_entries: 64 << 20,
        max_object_stream_bytes: 256 << 20,
    },
    cached_objects: 4096,
    cached_object_streams: 16,
    max_diagnostics: 10_000,
};
const SAVE: SaveOptions = SaveOptions {
    write: WriteOptions { seed: [0x5A; 32] },
    modified: Date {
        year: 2026,
        month: 11,
        day: 15,
        hour: 12,
        minute: 30,
        second: 5,
        offset_minutes: Some(0),
    },
};

fn numbers(list: &str) -> Result<Vec<u32>, String> {
    list.split(',')
        .map(|page| {
            page.trim()
                .parse()
                .map_err(|_| format!("not a page: {page}"))
        })
        .collect()
}

/// Makes `title` the title of the document in `session`.
fn set_title(session: &mut EditSession, title: &str) -> Result<(), PdfError> {
    let document = session.document().clone();
    let mut info = document.info()?.unwrap_or_else(Dictionary::new);
    info.insert(
        Name::new(b"Title".as_slice()),
        Object::new(Value::String(PdfString::new(title.as_bytes()))),
    );
    let info = Object::new(Value::Dictionary(info));
    let mut builder = ObjectBuilder::new(&document);
    match document.trailer().get(b"Info").map(Object::value) {
        Some(Value::Reference(id)) => builder.set(*id, info),
        _ => {
            let id = builder.add(info)?;
            builder.set_trailer(
                Name::new(b"Info".as_slice()),
                Some(Object::new(Value::Reference(id))),
            );
        }
    }
    session.apply(builder.finish())?;
    Ok(())
}

fn turn(session: &mut EditSession, pages: &[u32]) -> Result<(), PdfError> {
    let turned = rotate(session.document(), pages, Rotation::Cw90)?;
    session.push(turned)?;
    Ok(())
}

fn run(args: &[String]) -> Result<(), String> {
    let usage = || "usage: session OPERATION ... (see the file header)".to_string();
    let error = |e: PdfError| e.to_string();
    let operation = args.get(1).ok_or_else(usage)?;
    let (input, output, value) = (
        args.get(2).ok_or_else(usage)?,
        args.get(3).ok_or_else(usage)?,
        args.get(4).ok_or_else(usage)?,
    );
    let document = Document::open(Path::new(input), &OPTIONS, "").map_err(error)?;
    let base = document.file().source().len();
    let mut session = EditSession::new(document).map_err(error)?;
    match operation.as_str() {
        "title" => set_title(&mut session, value).map_err(error)?,
        "rotate" => turn(&mut session, &numbers(value)?).map_err(error)?,
        "remove" => {
            let removed = remove(session.document(), &numbers(value)?).map_err(error)?;
            session.push(removed).map_err(error)?;
        }
        "twice" => {
            let title = args.get(5).ok_or_else(usage)?;
            let mut file = std::fs::read(input).map_err(|e| e.to_string())?;
            turn(&mut session, &numbers(value)?).map_err(error)?;
            let first = session
                .append_since(base, &SAVE, &NoProgress)
                .map_err(error)?;
            file.extend_from_slice(&first.bytes);
            set_title(&mut session, title).map_err(error)?;
            let second = session
                .append_since(first.length, &SAVE, &NoProgress)
                .map_err(error)?;
            file.extend_from_slice(&second.bytes);
            std::fs::write(output, file).map_err(|e| e.to_string())?;
            return Ok(());
        }
        _ => return Err(usage()),
    }
    session
        .save(Path::new(output), Expect::Anything, &SAVE, &NoProgress)
        .map_err(error)?;
    for diagnostic in session.document().diagnostics() {
        eprintln!("diagnostic: {diagnostic:?}");
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    match run(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}
