// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The two spaces an annotation lives in: fractions of the displayed page
//! (7.7.3.3: `CropBox` clipped to `MediaBox`, `Rotate` applied) for the
//! caller, and the page's default user space (8.3.2.3) for the file.

use crate::content::Matrix;
use crate::diagnostic::Diagnostics;
use crate::display::PageView;
use crate::doc::Page;
use crate::{PdfError, Rect};

/// A point of user space.
pub(super) type Point = (f64, f64);

/// The four corners of a quadrilateral in the order `QuadPoints` lists
/// them in practice (12.5.6.10): top left, top right, bottom left, bottom
/// right, as the page is displayed.
pub(super) type Quad = [Point; 4];

/// A page's mapping between the displayed page and user space.
pub(super) struct Geometry {
    pub(super) view: PageView,
    to_user: Matrix,
}

impl Geometry {
    /// The geometry of `page`; what is wrong with its boxes goes to
    /// `diagnostics`. A page whose displayed size has no inverse (a
    /// `MediaBox` the view could not use) is [`PdfError::InvalidEntry`].
    pub(super) fn of(page: &Page, diagnostics: &mut Diagnostics) -> Result<Self, PdfError> {
        let view = PageView::of(page, diagnostics);
        let to_user = view.matrix.inverse().ok_or(PdfError::InvalidEntry {
            object: Some(page.id),
            key: "MediaBox",
        })?;
        Ok(Self { view, to_user })
    }

    /// The user space point at the fractions `(x, y)` of the displayed
    /// page.
    pub(super) fn user_point(&self, (x, y): Point) -> Point {
        self.to_user
            .apply(x * self.view.width, y * self.view.height)
    }

    /// The user space vector that moves a point by the fractions `(x, y)`
    /// of the displayed page.
    pub(super) fn user_delta(&self, (x, y): Point) -> Point {
        let origin = self.user_point((0.0, 0.0));
        let moved = self.user_point((x, y));
        (moved.0 - origin.0, moved.1 - origin.1)
    }

    /// The corners of `rect` in user space, in the order of [`Quad`].
    pub(super) fn user_quad(&self, rect: &Rect) -> Quad {
        let (left, right) = (rect.left.min(rect.right), rect.left.max(rect.right));
        let (top, bottom) = (rect.top.min(rect.bottom), rect.top.max(rect.bottom));
        [
            self.user_point((left, top)),
            self.user_point((right, top)),
            self.user_point((left, bottom)),
            self.user_point((right, bottom)),
        ]
    }

    /// The fractions of the displayed page that the user space point
    /// `(x, y)` is at.
    pub(super) fn fractions(&self, (x, y): Point) -> Point {
        let (shown_x, shown_y) = self.view.matrix.apply(x, y);
        self.view.fraction(shown_x, shown_y)
    }

    /// The box, as fractions of the displayed page, around user space
    /// points.
    pub(super) fn rect_around(&self, points: impl IntoIterator<Item = Point>) -> Option<Rect> {
        Rect::around(points.into_iter().map(|point| self.fractions(point)))
    }
}

/// The box of user space points, as `[llx, lly, urx, ury]`; `None` when
/// there are none.
pub(super) fn bounds(points: impl IntoIterator<Item = Point>) -> Option<[f64; 4]> {
    let mut points = points.into_iter();
    let (x, y) = points.next()?;
    let mut out = [x, y, x, y];
    for (x, y) in points {
        out[0] = out[0].min(x);
        out[1] = out[1].min(y);
        out[2] = out[2].max(x);
        out[3] = out[3].max(y);
    }
    Some(out)
}

/// `rect` grown by `by` on every side.
pub(super) fn grown(rect: [f64; 4], by: f64) -> [f64; 4] {
    [rect[0] - by, rect[1] - by, rect[2] + by, rect[3] + by]
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::doc::Rectangle;
    use crate::syntax::{Dictionary, ObjectId};

    fn geometry(rotate: u16) -> Geometry {
        let page = Page {
            id: ObjectId {
                number: 1,
                generation: 0,
            },
            dictionary: Dictionary::new(),
            resources: None,
            media_box: Some(Rectangle {
                llx: 10.0,
                lly: 20.0,
                urx: 110.0,
                ury: 220.0,
            }),
            crop_box: None,
            rotate,
        };
        Geometry::of(&page, &mut Diagnostics::new()).unwrap()
    }

    // The top left corner of the displayed page is the corner `Rotate`
    // brings there (7.7.3.3).
    #[test]
    fn the_top_left_of_the_displayed_page_is_the_corner_rotate_brings_there() {
        let corners = [
            (0, (10.0, 220.0)),
            (90, (10.0, 20.0)),
            (180, (110.0, 20.0)),
            (270, (110.0, 220.0)),
        ];
        for (rotate, corner) in corners {
            let (x, y) = geometry(rotate).user_point((0.0, 0.0));
            assert!(
                (x - corner.0).abs() < 1e-9 && (y - corner.1).abs() < 1e-9,
                "rotate {rotate}: {x} {y}"
            );
        }
    }

    #[test]
    fn user_points_and_fractions_are_inverse() {
        for rotate in [0, 90, 180, 270] {
            let geometry = geometry(rotate);
            let (x, y) = geometry.fractions(geometry.user_point((0.25, 0.75)));
            assert!(
                (x - 0.25).abs() < 1e-9 && (y - 0.75).abs() < 1e-9,
                "rotate {rotate}: {x} {y}"
            );
        }
    }

    #[test]
    fn a_delta_is_a_vector_and_ignores_the_origin() {
        // Unrotated: 10% of the 100 wide page to the right is 10 points of
        // x; 10% of the 200 high page down is 20 points less of y.
        let (dx, dy) = geometry(0).user_delta((0.1, 0.1));
        assert!((dx - 10.0).abs() < 1e-9 && (dy + 20.0).abs() < 1e-9);
        // Turned 90 degrees clockwise the page is 200 wide and 100 high
        // and its x runs up the user space y.
        let (dx, dy) = geometry(90).user_delta((0.1, 0.1));
        assert!((dx - 10.0).abs() < 1e-9 && (dy - 20.0).abs() < 1e-9);
    }

    #[test]
    fn a_quad_lists_the_displayed_top_corners_first() {
        let rect = Rect {
            left: 0.0,
            top: 0.0,
            right: 0.5,
            bottom: 0.25,
        };
        let quad = geometry(0).user_quad(&rect);
        // Top left, top right, bottom left, bottom right of the box
        // [10 220] to [60 170].
        let expected = [(10.0, 220.0), (60.0, 220.0), (10.0, 170.0), (60.0, 170.0)];
        for (got, want) in quad.iter().zip(expected) {
            assert!((got.0 - want.0).abs() < 1e-9 && (got.1 - want.1).abs() < 1e-9);
        }
    }

    #[test]
    fn bounds_of_no_points_is_none() {
        assert_eq!(bounds([]), None);
        assert_eq!(
            bounds([(1.0, 5.0), (-2.0, 3.0)]),
            Some([-2.0, 3.0, 1.0, 5.0])
        );
    }
}
