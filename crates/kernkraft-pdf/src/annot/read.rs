// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading the annotations of a page: ISO 32000-1:2008, 12.5.2 (Table 164),
//! 12.5.6.10 (Table 179: `QuadPoints`), 12.5.6.13 (Table 184: `InkList`).
//!
//! [`Parsed`] holds an annotation in user space, as the file has it. The
//! listing turns it into fractions of the displayed page; an edit works
//! from it directly.

use crate::colour::{ColourSpace, Rgb};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{decode_text_string, Date, Document};
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::{PdfError, Rect};

use super::geometry::{Geometry, Point, Quad};
use super::{Annotation, AnnotationKind, FLAG_NO_ROTATE};

pub(super) const ANNOTS_KEY: &[u8] = b"Annots";
pub(super) const SUBTYPE_KEY: &[u8] = b"Subtype";
pub(super) const RECT_KEY: &[u8] = b"Rect";
pub(super) const QUAD_POINTS_KEY: &[u8] = b"QuadPoints";
pub(super) const INK_LIST_KEY: &[u8] = b"InkList";
const CONTENTS_KEY: &[u8] = b"Contents";
const AUTHOR_KEY: &[u8] = b"T";
const MODIFIED_KEY: &[u8] = b"M";
const COLOUR_KEY: &[u8] = b"C";
const OPACITY_KEY: &[u8] = b"CA";
const FLAGS_KEY: &[u8] = b"F";

/// A quadrilateral in `QuadPoints` is eight numbers (Table 179).
const QUAD_NUMBERS: usize = 8;

/// The component counts of `C` (Table 164): none is transparent, then
/// gray, RGB and CMYK.
const GRAY: usize = 1;
const RGB: usize = 3;
const CMYK: usize = 4;

/// An annotation in user space, as the file holds it.
pub(super) struct Parsed {
    pub(super) kind: AnnotationKind,
    /// `[llx, lly, urx, ury]`.
    pub(super) rect: [f64; 4],
    pub(super) quads: Vec<Quad>,
    pub(super) strokes: Vec<Vec<Point>>,
    pub(super) contents: Option<String>,
    pub(super) author: Option<String>,
    pub(super) modified: Option<Date>,
    pub(super) colour: Option<Rgb>,
    pub(super) opacity: f64,
    pub(super) flags: i64,
}

pub(super) fn kind_of(subtype: &[u8]) -> AnnotationKind {
    match subtype {
        b"Highlight" => AnnotationKind::Highlight,
        b"Underline" => AnnotationKind::Underline,
        b"StrikeOut" => AnnotationKind::StrikeOut,
        b"Ink" => AnnotationKind::Ink,
        b"Text" => AnnotationKind::Note,
        other => AnnotationKind::Other(String::from_utf8_lossy(other).into_owned()),
    }
}

fn number(document: &Document, object: &Object) -> Result<Option<f64>, PdfError> {
    Ok(match document.resolve(object)?.value() {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    })
}

/// The array `key` of `dictionary` as numbers; `None` when it is absent
/// or not an array of numbers.
pub(super) fn numbers(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<Vec<f64>>, PdfError> {
    let resolved = document.get(dictionary, key)?;
    let Some(items) = resolved.array() else {
        return Ok(None);
    };
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        match number(document, item)? {
            Some(value) => out.push(value),
            None => return Ok(None),
        }
    }
    Ok(Some(out))
}

fn text_entry(
    document: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Result<Option<String>, PdfError> {
    Ok(match document.get(dictionary, key)?.value() {
        Value::String(string) => Some(decode_text_string(string.as_bytes()).text),
        _ => None,
    })
}

/// `C`: gray, RGB or CMYK as sRGB; `None` when it is empty (transparent)
/// or not a colour.
fn colour_of(document: &Document, dictionary: &Dictionary) -> Result<Option<Rgb>, PdfError> {
    let Some(components) = numbers(document, dictionary, COLOUR_KEY)? else {
        return Ok(None);
    };
    let space = match components.len() {
        GRAY => ColourSpace::DeviceGray,
        RGB => ColourSpace::DeviceRgb,
        CMYK => ColourSpace::DeviceCmyk,
        _ => return Ok(None),
    };
    space.to_rgb(&components).map(Some)
}

/// Reads `dictionary`. `None` when it has no `Rect` of four numbers.
/// A malformed `QuadPoints` or `InkList` is left out and reported.
pub(super) fn parse(
    document: &Document,
    dictionary: &Dictionary,
    diagnostics: &mut Diagnostics,
    offset: u64,
) -> Result<Option<Parsed>, PdfError> {
    let Value::Name(subtype) = document.get(dictionary, SUBTYPE_KEY)?.value().clone() else {
        return Ok(None);
    };
    let Some(&[x0, y0, x1, y1]) = numbers(document, dictionary, RECT_KEY)?
        .as_deref()
        .filter(|rect| rect.len() == 4)
    else {
        return Ok(None);
    };
    let ignored = |diagnostics: &mut Diagnostics, key: &[u8]| {
        diagnostics.push(
            DiagnosticKind::IgnoredEntry {
                key: Name::new(key),
            },
            offset,
        );
    };

    let mut quads = Vec::new();
    if let Some(values) = numbers(document, dictionary, QUAD_POINTS_KEY)? {
        let (chunks, rest) = values.as_chunks::<QUAD_NUMBERS>();
        if rest.is_empty() {
            quads = chunks
                .iter()
                .map(|q| [(q[0], q[1]), (q[2], q[3]), (q[4], q[5]), (q[6], q[7])])
                .collect();
        } else {
            ignored(diagnostics, QUAD_POINTS_KEY);
        }
    }

    let mut strokes = Vec::new();
    if let Some(list) = document.get(dictionary, INK_LIST_KEY)?.array() {
        for item in list {
            let resolved = document.resolve(item)?;
            let points = resolved.array().and_then(|items| {
                let mut values = Vec::with_capacity(items.len());
                for item in items {
                    values.push(number(document, item).ok().flatten()?);
                }
                Some(values)
            });
            match points {
                Some(values) if values.len() % 2 == 0 => {
                    let (pairs, _) = values.as_chunks::<2>();
                    strokes.push(pairs.iter().map(|pair| (pair[0], pair[1])).collect());
                }
                _ => ignored(diagnostics, INK_LIST_KEY),
            }
        }
    }

    let opacity = match document.get(dictionary, OPACITY_KEY)?.value() {
        Value::Integer(value) => *value as f64,
        Value::Real(value) => value.get(),
        _ => 1.0,
    }
    .clamp(0.0, 1.0);
    let flags = match document.get(dictionary, FLAGS_KEY)?.value() {
        Value::Integer(flags) => *flags,
        _ => 0,
    };
    let modified = match document.get(dictionary, MODIFIED_KEY)?.value() {
        Value::String(string) => Date::parse(&decode_text_string(string.as_bytes()).text),
        _ => None,
    };
    Ok(Some(Parsed {
        kind: kind_of(subtype.as_bytes()),
        rect: [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)],
        quads,
        strokes,
        contents: text_entry(document, dictionary, CONTENTS_KEY)?,
        author: text_entry(document, dictionary, AUTHOR_KEY)?,
        modified,
        colour: colour_of(document, dictionary)?,
        opacity,
        flags,
    }))
}

impl Parsed {
    /// Where the annotation shows, as fractions of the displayed page. An
    /// annotation that keeps upright (`NoRotate`) is drawn from the
    /// upper-left corner of its `Rect` as the page lies in user space,
    /// growing right and down, so that is how its box is taken
    /// (Table 165), at one point to the point.
    pub(super) fn displayed_rect(&self, geometry: &Geometry) -> Rect {
        let [llx, lly, urx, ury] = self.rect;
        if self.flags & FLAG_NO_ROTATE != 0 {
            let (x, y) = geometry.view.matrix.apply(llx, ury);
            let (width, height) = (urx - llx, ury - lly);
            let (left, top) = geometry.view.fraction(x, y);
            let (right, bottom) = geometry.view.fraction(x + width, y + height);
            return Rect {
                left,
                top,
                right,
                bottom,
            };
        }
        geometry
            .rect_around([(llx, lly), (urx, lly), (urx, ury), (llx, ury)])
            .unwrap_or_default()
    }
}

impl Document {
    /// The annotations of page `number`, counted from 1, in the order the
    /// page's `Annots` lists them. An annotation with no `Rect` of four
    /// numbers is left out and reported.
    pub fn annotations(&self, number: u32) -> Result<Vec<Annotation>, PdfError> {
        let page = self.page(number)?;
        let mut diagnostics = Diagnostics::new();
        diagnostics.set_object(Some(page.id));
        let geometry = Geometry::of(&page, &mut diagnostics)?;
        let annots = self.get(&page.dictionary, ANNOTS_KEY)?;
        let items = annots.array().map(<[Object]>::to_vec).unwrap_or_default();
        let mut out = Vec::new();
        for item in &items {
            let id = match item.value() {
                Value::Reference(id) => Some(*id),
                _ => None,
            };
            let resolved = match self.resolve(item) {
                Ok(resolved) => resolved,
                // A reference to a stream is no annotation.
                Err(PdfError::WrongKind { .. }) => continue,
                Err(error) => return Err(error),
            };
            let Some(dictionary) = resolved.dictionary() else {
                continue;
            };
            diagnostics.set_object(id.or(Some(page.id)));
            let offset = self.offset_of(id);
            let Some(parsed) = parse(self, dictionary, &mut diagnostics, offset)? else {
                diagnostics.push(
                    DiagnosticKind::IgnoredEntry {
                        key: Name::new(RECT_KEY),
                    },
                    offset,
                );
                continue;
            };
            let line_width = if parsed.kind == AnnotationKind::Ink {
                Some(self.border_width(dictionary)?)
            } else {
                None
            };
            let rect = parsed.displayed_rect(&geometry);
            let quads = parsed
                .quads
                .iter()
                .filter_map(|quad| geometry.rect_around(quad.iter().copied()))
                .collect();
            let strokes = parsed
                .strokes
                .iter()
                .map(|stroke| {
                    stroke
                        .iter()
                        .map(|&point| geometry.fractions(point))
                        .collect()
                })
                .collect();
            out.push(Annotation {
                id,
                kind: parsed.kind,
                rect,
                quads,
                strokes,
                contents: parsed.contents,
                author: parsed.author,
                modified: parsed.modified,
                colour: parsed.colour,
                opacity: parsed.opacity,
                line_width,
                flags: parsed.flags,
            });
        }
        diagnostics.set_object(None);
        self.record_diagnostics(diagnostics);
        Ok(out)
    }

    /// The object a page lists in its `Annots` as annotation `id`: true
    /// when the page's `Annots` has a reference to it.
    pub(super) fn lists_annotation(
        &self,
        page: &Dictionary,
        id: ObjectId,
    ) -> Result<bool, PdfError> {
        let annots = self.get(page, ANNOTS_KEY)?;
        Ok(annots
            .array()
            .unwrap_or_default()
            .iter()
            .any(|item| matches!(item.value(), Value::Reference(found) if *found == id)))
    }
}
