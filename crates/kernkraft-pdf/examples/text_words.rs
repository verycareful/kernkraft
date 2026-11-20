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
//!     LEFT TOP RIGHT BOTTOM OFF WORD
//!
//! The first four numbers are fractions of the displayed page's width and
//! height (`TOP` and `BOTTOM` grow downwards), the rectangle of
//! [`kernkraft_pdf::Rect`]. `OFF` is how far, in degrees from 0 to 45, the
//! word's text lies off the nearest axis of the page (0 for a horizontal or
//! vertical word and for a word of one character), measured from the first
//! character's centre to the last one's, when the word has three characters
//! or more whose centres lie on one line (see `off_axis_degrees`). A word is a run of characters of one line
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

/// A quarter turn, the angle between two axes of a page.
const QUARTER_TURN_DEGREES: f64 = 90.0;

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

/// How near a line, in character boxes, the centres of the characters of a
/// word must lie for the word to be one straight run of text and not, say, a
/// letter with a subscript: a tenth of the larger side of a character's box.
const COLLINEAR_BOXES: f64 = 0.1;

/// How far, in character boxes, the last character's centre must lie from
/// the first one's across the nearer axis for a word to be off the axes: half
/// a box. Boxes of different fonts or of a mark and a letter put the centres
/// of one row a point or two apart, and a few degrees follow from that on a
/// short word without the word leaving its row.
const OFF_AXIS_BOXES: f64 = 0.5;

/// How far the direction along `rects` (a word's drawn characters, in order,
/// as fractions of a page `width` by `height` points) lies off the nearest
/// axis of the page, in degrees from 0 to 45. It is 0 for fewer than three
/// characters, whose centres say nothing about a direction, for centres that
/// coincide, for a last centre less than `OFF_AXIS_BOXES` from the first
/// across the nearer axis, and for centres that are not on one line: a
/// subscript or a change of font moves a centre off the line of the others.
fn off_axis_degrees(rects: &[Rect], width: f64, height: f64) -> f64 {
    let centre = |a: f64, b: f64| (a + b) / 2.0;
    let centres: Vec<(f64, f64)> = rects
        .iter()
        .map(|r| {
            (
                centre(r.left, r.right) * width,
                centre(r.top, r.bottom) * height,
            )
        })
        .collect();
    let (Some(&(x0, y0)), Some(&(x1, y1))) = (centres.first(), centres.last()) else {
        return 0.0;
    };
    let (across, down) = (x1 - x0, y1 - y0);
    let length = across.hypot(down);
    if centres.len() < 3 || length == 0.0 {
        return 0.0;
    }
    let side = rects
        .iter()
        .map(|r| ((r.right - r.left) * width).max((r.bottom - r.top) * height))
        .fold(0.0, f64::max);
    let straight = centres.iter().all(|&(x, y)| {
        ((x - x0) * down - (y - y0) * across).abs() / length <= COLLINEAR_BOXES * side
    });
    if !straight || across.abs().min(down.abs()) < OFF_AXIS_BOXES * side {
        return 0.0;
    }
    let turn = down.atan2(across).to_degrees().abs() % QUARTER_TURN_DEGREES;
    turn.min(QUARTER_TURN_DEGREES - turn)
}

/// The words of one line: its characters between those that are white
/// space, each with the rectangle that holds its characters and how far
/// its text lies off the page's axes.
fn words(line: &TextLine, width: f64, height: f64) -> Vec<(Rect, f64, String)> {
    let mut words = Vec::new();
    let mut text = String::new();
    let mut rects = Vec::new();
    for ch in &line.chars {
        if ch.text.chars().all(char::is_whitespace) {
            push_word(&mut words, &mut text, &mut rects, width, height);
        } else {
            text.push_str(&ch.text);
            rects.push(ch.rect);
        }
    }
    push_word(&mut words, &mut text, &mut rects, width, height);
    words
}

fn push_word(
    words: &mut Vec<(Rect, f64, String)>,
    text: &mut String,
    rects: &mut Vec<Rect>,
    width: f64,
    height: f64,
) {
    // A character with no width (a zero-width space, a joiner) has no place
    // along the text, so it does not give the word's direction.
    let drawn: Vec<Rect> = rects.iter().copied().filter(|r| r.right > r.left).collect();
    let off = off_axis_degrees(&drawn, width, height);
    let around = Rect::around(
        rects
            .iter()
            .flat_map(|r| [(r.left, r.top), (r.right, r.bottom)]),
    );
    if let Some(rect) = around {
        words.push((rect, off, std::mem::take(text)));
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
                for (rect, off, word) in words(line, geometry.width, geometry.height) {
                    println!(
                        "{} {} {} {} {off:.1} {word}",
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
