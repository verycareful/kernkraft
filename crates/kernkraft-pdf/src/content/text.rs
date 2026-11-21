// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text in the interpreter: ISO 32000-1:2008, 9.3 (text state parameters
//! and operators, Table 104), 9.4 (text objects, Tables 105 to 109) and
//! 9.4.4 (text space details).
//!
//! The text state lives in [`GraphicsState::text`](crate::content::state::GraphicsState);
//! the text matrix and the text line matrix, which belong to a text object,
//! are the interpreter's [`TextObject`](super::interpreter).

use crate::content::device::{glyph_of, OperatorSpan, TextRun};
use crate::content::interpreter::{render_mode_of, Env, Interpreter};
use crate::content::matrix::Matrix;
use crate::content::operation::{operand_number, Operation};
use crate::content::state::{TextFont, GLYPH_UNITS_PER_EM, PERCENT};
use crate::diagnostic::DiagnosticKind;
use crate::error::PdfError;
use crate::syntax::{Name, Value};

impl Interpreter {
    pub(super) fn begin_text(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) {
        if !operation.operands.is_empty() {
            return self.invalid(operation);
        }
        if self.text.active {
            // BT cannot nest (9.4.1); the object starts again.
            self.misplaced(operation);
        }
        self.text.active = true;
        self.text.tm = Matrix::IDENTITY;
        self.text.tlm = Matrix::IDENTITY;
        let ctx = self.context(span);
        env.device.begin_text(&ctx);
    }

    pub(super) fn end_text(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) {
        if !operation.operands.is_empty() {
            return self.invalid(operation);
        }
        if !self.text.active {
            return self.misplaced(operation);
        }
        self.text.active = false;
        let ctx = self.context(span);
        env.device.end_text(&ctx);
    }

    /// `Tc`, `Tw`, `Tz`, `TL`, `Tf`, `Tr` and `Ts` (Table 104).
    pub(super) fn set_text_state(&mut self, operation: &Operation) -> Result<(), PdfError> {
        match operation.operator.as_slice() {
            b"Tf" => return self.set_font(operation),
            b"Tc" => {
                if let Some([value]) = self.numbers(operation) {
                    self.state.text.char_spacing = value;
                }
            }
            b"Tw" => {
                if let Some([value]) = self.numbers(operation) {
                    self.state.text.word_spacing = value;
                }
            }
            b"Tz" => {
                if let Some([percent]) = self.numbers(operation) {
                    self.state.text.horizontal_scaling = percent / PERCENT;
                }
            }
            b"TL" => {
                if let Some([value]) = self.numbers(operation) {
                    self.state.text.leading = value;
                }
            }
            b"Tr" => {
                if let Some([code]) = self.numbers::<1>(operation) {
                    match render_mode_of(code) {
                        Some(mode) => self.state.text.render_mode = mode,
                        None => self.invalid(operation),
                    }
                }
            }
            _ => {
                if let Some([value]) = self.numbers(operation) {
                    self.state.text.rise = value;
                }
            }
        }
        Ok(())
    }

    fn set_font(&mut self, operation: &Operation) -> Result<(), PdfError> {
        let [name, size] = operation.operands.as_slice() else {
            self.invalid(operation);
            return Ok(());
        };
        let (Value::Name(name), Some(size)) = (name.value(), operand_number(size)) else {
            self.invalid(operation);
            return Ok(());
        };
        let name = name.clone();
        let Some(entry) = self.lookup(b"Font", &name)? else {
            self.missing(b"Font", &name, operation);
            self.state.text.font = None;
            return Ok(());
        };
        self.state.text.font =
            self.load_font(&entry, Some(&name))?
                .map(|font| TextFont { name, font, size });
        Ok(())
    }

    /// `Td`, `TD`, `Tm` and `T*` (Table 108).
    pub(super) fn position_text(&mut self, operation: &Operation) {
        match operation.operator.as_slice() {
            b"Td" => {
                if let Some([tx, ty]) = self.numbers(operation) {
                    self.move_line(tx, ty);
                }
            }
            b"TD" => {
                if let Some([tx, ty]) = self.numbers(operation) {
                    self.state.text.leading = -ty;
                    self.move_line(tx, ty);
                }
            }
            b"Tm" => {
                if let Some(values) = self.numbers::<6>(operation) {
                    let matrix = Matrix::from_array(values);
                    self.text.tm = matrix;
                    self.text.tlm = matrix;
                }
            }
            _ => {
                if !operation.operands.is_empty() {
                    return self.invalid(operation);
                }
                self.next_line();
            }
        }
    }

    /// Moves to the start of a line offset `(tx, ty)` from the start of the
    /// current one (9.4.2): `Tlm = T(tx, ty) x Tlm`, and `Tm = Tlm`.
    fn move_line(&mut self, tx: f64, ty: f64) {
        self.text.tlm = Matrix::translation(tx, ty).then(self.text.tlm);
        self.text.tm = self.text.tlm;
    }

    /// `T*`: the next line, one leading down.
    fn next_line(&mut self) {
        let leading = self.state.text.leading;
        self.move_line(0.0, -leading);
    }

    /// `Tj`, `TJ`, `'` and `"` (Table 109).
    pub(super) fn show_text(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let operator = operation.operator.as_slice();
        let operands = operation.operands.as_slice();
        // The strings and adjustments to show, with their operand index.
        let items: Vec<(u32, Item<'_>)> = match (operator, operands) {
            (b"Tj", [string]) => match string.value() {
                Value::String(string) => vec![(0, Item::String(string.as_bytes()))],
                _ => {
                    self.invalid(operation);
                    return Ok(());
                }
            },
            (b"'", [string]) => match string.value() {
                Value::String(string) => vec![(0, Item::String(string.as_bytes()))],
                _ => {
                    self.invalid(operation);
                    return Ok(());
                }
            },
            (b"\"", [word, character, string]) => {
                match (
                    operand_number(word),
                    operand_number(character),
                    string.value(),
                ) {
                    (Some(word), Some(character), Value::String(string)) => {
                        self.state.text.word_spacing = word;
                        self.state.text.char_spacing = character;
                        vec![(0, Item::String(string.as_bytes()))]
                    }
                    _ => {
                        self.invalid(operation);
                        return Ok(());
                    }
                }
            }
            (b"TJ", [array]) => match array.value() {
                Value::Array(array) => {
                    let mut items = Vec::with_capacity(array.len());
                    for (index, item) in array.iter().enumerate() {
                        let index = u32::try_from(index).unwrap_or(u32::MAX);
                        match item.value() {
                            Value::String(string) => {
                                items.push((index, Item::String(string.as_bytes())))
                            }
                            _ => match operand_number(item) {
                                Some(number) => items.push((index, Item::Adjust(number))),
                                None => self.invalid(operation),
                            },
                        }
                    }
                    items
                }
                _ => {
                    self.invalid(operation);
                    return Ok(());
                }
            },
            _ => {
                self.invalid(operation);
                return Ok(());
            }
        };
        if !self.text.active {
            // Showing text outside a text object is not allowed (9.4.1);
            // it is done with the matrices as they stand.
            self.misplaced(operation);
        }
        if matches!(operator, b"'" | b"\"") {
            self.next_line();
        }
        let Some(text_font) = self.state.text.font.clone() else {
            self.diagnostics.push(
                DiagnosticKind::OperatorMisplaced {
                    operator: Name::new(operation.operator.clone()),
                },
                operation.span.start,
            );
            return Ok(());
        };
        let matrix_before = self.text.tm;
        let mut glyphs = Vec::new();
        let mut adjustments = Vec::new();
        let vertical = text_font.font.is_vertical();
        let state = self.state.text.clone();
        let size = text_font.size;
        let scaling = state.horizontal_scaling;
        for (operand, item) in items {
            match item {
                Item::Adjust(number) => {
                    // A number in a TJ array moves back by that many
                    // thousandths of the font size (9.4.3).
                    let shift = -number / GLYPH_UNITS_PER_EM * size;
                    let (tx, ty) = if vertical {
                        (0.0, shift)
                    } else {
                        (shift * scaling, 0.0)
                    };
                    self.text.tm = Matrix::translation(tx, ty).then(self.text.tm);
                    adjustments.push((operand, if vertical { ty } else { tx }));
                }
                Item::String(bytes) => {
                    for ch in text_font.font.chars(bytes) {
                        let mut glyph = glyph_of(&ch, operand);
                        let (origin_x, origin_y, advance) = match ch.vertical {
                            Some(metrics) if vertical => (
                                -metrics.origin_x / GLYPH_UNITS_PER_EM,
                                -metrics.origin_y / GLYPH_UNITS_PER_EM,
                                Some(metrics.advance),
                            ),
                            _ => (0.0, 0.0, None),
                        };
                        // The text rendering matrix (9.4.4):
                        // [Tfs*Th 0 0 Tfs 0 Trise] x Tm x CTM, the glyph
                        // moved first to its origin for vertical writing.
                        glyph.matrix = Matrix::translation(origin_x, origin_y)
                            .then(Matrix::new(size * scaling, 0.0, 0.0, size, 0.0, state.rise))
                            .then(self.text.tm)
                            .then(self.state.ctm);
                        let spacing =
                            state.char_spacing + if ch.is_space { state.word_spacing } else { 0.0 };
                        let (tx, ty) = match advance {
                            // Vertical: ty = w1 * Tfs + Tc + Tw (9.4.4).
                            Some(w1) => (0.0, w1 / GLYPH_UNITS_PER_EM * size + spacing),
                            // Horizontal: tx = (w0 * Tfs + Tc + Tw) * Th.
                            None => (
                                (ch.width / GLYPH_UNITS_PER_EM * size + spacing) * scaling,
                                0.0,
                            ),
                        };
                        glyph.advance = (tx, ty);
                        self.text.tm = Matrix::translation(tx, ty).then(self.text.tm);
                        glyphs.push(glyph);
                    }
                }
            }
        }
        let run = TextRun {
            font_name: text_font.name.clone(),
            font: text_font.font.clone(),
            font_size: size,
            glyphs,
            adjustments,
            matrix_before,
            matrix_after: self.text.tm,
        };
        let ctx = self.context(span);
        env.device.text(&ctx, &run);
        Ok(())
    }
}

/// One thing a text-showing operator shows.
enum Item<'a> {
    String(&'a [u8]),
    /// A `TJ` adjustment, in thousandths of text space units.
    Adjust(f64),
}
