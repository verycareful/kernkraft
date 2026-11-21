// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Patterns: ISO 32000-1:2008, 8.7.2 (the pattern dictionary and the
//! pattern matrix), 8.7.3 (tiling patterns: `PaintType`, `BBox`, `XStep`,
//! `YStep`) and 8.7.4 (shading patterns).
//!
//! A pattern matrix maps pattern space to the default coordinate system of
//! the content stream the pattern is used in (the page, or the form), not
//! to the user space the mark is in (8.7.3.1).
//!
//! A tiling pattern's cell is rendered once, as a picture as many pixels
//! across as the cell is on the page, and the picture is repeated over the
//! mark. A cell whose `BBox` is wider than its step overlaps the cells next
//! to it, which is drawn by painting the cell once for each neighbour it
//! reaches into, left to right and bottom to top, so that a cell is over
//! the ones before it. A shading pattern is the shading's pixels over the mark.

use std::sync::Arc;

use tiny_skia::{Pixmap, Transform};

use crate::content::{ColorSpace, Matrix, Paint, PatternCell};
use crate::diagnostic::{DiagnosticKind, PaintFault};
use crate::doc::Content;
use crate::render::canvas::{Area, Canvas};
use crate::render::shading::{number_entry, numbers};
use crate::render::shape;
use crate::syntax::{Dictionary, Object, ObjectId, Value};

use super::{Marked, Painter, Ready, Run, MAX_TILE_PIXELS};

/// The `PatternType` values (Table 74).
const TILING: i64 = 1;
const SHADING: i64 = 2;

/// The `PaintType` values (Table 75): coloured, and uncoloured.
const COLOURED: i64 = 1;
const UNCOLOURED: i64 = 2;

/// The most cells a tile is painted with, for a `BBox` far wider than the
/// step.
const MAX_OVERLAP_CELLS: i64 = 64;

/// The bytes of rendered cells kept.
const TILE_CACHE_BYTES: usize = 128 << 20;

/// What a cell was rendered for: the pattern, its size in pixels, and the
/// colour of an uncoloured one.
#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) struct TileKey {
    id: ObjectId,
    width: u32,
    height: u32,
    under: Option<[u8; 3]>,
}

impl Painter<'_, '_> {
    fn note_pattern_fault(&mut self, fault: PaintFault) {
        self.diagnostics
            .push(DiagnosticKind::PatternUnusable { fault }, 0);
    }

    fn pattern_fault(&mut self, fault: PaintFault) -> Option<Ready> {
        self.note_pattern_fault(fault);
        None
    }

    /// The pixels `object`, a pattern, is painted with, over `mark`.
    pub(super) fn pattern_ink(
        &mut self,
        object: &Object,
        under: Option<[u8; 3]>,
        alpha: f32,
        mark: Option<super::Marked<'_>>,
    ) -> Option<Ready> {
        let doc = self.doc;
        let (id, dictionary, stream) = match object.value() {
            Value::Reference(id) => match doc.required(*id) {
                Ok(loaded) => match &loaded.content {
                    Content::Stream(stream) => (Some(*id), stream.dictionary.clone(), true),
                    Content::Object(inner) => match inner.value() {
                        Value::Dictionary(dictionary) => (Some(*id), dictionary.clone(), false),
                        _ => return self.pattern_fault(PaintFault::Entry),
                    },
                },
                Err(error) => {
                    self.error.get_or_insert(error);
                    return None;
                }
            },
            Value::Dictionary(dictionary) => (None, dictionary.clone(), false),
            _ => return self.pattern_fault(PaintFault::Entry),
        };
        let matrix =
            numbers::<6>(doc, &dictionary, b"Matrix").map_or(Matrix::IDENTITY, Matrix::from_array);
        let base = self.run.bases.last().copied().unwrap_or(Matrix::IDENTITY);
        let to_device = matrix.then(base);
        match number_entry(doc, &dictionary, b"PatternType") {
            Some(TILING) if stream => {
                let id = id?;
                self.tiling(id, &dictionary, to_device, under, alpha)
            }
            Some(SHADING) if !stream => self.shading_pattern(&dictionary, to_device, alpha, mark),
            _ => self.pattern_fault(PaintFault::Type),
        }
    }

    fn shading_pattern(
        &mut self,
        dictionary: &Dictionary,
        to_device: Matrix,
        alpha: f32,
        mark: Option<Marked<'_>>,
    ) -> Option<Ready> {
        let shading_object = dictionary.get(b"Shading")?;
        if dictionary.get(b"ExtGState").is_some() {
            self.note("shading pattern graphics state");
        }
        let shading = self.shading_of(shading_object)?;
        let area = self.mark_area(&mark?)?;
        match shading.paint(area, &to_device, true, self.progress) {
            Ok(pixmap) => Some(Ready::Pixels {
                pixmap: Arc::new(pixmap),
                transform: Transform::from_translate(area.x as f32, area.y as f32),
                repeat: false,
                smooth: false,
                alpha,
            }),
            Err(error) => {
                self.shading_failed(error);
                None
            }
        }
    }

    fn tiling(
        &mut self,
        id: ObjectId,
        dictionary: &Dictionary,
        to_device: Matrix,
        under: Option<[u8; 3]>,
        alpha: f32,
    ) -> Option<Ready> {
        let doc = self.doc;
        let paint_type = number_entry(doc, dictionary, b"PaintType")?;
        let Some([x0, y0, x1, y1]) = numbers::<4>(doc, dictionary, b"BBox") else {
            return self.pattern_fault(PaintFault::Entry);
        };
        let (Some(x_step), Some(y_step)) = (
            crate::render::shading::number(doc, dictionary.get(b"XStep")?),
            crate::render::shading::number(doc, dictionary.get(b"YStep")?),
        ) else {
            return self.pattern_fault(PaintFault::Entry);
        };
        let colour = match paint_type {
            COLOURED => None,
            UNCOLOURED => match under {
                Some(rgb) => Some(rgb),
                // An uncoloured pattern needs the colour it is painted in.
                None => return self.pattern_fault(PaintFault::Entry),
            },
            _ => return self.pattern_fault(PaintFault::Type),
        };
        let (step_x, step_y) = (x_step.abs(), y_step.abs());
        let (left, bottom) = (x0.min(x1), y0.min(y1));
        let (box_width, box_height) = ((x1 - x0).abs(), (y1 - y0).abs());
        let (scale_x, scale_y) = (
            to_device.a.hypot(to_device.b),
            to_device.c.hypot(to_device.d),
        );
        let sizes = [step_x, step_y, box_width, box_height, scale_x, scale_y];
        if sizes.iter().any(|n| !n.is_finite())
            || step_x == 0.0
            || step_y == 0.0
            || box_width == 0.0
            || box_height == 0.0
            || scale_x == 0.0
            || scale_y == 0.0
        {
            return self.pattern_fault(PaintFault::Size);
        }
        let (width, height) = (
            (step_x * scale_x).ceil().max(1.0),
            (step_y * scale_y).ceil().max(1.0),
        );
        if width * height > MAX_TILE_PIXELS as f64 {
            return self.pattern_fault(PaintFault::Size);
        }
        let (width, height) = (width as u32, height as u32);
        let key = TileKey {
            id,
            width,
            height,
            under: colour,
        };
        let tile = match self.tiles.get(&key) {
            Some(tile) => tile.clone(),
            None => {
                let cell = Cell {
                    id,
                    left,
                    bottom,
                    box_width,
                    box_height,
                    step_x,
                    step_y,
                    width,
                    height,
                    colour,
                };
                let tile = Arc::new(self.render_cell(dictionary, &cell)?);
                let bytes = tile.data().len();
                if self.tile_bytes + bytes > TILE_CACHE_BYTES {
                    self.tiles.clear();
                    self.tile_bytes = 0;
                }
                self.tile_bytes += bytes;
                self.tiles.insert(key, tile.clone());
                tile
            }
        };
        // Pixel of the tile -> pattern space -> device.
        let into_device = shape::transform(&to_device)?;
        let transform = Transform::from_scale(
            (step_x / f64::from(width)) as f32,
            (step_y / f64::from(height)) as f32,
        )
        .post_translate(left as f32, bottom as f32)
        .post_concat(into_device);
        Some(Ready::Pixels {
            pixmap: tile,
            transform,
            repeat: true,
            smooth: true,
            alpha,
        })
    }

    /// Renders one tile of the pattern: the cell `[left, left + step_x)` by
    /// `[bottom, bottom + step_y)`, with the cells that overlap it.
    fn render_cell(&mut self, dictionary: &Dictionary, cell: &Cell) -> Option<Pixmap> {
        let doc = self.doc;
        let resources = match doc.get(dictionary, b"Resources") {
            Ok(resolved) => resolved.dictionary().cloned(),
            Err(error) => {
                self.error.get_or_insert(error);
                return None;
            }
        };
        let Some(resources) = resources else {
            self.note_pattern_fault(PaintFault::Entry);
            return None;
        };
        let into_tile = Matrix::translation(-cell.left, -cell.bottom).then(Matrix::scaling(
            f64::from(cell.width) / cell.step_x,
            f64::from(cell.height) / cell.step_y,
        ));
        // The cells that reach into this one lie at whole steps from it.
        let reach_x = (cell.box_width / cell.step_x).ceil() as i64;
        let reach_y = (cell.box_height / cell.step_y).ceil() as i64;
        if reach_x * reach_y > MAX_OVERLAP_CELLS {
            self.note_pattern_fault(PaintFault::Size);
            return None;
        }
        let paint = cell.colour.map(|[r, g, b]| Paint {
            space: ColorSpace::DeviceRGB,
            components: vec![
                f64::from(r) / 255.0,
                f64::from(g) / 255.0,
                f64::from(b) / 255.0,
            ],
            pattern: None,
        });
        let canvas = Canvas::offscreen(
            Area {
                x: 0,
                y: 0,
                width: cell.width,
                height: cell.height,
            },
            None,
        )?;
        let outer_canvas = std::mem::replace(&mut self.canvas, canvas);
        let outer_run = std::mem::replace(&mut self.run, Run::new(into_tile));
        let mut result = Ok(());
        'cells: for j in -(reach_y - 1)..=0 {
            for i in -(reach_x - 1)..=0 {
                let placed = Matrix::translation(i as f64 * cell.step_x, j as f64 * cell.step_y)
                    .then(into_tile);
                let bbox = [
                    cell.left,
                    cell.bottom,
                    cell.left + cell.box_width,
                    cell.bottom + cell.box_height,
                ];
                self.run = Run::new(placed);
                let ran = self.sub_run(|painter, interpreter| {
                    let progress = painter.progress;
                    interpreter.run_pattern(
                        &PatternCell {
                            id: cell.id,
                            resources: &resources,
                            matrix: placed,
                            bbox,
                            colour: paint.as_ref(),
                        },
                        painter,
                        progress,
                    )
                });
                if let Err(error) = ran {
                    result = Err(error);
                    break 'cells;
                }
            }
        }
        let canvas = std::mem::replace(&mut self.canvas, outer_canvas);
        self.run = outer_run;
        if let Err(error) = result {
            self.error.get_or_insert(error);
            return None;
        }
        Some(canvas.into_pixmap())
    }
}

/// The measures of a tiling pattern's cell, in pattern space and pixels.
struct Cell {
    id: ObjectId,
    left: f64,
    bottom: f64,
    box_width: f64,
    box_height: f64,
    step_x: f64,
    step_y: f64,
    width: u32,
    height: u32,
    colour: Option<[u8; 3]>,
}
