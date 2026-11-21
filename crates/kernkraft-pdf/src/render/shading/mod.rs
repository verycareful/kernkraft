// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Shadings: ISO 32000-1:2008, 8.7.4 (shading patterns and the `sh`
//! operator), 8.7.4.3 (the shading dictionary: `ColorSpace`, `Background`,
//! `BBox`), 8.7.4.5.2 to 8.7.4.5.8 (the seven shading types) and 7.10
//! (functions).
//!
//! A [`Shading`] is read once from its dictionary or stream. Painting it
//! gives a pixmap of the page's device pixels over an [`Area`]: each pixel
//! is the shading's colour at the point of shading space the pixel's centre
//! maps back to, and transparent where the shading paints nothing (outside
//! the extent of a gradient that is not extended, outside the triangles of a
//! mesh). The pixmap is then used as the ink of a path (a shading pattern)
//! or drawn through the clip (`sh`).

mod gradient;
mod mesh;

use kernkraft_fs::Progress;
use tiny_skia::{Mask, Pixmap};

use crate::colour::{ColourSpace, Rgb};
use crate::content::{Matrix, DEFAULT_MAX_CONTENT_BYTES};
use crate::diagnostic::{DiagnosticKind, Diagnostics, PaintFault};
use crate::doc::{Content, Document};
use crate::render::canvas::Area;
use crate::render::shape;
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

/// The colours a gradient's function is sampled at: the function of a
/// shading is a smooth curve of one variable, and a table of this many
/// colours along it is finer than any 8-bit step.
const RAMP_SIZE: usize = 1024;

/// The shading types of Table 78.
const FUNCTION_BASED: i64 = 1;
const AXIAL: i64 = 2;
const RADIAL: i64 = 3;
const FREE_FORM: i64 = 4;
const LATTICE: i64 = 5;
const COONS: i64 = 6;
const TENSOR: i64 = 7;

/// The colours of a function along its domain, as a table.
#[derive(Debug, Clone)]
pub(super) struct Ramp {
    domain: [f64; 2],
    colours: Vec<[u8; 3]>,
}

impl Ramp {
    /// The colour at `t`, which is held to the domain.
    pub fn at(&self, t: f64) -> [u8; 3] {
        let [t0, t1] = self.domain;
        let span = t1 - t0;
        let s = if span == 0.0 || !t.is_finite() {
            0.0
        } else {
            ((t - t0) / span).clamp(0.0, 1.0)
        };
        let last = self.colours.len() - 1;
        self.colours[((s * last as f64) + 0.5) as usize]
    }
}

/// The function or functions of a shading: one function with a value for
/// each component, or one function for each component (8.7.4.3).
#[derive(Debug, Clone)]
pub(super) struct Functions(Vec<crate::colour::Function>);

impl Functions {
    fn read(doc: &Document, object: &Object) -> Result<Option<Self>, PdfError> {
        // A function is a dictionary or a stream (which is not resolved
        // like other objects); several are an array.
        let items: Vec<Object> = match object.value() {
            Value::Array(items) => items.clone(),
            Value::Reference(id) => match doc.required(*id)?.object().map(Object::value) {
                Some(Value::Array(items)) => items.clone(),
                _ => vec![object.clone()],
            },
            _ => vec![object.clone()],
        };
        let mut functions = Vec::with_capacity(items.len());
        for item in &items {
            match doc.function(item) {
                Ok(function) => functions.push(function),
                Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
                Err(_) => return Ok(None),
            }
        }
        Ok(Some(Self(functions)))
    }

    /// Whether the functions take `inputs` values and give `outputs`.
    fn fits(&self, inputs: usize, outputs: usize) -> bool {
        let each: usize = self.0.iter().map(|f| f.outputs()).sum();
        !self.0.is_empty()
            && self.0.iter().all(|f| f.inputs() == inputs)
            && each == outputs
            && (self.0.len() == 1 || self.0.iter().all(|f| f.outputs() == 1))
    }

    fn eval(&self, input: &[f64], out: &mut Vec<f64>) -> Result<(), PdfError> {
        out.clear();
        let mut part = Vec::new();
        for function in &self.0 {
            function.eval(input, &mut part)?;
            out.extend_from_slice(&part);
        }
        Ok(())
    }
}

/// A shading, read.
#[derive(Debug, Clone)]
pub(super) struct Shading {
    space: ColourSpace,
    background: Option<[u8; 3]>,
    bbox: Option<[f64; 4]>,
    kind: Kind,
}

#[derive(Debug, Clone)]
enum Kind {
    /// Type 1: a function of two variables over a domain, placed by a
    /// matrix.
    Function {
        domain: [f64; 4],
        matrix: Matrix,
        functions: Functions,
    },
    /// Type 2.
    Axial {
        coords: [f64; 4],
        extend: [bool; 2],
        ramp: Ramp,
    },
    /// Type 3.
    Radial {
        coords: [f64; 6],
        extend: [bool; 2],
        ramp: Ramp,
    },
    /// Types 4 to 7.
    Mesh(mesh::Mesh),
}

pub(in crate::render) fn numbers<const N: usize>(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Option<[f64; N]> {
    let resolved = doc.get(dictionary, key).ok()?;
    let items = resolved.array()?;
    if items.len() != N {
        return None;
    }
    let mut out = [0.0; N];
    for (slot, item) in out.iter_mut().zip(items) {
        *slot = number(doc, item)?;
    }
    Some(out)
}

pub(in crate::render) fn number(doc: &Document, object: &Object) -> Option<f64> {
    match doc.resolve(object).ok()?.value() {
        Value::Integer(n) => Some(*n as f64),
        Value::Real(r) => Some(r.get()),
        _ => None,
    }
}

fn flags(doc: &Document, dictionary: &Dictionary, key: &[u8]) -> [bool; 2] {
    let Ok(resolved) = doc.get(dictionary, key) else {
        return [false; 2];
    };
    let Some(items) = resolved.array() else {
        return [false; 2];
    };
    let flag = |at: usize| {
        items
            .get(at)
            .and_then(|item| doc.resolve(item).ok())
            .is_some_and(|item| matches!(item.value(), Value::Boolean(true)))
    };
    [flag(0), flag(1)]
}

impl Shading {
    /// Reads the shading that `object` is or refers to. A shading that
    /// cannot be used is `None`, with a diagnostic saying why.
    pub fn read(
        doc: &Document,
        object: &Object,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Self>, PdfError> {
        let unusable = |diagnostics: &mut Diagnostics, fault| {
            diagnostics.push(DiagnosticKind::ShadingUnusable { fault }, 0);
            Ok(None)
        };
        let (dictionary, stream_id) = match object.value() {
            Value::Reference(id) => {
                let loaded = doc.required(*id)?;
                match &loaded.content {
                    Content::Stream(stream) => (stream.dictionary.clone(), Some(*id)),
                    Content::Object(inner) => match inner.value() {
                        Value::Dictionary(dictionary) => (dictionary.clone(), None),
                        _ => return unusable(diagnostics, PaintFault::Entry),
                    },
                }
            }
            Value::Dictionary(dictionary) => (dictionary.clone(), None),
            _ => return unusable(diagnostics, PaintFault::Entry),
        };
        let Some(kind) = number_entry(doc, &dictionary, b"ShadingType") else {
            return unusable(diagnostics, PaintFault::Entry);
        };
        let Some(space_object) = dictionary.get(b"ColorSpace") else {
            return unusable(diagnostics, PaintFault::ColourSpace);
        };
        let space = match doc.colour_space(space_object) {
            Ok(space) if !matches!(space, ColourSpace::Pattern(_)) => space,
            Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
            _ => return unusable(diagnostics, PaintFault::ColourSpace),
        };
        let background = match doc.get(&dictionary, b"Background")?.array() {
            Some(items) => {
                let components: Option<Vec<f64>> =
                    items.iter().map(|item| number(doc, item)).collect();
                match components {
                    Some(components) if components.len() == space.components() => {
                        space.to_rgb(&components).ok().map(Rgb::to_bytes)
                    }
                    _ => None,
                }
            }
            None => None,
        };
        let bbox = numbers::<4>(doc, &dictionary, b"BBox")
            .map(|[x0, y0, x1, y1]| [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)]);

        let functions = match dictionary.get(b"Function") {
            Some(object) => match Functions::read(doc, object)? {
                Some(functions) => Some(functions),
                None => return unusable(diagnostics, PaintFault::Function),
            },
            None => None,
        };
        let components = space.components();
        let kind = match kind {
            FUNCTION_BASED => {
                let domain =
                    numbers::<4>(doc, &dictionary, b"Domain").unwrap_or([0.0, 1.0, 0.0, 1.0]);
                let matrix = numbers::<6>(doc, &dictionary, b"Matrix")
                    .map_or(Matrix::IDENTITY, Matrix::from_array);
                let Some(functions) = functions.filter(|f| f.fits(2, components)) else {
                    return unusable(diagnostics, PaintFault::Function);
                };
                if domain[1] < domain[0] || domain[3] < domain[2] || matrix.inverse().is_none() {
                    return unusable(diagnostics, PaintFault::Entry);
                }
                Kind::Function {
                    domain,
                    matrix,
                    functions,
                }
            }
            AXIAL | RADIAL => {
                let wanted = if kind == AXIAL { 4 } else { 6 };
                let coords: Option<Vec<f64>> = doc
                    .get(&dictionary, b"Coords")?
                    .array()
                    .and_then(|items| items.iter().map(|item| number(doc, item)).collect());
                let Some(coords) = coords.filter(|c| c.len() == wanted) else {
                    return unusable(diagnostics, PaintFault::Entry);
                };
                let domain = numbers::<2>(doc, &dictionary, b"Domain").unwrap_or([0.0, 1.0]);
                let Some(functions) = functions.filter(|f| f.fits(1, components)) else {
                    return unusable(diagnostics, PaintFault::Function);
                };
                let ramp = match ramp(&space, &functions, domain) {
                    Ok(ramp) => ramp,
                    Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
                    Err(_) => return unusable(diagnostics, PaintFault::Function),
                };
                let extend = flags(doc, &dictionary, b"Extend");
                if kind == AXIAL {
                    Kind::Axial {
                        coords: [coords[0], coords[1], coords[2], coords[3]],
                        extend,
                        ramp,
                    }
                } else {
                    if coords[2] < 0.0 || coords[5] < 0.0 {
                        return unusable(diagnostics, PaintFault::Entry);
                    }
                    Kind::Radial {
                        coords: [
                            coords[0], coords[1], coords[2], coords[3], coords[4], coords[5],
                        ],
                        extend,
                        ramp,
                    }
                }
            }
            FREE_FORM | LATTICE | COONS | TENSOR => {
                let Some(id) = stream_id else {
                    return unusable(diagnostics, PaintFault::Entry);
                };
                let loaded = doc.required(id)?;
                let Some(stream) = loaded.stream() else {
                    return unusable(diagnostics, PaintFault::Entry);
                };
                let data = match doc.decode_stream(id, stream, DEFAULT_MAX_CONTENT_BYTES) {
                    Ok(decoded) => decoded.data.into_owned(),
                    Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
                    Err(_) => return unusable(diagnostics, PaintFault::Data),
                };
                let Some(mesh) = mesh::Mesh::read(
                    doc,
                    &dictionary,
                    kind,
                    &data,
                    components,
                    functions.as_ref(),
                    diagnostics,
                ) else {
                    return unusable(diagnostics, PaintFault::Entry);
                };
                let ramp = match &functions {
                    Some(functions) => match ramp(&space, functions, mesh.parametric_domain()) {
                        Ok(ramp) => Some(ramp),
                        Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
                        Err(_) => return unusable(diagnostics, PaintFault::Function),
                    },
                    None => None,
                };
                Kind::Mesh(mesh.with_ramp(ramp))
            }
            _ => return unusable(diagnostics, PaintFault::Type),
        };
        Ok(Some(Self {
            space,
            background,
            bbox,
            kind,
        }))
    }

    /// The pixels of the shading over `area`, with `to_device` taking its
    /// space to the page's device pixels. `with_background` fills where the
    /// shading paints nothing with its `Background`, as a shading pattern
    /// does and `sh` does not (8.7.4.3).
    pub fn paint(
        &self,
        area: Area,
        to_device: &Matrix,
        with_background: bool,
        progress: &dyn Progress,
    ) -> Result<Pixmap, PdfError> {
        let mut pixmap = Pixmap::new(area.width, area.height).ok_or_else(|| {
            PdfError::Invalid(format!(
                "a shading over {} by {} pixels",
                area.width, area.height
            ))
        })?;
        let Some(inverse) = to_device.inverse() else {
            return Ok(pixmap);
        };
        if let (true, Some([r, g, b])) = (with_background, self.background) {
            pixmap.fill(tiny_skia::Color::from_rgba8(r, g, b, 255));
        }
        let target = Target {
            area,
            to_device,
            inverse: &inverse,
            progress,
        };
        match &self.kind {
            Kind::Function {
                domain,
                matrix,
                functions,
            } => gradient::function_based(
                &mut pixmap,
                &target,
                &self.space,
                *domain,
                matrix,
                functions,
            )?,
            Kind::Axial {
                coords,
                extend,
                ramp,
            } => gradient::axial(&mut pixmap, &target, *coords, *extend, ramp)?,
            Kind::Radial {
                coords,
                extend,
                ramp,
            } => gradient::radial(&mut pixmap, &target, *coords, *extend, ramp)?,
            Kind::Mesh(mesh) => mesh.paint(&mut pixmap, &target, &self.space)?,
        }
        if let Some(bbox) = self.bbox {
            clip_to(&mut pixmap, area, to_device, bbox);
        }
        Ok(pixmap)
    }
}

/// Where a shading is painted: the pixels, and how they map back to the
/// shading's space.
pub(super) struct Target<'a> {
    pub area: Area,
    pub to_device: &'a Matrix,
    pub inverse: &'a Matrix,
    pub progress: &'a dyn Progress,
}

impl Target<'_> {
    /// The point of shading space under the centre of pixel `(x, y)` of
    /// the pixmap.
    pub fn point(&self, x: u32, y: u32) -> (f64, f64) {
        self.inverse.apply(
            f64::from(self.area.x) + f64::from(x) + 0.5,
            f64::from(self.area.y) + f64::from(y) + 0.5,
        )
    }

    pub fn check(&self) -> Result<(), PdfError> {
        if self.progress.is_cancelled() {
            Err(PdfError::Cancelled)
        } else {
            Ok(())
        }
    }
}

/// An opaque pixel, premultiplied.
pub(super) fn opaque([r, g, b]: [u8; 3]) -> [u8; 4] {
    [r, g, b, 255]
}

/// The integer entry `key`.
pub(in crate::render) fn number_entry(
    doc: &Document,
    dictionary: &Dictionary,
    key: &[u8],
) -> Option<i64> {
    match doc.get(dictionary, key).ok()?.value() {
        Value::Integer(n) => Some(*n),
        Value::Real(r) if r.get().fract() == 0.0 => Some(r.get() as i64),
        _ => None,
    }
}

/// The colours of `functions` over `domain`.
fn ramp(space: &ColourSpace, functions: &Functions, domain: [f64; 2]) -> Result<Ramp, PdfError> {
    let mut colours = Vec::with_capacity(RAMP_SIZE);
    let mut values = Vec::new();
    for step in 0..RAMP_SIZE {
        let s = step as f64 / (RAMP_SIZE - 1) as f64;
        functions.eval(&[domain[0] + s * (domain[1] - domain[0])], &mut values)?;
        colours.push(space.to_rgb(&values)?.to_bytes());
    }
    Ok(Ramp { domain, colours })
}

/// Clears the pixels outside `bbox`, a rectangle of the shading's space.
fn clip_to(pixmap: &mut Pixmap, area: Area, to_device: &Matrix, bbox: [f64; 4]) {
    let [x0, y0, x1, y1] = bbox;
    let mut builder = tiny_skia::PathBuilder::new();
    builder.move_to(x0 as f32, y0 as f32);
    builder.line_to(x1 as f32, y0 as f32);
    builder.line_to(x1 as f32, y1 as f32);
    builder.line_to(x0 as f32, y1 as f32);
    builder.close();
    let (Some(path), Some(ts)) = (
        builder.finish(),
        shape::transform(
            &to_device.then(Matrix::translation(-f64::from(area.x), -f64::from(area.y))),
        ),
    ) else {
        pixmap.fill(tiny_skia::Color::TRANSPARENT);
        return;
    };
    let Some(mut mask) = Mask::new(area.width, area.height) else {
        return;
    };
    mask.fill_path(&path, tiny_skia::FillRule::Winding, true, ts);
    pixmap.apply_mask(&mask);
}

/// The object a shading is, for a pattern that names it: used to key the
/// shadings already read.
pub(super) fn object_id(object: &Object) -> Option<ObjectId> {
    match object.value() {
        Value::Reference(id) => Some(*id),
        _ => None,
    }
}
