// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Annotation appearances: ISO 32000-1:2008, 12.5.2 (Table 164: `Annots`,
//! Table 165: the flags), 12.5.5 (appearance streams and the matrix `A` of
//! algorithm 8.1) and 8.11.3.2 (the `OC` entry).
//!
//! An annotation is drawn after the page's content, in the order `Annots`
//! lists them, as the normal appearance stream (`AP`, `N`, or the one named
//! by `AS` when `N` is a dictionary of states) placed in the annotation's
//! `Rect`. An annotation the file marks `Hidden` or `NoView`, a `Popup`, one
//! that optional content hides, and one with no appearance, draw nothing.
//!
//! `NoRotate` keeps the appearance from turning with the page's `Rotate`,
//! and `NoZoom` keeps it at the size it has at one pixel to the point,
//! whatever the scale of the render. Either way the upper-left corner of the
//! `Rect` (in the page's unrotated user space) stays where it is: the
//! appearance grows from it to the right and down (Table 165).

use crate::content::Matrix;
use crate::diagnostic::{DiagnosticKind, PaintFault};
use crate::doc::{Content, Page};
use crate::render::shading::numbers;
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

use super::{Painter, Run};

/// Table 165: bit 2 (`Hidden`) and bit 6 (`NoView`).
const FLAG_HIDDEN: i64 = 1 << 1;
const FLAG_NO_ZOOM: i64 = 1 << 3;
const FLAG_NO_ROTATE: i64 = 1 << 4;
const FLAG_NO_VIEW: i64 = 1 << 5;

impl Painter<'_, '_> {
    fn appearance_fault(&mut self, fault: PaintFault) {
        self.diagnostics
            .push(DiagnosticKind::AppearanceUnusable { fault }, 0);
    }

    /// Draws the annotations of `page`. `base` takes the page's default
    /// user space to the pixels, at `scale` pixels to the point.
    pub(in crate::render) fn paint_annotations(&mut self, page: &Page, base: Matrix, scale: f64) {
        let doc = self.doc;
        let items = match doc.get(&page.dictionary, b"Annots") {
            Ok(annots) => annots.array().map(<[Object]>::to_vec).unwrap_or_default(),
            Err(error) => {
                self.error.get_or_insert(error);
                return;
            }
        };
        for item in &items {
            if self.progress.is_cancelled() {
                self.error.get_or_insert(PdfError::Cancelled);
                return;
            }
            if let Err(error) = self.paint_annotation(item, base, scale) {
                self.error.get_or_insert(error);
                return;
            }
        }
    }

    fn paint_annotation(
        &mut self,
        item: &Object,
        base: Matrix,
        scale: f64,
    ) -> Result<(), PdfError> {
        let doc = self.doc;
        let resolved = match doc.resolve(item) {
            Ok(resolved) => resolved,
            // A reference to a stream is no annotation.
            Err(PdfError::WrongKind { .. }) => return Ok(()),
            Err(error) => return Err(error),
        };
        let Some(annotation) = resolved.dictionary() else {
            return Ok(());
        };
        if matches!(
            doc.get(annotation, b"Subtype")?.value(),
            Value::Name(name) if name.as_bytes() == b"Popup"
        ) {
            return Ok(());
        }
        let flags = match *doc.get(annotation, b"F")?.value() {
            Value::Integer(flags) => flags,
            _ => 0,
        };
        if flags & (FLAG_HIDDEN | FLAG_NO_VIEW) != 0 {
            return Ok(());
        }
        let Some(appearance) = normal_appearance(doc, annotation)? else {
            // A link is a region, and draws nothing of its own.
            let is_link = matches!(
                doc.get(annotation, b"Subtype")?.value(),
                Value::Name(name) if name.as_bytes() == b"Link"
            );
            if !is_link {
                self.note("annotation without an appearance");
            }
            return Ok(());
        };
        let Some([x0, y0, x1, y1]) = numbers::<4>(doc, annotation, b"Rect") else {
            self.appearance_fault(PaintFault::Entry);
            return Ok(());
        };
        let rect = [x0.min(x1), y0.min(y1), x0.max(x1), y0.max(y1)];

        let loaded = doc.required(appearance)?;
        let Content::Stream(stream) = &loaded.content else {
            self.appearance_fault(PaintFault::Entry);
            return Ok(());
        };
        let form = &stream.dictionary;
        let Some(bbox) = numbers::<4>(doc, form, b"BBox") else {
            self.appearance_fault(PaintFault::Entry);
            return Ok(());
        };
        let form_matrix =
            numbers::<6>(doc, form, b"Matrix").map_or(Matrix::IDENTITY, Matrix::from_array);
        let Some(fit) = fit_to_rect(bbox, &form_matrix, rect) else {
            self.appearance_fault(PaintFault::Size);
            return Ok(());
        };
        let anchored = anchored_base(
            base,
            scale,
            (rect[0], rect[3]),
            flags & FLAG_NO_ROTATE != 0,
            flags & FLAG_NO_ZOOM != 0,
        );
        let ctm = fit.then(anchored);

        let annotation = annotation.clone();
        let outer_run = std::mem::replace(&mut self.run, Run::new(ctm));
        let ran = self.sub_run(|painter, interpreter| {
            if interpreter.hides_content(&annotation)? {
                return Ok(());
            }
            let progress = painter.progress;
            interpreter.run_form(appearance, ctm, painter, progress)
        });
        self.run = outer_run;
        ran
    }
}

/// The matrix that takes the page's default user space to the pixels for an
/// annotation whose upper-left corner is `anchor`: the page's own `base`
/// (which has `scale` pixels to the point and the page's rotation), less its
/// rotation when the annotation has `NoRotate` and less its scale when it
/// has `NoZoom`, with `anchor` where `base` puts it.
fn anchored_base(
    base: Matrix,
    scale: f64,
    anchor: (f64, f64),
    no_rotate: bool,
    no_zoom: bool,
) -> Matrix {
    if !(no_rotate || no_zoom) || !scale.is_finite() || scale <= 0.0 {
        return base;
    }
    // The page's rotation and the flip from y up to y down, without the
    // scale; the flip alone when the page is not to turn.
    let turned = if no_rotate {
        [1.0, 0.0, 0.0, -1.0]
    } else {
        [
            base.a / scale,
            base.b / scale,
            base.c / scale,
            base.d / scale,
        ]
    };
    let size = if no_zoom { 1.0 } else { scale };
    let [a, b, c, d] = turned.map(|n| n * size);
    let (target_x, target_y) = base.apply(anchor.0, anchor.1);
    Matrix::new(
        a,
        b,
        c,
        d,
        target_x - (a * anchor.0 + c * anchor.1),
        target_y - (b * anchor.0 + d * anchor.1),
    )
}

/// The matrix `A` of algorithm 8.1 (12.5.5): it takes the appearance's
/// box, transformed by its own matrix, onto the annotation's rectangle.
/// `None` when that box has no width or no height, or a number is not
/// finite.
fn fit_to_rect(bbox: [f64; 4], form_matrix: &Matrix, rect: [f64; 4]) -> Option<Matrix> {
    let [x0, y0, x1, y1] = bbox;
    let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| form_matrix.apply(x, y));
    let low = |pick: fn(&(f64, f64)) -> f64| corners.iter().map(pick).fold(f64::MAX, f64::min);
    let high = |pick: fn(&(f64, f64)) -> f64| corners.iter().map(pick).fold(f64::MIN, f64::max);
    let (left, bottom) = (low(|c| c.0), low(|c| c.1));
    let (width, height) = (high(|c| c.0) - left, high(|c| c.1) - bottom);
    let (rect_width, rect_height) = (rect[2] - rect[0], rect[3] - rect[1]);
    let measures = [left, bottom, width, height, rect_width, rect_height];
    if !measures.iter().all(|n| n.is_finite()) || width <= 0.0 || height <= 0.0 {
        return None;
    }
    Some(
        Matrix::translation(-left, -bottom)
            .then(Matrix::scaling(rect_width / width, rect_height / height))
            .then(Matrix::translation(rect[0], rect[1])),
    )
}

/// The object of the normal appearance stream of `annotation`: `AP`'s `N`,
/// or when that is a dictionary of states the one `AS` names.
fn normal_appearance(
    doc: &crate::doc::Document,
    annotation: &Dictionary,
) -> Result<Option<ObjectId>, PdfError> {
    let Some(appearances) = doc.get(annotation, b"AP")?.dictionary().cloned() else {
        return Ok(None);
    };
    let Some(normal) = appearances.get(b"N") else {
        return Ok(None);
    };
    if let Value::Reference(id) = normal.value() {
        // A stream is the appearance; a dictionary is a table of states.
        if doc.required(*id)?.stream().is_some() {
            return Ok(Some(*id));
        }
    }
    let states = match doc.resolve(normal) {
        Ok(resolved) => resolved.dictionary().cloned(),
        Err(PdfError::WrongKind { .. }) => None,
        Err(error) => return Err(error),
    };
    let Some(states) = states else {
        return Ok(None);
    };
    let Value::Name(state) = doc.get(annotation, b"AS")?.value().clone() else {
        return Ok(None);
    };
    match states.get(state.as_bytes()).map(Object::value) {
        Some(Value::Reference(id)) => Ok(Some(*id)),
        _ => Ok(None),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    #[test]
    fn a_box_is_fitted_to_the_rectangle_by_scale_and_translation() {
        // BBox 0 0 10 20 onto 100 200 to 120 240: x doubles, y doubles.
        let a = fit_to_rect(
            [0.0, 0.0, 10.0, 20.0],
            &Matrix::IDENTITY,
            [100.0, 200.0, 120.0, 240.0],
        )
        .unwrap();
        assert_eq!(a.apply(0.0, 0.0), (100.0, 200.0));
        assert_eq!(a.apply(10.0, 20.0), (120.0, 240.0));
    }

    #[test]
    fn the_form_matrix_is_applied_before_the_fit() {
        // A quarter turn: the 10 by 20 box becomes 20 wide and 10 high.
        let quarter = Matrix::new(0.0, 1.0, -1.0, 0.0, 0.0, 0.0);
        let a = fit_to_rect([0.0, 0.0, 10.0, 20.0], &quarter, [0.0, 0.0, 20.0, 10.0]).unwrap();
        // The corner (10, 0) turns to (0, 10); the turned box spans x from
        // -20 to 0, so the fit moves it 20 to the right.
        let (x, y) = quarter.then(a).apply(10.0, 0.0);
        assert_eq!((x.round(), y.round()), (20.0, 10.0));
    }

    #[test]
    fn the_anchor_stays_put_when_the_rotation_or_scale_is_taken_off() {
        // A page turned a quarter at 2 pixels to the point: (x, y) goes to
        // (2y, 2x) plus the page's 100 points of height.
        let base = Matrix::new(0.0, 2.0, 2.0, 0.0, 0.0, 0.0);
        let anchor = (10.0, 20.0);
        let fixed = base.apply(anchor.0, anchor.1);
        for (no_rotate, no_zoom) in [(true, false), (false, true), (true, true)] {
            let m = anchored_base(base, 2.0, anchor, no_rotate, no_zoom);
            let (x, y) = m.apply(anchor.0, anchor.1);
            assert!((x - fixed.0).abs() < 1e-9 && (y - fixed.1).abs() < 1e-9);
        }
        // Without the rotation the page's x runs to the right and its y
        // up the picture, at the scale of the render.
        let m = anchored_base(base, 2.0, anchor, true, false);
        assert_eq!((m.a, m.b, m.c, m.d), (2.0, 0.0, 0.0, -2.0));
        // Without the zoom the turn stays and the scale is 1.
        let m = anchored_base(base, 2.0, anchor, false, true);
        assert_eq!((m.a, m.b, m.c, m.d), (0.0, 1.0, 1.0, 0.0));
        // With neither, nothing changes.
        assert_eq!(anchored_base(base, 2.0, anchor, false, false), base);
    }

    #[test]
    fn a_box_with_no_area_cannot_be_fitted() {
        assert!(fit_to_rect(
            [0.0, 0.0, 0.0, 5.0],
            &Matrix::IDENTITY,
            [0.0, 0.0, 1.0, 1.0]
        )
        .is_none());
    }
}
