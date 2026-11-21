// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The `sh` operator: ISO 32000-1:2008, 8.7.4.2. The shading fills the
//! current clipping path; the coordinate system it is defined in is the
//! user space at the operator, and the `Background` entry is not used.

use std::sync::Arc;

use crate::content::Context;
use crate::diagnostic::DiagnosticKind;
use crate::render::blend::blend_mode;
use crate::render::canvas::Area;
use crate::render::shading::{object_id, Shading};
use crate::syntax::Object;
use crate::PdfError;

use super::Painter;

impl Painter<'_, '_> {
    /// The shading `object` is or names, read once.
    pub(super) fn shading_of(&mut self, object: &Object) -> Option<Arc<Shading>> {
        let id = object_id(object);
        if let Some(known) = id.and_then(|id| self.shadings.get(&id)) {
            return known.clone();
        }
        let shading = match Shading::read(self.doc, object, &mut self.diagnostics) {
            Ok(shading) => shading.map(Arc::new),
            Err(error) => {
                self.error.get_or_insert(error);
                return None;
            }
        };
        if let Some(id) = id {
            self.shadings.insert(id, shading.clone());
        }
        shading
    }

    /// Reports a failure of a shading's painting: a function that fails is
    /// the shading's fault, anything else (a cancellation) is the render's.
    pub(super) fn shading_failed(&mut self, error: PdfError) {
        match error {
            PdfError::Function(_) => self.diagnostics.push(
                DiagnosticKind::ShadingUnusable {
                    fault: crate::PaintFault::Function,
                },
                0,
            ),
            PdfError::ColourComponents { .. } => self.diagnostics.push(
                DiagnosticKind::ShadingUnusable {
                    fault: crate::PaintFault::ColourSpace,
                },
                0,
            ),
            other => {
                self.error.get_or_insert(other);
            }
        }
    }

    pub(super) fn paint_shading(&mut self, ctx: &Context<'_>, object: &Object) {
        if !ctx.visible {
            return;
        }
        if self.progress.is_cancelled() {
            self.error.get_or_insert(PdfError::Cancelled);
            return;
        }
        let Some(shading) = self.shading_of(object) else {
            return;
        };
        let state = ctx.state;
        let Some(area) = self.canvas.cut_bounds() else {
            return;
        };
        let area: Area = area;
        match shading.paint(area, &state.ctm, false, self.progress) {
            Ok(pixmap) => self.canvas.draw_pixmap(
                &pixmap,
                area.x,
                area.y,
                state.fill_alpha as f32,
                blend_mode(&state.blend_mode),
            ),
            Err(error) => self.shading_failed(error),
        }
    }
}
