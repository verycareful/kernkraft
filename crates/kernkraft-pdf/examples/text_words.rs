// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Lists the words of every page of a PDF with the rectangle each covers,
//! for scripts/check-text.sh.
//!
//!     text_words INPUT
//!
//! The input is opened with the empty password. Standard output has one
//! line for each page, counted from 1, giving the displayed page's size in
//! points and its `Rotate` in degrees,
//!
//!     page N WIDTH HEIGHT ROTATE
//!
//! and under it one line for each word of the page's text layer in reading
//! order:
//!
//!     LEFT TOP RIGHT BOTTOM WORD
//!
//! The four numbers are fractions of the displayed page's width and height
//! (`TOP` and `BOTTOM` grow downwards), the rectangle of
//! [`kernkraft_pdf::Rect`]. A word is a run of characters of one line
//! between white space; a space the layout read between two glyphs ends a
//! word like any other. Exits with 0 when every page is listed, and 1 on
//! any failure.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::text::{TextExtractor, TextLine};
use kernkraft_pdf::{PdfError, Rect};

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

/// The words of one line: its characters between those that are white
/// space, each with the rectangle that holds its characters.
fn words(line: &TextLine) -> Vec<(Rect, String)> {
    let mut words = Vec::new();
    let mut text = String::new();
    let mut rects = Vec::new();
    for ch in &line.chars {
        if ch.text.chars().all(char::is_whitespace) {
            push_word(&mut words, &mut text, &mut rects);
        } else {
            text.push_str(&ch.text);
            rects.push(ch.rect);
        }
    }
    push_word(&mut words, &mut text, &mut rects);
    words
}

fn push_word(words: &mut Vec<(Rect, String)>, text: &mut String, rects: &mut Vec<Rect>) {
    let around = Rect::around(
        rects
            .iter()
            .flat_map(|r| [(r.left, r.top), (r.right, r.bottom)]),
    );
    if let Some(rect) = around {
        words.push((rect, std::mem::take(text)));
    }
    text.clear();
    rects.clear();
}

fn run(path: &str) -> Result<(), PdfError> {
    let doc = Document::open(Path::new(path), &OPTIONS, "")?;
    let count = doc.page_count()?;
    let mut extractor = TextExtractor::new(&doc)?;
    for number in 1..=count {
        let geometry = doc.page_geometry(number)?;
        println!(
            "page {number} {} {} {}",
            geometry.width, geometry.height, geometry.rotate
        );
        let page = extractor.page(number, &NoProgress)?;
        for block in &page.blocks {
            for line in &block.lines {
                for (rect, word) in words(line) {
                    println!(
                        "{} {} {} {} {word}",
                        rect.left, rect.top, rect.right, rect.bottom
                    );
                }
            }
        }
    }
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let [input] = args.as_slice() else {
        eprintln!("usage: text_words INPUT");
        return ExitCode::from(1);
    };
    match run(input) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::from(1)
        }
    }
}
