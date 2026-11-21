// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The mesh shadings: ISO 32000-1:2008, 8.7.4.5.5 (type 4, free-form
//! triangles), 8.7.4.5.6 (type 5, lattice triangles), 8.7.4.5.7 (type 6,
//! Coons patches) and 8.7.4.5.8 (type 7, tensor-product patches).
//!
//! The stream is read into triangles and patches in the shading's space. A
//! patch is cut into a grid of small quadrilaterals when it is painted, each
//! of two triangles, the colour varying bilinearly across the patch. A
//! triangle's colour is interpolated across it from its corners, in the
//! shading's colour space (or, with a function, as the parameter that the
//! function then colours).

use tiny_skia::Pixmap;

use crate::colour::ColourSpace;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::error::Limit;
use crate::syntax::{Dictionary, Value};
use crate::PdfError;

use super::{opaque, Functions, Ramp, Target};

/// The most vertices and patches one mesh may hold.
const MAX_MESH_ELEMENTS: usize = 1 << 22;

/// A patch is cut into a grid with about this many device pixels to a side
/// of a cell, and at least `MIN_CELLS` and at most `MAX_CELLS` cells to a
/// side.
const CELL_PIXELS: f64 = 3.0;
const MIN_CELLS: usize = 4;
const MAX_CELLS: usize = 96;

/// Slack for a pixel centre on the shared edge of two triangles, so that no
/// crack is left between them.
const EDGE_SLACK: f64 = 1e-7;

/// The shading types.
const FREE_FORM: i64 = 4;
const LATTICE: i64 = 5;
const COONS: i64 = 6;

/// The number of points of a patch's boundary, and of a tensor patch.
const BOUNDARY_POINTS: usize = 12;
const TENSOR_POINTS: usize = 16;

/// Where each of a patch's control points is in the 4 by 4 grid `p[i][j]`
/// (`i` along `u`, `j` along `v`): the order the data gives them in
/// (Table 85 and Table 86).
const GRID_OF: [(usize, usize); TENSOR_POINTS] = [
    (0, 0),
    (0, 1),
    (0, 2),
    (0, 3),
    (1, 3),
    (2, 3),
    (3, 3),
    (3, 2),
    (3, 1),
    (3, 0),
    (2, 0),
    (1, 0),
    (1, 1),
    (1, 2),
    (2, 2),
    (2, 1),
];

/// Which of the previous patch's boundary points and corner colours the
/// first four points and two colours of a patch with edge flag 1, 2 and 3
/// are (Table 85).
const SHARED_POINTS: [[usize; 4]; 3] = [[3, 4, 5, 6], [6, 7, 8, 9], [9, 10, 11, 0]];
const SHARED_COLOURS: [[usize; 2]; 3] = [[1, 2], [2, 3], [3, 0]];

#[derive(Debug, Clone)]
struct Patch {
    p: [[[f64; 2]; 4]; 4],
    /// Indices of the colours of the corners `(u, v)` = (0, 0), (0, 1),
    /// (1, 1) and (1, 0).
    colours: [usize; 4],
}

#[derive(Debug, Clone)]
pub(super) struct Mesh {
    /// Values to a vertex: the colour components, or 1 for the parameter
    /// of a function.
    stride: usize,
    values: Vec<f64>,
    points: Vec<[f64; 2]>,
    /// Indices into `points` and, at the same position, `values`.
    triangles: Vec<[usize; 3]>,
    patches: Vec<Patch>,
    parameter: [f64; 2],
    ramp: Option<Ramp>,
}

/// The bits of a stream, most significant first.
struct Bits<'a> {
    data: &'a [u8],
    position: usize,
}

impl Bits<'_> {
    fn remaining(&self) -> usize {
        self.data.len() * 8 - self.position
    }

    fn align(&mut self) {
        self.position = (self.position + 7) & !7;
    }

    fn read(&mut self, count: u32) -> Option<u64> {
        let count = count as usize;
        if count > self.remaining() {
            return None;
        }
        let mut value = 0u64;
        for _ in 0..count {
            let byte = self.data[self.position / 8];
            let bit = (byte >> (7 - self.position % 8)) & 1;
            value = (value << 1) | u64::from(bit);
            self.position += 1;
        }
        Some(value)
    }
}

/// What the dictionary says about how a mesh stream is laid out.
struct Layout {
    coordinate_bits: u32,
    component_bits: u32,
    flag_bits: u32,
    /// `[xmin xmax ymin ymax]` then a `[min max]` for each value.
    decode: Vec<f64>,
    stride: usize,
}

impl Layout {
    fn coordinate(&self, bits: &mut Bits<'_>) -> Option<[f64; 2]> {
        let largest = (1u64 << self.coordinate_bits) - 1;
        let x = bits.read(self.coordinate_bits)? as f64 / largest as f64;
        let y = bits.read(self.coordinate_bits)? as f64 / largest as f64;
        Some([
            self.decode[0] + x * (self.decode[1] - self.decode[0]),
            self.decode[2] + y * (self.decode[3] - self.decode[2]),
        ])
    }

    fn values(&self, bits: &mut Bits<'_>, out: &mut Vec<f64>) -> Option<()> {
        let largest = ((1u64 << self.component_bits) - 1) as f64;
        for at in 0..self.stride {
            let raw = bits.read(self.component_bits)? as f64 / largest;
            let (low, high) = (self.decode[4 + 2 * at], self.decode[5 + 2 * at]);
            out.push(low + raw * (high - low));
        }
        Some(())
    }
}

fn bits_entry(doc: &Document, dictionary: &Dictionary, key: &[u8], allowed: &[u32]) -> Option<u32> {
    let Value::Integer(n) = *doc.get(dictionary, key).ok()?.value() else {
        return None;
    };
    u32::try_from(n).ok().filter(|n| allowed.contains(n))
}

impl Mesh {
    /// Reads the mesh of shading `kind` from its decoded `data`. `None`
    /// when the dictionary does not describe a readable layout.
    pub fn read(
        doc: &Document,
        dictionary: &Dictionary,
        kind: i64,
        data: &[u8],
        components: usize,
        functions: Option<&Functions>,
        diagnostics: &mut Diagnostics,
    ) -> Option<Self> {
        let stride = if functions.is_some() { 1 } else { components };
        if stride == 0 || functions.is_some_and(|f| !f.fits(1, components)) {
            return None;
        }
        let coordinate_bits = bits_entry(
            doc,
            dictionary,
            b"BitsPerCoordinate",
            &[1, 2, 4, 8, 12, 16, 24, 32],
        )?;
        let component_bits =
            bits_entry(doc, dictionary, b"BitsPerComponent", &[1, 2, 4, 8, 12, 16])?;
        let flag_bits = if kind == LATTICE {
            0
        } else {
            bits_entry(doc, dictionary, b"BitsPerFlag", &[2, 4, 8])?
        };
        let items = doc.get(dictionary, b"Decode").ok()?.array()?.to_vec();
        let decode: Option<Vec<f64>> = items.iter().map(|item| super::number(doc, item)).collect();
        let decode = decode.filter(|d| d.len() == 4 + 2 * stride)?;
        let layout = Layout {
            coordinate_bits,
            component_bits,
            flag_bits,
            decode,
            stride,
        };
        let mut mesh = Mesh {
            stride,
            values: Vec::new(),
            points: Vec::new(),
            triangles: Vec::new(),
            patches: Vec::new(),
            parameter: if functions.is_some() {
                [layout.decode[4], layout.decode[5]]
            } else {
                [0.0, 1.0]
            },
            ramp: None,
        };
        let mut bits = Bits { data, position: 0 };
        let complete = match kind {
            FREE_FORM => mesh.read_free_form(&layout, &mut bits),
            LATTICE => {
                let per_row = match *doc.get(dictionary, b"VerticesPerRow").ok()?.value() {
                    Value::Integer(n) => usize::try_from(n).ok().filter(|n| *n >= 2)?,
                    _ => return None,
                };
                mesh.read_lattice(&layout, &mut bits, per_row)
            }
            _ => mesh.read_patches(&layout, &mut bits, kind == COONS),
        };
        if !complete {
            diagnostics.push(DiagnosticKind::ShadingDataShort, 0);
        }
        Some(mesh)
    }

    pub fn with_ramp(mut self, ramp: Option<Ramp>) -> Self {
        self.ramp = ramp;
        self
    }

    /// The range the parameter of a function-coloured mesh takes.
    pub fn parametric_domain(&self) -> [f64; 2] {
        self.parameter
    }

    fn full(&self) -> bool {
        self.points.len() >= MAX_MESH_ELEMENTS || self.patches.len() >= MAX_MESH_ELEMENTS
    }

    fn push_vertex(&mut self, point: [f64; 2], values: &[f64]) -> usize {
        self.points.push(point);
        self.values.extend_from_slice(values);
        self.points.len() - 1
    }

    /// Type 4: each vertex has an edge flag (Table 80); each is padded to
    /// a whole byte. False when the data ends in the middle of a vertex.
    fn read_free_form(&mut self, layout: &Layout, bits: &mut Bits<'_>) -> bool {
        let mut values = Vec::with_capacity(layout.stride);
        let mut previous: Option<[usize; 3]> = None;
        let mut vertex = |mesh: &mut Mesh, bits: &mut Bits<'_>| -> Option<(u64, usize)> {
            let flag = bits.read(layout.flag_bits)?;
            let point = layout.coordinate(bits)?;
            values.clear();
            layout.values(bits, &mut values)?;
            bits.align();
            Some((flag, mesh.push_vertex(point, &values)))
        };
        while bits.remaining() >= 8 && !self.full() {
            let Some((flag, first)) = vertex(self, bits) else {
                return false;
            };
            let triangle = match (flag, previous) {
                (1, Some([_, b, c])) => [b, c, first],
                (2, Some([a, _, c])) => [a, c, first],
                _ => {
                    // Flag 0 starts a triangle of three vertices; their
                    // second and third flags are not used. An edge flag
                    // with no triangle to continue is read the same.
                    let (Some((_, second)), Some((_, third))) =
                        (vertex(self, bits), vertex(self, bits))
                    else {
                        return false;
                    };
                    [first, second, third]
                }
            };
            self.triangles.push(triangle);
            previous = Some(triangle);
        }
        true
    }

    /// Type 5: rows of `per_row` vertices, joined to the row before.
    fn read_lattice(&mut self, layout: &Layout, bits: &mut Bits<'_>, per_row: usize) -> bool {
        let mut values = Vec::with_capacity(layout.stride);
        let mut complete = true;
        while bits.remaining() >= 8 && !self.full() {
            let Some(point) = layout.coordinate(bits) else {
                complete = false;
                break;
            };
            values.clear();
            if layout.values(bits, &mut values).is_none() {
                complete = false;
                break;
            }
            bits.align();
            self.push_vertex(point, &values);
        }
        let rows = self.points.len() / per_row;
        if !self.points.len().is_multiple_of(per_row) {
            complete = false;
        }
        for row in 1..rows {
            for column in 1..per_row {
                let bottom_right = row * per_row + column;
                let top_right = bottom_right - per_row;
                self.triangles
                    .push([top_right - 1, top_right, bottom_right - 1]);
                self.triangles
                    .push([top_right, bottom_right, bottom_right - 1]);
            }
        }
        complete
    }

    /// Types 6 and 7: patches with edge flags (Tables 84 to 86). Each
    /// patch is padded to a whole byte.
    fn read_patches(&mut self, layout: &Layout, bits: &mut Bits<'_>, coons: bool) -> bool {
        let wanted = if coons {
            BOUNDARY_POINTS
        } else {
            TENSOR_POINTS
        };
        let mut previous: Option<([[f64; 2]; BOUNDARY_POINTS], [usize; 4])> = None;
        let mut values = Vec::with_capacity(layout.stride);
        while bits.remaining() >= 8 && !self.full() {
            let Some(flag) = bits.read(layout.flag_bits) else {
                return false;
            };
            let shared = match (flag, &previous) {
                (1..=3, Some((points, colours))) => {
                    let edge = flag as usize - 1;
                    Some((
                        SHARED_POINTS[edge].map(|at| points[at]),
                        SHARED_COLOURS[edge].map(|at| colours[at]),
                    ))
                }
                _ => None,
            };
            let own_points = wanted - shared.as_ref().map_or(0, |_| 4);
            let own_colours = 4 - shared.as_ref().map_or(0, |_| 2);
            let mut points = [[0.0; 2]; TENSOR_POINTS];
            let mut colours = [0usize; 4];
            let mut first_point = 0;
            let mut first_colour = 0;
            if let Some((shared_points, shared_colours)) = &shared {
                points[..4].copy_from_slice(shared_points);
                colours[..2].copy_from_slice(shared_colours);
                first_point = 4;
                first_colour = 2;
            }
            for slot in points.iter_mut().skip(first_point).take(own_points) {
                let Some(point) = layout.coordinate(bits) else {
                    return false;
                };
                *slot = point;
            }
            for slot in colours.iter_mut().skip(first_colour).take(own_colours) {
                values.clear();
                if layout.values(bits, &mut values).is_none() {
                    return false;
                }
                self.values.extend_from_slice(&values);
                *slot = self.values.len() / self.stride - 1;
            }
            bits.align();

            let mut boundary = [[0.0; 2]; BOUNDARY_POINTS];
            boundary.copy_from_slice(&points[..BOUNDARY_POINTS]);
            previous = Some((boundary, colours));

            let mut p = [[[0.0; 2]; 4]; 4];
            for (point, (i, j)) in points.iter().zip(GRID_OF).take(wanted) {
                p[i][j] = *point;
            }
            if coons {
                coons_interior(&mut p);
            }
            self.patches.push(Patch { p, colours });
        }
        true
    }

    /// Paints the mesh onto `pixmap`.
    pub fn paint(
        &self,
        pixmap: &mut Pixmap,
        target: &Target<'_>,
        space: &ColourSpace,
    ) -> Result<(), PdfError> {
        if self.points.len() > MAX_MESH_ELEMENTS || self.patches.len() > MAX_MESH_ELEMENTS {
            return Err(PdfError::LimitExceeded {
                offset: 0,
                limit: Limit::MeshElements {
                    max: MAX_MESH_ELEMENTS as u64,
                },
            });
        }
        let mut raster = Raster {
            pixmap,
            space,
            ramp: self.ramp.as_ref(),
            scratch: vec![0.0; self.stride],
        };
        let origin = (f64::from(target.area.x), f64::from(target.area.y));
        let device = |point: [f64; 2]| {
            let (x, y) = target.to_device.apply(point[0], point[1]);
            [x - origin.0, y - origin.1]
        };
        for (count, triangle) in self.triangles.iter().enumerate() {
            if count % 4096 == 0 {
                target.check()?;
            }
            let [a, b, c] = *triangle;
            let at = |i: usize| &self.values[i * self.stride..(i + 1) * self.stride];
            raster.triangle(
                [
                    device(self.points[a]),
                    device(self.points[b]),
                    device(self.points[c]),
                ],
                [at(a), at(b), at(c)],
            )?;
        }
        for (count, patch) in self.patches.iter().enumerate() {
            if count % 64 == 0 {
                target.check()?;
            }
            self.paint_patch(&mut raster, patch, &device)?;
        }
        Ok(())
    }

    fn paint_patch(
        &self,
        raster: &mut Raster<'_>,
        patch: &Patch,
        device: &impl Fn([f64; 2]) -> [f64; 2],
    ) -> Result<(), PdfError> {
        let mut p = patch.p;
        let (mut low, mut high) = ([f64::MAX; 2], [f64::MIN; 2]);
        for row in &mut p {
            for point in row {
                *point = device(*point);
                for axis in 0..2 {
                    low[axis] = low[axis].min(point[axis]);
                    high[axis] = high[axis].max(point[axis]);
                }
            }
        }
        let extent = (high[0] - low[0]).max(high[1] - low[1]);
        if !extent.is_finite() {
            return Ok(());
        }
        let cells = ((extent / CELL_PIXELS).ceil() as usize).clamp(MIN_CELLS, MAX_CELLS);
        let corner = |at: usize| &self.values[patch.colours[at] * self.stride..][..self.stride];
        let (c00, c01, c11, c10) = (corner(0), corner(1), corner(2), corner(3));
        let stride = self.stride;
        let grid = cells + 1;
        let mut points = Vec::with_capacity(grid * grid);
        let mut values = Vec::with_capacity(grid * grid * stride);
        for i in 0..grid {
            let u = i as f64 / cells as f64;
            for j in 0..grid {
                let v = j as f64 / cells as f64;
                points.push(surface(&p, u, v));
                for k in 0..stride {
                    values.push(
                        (1.0 - u) * (1.0 - v) * c00[k]
                            + (1.0 - u) * v * c01[k]
                            + u * v * c11[k]
                            + u * (1.0 - v) * c10[k],
                    );
                }
            }
        }
        let value = |at: usize| &values[at * stride..(at + 1) * stride];
        for i in 0..cells {
            for j in 0..cells {
                let a = i * grid + j;
                let (b, c, d) = (a + 1, a + grid, a + grid + 1);
                raster.triangle(
                    [points[a], points[b], points[c]],
                    [value(a), value(b), value(c)],
                )?;
                raster.triangle(
                    [points[b], points[d], points[c]],
                    [value(b), value(d), value(c)],
                )?;
            }
        }
        Ok(())
    }
}

/// The four interior control points of a Coons patch, from its boundary
/// (8.7.4.5.7, the equations after Table 85).
fn coons_interior(p: &mut [[[f64; 2]; 4]; 4]) {
    let mix = |p: &[[[f64; 2]; 4]; 4], terms: &[(f64, usize, usize)]| {
        let mut out = [0.0; 2];
        for (weight, i, j) in terms {
            for axis in 0..2 {
                out[axis] += weight * p[*i][*j][axis];
            }
        }
        [out[0] / 9.0, out[1] / 9.0]
    };
    p[1][1] = mix(
        p,
        &[
            (-4.0, 0, 0),
            (6.0, 0, 1),
            (6.0, 1, 0),
            (-2.0, 0, 3),
            (-2.0, 3, 0),
            (3.0, 3, 1),
            (3.0, 1, 3),
            (-1.0, 3, 3),
        ],
    );
    p[1][2] = mix(
        p,
        &[
            (-4.0, 0, 3),
            (6.0, 0, 2),
            (6.0, 1, 3),
            (-2.0, 0, 0),
            (-2.0, 3, 3),
            (3.0, 3, 2),
            (3.0, 1, 0),
            (-1.0, 3, 0),
        ],
    );
    p[2][1] = mix(
        p,
        &[
            (-4.0, 3, 0),
            (6.0, 3, 1),
            (6.0, 2, 0),
            (-2.0, 3, 3),
            (-2.0, 0, 0),
            (3.0, 0, 1),
            (3.0, 2, 3),
            (-1.0, 0, 3),
        ],
    );
    p[2][2] = mix(
        p,
        &[
            (-4.0, 3, 3),
            (6.0, 3, 2),
            (6.0, 2, 3),
            (-2.0, 3, 0),
            (-2.0, 0, 3),
            (3.0, 0, 2),
            (3.0, 2, 0),
            (-1.0, 0, 0),
        ],
    );
}

/// The point of the patch at `(u, v)`: the sum of the control points
/// weighted by the cubic Bernstein polynomials of each.
fn surface(p: &[[[f64; 2]; 4]; 4], u: f64, v: f64) -> [f64; 2] {
    let bernstein = |t: f64| {
        let s = 1.0 - t;
        [s * s * s, 3.0 * t * s * s, 3.0 * t * t * s, t * t * t]
    };
    let (bu, bv) = (bernstein(u), bernstein(v));
    let mut out = [0.0; 2];
    for i in 0..4 {
        for j in 0..4 {
            for axis in 0..2 {
                out[axis] += p[i][j][axis] * bu[i] * bv[j];
            }
        }
    }
    out
}

/// Triangles drawn onto a pixmap.
struct Raster<'a> {
    pixmap: &'a mut Pixmap,
    space: &'a ColourSpace,
    ramp: Option<&'a Ramp>,
    scratch: Vec<f64>,
}

impl Raster<'_> {
    /// Fills the triangle `corners` (in the pixmap's pixels) with the
    /// values interpolated from those at its corners.
    fn triangle(&mut self, corners: [[f64; 2]; 3], values: [&[f64]; 3]) -> Result<(), PdfError> {
        let [[x0, y0], [x1, y1], [x2, y2]] = corners;
        if ![x0, y0, x1, y1, x2, y2].iter().all(|n| n.is_finite()) {
            return Ok(());
        }
        let denominator = (y1 - y2) * (x0 - x2) + (x2 - x1) * (y0 - y2);
        if denominator == 0.0 {
            return Ok(());
        }
        let (width, height) = (self.pixmap.width(), self.pixmap.height());
        let first_x = (x0.min(x1).min(x2) - 0.5).floor().max(0.0) as u32;
        let first_y = (y0.min(y1).min(y2) - 0.5).floor().max(0.0) as u32;
        let last_x = ((x0.max(x1).max(x2) + 0.5).ceil().min(f64::from(width))) as u32;
        let last_y = ((y0.max(y1).max(y2) + 0.5).ceil().min(f64::from(height))) as u32;
        for y in first_y..last_y {
            let py = f64::from(y) + 0.5;
            for x in first_x..last_x {
                let px = f64::from(x) + 0.5;
                let l0 = ((y1 - y2) * (px - x2) + (x2 - x1) * (py - y2)) / denominator;
                let l1 = ((y2 - y0) * (px - x2) + (x0 - x2) * (py - y2)) / denominator;
                let l2 = 1.0 - l0 - l1;
                if l0 < -EDGE_SLACK || l1 < -EDGE_SLACK || l2 < -EDGE_SLACK {
                    continue;
                }
                for (k, slot) in self.scratch.iter_mut().enumerate() {
                    *slot = l0 * values[0][k] + l1 * values[1][k] + l2 * values[2][k];
                }
                let colour = match self.ramp {
                    Some(ramp) => ramp.at(self.scratch[0]),
                    None => self.space.to_rgb(&self.scratch)?.to_bytes(),
                };
                let at = (y as usize * width as usize + x as usize) * 4;
                self.pixmap.data_mut()[at..at + 4].copy_from_slice(&opaque(colour));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn bits_are_read_most_significant_first_and_a_vertex_ends_on_a_byte() {
        let mut bits = Bits {
            data: &[0b1011_0010, 0b1111_0000],
            position: 0,
        };
        assert_eq!(bits.read(3), Some(0b101));
        assert_eq!(bits.read(4), Some(0b1001));
        bits.align();
        assert_eq!(bits.read(4), Some(0b1111));
        assert_eq!(bits.remaining(), 4);
        assert_eq!(bits.read(5), None);
    }

    /// The 4 by 4 grid of control points of a square 0 to 9 with the
    /// boundary at thirds and the interior left zero.
    fn square_boundary() -> [[[f64; 2]; 4]; 4] {
        let mut p = [[[0.0; 2]; 4]; 4];
        for (i, column) in p.iter_mut().enumerate() {
            for (j, point) in column.iter_mut().enumerate() {
                if i == 0 || i == 3 || j == 0 || j == 3 {
                    *point = [3.0 * i as f64, 3.0 * j as f64];
                }
            }
        }
        p
    }

    #[test]
    fn the_interior_of_a_straight_patch_with_even_edges_is_the_grid_of_thirds() {
        let mut p = square_boundary();
        coons_interior(&mut p);
        for (i, column) in p.iter().enumerate().skip(1).take(2) {
            for (j, point) in column.iter().enumerate().skip(1).take(2) {
                let wanted = [3.0 * i as f64, 3.0 * j as f64];
                assert!(
                    (point[0] - wanted[0]).abs() < 1e-9 && (point[1] - wanted[1]).abs() < 1e-9,
                    "p[{i}][{j}] = {point:?}, wanted {wanted:?}"
                );
            }
        }
    }

    #[test]
    fn the_surface_of_that_patch_is_the_square_itself() {
        let mut p = square_boundary();
        coons_interior(&mut p);
        for (u, v) in [(0.0, 0.0), (0.25, 0.5), (1.0, 1.0), (0.9, 0.1)] {
            let [x, y] = surface(&p, u, v);
            assert!((x - 9.0 * u).abs() < 1e-9 && (y - 9.0 * v).abs() < 1e-9);
        }
    }
}
