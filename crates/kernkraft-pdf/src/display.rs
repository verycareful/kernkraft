// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The displayed page: ISO 32000-1:2008, 7.7.3.3 (Table 30: `MediaBox`,
//! `CropBox`, `Rotate`) and 8.3.2.3 (user space).
//!
//! A page is shown as its `CropBox` (the `MediaBox` when there is none),
//! clipped to the `MediaBox`, turned clockwise by `Rotate`. The text layer,
//! the links and the destinations give their geometry as fractions of that
//! displayed page: `(0, 0)` is its top left corner, `(1, 1)` its bottom
//! right, and `y` grows downwards. A caller scales the fractions to any
//! size it shows the page at.

use crate::content::Matrix;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{Page, Rectangle};
use crate::syntax::Name;

/// The size a page is taken to have when its `MediaBox` is missing or has
/// no area, in points: US Letter, the size readers of the format fall back
/// to when 7.7.3.3 gives none.
const DEFAULT_PAGE_SIZE: (f64, f64) = (612.0, 792.0);

/// A rectangle on the displayed page, as fractions of its width and
/// height, `y` growing downwards.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Rect {
    pub left: f64,
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
}

impl Rect {
    /// The smallest rectangle that holds all of `points`, or `None` when
    /// there are none.
    pub fn around(points: impl IntoIterator<Item = (f64, f64)>) -> Option<Rect> {
        let mut points = points.into_iter();
        let (x, y) = points.next()?;
        let mut rect = Rect {
            left: x,
            top: y,
            right: x,
            bottom: y,
        };
        for (x, y) in points {
            rect.left = rect.left.min(x);
            rect.top = rect.top.min(y);
            rect.right = rect.right.max(x);
            rect.bottom = rect.bottom.max(y);
        }
        Some(rect)
    }

    pub fn width(&self) -> f64 {
        self.right - self.left
    }

    pub fn height(&self) -> f64 {
        self.bottom - self.top
    }

    /// Whether the point lies inside, edges included.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.left <= x && x <= self.right && self.top <= y && y <= self.bottom
    }

    /// Whether the two share any area or edge.
    pub fn intersects(&self, other: &Rect) -> bool {
        self.left <= other.right
            && other.left <= self.right
            && self.top <= other.bottom
            && other.top <= self.bottom
    }

    /// The smallest rectangle that holds both.
    pub fn union(&self, other: &Rect) -> Rect {
        Rect {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }

    /// How far the point is from the rectangle: 0 inside it.
    pub fn distance_to(&self, x: f64, y: f64) -> f64 {
        let dx = (self.left - x).max(0.0).max(x - self.right);
        let dy = (self.top - y).max(0.0).max(y - self.bottom);
        dx.hypot(dy)
    }
}

/// How a page's default user space maps to the displayed page.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PageView {
    /// The displayed page's width and height, in points.
    pub width: f64,
    pub height: f64,
    /// The visible box in user space.
    pub bounds: Rectangle,
    /// `Rotate`, in degrees clockwise.
    pub rotate: u16,
    /// Maps user space to the displayed page in points: the origin at its
    /// top left corner, `y` down.
    pub matrix: Matrix,
}

impl PageView {
    /// The view of `page`. A `MediaBox` or `CropBox` that has no area is
    /// left out and reported.
    pub(crate) fn of(page: &Page, diagnostics: &mut Diagnostics) -> PageView {
        let media = usable(page.media_box, b"MediaBox", diagnostics);
        let crop = usable(page.crop_box, b"CropBox", diagnostics);
        let media_box = media.unwrap_or(Rectangle {
            llx: 0.0,
            lly: 0.0,
            urx: DEFAULT_PAGE_SIZE.0,
            ury: DEFAULT_PAGE_SIZE.1,
        });
        let bounds = match crop {
            Some(crop) => match intersection(&crop, &media_box) {
                Some(visible) => visible,
                None => {
                    diagnostics.push(
                        DiagnosticKind::IgnoredEntry {
                            key: Name::new(&b"CropBox"[..]),
                        },
                        0,
                    );
                    media_box
                }
            },
            None => media_box,
        };
        let (width, height, matrix) = match page.rotate {
            90 => (
                bounds.height(),
                bounds.width(),
                Matrix::new(0.0, 1.0, 1.0, 0.0, -bounds.lly, -bounds.llx),
            ),
            180 => (
                bounds.width(),
                bounds.height(),
                Matrix::new(-1.0, 0.0, 0.0, 1.0, bounds.urx, -bounds.lly),
            ),
            270 => (
                bounds.height(),
                bounds.width(),
                Matrix::new(0.0, -1.0, -1.0, 0.0, bounds.ury, bounds.urx),
            ),
            _ => (
                bounds.width(),
                bounds.height(),
                Matrix::new(1.0, 0.0, 0.0, -1.0, -bounds.llx, bounds.ury),
            ),
        };
        PageView {
            width,
            height,
            bounds,
            rotate: page.rotate,
            matrix,
        }
    }

    /// The fractions of the displayed page that a point of the displayed
    /// page, in points, is at.
    pub(crate) fn fraction(&self, x: f64, y: f64) -> (f64, f64) {
        (x / self.width, y / self.height)
    }

    /// The rectangle of fractions around points of the displayed page, in
    /// points.
    pub(crate) fn rect_of(&self, points: impl IntoIterator<Item = (f64, f64)>) -> Option<Rect> {
        Rect::around(points.into_iter().map(|(x, y)| self.fraction(x, y)))
    }

    /// The rectangle of fractions that user space rectangle `rect` covers.
    pub(crate) fn user_rect(&self, rect: &Rectangle) -> Rect {
        let corners = [
            (rect.llx, rect.lly),
            (rect.urx, rect.lly),
            (rect.urx, rect.ury),
            (rect.llx, rect.ury),
        ];
        self.rect_of(corners.map(|(x, y)| self.matrix.apply(x, y)))
            .unwrap_or_default()
    }

    /// The fractions of the displayed page that a user space point is at,
    /// for each axis whose user space coordinate is known. A displayed
    /// axis follows one user space axis: `x` for a page turned 0 or 180
    /// degrees, `y` for one turned 90 or 270, and the other way round.
    pub(crate) fn locate(&self, x: Option<f64>, y: Option<f64>) -> (Option<f64>, Option<f64>) {
        let (user_x, user_y) = (x.unwrap_or(self.bounds.llx), y.unwrap_or(self.bounds.lly));
        let (shown_x, shown_y) = self.matrix.apply(user_x, user_y);
        let (fraction_x, fraction_y) = self.fraction(shown_x, shown_y);
        let quarter = self.rotate == 90 || self.rotate == 270;
        let (x_from, y_from) = if quarter { (y, x) } else { (x, y) };
        (x_from.map(|_| fraction_x), y_from.map(|_| fraction_y))
    }
}

/// `rectangle` when it has an area, else `None`, reported.
fn usable(
    rectangle: Option<Rectangle>,
    key: &'static [u8],
    diagnostics: &mut Diagnostics,
) -> Option<Rectangle> {
    let rectangle = rectangle?;
    let area = rectangle.width() > 0.0
        && rectangle.height() > 0.0
        && rectangle.width().is_finite()
        && rectangle.height().is_finite();
    if !area {
        diagnostics.push(
            DiagnosticKind::IgnoredEntry {
                key: Name::new(key),
            },
            0,
        );
        return None;
    }
    Some(rectangle)
}

/// The rectangle both cover, when they share an area.
fn intersection(a: &Rectangle, b: &Rectangle) -> Option<Rectangle> {
    let rectangle = Rectangle {
        llx: a.llx.max(b.llx),
        lly: a.lly.max(b.lly),
        urx: a.urx.min(b.urx),
        ury: a.ury.min(b.ury),
    };
    (rectangle.width() > 0.0 && rectangle.height() > 0.0).then_some(rectangle)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn view(rotate: u16) -> PageView {
        let page = Page {
            id: crate::syntax::ObjectId {
                number: 1,
                generation: 0,
            },
            dictionary: crate::syntax::Dictionary::new(),
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
        PageView::of(&page, &mut Diagnostics::new())
    }

    // `Rotate` turns the page clockwise (7.7.3.3), so the corner that comes
    // to the top left is the upper left for 0, the lower left for 90, the
    // lower right for 180 and the upper right for 270.
    #[test]
    fn each_turn_brings_its_corner_to_the_top_left() {
        let corners = [
            (0, (10.0, 220.0)),
            (90, (10.0, 20.0)),
            (180, (110.0, 20.0)),
            (270, (110.0, 220.0)),
        ];
        for (rotate, (x, y)) in corners {
            let view = view(rotate);
            let (shown_x, shown_y) = view.matrix.apply(x, y);
            assert_eq!((shown_x, shown_y), (0.0, 0.0), "rotate {rotate}");
        }
    }

    #[test]
    fn a_quarter_turn_swaps_the_size() {
        assert_eq!((view(0).width, view(0).height), (100.0, 200.0));
        assert_eq!((view(90).width, view(90).height), (200.0, 100.0));
        assert_eq!((view(180).width, view(180).height), (100.0, 200.0));
        assert_eq!((view(270).width, view(270).height), (200.0, 100.0));
    }
}
