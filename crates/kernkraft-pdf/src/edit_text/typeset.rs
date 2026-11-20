// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Setting new text in the width of the block it replaces: ISO 32000-1:2008,
//! 9.3.2 (`Tc`), 9.3.3 (`Tw`), 9.3.4 (`Tz`) and 9.4.4 (the glyph
//! displacement `tx = ((w0 * Tfs + Tc + Tw) * Th)`).
//!
//! Text breaks only where the new text has a line feed or where a space
//! lets a word move to the next line. A word wider than the block stays
//! whole on a line of its own.

use crate::PdfError;

/// What a line is measured with: the horizontal displacement of one
/// character in text space units.
pub(super) type Advance<'a> = dyn Fn(char) -> Result<f64, PdfError> + 'a;

/// The relative error of one addition of two `f64`s is at most
/// [`f64::EPSILON`]; a line's width is a sum of one term for each of its
/// characters, so a line that fills the block exactly, up to the rounding of
/// that sum, is allowed this many `EPSILON` more than the block's width.
fn fit_slack(characters: usize) -> f64 {
    f64::EPSILON * (characters + 1) as f64
}

/// `text` broken into lines no wider than `width` (text space units), or
/// not broken but at its line feeds when `width` is not a positive number.
pub(super) fn lines(
    text: &str,
    width: f64,
    advance: &Advance<'_>,
) -> Result<Vec<String>, PdfError> {
    let mut out = Vec::new();
    for paragraph in text.split('\n') {
        wrap(paragraph, width, advance, &mut out)?;
    }
    Ok(out)
}

fn measure(text: &str, advance: &Advance<'_>) -> Result<f64, PdfError> {
    text.chars().map(advance).sum()
}

fn wrap(
    paragraph: &str,
    width: f64,
    advance: &Advance<'_>,
    out: &mut Vec<String>,
) -> Result<(), PdfError> {
    let limit = width * (1.0 + fit_slack(paragraph.chars().count()));
    let mut line = String::new();
    let mut taken = 0.0;
    for (index, word) in paragraph.split(' ').enumerate() {
        let word_width = measure(word, advance)?;
        if index == 0 {
            line.push_str(word);
            taken = word_width;
            continue;
        }
        let space = advance(' ')?;
        if width > 0.0 && !line.is_empty() && taken + space + word_width > limit {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
            taken = word_width;
        } else {
            line.push(' ');
            line.push_str(word);
            taken += space + word_width;
        }
    }
    out.push(line);
    Ok(())
}
