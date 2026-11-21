// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Paths and matrices in the rasteriser's terms: ISO 32000-1:2008, 8.5.2
//! (path construction) and 8.3.4 (matrices).

use kernkraft_font::{Outline, Segment};
use tiny_skia::{Path, PathBuilder, Transform};

use crate::content::{Matrix, Path as PdfPath, PathSegment};

/// The rasteriser's transform for a PDF matrix. `None` when a component is
/// not finite as a single-precision number.
pub(super) fn transform(matrix: &Matrix) -> Option<Transform> {
    let t = Transform::from_row(
        matrix.a as f32,
        matrix.b as f32,
        matrix.c as f32,
        matrix.d as f32,
        matrix.e as f32,
        matrix.f as f32,
    );
    t.is_finite().then_some(t)
}

/// A content-stream path as a rasteriser path, in the same space. `None`
/// for a path with nothing to paint (no segments, or no finite one).
pub(super) fn path(path: &PdfPath) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in &path.segments {
        match *segment {
            PathSegment::MoveTo(x, y) => builder.move_to(x as f32, y as f32),
            PathSegment::LineTo(x, y) => builder.line_to(x as f32, y as f32),
            PathSegment::CurveTo(x1, y1, x2, y2, x3, y3) => builder.cubic_to(
                x1 as f32, y1 as f32, x2 as f32, y2 as f32, x3 as f32, y3 as f32,
            ),
            PathSegment::Close => builder.close(),
        }
    }
    builder.finish()
}

/// A glyph outline as a rasteriser path in design units, y up.
pub(super) fn outline(outline: &Outline) -> Option<Path> {
    let mut builder = PathBuilder::new();
    for segment in &outline.segments {
        match *segment {
            Segment::MoveTo(p) => builder.move_to(p.x, p.y),
            Segment::LineTo(p) => builder.line_to(p.x, p.y),
            Segment::QuadTo(c, p) => builder.quad_to(c.x, c.y, p.x, p.y),
            Segment::CubicTo(a, b, p) => builder.cubic_to(a.x, a.y, b.x, b.y, p.x, p.y),
            Segment::Close => builder.close(),
        }
    }
    builder.finish()
}
