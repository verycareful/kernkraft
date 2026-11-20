// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Annotates the first page of a PDF in an edit session and saves it as an
//! incremental update, for scripts/check-annot.sh.
//!
//!     annotate INPUT OUTPUT
//!
//! Adds a highlight, an underline, a strikeout, an ink stroke and a note to
//! the first page, recolours the highlight, and deletes the note, so that
//! the file has the four that remain. The input is opened with the empty
//! password. The seed and the time are fixed, so the same input gives the
//! same output.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::doc::{Date, Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::write::WriteOptions;
use kernkraft_pdf::{
    add_annotation, delete_annotation, edit_annotation, AnnotationEdit, EditSession, NewAnnotation,
    NewShape, PdfError, Rect, SaveOptions,
};

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
const WHEN: Date = Date {
    year: 2026,
    month: 11,
    day: 20,
    hour: 12,
    minute: 30,
    second: 5,
    offset_minutes: Some(0),
};
const SAVE: SaveOptions = SaveOptions {
    write: WriteOptions { seed: [0x5A; 32] },
    modified: WHEN,
};

/// What remains of the five annotations: the highlight, underline, strikeout
/// and ink.
const KEPT: usize = 4;

const YELLOW: Rgb = Rgb {
    r: 1.0,
    g: 1.0,
    b: 0.0,
};
const MAGENTA: Rgb = Rgb {
    r: 1.0,
    g: 0.0,
    b: 1.0,
};
const RED: Rgb = Rgb {
    r: 1.0,
    g: 0.0,
    b: 0.0,
};

fn new(shape: NewShape, colour: Rgb) -> NewAnnotation {
    NewAnnotation {
        shape,
        colour,
        opacity: 1.0,
        contents: Some("added by annotate".to_string()),
        author: Some("annotate".to_string()),
        modified: WHEN,
    }
}

fn band(top: f64) -> Rect {
    Rect {
        left: 0.1,
        top,
        right: 0.6,
        bottom: top + 0.04,
    }
}

fn annotate(input: &str, output: &str) -> Result<(), PdfError> {
    let document = Document::open(Path::new(input), &OPTIONS, "")?;
    let before = document.annotations(1)?.len();
    let mut session = EditSession::new(document)?;
    let shapes = [
        (
            NewShape::Highlight {
                quads: vec![band(0.1)],
            },
            YELLOW,
        ),
        (
            NewShape::Underline {
                quads: vec![band(0.2)],
            },
            RED,
        ),
        (
            NewShape::StrikeOut {
                quads: vec![band(0.3)],
            },
            RED,
        ),
        (
            NewShape::Ink {
                strokes: vec![vec![(0.2, 0.5), (0.4, 0.55), (0.6, 0.5)], vec![(0.3, 0.6)]],
                width: 3.0,
            },
            RED,
        ),
        (NewShape::Note { at: (0.7, 0.1) }, YELLOW),
    ];
    let mut ids = Vec::new();
    for (shape, colour) in shapes {
        let added = add_annotation(session.document(), 1, &new(shape, colour))?;
        session.push(added.document)?;
        ids.push(added.id);
    }
    let recoloured = edit_annotation(
        session.document(),
        1,
        ids[0],
        &AnnotationEdit {
            colour: Some(MAGENTA),
            ..Default::default()
        },
        &WHEN,
    )?;
    session.push(recoloured)?;
    let deleted = delete_annotation(session.document(), 1, ids[4])?;
    session.push(deleted)?;
    session.save(Path::new(output), Expect::Anything, &SAVE, &NoProgress)?;

    let saved = Document::open(Path::new(output), &OPTIONS, "")?;
    let found = saved.annotations(1)?.len();
    if found != before + KEPT {
        return Err(PdfError::Invalid(format!(
            "the saved file has {found} annotations on page 1, not {}",
            before + KEPT
        )));
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let (Some(input), Some(output)) = (args.get(1), args.get(2)) else {
        eprintln!("usage: annotate INPUT OUTPUT");
        return ExitCode::from(2);
    };
    match annotate(input, output) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}
