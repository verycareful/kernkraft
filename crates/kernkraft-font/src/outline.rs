// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A glyph outline as path segments in the font's design units, y up.

/// A point in design units.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

impl Point {
    pub fn new(x: f32, y: f32) -> Point {
        Point { x, y }
    }
}

/// One step of a path.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Segment {
    MoveTo(Point),
    LineTo(Point),
    /// A quadratic Bezier: control point, end point (TrueType).
    QuadTo(Point, Point),
    /// A cubic Bezier: two control points, end point (CFF, Type 1).
    CubicTo(Point, Point, Point),
    /// Closes the current contour with a straight line to its start. A
    /// contour may already end with a line to its start point; the two
    /// forms are the same shape.
    Close,
}

/// The contours of one glyph. A glyph with no marks (a space) has no
/// segments.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Outline {
    pub segments: Vec<Segment>,
}

impl Outline {
    pub fn is_empty(&self) -> bool {
        self.segments.is_empty()
    }

    /// The box that holds every point of the outline, control points
    /// included, as `[min_x, min_y, max_x, max_y]`; `None` for an empty
    /// outline.
    pub fn control_box(&self) -> Option<[f32; 4]> {
        let mut found: Option<[f32; 4]> = None;
        let mut take = |p: &Point| match &mut found {
            None => found = Some([p.x, p.y, p.x, p.y]),
            Some(b) => {
                b[0] = b[0].min(p.x);
                b[1] = b[1].min(p.y);
                b[2] = b[2].max(p.x);
                b[3] = b[3].max(p.y);
            }
        };
        for s in &self.segments {
            match s {
                Segment::MoveTo(a) | Segment::LineTo(a) => take(a),
                Segment::QuadTo(a, b) => {
                    take(a);
                    take(b);
                }
                Segment::CubicTo(a, b, c) => {
                    take(a);
                    take(b);
                    take(c);
                }
                Segment::Close => {}
            }
        }
        found
    }

    pub(crate) fn move_to(&mut self, x: f32, y: f32) {
        self.segments.push(Segment::MoveTo(Point::new(x, y)));
    }

    pub(crate) fn line_to(&mut self, x: f32, y: f32) {
        self.segments.push(Segment::LineTo(Point::new(x, y)));
    }

    pub(crate) fn cubic_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        self.segments.push(Segment::CubicTo(
            Point::new(x1, y1),
            Point::new(x2, y2),
            Point::new(x, y),
        ));
    }

    pub(crate) fn close(&mut self) {
        self.segments.push(Segment::Close);
    }
}

impl skrifa::outline::OutlinePen for Outline {
    fn move_to(&mut self, x: f32, y: f32) {
        Outline::move_to(self, x, y);
    }

    fn line_to(&mut self, x: f32, y: f32) {
        Outline::line_to(self, x, y);
    }

    fn quad_to(&mut self, cx0: f32, cy0: f32, x: f32, y: f32) {
        self.segments
            .push(Segment::QuadTo(Point::new(cx0, cy0), Point::new(x, y)));
    }

    fn curve_to(&mut self, cx0: f32, cy0: f32, cx1: f32, cy1: f32, x: f32, y: f32) {
        Outline::cubic_to(self, cx0, cy0, cx1, cy1, x, y);
    }

    fn close(&mut self) {
        Outline::close(self);
    }
}
