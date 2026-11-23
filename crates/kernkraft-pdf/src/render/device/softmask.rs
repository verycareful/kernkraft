// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Soft masks: ISO 32000-1:2008, 11.6.5 (soft-mask dictionaries, Table
//! 144: `S`, `G`, `BC`, `TR`) and 11.6.5.2 (the mask values).
//!
//! The mask's group is run when the `gs` operator sets it, in the
//! coordinate system of that moment, and kept as coverage over the layer
//! being painted. A luminosity mask is the group composed on its backdrop
//! colour `BC` (black when absent), each pixel's luminosity being its value;
//! an alpha mask is the group's own alpha. The transfer function `TR` maps
//! each value. What the group does not reach has the value of the empty
//! backdrop.

use tiny_skia::{Color, Mask};

use crate::colour::ColourSpace;
use crate::content::{Context, ExtGStateInfo, GraphicsState, Matrix, SoftMask};
use crate::diagnostic::{DiagnosticKind, PaintFault};
use crate::doc::Content;
use crate::render::canvas::{Area, Canvas};
use crate::syntax::{Object, Value};
use crate::PdfError;

use super::{Painter, Run};

/// The weights of the red, green and blue of a colour in its gray, which
/// 8.6.4.2 gives for converting DeviceRGB to DeviceGray.
const LUMA: [f64; 3] = [0.30, 0.59, 0.11];

impl Painter<'_, '_> {
    /// `gs` ran: when the dictionary has an `SMask`, the state's soft mask
    /// is the one now.
    pub(super) fn apply_soft_mask(&mut self, ctx: &Context<'_>, info: &ExtGStateInfo) {
        if info.dictionary.get(b"SMask").is_none() {
            return;
        }
        let state = ctx.state;
        let SoftMask::Dictionary(object) = &state.soft_mask else {
            self.canvas.set_soft_mask(None);
            return;
        };
        match self.build_soft_mask(state, object) {
            Ok(mask) => self.canvas.set_soft_mask(mask),
            Err(error) => {
                self.error.get_or_insert(error);
            }
        }
    }

    fn soft_mask_fault(&mut self, fault: PaintFault) -> Result<Option<Mask>, PdfError> {
        self.diagnostics
            .push(DiagnosticKind::SoftMaskUnusable { fault }, 0);
        Ok(None)
    }

    fn build_soft_mask(
        &mut self,
        state: &GraphicsState,
        object: &Object,
    ) -> Result<Option<Mask>, PdfError> {
        let doc = self.doc;
        let resolved = doc.resolve(object)?;
        let Some(dictionary) = resolved.dictionary() else {
            return self.soft_mask_fault(PaintFault::Entry);
        };
        let luminosity = match doc.get(dictionary, b"S")?.value() {
            Value::Name(name) if name.as_bytes() == b"Luminosity" => true,
            Value::Name(name) if name.as_bytes() == b"Alpha" => false,
            _ => return self.soft_mask_fault(PaintFault::Entry),
        };
        let Some(Value::Reference(group_id)) = dictionary.get(b"G").map(Object::value) else {
            return self.soft_mask_fault(PaintFault::Entry);
        };
        let group_id = *group_id;
        let group = doc.required(group_id)?;
        let Content::Stream(stream) = &group.content else {
            return self.soft_mask_fault(PaintFault::Entry);
        };
        let group_dictionary = &stream.dictionary;

        // The group's own space gives the backdrop its colour.
        let space = match doc.get(group_dictionary, b"Group")?.dictionary() {
            Some(attributes) => match attributes.get(b"CS") {
                Some(space) => doc.colour_space(space).ok(),
                None => None,
            },
            None => None,
        };
        let backdrop = if luminosity {
            match backdrop_colour(doc, dictionary, space.as_ref()) {
                Ok(colour) => Some(colour),
                Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
                Err(_) => return self.soft_mask_fault(PaintFault::ColourSpace),
            }
        } else {
            None
        };
        let table = match transfer(doc, dictionary) {
            Ok(table) => table,
            Err(PdfError::Cancelled) => return Err(PdfError::Cancelled),
            Err(_) => return self.soft_mask_fault(PaintFault::Function),
        };

        let layer = self.canvas.bounds();
        let form_matrix = match doc.get(group_dictionary, b"Matrix")?.array() {
            Some(items) if items.len() == 6 => {
                let numbers: Option<Vec<f64>> = items
                    .iter()
                    .map(|item| match doc.resolve(item).ok()?.value() {
                        Value::Integer(n) => Some(*n as f64),
                        Value::Real(r) => Some(r.get()),
                        _ => None,
                    })
                    .collect();
                numbers
                    .and_then(|n| <[f64; 6]>::try_from(n).ok())
                    .map_or(Matrix::IDENTITY, Matrix::from_array)
            }
            _ => Matrix::IDENTITY,
        };
        let reach = form_area(doc, group_dictionary, &form_matrix, &state.ctm, layer);

        // The value outside the group's reach: the backdrop's.
        let outside = match backdrop {
            Some([r, g, b]) => table[luma([r, g, b])],
            None => table[0],
        };
        let mut mask = Mask::new(layer.width, layer.height)
            .ok_or_else(|| PdfError::Invalid("a soft mask over no pixels".into()))?;
        mask.data_mut().fill(outside);
        let Some(area) = reach else {
            return Ok(Some(mask));
        };

        let Some(offscreen) = Canvas::offscreen(
            area,
            backdrop.map(|[r, g, b]| Color::from_rgba8(r, g, b, 255)),
        ) else {
            return self.soft_mask_fault(PaintFault::Size);
        };
        let outer_canvas = std::mem::replace(&mut self.canvas, offscreen);
        let outer_run = std::mem::replace(&mut self.run, Run::new(state.ctm));
        let ran = self.sub_run(|painter, interpreter| {
            let progress = painter.progress;
            interpreter.run_form(group_id, state.ctm, painter, progress)
        });
        let offscreen = std::mem::replace(&mut self.canvas, outer_canvas);
        self.run = outer_run;
        ran?;

        let pixmap = offscreen.into_pixmap();
        let (columns, ox, oy) = (
            area.width as usize,
            (area.x - layer.x) as usize,
            (area.y - layer.y) as usize,
        );
        let stride = layer.width as usize;
        let data = mask.data_mut();
        for (row, pixels) in pixmap.data().chunks_exact(columns * 4).enumerate() {
            let line = &mut data[(oy + row) * stride + ox..][..columns];
            for (slot, pixel) in line.iter_mut().zip(pixels.as_chunks::<4>().0) {
                let value = if luminosity {
                    // The pixel is opaque: the backdrop is under it.
                    luma([pixel[0], pixel[1], pixel[2]])
                } else {
                    usize::from(pixel[3])
                };
                *slot = table[value];
            }
        }
        Ok(Some(mask))
    }
}

/// The luminosity of an sRGB colour, 0 to 255.
fn luma([r, g, b]: [u8; 3]) -> usize {
    let value = LUMA[0] * f64::from(r) + LUMA[1] * f64::from(g) + LUMA[2] * f64::from(b);
    (value.round() as usize).min(255)
}

/// The backdrop colour `BC` in the group's colour space (the space's
/// initial colour, black, when `BC` is absent).
fn backdrop_colour(
    doc: &crate::doc::Document,
    dictionary: &crate::syntax::Dictionary,
    space: Option<&ColourSpace>,
) -> Result<[u8; 3], PdfError> {
    let components: Option<Vec<f64>> = doc.get(dictionary, b"BC")?.array().map(|items| {
        items
            .iter()
            .filter_map(|item| match doc.resolve(item).ok()?.value() {
                Value::Integer(n) => Some(*n as f64),
                Value::Real(r) => Some(r.get()),
                _ => None,
            })
            .collect()
    });
    let space = match (space, &components) {
        (Some(space), _) => space.clone(),
        (None, Some(components)) => match components.len() {
            3 => ColourSpace::DeviceRgb,
            4 => doc.device_cmyk()?,
            _ => ColourSpace::DeviceGray,
        },
        (None, None) => ColourSpace::DeviceGray,
    };
    let components = components.unwrap_or_else(|| space.initial_colour());
    Ok(space.to_rgb(&components)?.to_bytes())
}

/// The transfer function `TR` as a table over the values 0 to 255:
/// `Identity` (or no entry) maps each to itself.
fn transfer(
    doc: &crate::doc::Document,
    dictionary: &crate::syntax::Dictionary,
) -> Result<[u8; 256], PdfError> {
    let mut table = [0u8; 256];
    for (at, slot) in table.iter_mut().enumerate() {
        *slot = at as u8;
    }
    let Some(entry) = dictionary.get(b"TR") else {
        return Ok(table);
    };
    if matches!(doc.resolve(entry)?.value(), Value::Name(name) if name.as_bytes() == b"Identity") {
        return Ok(table);
    }
    let function = doc.function(entry)?;
    for (at, slot) in table.iter_mut().enumerate() {
        let out = function.call(&[at as f64 / 255.0])?;
        let value = out.first().copied().unwrap_or(0.0).clamp(0.0, 1.0);
        *slot = (value * 255.0).round() as u8;
    }
    Ok(table)
}

/// The pixels of the layer that the group's `BBox` covers.
fn form_area(
    doc: &crate::doc::Document,
    dictionary: &crate::syntax::Dictionary,
    form_matrix: &Matrix,
    ctm: &Matrix,
    layer: Area,
) -> Option<Area> {
    let items = doc.get(dictionary, b"BBox").ok()?.array()?.to_vec();
    let numbers: Vec<f64> = items
        .iter()
        .filter_map(|item| match doc.resolve(item).ok()?.value() {
            Value::Integer(n) => Some(*n as f64),
            Value::Real(r) => Some(r.get()),
            _ => None,
        })
        .collect();
    let [x0, y0, x1, y1] = <[f64; 4]>::try_from(numbers).ok()?;
    let to_device = form_matrix.then(*ctm);
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| to_device.apply(x, y));
    let fold = |pick: fn(&(f64, f64)) -> f64, init: f64, f: fn(f64, f64) -> f64| {
        corners.iter().map(pick).fold(init, f)
    };
    let (left, top) = (
        fold(|c| c.0, f64::MAX, f64::min),
        fold(|c| c.1, f64::MAX, f64::min),
    );
    let (right, bottom) = (
        fold(|c| c.0, f64::MIN, f64::max),
        fold(|c| c.1, f64::MIN, f64::max),
    );
    if ![left, top, right, bottom].iter().all(|n| n.is_finite()) {
        return Some(layer);
    }
    let clamp = |n: f64| n.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i64;
    let (x, y) = (clamp(left.floor() - 1.0), clamp(top.floor() - 1.0));
    Area {
        x: x as i32,
        y: y as i32,
        width: (clamp(right.ceil() + 1.0) - x).clamp(0, i64::from(u32::MAX)) as u32,
        height: (clamp(bottom.ceil() + 1.0) - y).clamp(0, i64::from(u32::MAX)) as u32,
    }
    .intersect(layer)
}
