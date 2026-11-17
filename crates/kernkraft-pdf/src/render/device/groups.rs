// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Form XObjects and their transparency groups: ISO 32000-1:2008, 8.10
//! (form XObjects), 11.4 (transparency groups) and 11.6.6 (the group
//! attributes of a form: `I` isolated, `K` knockout).
//!
//! A form with no group is run into the page as it is: each mark in it is
//! composed with the state's alpha and blend mode, one by one. A form with a
//! group is composed as one object, with the alpha, blend mode and soft
//! mask it was invoked with. That needs a layer of its own when the group
//! is isolated or a knockout, or when it is composed with anything but full
//! opacity and Normal: a non-isolated group that is composed with neither
//! is painted straight into the page, which gives the same pixels, since
//! Normal composition with full opacity is associative.

use crate::content::{Context, FormInfo, Matrix};
use crate::render::blend::blend_mode;
use crate::render::canvas::{Area, GroupParams};
use crate::syntax::Value;

use super::Painter;

/// How far a group's layer reaches past the bounding box of its `BBox`, in
/// pixels, so that the edge's anti-aliased pixels are inside it.
const EDGE: f64 = 1.0;

/// Whether the group dictionary sets flag `key` (`I` or `K`).
fn flag(form: &FormInfo, key: &[u8]) -> bool {
    form.group
        .as_ref()
        .and_then(|group| group.get(key))
        .is_some_and(|value| matches!(value.value(), Value::Boolean(true)))
}

impl Painter<'_, '_> {
    /// A form XObject starts.
    pub(super) fn open_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {
        let state = ctx.state;
        let alpha = state.fill_alpha as f32;
        let blend = blend_mode(&state.blend_mode);
        let composed = alpha < 1.0
            || blend != tiny_skia::BlendMode::SourceOver
            || !matches!(state.soft_mask, crate::content::SoftMask::None);
        let layered = ctx.visible
            && form.group.is_some()
            && (composed || flag(form, b"I") || flag(form, b"K"));
        let pushed = layered && {
            let area = self.group_area(form, &state.ctm);
            self.canvas.push_group(
                area,
                GroupParams {
                    alpha,
                    blend,
                    isolated: flag(form, b"I"),
                    knockout: flag(form, b"K"),
                },
            )
        };
        self.run.forms.push(pushed);
        self.run.bases.push(state.ctm);
    }

    /// The form ends: a layer it was painted on is composed.
    pub(super) fn close_form(&mut self) {
        self.run.bases.pop();
        if self.run.forms.pop() == Some(true) {
            self.canvas.pop_group();
        }
    }

    /// The pixels a form's `BBox` covers under `ctm` (which has the form's
    /// own matrix applied), or the layer when it has none.
    fn group_area(&self, form: &FormInfo, ctm: &Matrix) -> Area {
        let layer = self.canvas.bounds();
        let Some([x0, y0, x1, y1]) = form.bbox else {
            return layer;
        };
        let corners = [(x0, y0), (x1, y0), (x1, y1), (x0, y1)].map(|(x, y)| ctm.apply(x, y));
        let low = |axis: fn(&(f64, f64)) -> f64| corners.iter().map(axis).fold(f64::MAX, f64::min);
        let high = |axis: fn(&(f64, f64)) -> f64| corners.iter().map(axis).fold(f64::MIN, f64::max);
        let (left, top) = (low(|c| c.0) - EDGE, low(|c| c.1) - EDGE);
        let (right, bottom) = (high(|c| c.0) + EDGE, high(|c| c.1) + EDGE);
        if ![left, top, right, bottom].iter().all(|n| n.is_finite()) {
            return layer;
        }
        let clamp = |n: f64| n.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i64;
        let (x, y) = (clamp(left.floor()), clamp(top.floor()));
        Area {
            x: x as i32,
            y: y as i32,
            width: (clamp(right.ceil()) - x).clamp(0, i64::from(u32::MAX)) as u32,
            height: (clamp(bottom.ceil()) - y).clamp(0, i64::from(u32::MAX)) as u32,
        }
    }
}
