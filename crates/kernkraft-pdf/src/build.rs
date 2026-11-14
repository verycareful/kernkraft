// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Building content and objects: content streams (ISO 32000-1:2008, 7.8.2
//! and the operators of Annex A) and the objects that hold them, through
//! [`ObjectBuilder`](crate::doc::ObjectBuilder).
//!
//! Annotations, added text, embedded fonts and page operations all write
//! through these; they are public for applications that write their own.

pub use crate::doc::{Changes, ObjectBuilder, StreamEncoding};

use crate::syntax::chars::is_regular;
use crate::syntax::{write_object, Name, Object, PdfString, Real, Value};
use crate::PdfError;

/// The operators and operands of one content stream, written one operator
/// to a line (7.8.2: operands, then the operator that takes them).
#[derive(Debug, Clone, Default)]
pub struct ContentBuilder {
    out: Vec<u8>,
}

/// An operand that is a number: an integer when it is a whole number an
/// integer can hold, a real otherwise. Infinity and NaN, which no number
/// of the syntax writes (7.3.3), are refused.
pub fn number(value: f64) -> Result<Object, PdfError> {
    // The integers within 2^53 are exactly the whole f64s that convert
    // to an i64 and back unchanged.
    let integral = value.fract() == 0.0 && value.abs() < (1u64 << f64::MANTISSA_DIGITS) as f64;
    if integral {
        return Ok(Object::new(Value::Integer(value as i64)));
    }
    Real::new(value)
        .map(|real| Object::new(Value::Real(real)))
        .ok_or_else(|| PdfError::Invalid(format!("{value} is not a number a PDF can hold")))
}

fn name(bytes: &[u8]) -> Object {
    Object::new(Value::Name(Name::new(bytes)))
}

impl ContentBuilder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends `operator` with `operands` before it. An operator is a
    /// keyword: one or more regular characters (7.2.2, 7.8.2); anything
    /// else is [`PdfError::Invalid`].
    pub fn operator(&mut self, operands: &[Object], operator: &str) -> Result<&mut Self, PdfError> {
        if operator.is_empty() || !operator.bytes().all(is_regular) {
            return Err(PdfError::Invalid(format!(
                "{operator:?} is not an operator"
            )));
        }
        for operand in operands {
            write_object(operand, &mut self.out)?;
            self.out.push(b' ');
        }
        self.out.extend_from_slice(operator.as_bytes());
        self.out.push(b'\n');
        Ok(self)
    }

    fn numbers(&mut self, values: &[f64], operator: &str) -> Result<&mut Self, PdfError> {
        let operands = values
            .iter()
            .map(|&value| number(value))
            .collect::<Result<Vec<_>, _>>()?;
        self.operator(&operands, operator)
    }

    /// `q`: saves the graphics state (8.4.2).
    pub fn save_state(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "q")
    }

    /// `Q`: restores the graphics state saved last.
    pub fn restore_state(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "Q")
    }

    /// `cm`: concatenates `[a b c d e f]` to the transformation matrix
    /// (8.4.4).
    pub fn transform(&mut self, matrix: [f64; 6]) -> Result<&mut Self, PdfError> {
        self.numbers(&matrix, "cm")
    }

    /// `w`: the line width (8.4.4).
    pub fn line_width(&mut self, width: f64) -> Result<&mut Self, PdfError> {
        self.numbers(&[width], "w")
    }

    /// `rg` and `RG`: a DeviceRGB colour for filling and for stroking
    /// (8.6.8).
    pub fn fill_rgb(&mut self, red: f64, green: f64, blue: f64) -> Result<&mut Self, PdfError> {
        self.numbers(&[red, green, blue], "rg")
    }

    pub fn stroke_rgb(&mut self, red: f64, green: f64, blue: f64) -> Result<&mut Self, PdfError> {
        self.numbers(&[red, green, blue], "RG")
    }

    /// `m`, `l`, `h` and `re`: path construction (8.5.2).
    pub fn move_to(&mut self, x: f64, y: f64) -> Result<&mut Self, PdfError> {
        self.numbers(&[x, y], "m")
    }

    pub fn line_to(&mut self, x: f64, y: f64) -> Result<&mut Self, PdfError> {
        self.numbers(&[x, y], "l")
    }

    pub fn close_path(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "h")
    }

    pub fn rectangle(
        &mut self,
        x: f64,
        y: f64,
        width: f64,
        height: f64,
    ) -> Result<&mut Self, PdfError> {
        self.numbers(&[x, y, width, height], "re")
    }

    /// `f` and `S`: paints the path by filling (nonzero winding) or by
    /// stroking (8.5.3).
    pub fn fill(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "f")
    }

    pub fn stroke(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "S")
    }

    /// `BT` and `ET`: a text object (9.4).
    pub fn begin_text(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "BT")
    }

    pub fn end_text(&mut self) -> Result<&mut Self, PdfError> {
        self.operator(&[], "ET")
    }

    /// `Tf`: the font, a name in the resource dictionary's `Font`, and
    /// its size (9.3).
    pub fn font(&mut self, font: &[u8], size: f64) -> Result<&mut Self, PdfError> {
        self.operator(&[name(font), number(size)?], "Tf")
    }

    /// `Td`: moves to the start of the next line, offset by `(x, y)` from
    /// the start of this one (9.4.2).
    pub fn text_position(&mut self, x: f64, y: f64) -> Result<&mut Self, PdfError> {
        self.numbers(&[x, y], "Td")
    }

    /// `Tj`: shows `codes`, character codes of the current font (9.4.3).
    pub fn show_text(&mut self, codes: &[u8]) -> Result<&mut Self, PdfError> {
        self.operator(&[Object::new(Value::String(PdfString::new(codes)))], "Tj")
    }

    /// `Do`: paints the XObject `xobject`, a name in the resource
    /// dictionary's `XObject` (8.8).
    pub fn xobject(&mut self, xobject: &[u8]) -> Result<&mut Self, PdfError> {
        self.operator(&[name(xobject)], "Do")
    }

    /// The content stream's bytes.
    pub fn finish(self) -> Vec<u8> {
        self.out
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn writes_one_operator_to_a_line() {
        let mut content = ContentBuilder::new();
        content
            .save_state()
            .unwrap()
            .transform([1.0, 0.0, 0.0, 1.0, 72.0, 36.5])
            .unwrap()
            .begin_text()
            .unwrap()
            .font(b"F1", 12.0)
            .unwrap()
            .show_text(b"(a)")
            .unwrap()
            .end_text()
            .unwrap()
            .restore_state()
            .unwrap();
        assert_eq!(
            content.finish(),
            b"q\n1 0 0 1 72 36.5 cm\nBT\n/F1 12 Tf\n(\\(a\\)) Tj\nET\nQ\n"
        );
    }

    #[test]
    fn refuses_what_is_no_operator_or_number() {
        let mut content = ContentBuilder::new();
        assert!(content.operator(&[], "").is_err());
        assert!(content.operator(&[], "a b").is_err());
        assert!(content.operator(&[], "(x)").is_err());
        assert!(number(f64::NAN).is_err());
        assert!(number(f64::INFINITY).is_err());
        assert_eq!(content.finish(), b"");
    }

    #[test]
    fn numbers_take_the_shortest_form() {
        assert_eq!(number(3.0).unwrap().value(), &Value::Integer(3));
        assert_eq!(number(-0.0).unwrap().value(), &Value::Integer(0));
        assert!(matches!(number(0.25).unwrap().value(), Value::Real(_)));
        // Past 2^53 a whole f64 is written as a real.
        assert!(matches!(number(1e300).unwrap().value(), Value::Real(_)));
    }
}
