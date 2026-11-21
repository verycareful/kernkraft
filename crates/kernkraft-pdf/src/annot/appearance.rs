// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The appearance streams of the annotations the library writes: ISO
//! 32000-1:2008, 12.5.5 (an appearance is a form XObject, Table 95, drawn
//! in the annotation's `Rect` by algorithm 8.1), 12.5.6.10 (a highlight
//! multiplies its colour with the page; underline and strikeout draw
//! lines), 12.5.6.13 (ink) and 8.4.5 (Table 58: `CA` and `ca` for the
//! opacity of strokes and fills, `BM` for the blend mode).
//!
//! Each appearance is drawn in user space with its `BBox` equal to the
//! annotation's `Rect` and no `Matrix`, so that the matrix `A` is the
//! identity and the page's numbers are the stream's.

use crate::build::{number, ContentBuilder};
use crate::colour::Rgb;
use crate::syntax::{Dictionary, Name, Object, Value};
use crate::PdfError;

use super::geometry::{Point, Quad};

/// The name the appearance gives its graphics state parameters.
const STATE_NAME: &[u8] = b"GS";

/// The width of the lines of an underline and a strikeout, and of a
/// note icon's outline: the default width of an annotation's border
/// (Table 166: `Border` is `[0 0 1]`).
pub(super) const LINE_WIDTH: f64 = 1.0;

/// A note icon is a square of this many points. The standard gives no
/// size; this is the size readers draw note icons at.
pub(super) const NOTE_SIZE: f64 = 20.0;

/// The lines drawn inside a note icon, as fractions of its size: where
/// each one sits and how far it reaches from the left side.
const NOTE_LINES: [f64; 3] = [0.3, 0.5, 0.7];
const NOTE_LINE_FROM: f64 = 0.2;
const NOTE_LINE_TO: f64 = 0.8;

/// A round line cap and a round line join (Tables 54 and 55).
const ROUND: i64 = 1;

/// What the appearances share: the colour and the opacity.
#[derive(Debug, Clone, Copy)]
pub(super) struct Look {
    pub(super) colour: Rgb,
    pub(super) opacity: f64,
}

/// An appearance stream: its dictionary (without `Length`) and its data.
pub(super) struct Appearance {
    pub(super) dictionary: Dictionary,
    pub(super) data: Vec<u8>,
}

fn name(bytes: &[u8]) -> Object {
    Object::new(Value::Name(Name::new(bytes)))
}

fn numbers(values: &[f64]) -> Result<Object, PdfError> {
    let items = values
        .iter()
        .map(|&value| number(value))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Object::new(Value::Array(items)))
}

fn direct(dictionary: Dictionary) -> Object {
    Object::new(Value::Dictionary(dictionary))
}

fn entry(dictionary: &mut Dictionary, key: &[u8], value: Object) {
    dictionary.insert(Name::new(key), value);
}

/// The form XObject dictionary for an appearance over `rect`, with one
/// graphics state parameter dictionary, `GS`, that sets the blend mode and
/// the opacity of strokes and fills.
fn form(rect: [f64; 4], blend: &[u8], opacity: f64) -> Result<Dictionary, PdfError> {
    let mut state = Dictionary::new();
    entry(&mut state, b"Type", name(b"ExtGState"));
    entry(&mut state, b"BM", name(blend));
    entry(&mut state, b"CA", number(opacity)?);
    entry(&mut state, b"ca", number(opacity)?);
    let mut states = Dictionary::new();
    entry(&mut states, STATE_NAME, direct(state));
    let mut resources = Dictionary::new();
    entry(&mut resources, b"ExtGState", direct(states));

    let mut form = Dictionary::new();
    entry(&mut form, b"Type", name(b"XObject"));
    entry(&mut form, b"Subtype", name(b"Form"));
    entry(&mut form, b"BBox", numbers(&rect)?);
    entry(&mut form, b"Resources", direct(resources));
    Ok(form)
}

/// The corner pairs of a quadrilateral that go round it: top left, top
/// right, bottom right, bottom left.
fn outline(quad: &Quad) -> [Point; 4] {
    [quad[0], quad[1], quad[3], quad[2]]
}

fn midpoint(a: Point, b: Point) -> Point {
    ((a.0 + b.0) / 2.0, (a.1 + b.1) / 2.0)
}

fn finish(
    rect: [f64; 4],
    blend: &[u8],
    look: &Look,
    content: ContentBuilder,
) -> Result<Appearance, PdfError> {
    Ok(Appearance {
        dictionary: form(rect, blend, look.opacity)?,
        data: content.finish(),
    })
}

/// A highlight: every quadrilateral filled with the colour, multiplied
/// with what is below it.
pub(super) fn highlight(
    quads: &[Quad],
    rect: [f64; 4],
    look: &Look,
) -> Result<Appearance, PdfError> {
    let mut content = ContentBuilder::new();
    content.save_state()?.graphics_state(STATE_NAME)?.fill_rgb(
        look.colour.r,
        look.colour.g,
        look.colour.b,
    )?;
    for quad in quads {
        let [first, rest @ ..] = outline(quad);
        content.move_to(first.0, first.1)?;
        for point in rest {
            content.line_to(point.0, point.1)?;
        }
        content.close_path()?.fill()?;
    }
    content.restore_state()?;
    finish(rect, b"Multiply", look, content)
}

/// An underline or a strikeout: one line for each quadrilateral, along its
/// bottom edge, or from the middle of its left edge to the middle of its
/// right one.
pub(super) fn line_markup(
    quads: &[Quad],
    through_middle: bool,
    rect: [f64; 4],
    look: &Look,
) -> Result<Appearance, PdfError> {
    let mut content = ContentBuilder::new();
    content
        .save_state()?
        .graphics_state(STATE_NAME)?
        .stroke_rgb(look.colour.r, look.colour.g, look.colour.b)?
        .line_width(LINE_WIDTH)?;
    for quad in quads {
        let (from, to) = if through_middle {
            (midpoint(quad[0], quad[2]), midpoint(quad[1], quad[3]))
        } else {
            (quad[2], quad[3])
        };
        content
            .move_to(from.0, from.1)?
            .line_to(to.0, to.1)?
            .stroke()?;
    }
    content.restore_state()?;
    finish(rect, b"Normal", look, content)
}

/// Ink: each stroke a polyline of `width` points with round caps and
/// joins. A stroke of one point is drawn as a segment of no length, which
/// a round cap turns into a dot (8.5.3.3).
pub(super) fn ink(
    strokes: &[Vec<Point>],
    width: f64,
    rect: [f64; 4],
    look: &Look,
) -> Result<Appearance, PdfError> {
    let mut content = ContentBuilder::new();
    content
        .save_state()?
        .graphics_state(STATE_NAME)?
        .stroke_rgb(look.colour.r, look.colour.g, look.colour.b)?
        .line_width(width)?
        .line_cap(ROUND)?
        .line_join(ROUND)?;
    for stroke in strokes {
        let Some((first, rest)) = stroke.split_first() else {
            continue;
        };
        content.move_to(first.0, first.1)?;
        if rest.is_empty() {
            content.line_to(first.0, first.1)?;
        }
        for point in rest {
            content.line_to(point.0, point.1)?;
        }
        content.stroke()?;
    }
    content.restore_state()?;
    finish(rect, b"Normal", look, content)
}

/// A note icon: a square of the colour with a black outline and three
/// lines of text in it. `rect` is the icon's [`NOTE_SIZE`] square.
pub(super) fn note(rect: [f64; 4], look: &Look) -> Result<Appearance, PdfError> {
    let [left, bottom, right, top] = rect;
    let (width, height) = (right - left, top - bottom);
    let half = LINE_WIDTH / 2.0;
    let mut content = ContentBuilder::new();
    content
        .save_state()?
        .graphics_state(STATE_NAME)?
        .fill_rgb(look.colour.r, look.colour.g, look.colour.b)?
        .stroke_rgb(0.0, 0.0, 0.0)?
        .line_width(LINE_WIDTH)?
        // The outline is centred on the path, so the path is drawn half
        // a line width inside the box to keep the whole line in it.
        .rectangle(
            left + half,
            bottom + half,
            width - LINE_WIDTH,
            height - LINE_WIDTH,
        )?
        .fill_and_stroke()?;
    for line in NOTE_LINES {
        let y = bottom + height * line;
        content
            .move_to(left + width * NOTE_LINE_FROM, y)?
            .line_to(left + width * NOTE_LINE_TO, y)?
            .stroke()?;
    }
    content.restore_state()?;
    finish(rect, b"Normal", look, content)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    const LOOK: Look = Look {
        colour: Rgb {
            r: 1.0,
            g: 0.5,
            b: 0.0,
        },
        opacity: 0.5,
    };

    fn text(appearance: &Appearance) -> String {
        String::from_utf8(appearance.data.clone()).unwrap()
    }

    fn quad() -> Quad {
        // Top left, top right, bottom left, bottom right of a 10 by 4 box.
        [(0.0, 4.0), (10.0, 4.0), (0.0, 0.0), (10.0, 0.0)]
    }

    #[test]
    fn a_highlight_fills_each_quad_round_in_order() {
        let appearance = highlight(&[quad()], [0.0, 0.0, 10.0, 4.0], &LOOK).unwrap();
        assert_eq!(
            text(&appearance),
            "q\n/GS gs\n1 0.5 0 rg\n0 4 m\n10 4 l\n10 0 l\n0 0 l\nh\nf\nQ\n"
        );
    }

    #[test]
    fn the_form_names_its_box_blend_mode_and_opacity() {
        let appearance = highlight(&[quad()], [0.0, 0.0, 10.0, 4.0], &LOOK).unwrap();
        let dictionary = &appearance.dictionary;
        assert_eq!(
            dictionary.get(b"Subtype").unwrap().value(),
            &Value::Name(Name::new(&b"Form"[..]))
        );
        let Value::Array(bbox) = dictionary.get(b"BBox").unwrap().value() else {
            panic!("BBox is not an array");
        };
        assert_eq!(bbox.len(), 4);
        let Value::Dictionary(resources) = dictionary.get(b"Resources").unwrap().value() else {
            panic!("no Resources");
        };
        let Value::Dictionary(states) = resources.get(b"ExtGState").unwrap().value() else {
            panic!("no ExtGState");
        };
        let Value::Dictionary(state) = states.get(b"GS").unwrap().value() else {
            panic!("no GS");
        };
        assert_eq!(
            state.get(b"BM").unwrap().value(),
            &Value::Name(Name::new(&b"Multiply"[..]))
        );
        assert_eq!(state.get(b"ca"), state.get(b"CA"));
    }

    #[test]
    fn an_underline_runs_along_the_bottom_and_a_strikeout_through_the_middle() {
        let under = line_markup(&[quad()], false, [0.0, 0.0, 10.0, 4.0], &LOOK).unwrap();
        assert_eq!(
            text(&under),
            "q\n/GS gs\n1 0.5 0 RG\n1 w\n0 0 m\n10 0 l\nS\nQ\n"
        );
        let strike = line_markup(&[quad()], true, [0.0, 0.0, 10.0, 4.0], &LOOK).unwrap();
        assert_eq!(
            text(&strike),
            "q\n/GS gs\n1 0.5 0 RG\n1 w\n0 2 m\n10 2 l\nS\nQ\n"
        );
    }

    #[test]
    fn ink_strokes_are_round_and_a_single_point_is_a_dot() {
        let strokes = vec![vec![(1.0, 1.0), (2.0, 3.0)], vec![(5.0, 5.0)]];
        let appearance = ink(&strokes, 2.0, [0.0, 0.0, 9.0, 9.0], &LOOK).unwrap();
        assert_eq!(
            text(&appearance),
            "q\n/GS gs\n1 0.5 0 RG\n2 w\n1 J\n1 j\n1 1 m\n2 3 l\nS\n5 5 m\n5 5 l\nS\nQ\n"
        );
    }

    #[test]
    fn a_note_is_a_filled_square_with_three_lines() {
        let appearance = note([0.0, 0.0, 20.0, 20.0], &LOOK).unwrap();
        let drawn = text(&appearance);
        assert!(drawn.contains("0.5 0.5 19 19 re\nB\n"));
        assert_eq!(drawn.matches(" l\nS\n").count(), NOTE_LINES.len());
    }
}
