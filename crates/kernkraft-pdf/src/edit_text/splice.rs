// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Taking glyphs out of a showing operation without moving the text after
//! it: ISO 32000-1:2008, 9.4.3 (the operators `Tj`, `TJ`, `'` and `\"` and
//! Table 109), 9.4.4 (the glyph displacement `tx` and `ty`) and 9.4.2
//! (the number items of a `TJ` array).
//!
//! A glyph moves the text matrix by `tx = ((w0 * Tfs + Tc + Tw) * Th)`
//! (`ty = w1 * Tfs + Tc + Tw` in vertical writing). A number `n` in a `TJ`
//! array moves it by `-n / 1000 * Tfs * Th` (by `-n / 1000 * Tfs` along y
//! in vertical writing). So a glyph taken out is made up for by the number
//! `-tx * 1000 / (Tfs * Th)`, and the line matrix is not touched, which a
//! later `T*`, `'` or `Td` needs.

use std::collections::HashSet;

use crate::build::{number, ContentBuilder};
use crate::content::Operation;
use crate::edit_text::collect::ShowOp;
use crate::syntax::{Object, PdfString, Value};
use crate::PdfError;

/// The thousandths of a text space unit a number of a `TJ` array counts in
/// (9.4.3, Table 109).
const TJ_UNITS: f64 = 1000.0;

/// One element of the rebuilt `TJ` array.
enum Item {
    Text(Vec<u8>),
    Number(Object),
}

/// The bytes of one string operand of the operation and the glyphs it was
/// read as: `(offset, len, advance)` for each.
struct Piece {
    glyphs: Vec<(usize, usize, f64)>,
}

fn invalid(what: &str) -> PdfError {
    PdfError::Invalid(format!("the text to edit: {what}"))
}

/// The operations that show what `operation` showed, less the glyphs
/// `removed` (each as `(operand, offset)`, as [`GlyphSource`] gives them).
///
/// [`GlyphSource`]: crate::text::GlyphSource
pub(super) fn without_glyphs(
    show: &ShowOp,
    operation: &Operation,
    removed: &HashSet<(u32, u32)>,
) -> Result<Vec<u8>, PdfError> {
    let mut out = ContentBuilder::new();
    let operands = &operation.operands;
    let strings: Vec<(u32, Vec<u8>)> = match operation.operator.as_slice() {
        b"Tj" => vec![(0, string_of(operands.first())?)],
        b"'" => {
            out.operator(&[], "T*")?;
            vec![(0, string_of(operands.first())?)]
        }
        b"\"" => {
            let [word, character, text] = operands.as_slice() else {
                return Err(invalid("a \\\" operation with other than three operands"));
            };
            out.operator(std::slice::from_ref(word), "Tw")?;
            out.operator(std::slice::from_ref(character), "Tc")?;
            out.operator(&[], "T*")?;
            vec![(0, string_of(Some(text))?)]
        }
        b"TJ" => Vec::new(),
        _ => return Err(invalid("the operation is not a text-showing one")),
    };

    let mut rebuild = Rebuild {
        pieces: pieces_of(show),
        removed,
        scale: scale_of(show)?,
        items: Vec::new(),
        pending: 0.0,
    };
    if operation.operator == b"TJ" {
        let Some(Value::Array(array)) = operands.first().map(Object::value) else {
            return Err(invalid("a TJ operation without an array"));
        };
        for (index, item) in array.iter().enumerate() {
            match item.value() {
                Value::String(string) => rebuild.string(index as u32, string.as_bytes())?,
                Value::Integer(_) | Value::Real(_) => {
                    rebuild.flush()?;
                    rebuild.items.push(Item::Number(item.clone()));
                }
                _ => {
                    return Err(invalid(
                        "an item of a TJ array that is not a string or number",
                    ))
                }
            }
        }
    } else {
        for (index, bytes) in &strings {
            rebuild.string(*index, bytes)?;
        }
    }
    rebuild.flush()?;
    let items = rebuild.items;

    let array = items
        .into_iter()
        .map(|item| match item {
            Item::Text(bytes) => Object::new(Value::String(PdfString::new(bytes))),
            Item::Number(object) => object,
        })
        .collect();
    out.operator(&[Object::new(Value::Array(array))], "TJ")?;
    let mut bytes = out.finish();
    // The operation sits among the stream's own white space.
    while bytes.last().is_some_and(u8::is_ascii_whitespace) {
        bytes.pop();
    }
    Ok(bytes)
}

/// The array being rebuilt, and the move of the glyphs taken out since its
/// last element.
struct Rebuild<'a> {
    pieces: std::collections::HashMap<u32, Piece>,
    removed: &'a HashSet<(u32, u32)>,
    scale: f64,
    items: Vec<Item>,
    pending: f64,
}

impl Rebuild<'_> {
    /// Writes the move made up for so far as a number.
    fn flush(&mut self) -> Result<(), PdfError> {
        if self.pending != 0.0 {
            self.items.push(Item::Number(number(self.pending)?));
            self.pending = 0.0;
        }
        Ok(())
    }

    /// The string `bytes`, operand `index` of the operation, less the
    /// glyphs removed.
    fn string(&mut self, index: u32, bytes: &[u8]) -> Result<(), PdfError> {
        let Some(piece) = self.pieces.get(&index) else {
            return Err(invalid("a string the interpreter did not read"));
        };
        let glyphs = piece.glyphs.clone();
        let mut text: Vec<u8> = Vec::new();
        let mut covered = 0;
        for (offset, len, advance) in glyphs {
            let Some(code) = bytes.get(offset..offset + len) else {
                return Err(invalid("a glyph past the end of its string"));
            };
            covered = offset + len;
            if self.removed.contains(&(index, offset as u32)) {
                if !text.is_empty() {
                    self.items.push(Item::Text(std::mem::take(&mut text)));
                }
                self.pending += -advance * TJ_UNITS / self.scale;
            } else {
                if self.pending != 0.0 {
                    self.flush()?;
                }
                text.extend_from_slice(code);
            }
        }
        if covered != bytes.len() {
            return Err(invalid("a string the glyphs do not cover"));
        }
        if !text.is_empty() {
            self.items.push(Item::Text(text));
        }
        Ok(())
    }
}

fn string_of(operand: Option<&Object>) -> Result<Vec<u8>, PdfError> {
    match operand.map(Object::value) {
        Some(Value::String(string)) => Ok(string.as_bytes().to_vec()),
        _ => Err(invalid("a showing operation without its string")),
    }
}

/// `Tfs * Th`, or `Tfs` in vertical writing: what a unit of a `TJ` number
/// is multiplied by (the number counts thousandths of it).
fn scale_of(show: &ShowOp) -> Result<f64, PdfError> {
    let scale = if show.run.font.is_vertical() {
        show.run.font_size
    } else {
        show.run.font_size * show.horizontal_scaling
    };
    if scale == 0.0 || !scale.is_finite() {
        return Err(invalid("a font size or scaling of 0"));
    }
    Ok(scale)
}

/// The glyphs of the operation by the string they are in. The advance is
/// the one along the writing direction, spacing included.
fn pieces_of(show: &ShowOp) -> std::collections::HashMap<u32, Piece> {
    let vertical = show.run.font.is_vertical();
    let mut pieces: std::collections::HashMap<u32, Piece> = std::collections::HashMap::new();
    for glyph in &show.run.glyphs {
        let piece = pieces
            .entry(glyph.operand)
            .or_insert_with(|| Piece { glyphs: Vec::new() });
        let offset = piece
            .glyphs
            .last()
            .map_or(0, |(offset, len, _)| offset + len);
        let advance = if vertical {
            glyph.advance.1
        } else {
            glyph.advance.0
        };
        piece.glyphs.push((offset, glyph.len, advance));
    }
    pieces
}
