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
//!     LEFT TOP RIGHT BOTTOM OFF S0X S0Y S3X S3Y E1X E1Y E2X E2Y WORD
//!
//! The first four numbers are fractions of the displayed page's width and
//! height (`TOP` and `BOTTOM` grow downwards), the rectangle of
//! [`kernkraft_pdf::Rect`]. The eight after `OFF` are four `x y` pairs of
//! the same fractions: the start side of the word's first drawn glyph (its quad's
//! corner 0, bottom left, and corner 3, top left) and the end side of its
//! last (corner 1, bottom right, and corner 2, top right), see
//! [`kernkraft_pdf::text::TextChar::quad`]. Upright glyphs have a side
//! on one line of the page; a skewed glyph has a side that leans, and the
//! box of the word reaches to the farther end of it. `OFF` is how far, in degrees from 0 to 45, the
//! word's text lies off the nearest axis of the page (0 for a horizontal or
//! vertical word), the larger of two measures: from the first character's
//! centre to the last one's, when the word has three characters or more whose
//! centres lie on one line (see `off_axis_degrees`), and the direction of the
//! first drawn glyph's own baseline, its quad's corner 0 to corner 1 (see
//! `glyph_off_degrees`), which a word of one or two characters has too. A word is a run of characters of one line
//! between white space; a space the layout read between two glyphs ends a
//! word like any other. Exits with 0 when every page is listed, and 1 on
//! any failure.

use std::path::Path;
use std::process::ExitCode;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::{Document, OpenOptions};
use kernkraft_pdf::file::ReadLimits;
use kernkraft_pdf::text::{TextChar, TextExtractor, TextLine};
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

/// How far the baseline of a glyph, from `quad`'s corner 0 to corner 1 (as
/// fractions of a page `width` by `height` points), lies off the nearest
/// axis of the page, in degrees from 0 to 45. A glyph's own baseline says
/// what a word of one or two characters cannot say by its centres, and a
/// skewed glyph's baseline stays on its line, so a slant is not a turn.
fn glyph_off_degrees(quad: &[(f64, f64); 4], width: f64, height: f64) -> f64 {
    let across = (quad[1].0 - quad[0].0) * width;
    let down = (quad[1].1 - quad[0].1) * height;
    if across.hypot(down) == 0.0 {
        return 0.0;
    }
    let turn = down.atan2(across).to_degrees().abs() % QUARTER_TURN_DEGREES;
    turn.min(QUARTER_TURN_DEGREES - turn)
}

/// A word of the text layer.
struct Word {
    /// The rectangle that holds its characters.
    rect: Rect,
    /// How far its text lies off the page's axes, in degrees.
    off: f64,
    /// The corners of the start side of its first drawn glyph (bottom left,
    /// top left) and of the end side of its last (bottom right, top
    /// right), as fractions of the page.
    sides: [(f64, f64); 4],
    text: String,
}

/// The words of one line: its characters between those that are white
/// space.
fn words(line: &TextLine, width: f64, height: f64) -> Vec<Word> {
    let mut words = Vec::new();
    let mut text = String::new();
    let mut chars: Vec<(usize, &TextChar)> = Vec::new();
    for (index, ch) in line.chars.iter().enumerate() {
        if ch.text.chars().all(char::is_whitespace) {
            push_word(&mut words, &mut text, &mut chars, width, height);
        } else {
            text.push_str(&ch.text);
            chars.push((line.visual_index(index), ch));
        }
    }
    push_word(&mut words, &mut text, &mut chars, width, height);
    words
}

fn push_word(
    words: &mut Vec<Word>,
    text: &mut String,
    chars: &mut Vec<(usize, &TextChar)>,
    width: f64,
    height: f64,
) {
    // A character with no width (a zero-width space, a joiner) has no place
    // along the text, so it does not give the word's direction or its
    // sides. The word's first and last glyphs are those at the two ends
    // along the line, whatever their order in reading.
    let mut drawn: Vec<(usize, &TextChar)> = chars
        .iter()
        .copied()
        .filter(|(_, ch)| ch.rect.right > ch.rect.left)
        .collect();
    drawn.sort_by_key(|(place, _)| *place);
    let drawn: Vec<&TextChar> = drawn.into_iter().map(|(_, ch)| ch).collect();
    let rects: Vec<Rect> = drawn.iter().map(|ch| ch.rect).collect();
    let off = off_axis_degrees(&rects, width, height).max(
        drawn
            .first()
            .map_or(0.0, |ch| glyph_off_degrees(&ch.quad, width, height)),
    );
    let around = Rect::around(
        chars
            .iter()
            .flat_map(|(_, ch)| [(ch.rect.left, ch.rect.top), (ch.rect.right, ch.rect.bottom)]),
    );
    if let Some(rect) = around {
        let sides = match (drawn.first(), drawn.last()) {
            (Some(first), Some(last)) => [first.quad[0], first.quad[3], last.quad[1], last.quad[2]],
            _ => [
                (rect.left, rect.bottom),
                (rect.left, rect.top),
                (rect.right, rect.bottom),
                (rect.right, rect.top),
            ],
        };
        words.push(Word {
            rect,
            off,
            sides,
            text: std::mem::take(text),
        });
    }
    text.clear();
    chars.clear();
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
                for word in words(line, geometry.width, geometry.height) {
                    let Word {
                        rect,
                        off,
                        sides,
                        text,
                    } = word;
                    let sides: Vec<String> =
                        sides.iter().map(|(x, y)| format!("{x} {y}")).collect();
                    println!(
                        "{} {} {} {} {off:.1} {} {text}",
                        rect.left,
                        rect.top,
                        rect.right,
                        rect.bottom,
                        sides.join(" ")
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
