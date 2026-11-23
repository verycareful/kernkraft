// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The bytes that show the new text, and the objects they need: ISO
//! 32000-1:2008, 8.4.2 (`q` and `Q`), 8.6 (colour operators), 9.3
//! (text state operators), 9.4.2 (`Tm`) and 7.8.3 (resource dictionaries).
//!
//! The new text goes right after the `ET` of the block's last text object,
//! in a `q ... Q` of its own with its own text object and every parameter
//! it uses set, so that nothing it sets reaches the objects after it and
//! nothing the stream had set before is assumed.

use crate::build::{number, ContentBuilder};
use crate::colour::Rgb;
use crate::content::{ColorSpace, GraphicsState, Matrix, Paint, RenderMode};
use crate::pages::rewrite::{direct, name_object, reference_object, Rewriter};
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

/// The number a `Tr` operator takes for a render mode (Table 106).
fn render_code(mode: RenderMode) -> i64 {
    match mode {
        RenderMode::Fill => 0,
        RenderMode::Stroke => 1,
        RenderMode::FillStroke => 2,
        RenderMode::Invisible => 3,
        RenderMode::FillClip => 4,
        RenderMode::StrokeClip => 5,
        RenderMode::FillStrokeClip => 6,
        RenderMode::Clip => 7,
    }
}

fn numbers(values: &[f64]) -> Result<Vec<Object>, PdfError> {
    values.iter().map(|&value| number(value)).collect()
}

/// The operators that make `paint` the stroking (`stroke`) or non-stroking
/// colour (8.6.8, Table 74).
fn set_paint(out: &mut ContentBuilder, paint: &Paint, stroke: bool) -> Result<(), PdfError> {
    let pick = |lower: &'static str, upper: &'static str| if stroke { upper } else { lower };
    match (&paint.space, &paint.pattern) {
        (ColorSpace::DeviceGray, None) => {
            out.operator(&numbers(&paint.components)?, pick("g", "G"))?;
        }
        (ColorSpace::DeviceRGB, None) => {
            out.operator(&numbers(&paint.components)?, pick("rg", "RG"))?;
        }
        (ColorSpace::DeviceCMYK, None) => {
            out.operator(&numbers(&paint.components)?, pick("k", "K"))?;
        }
        (ColorSpace::Resource { name, .. }, None) => {
            out.operator(&[name_object(name.as_bytes())], pick("cs", "CS"))?;
            out.operator(&numbers(&paint.components)?, pick("scn", "SCN"))?;
        }
        (space, Some((pattern, _))) => {
            match space {
                ColorSpace::Resource { name, .. } => {
                    out.operator(&[name_object(name.as_bytes())], pick("cs", "CS"))?;
                }
                _ => {
                    out.operator(&[name_object(b"Pattern")], pick("cs", "CS"))?;
                }
            }
            let mut operands = numbers(&paint.components)?;
            operands.push(name_object(pattern.as_bytes()));
            out.operator(&operands, pick("scn", "SCN"))?;
        }
        (ColorSpace::Pattern, None) => {
            return Err(PdfError::Invalid(
                "the text to edit has a pattern colour with no pattern".to_owned(),
            ));
        }
    }
    Ok(())
}

/// What the new text is set with.
pub(super) struct Setting<'a> {
    /// The resource name of the font.
    pub font: &'a Name,
    pub size: f64,
    /// The state of the block's first glyph: spacing, scaling, rise,
    /// render mode and colours.
    pub state: &'a GraphicsState,
    /// A colour that replaces the fill colour (and the stroke colour when
    /// the render mode strokes).
    pub colour: Option<Rgb>,
    /// The text matrix of the first line, then the move from one line to
    /// the next in text space.
    pub first: Matrix,
    pub pitch: f64,
    /// A matrix to concatenate to the CTM first, when the CTM where the
    /// text goes is not the one the block was shown under.
    pub cm: Option<Matrix>,
}

/// The bytes of a string and where it starts on its line.
pub(super) struct Piece {
    /// The distance from the first glyph's origin along the line, in text
    /// space units.
    pub left: f64,
    pub codes: Vec<u8>,
}

/// The `q ... Q` that shows `lines`, each the pieces of one line.
pub(super) fn show(setting: &Setting<'_>, lines: &[Vec<Piece>]) -> Result<Vec<u8>, PdfError> {
    let text = &setting.state.text;
    let mut out = ContentBuilder::new();
    out.save_state()?;
    if let Some(cm) = setting.cm {
        out.transform(cm.to_array())?;
    }
    let fill = |out: &mut ContentBuilder| match setting.colour {
        Some(colour) => out.fill_rgb(colour.r, colour.g, colour.b).map(|_| ()),
        None => set_paint(out, &setting.state.fill, false),
    };
    let stroke = |out: &mut ContentBuilder| match setting.colour {
        Some(colour) => out.stroke_rgb(colour.r, colour.g, colour.b).map(|_| ()),
        None => set_paint(out, &setting.state.stroke, true),
    };
    fill(&mut out)?;
    if text.render_mode.strokes() {
        stroke(&mut out)?;
    }
    out.begin_text()?;
    out.font(setting.font.as_bytes(), setting.size)?;
    out.operator(&numbers(&[text.char_spacing])?, "Tc")?;
    out.operator(&numbers(&[text.word_spacing])?, "Tw")?;
    out.operator(&numbers(&[text.horizontal_scaling * 100.0])?, "Tz")?;
    out.operator(
        &[direct(Value::Integer(render_code(text.render_mode)))],
        "Tr",
    )?;
    out.operator(&numbers(&[text.rise])?, "Ts")?;
    for (index, pieces) in lines.iter().enumerate() {
        for piece in pieces.iter().filter(|piece| !piece.codes.is_empty()) {
            let at =
                Matrix::translation(piece.left, -setting.pitch * index as f64).then(setting.first);
            out.operator(&numbers(&at.to_array())?, "Tm")?;
            out.show_text(&piece.codes)?;
        }
    }
    out.end_text()?;
    out.restore_state()?;
    Ok(out.finish())
}

/// `resources` with `object` added to its `category` (`Font`, `XObject`)
/// under a name no entry of the category has: `prefix` and the least
/// number that is free. The dictionary the page or form has is not changed;
/// what is given back is a copy of its entries with the one more, for the
/// page or form to hold as its own. Gives the copy and the name.
pub(super) fn with_resource(
    rewriter: &Rewriter<'_>,
    resources: &Dictionary,
    category: &[u8],
    prefix: &str,
    object: ObjectId,
) -> Result<(Dictionary, Name), PdfError> {
    let mut entries = match resources.get(category).map(Object::value) {
        Some(Value::Dictionary(entries)) => entries.clone(),
        Some(Value::Reference(id)) => rewriter.read_dictionary(*id)?.unwrap_or_default(),
        _ => Dictionary::new(),
    };
    let mut number = 1u32;
    let name = loop {
        let candidate = Name::new(format!("{prefix}{number}").into_bytes());
        if entries.get(candidate.as_bytes()).is_none() {
            break candidate;
        }
        number += 1;
    };
    entries.insert(name.clone(), reference_object(object));
    let mut copy = resources.clone();
    copy.insert(Name::new(category), direct(Value::Dictionary(entries)));
    Ok((copy, name))
}
