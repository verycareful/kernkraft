// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The shadings that colour each point by a formula: ISO 32000-1:2008,
//! 8.7.4.5.2 (type 1, function-based), 8.7.4.5.3 (type 2, axial) and
//! 8.7.4.5.4 (type 3, radial).

use tiny_skia::Pixmap;

use crate::colour::ColourSpace;
use crate::content::Matrix;
use crate::PdfError;

use super::{opaque, Functions, Ramp, Target};

/// How close to zero the quadratic's leading coefficient of a radial
/// shading may be before the equation is solved as a linear one.
const NEARLY_LINEAR: f64 = 1e-12;

/// Calls `colour` with the shading-space point under each pixel's centre
/// and writes what it returns.
fn each_pixel(
    pixmap: &mut Pixmap,
    target: &Target<'_>,
    mut colour: impl FnMut(f64, f64) -> Result<Option<[u8; 3]>, PdfError>,
) -> Result<(), PdfError> {
    let width = target.area.width;
    let rows = pixmap.data_mut().chunks_exact_mut(width as usize * 4);
    for (y, row) in rows.enumerate() {
        target.check()?;
        for (x, pixel) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (px, py) = target.point(x as u32, y as u32);
            if let Some(rgb) = colour(px, py)? {
                *pixel = opaque(rgb);
            }
        }
    }
    Ok(())
}

/// Type 1: the function of the point, in the domain.
pub(super) fn function_based(
    pixmap: &mut Pixmap,
    target: &Target<'_>,
    space: &ColourSpace,
    domain: [f64; 4],
    matrix: &Matrix,
    functions: &Functions,
) -> Result<(), PdfError> {
    let Some(back) = matrix.inverse() else {
        return Ok(());
    };
    let [x0, x1, y0, y1] = domain;
    let mut values = Vec::new();
    each_pixel(pixmap, target, |x, y| {
        let (u, v) = back.apply(x, y);
        if !(x0..=x1).contains(&u) || !(y0..=y1).contains(&v) {
            return Ok(None);
        }
        functions.eval(&[u, v], &mut values)?;
        Ok(Some(space.to_rgb(&values)?.to_bytes()))
    })
}

/// Type 2: the parameter of the point's projection on the axis.
pub(super) fn axial(
    pixmap: &mut Pixmap,
    target: &Target<'_>,
    coords: [f64; 4],
    extend: [bool; 2],
    ramp: &Ramp,
) -> Result<(), PdfError> {
    let [x0, y0, x1, y1] = coords;
    let (dx, dy) = (x1 - x0, y1 - y0);
    let length_squared = dx * dx + dy * dy;
    if length_squared == 0.0 || !length_squared.is_finite() {
        return Ok(());
    }
    let [t0, t1] = ramp.domain;
    each_pixel(pixmap, target, |x, y| {
        let s = ((x - x0) * dx + (y - y0) * dy) / length_squared;
        if (s < 0.0 && !extend[0]) || (s > 1.0 && !extend[1]) {
            return Ok(None);
        }
        Ok(Some(ramp.at(t0 + s.clamp(0.0, 1.0) * (t1 - t0))))
    })
}

/// Type 3: the largest parameter at whose circle the point lies, among
/// those the shading covers.
pub(super) fn radial(
    pixmap: &mut Pixmap,
    target: &Target<'_>,
    coords: [f64; 6],
    extend: [bool; 2],
    ramp: &Ramp,
) -> Result<(), PdfError> {
    let [x0, y0, r0, x1, y1, r1] = coords;
    let (dx, dy, dr) = (x1 - x0, y1 - y0, r1 - r0);
    let a = dx * dx + dy * dy - dr * dr;
    let [t0, t1] = ramp.domain;
    each_pixel(pixmap, target, |x, y| {
        let (px, py) = (x - x0, y - y0);
        let b = px * dx + py * dy + r0 * dr;
        let c = px * px + py * py - r0 * r0;
        let covers =
            |s: f64| r0 + s * dr >= 0.0 && (s >= 0.0 || extend[0]) && (s <= 1.0 || extend[1]);
        let s = if a.abs() < NEARLY_LINEAR {
            if b == 0.0 {
                return Ok(None);
            }
            let s = c / (2.0 * b);
            covers(s).then_some(s)
        } else {
            let discriminant = b * b - a * c;
            if discriminant < 0.0 {
                return Ok(None);
            }
            let root = discriminant.sqrt();
            let (first, second) = ((b + root) / a, (b - root) / a);
            let (larger, smaller) = if first >= second {
                (first, second)
            } else {
                (second, first)
            };
            if covers(larger) {
                Some(larger)
            } else if covers(smaller) {
                Some(smaller)
            } else {
                None
            }
        };
        Ok(s.map(|s| ramp.at(t0 + s.clamp(0.0, 1.0) * (t1 - t0))))
    })
}
