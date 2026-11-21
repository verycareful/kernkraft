// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! CIE-based colour spaces, ISO 32000-1:2008, 8.6.5: CalGray (8.6.5.2),
//! CalRGB (8.6.5.3) and Lab (8.6.5.4), and the conversion of a CIE 1931
//! XYZ colour to the sRGB of the display.
//!
//! The XYZ of a colour is relative to the space's `WhitePoint`. It is
//! adapted to the sRGB white (D65) by the Bradford transform of ICC.1:2010,
//! Annex E.3, and then turned into linear sRGB by the matrix of IEC
//! 61966-2-1 and encoded with that standard's transfer function. A
//! `BlackPoint` is not applied: no black-point compensation is done.

use super::Rgb;

type Matrix = [[f64; 3]; 3];

/// ICC.1:2010, Annex E.3: the Bradford cone response matrix.
const BRADFORD: Matrix = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// IEC 61966-2-1: linear sRGB to XYZ, relative to D65.
const SRGB_TO_XYZ: Matrix = [
    [0.4124, 0.3576, 0.1805],
    [0.2126, 0.7152, 0.0722],
    [0.0193, 0.1192, 0.9505],
];

/// IEC 61966-2-1: the sRGB encoding is linear below this value of the
/// linear component,
const SRGB_LINEAR_LIMIT: f64 = 0.0031308;
/// with this slope,
const SRGB_LINEAR_SLOPE: f64 = 12.92;
/// and above it `1.055 * v^(1/2.4) - 0.055`.
const SRGB_GAMMA: f64 = 2.4;
const SRGB_SCALE: f64 = 1.055;
const SRGB_OFFSET: f64 = 0.055;

/// 8.6.5.4: the Lab function `g` is cubic from this value of its
/// argument, 6/29, and linear below it.
const LAB_KNEE: f64 = 6.0 / 29.0;
/// 8.6.5.4: `L* + 16` is divided by 116, `a*` by 500 and `b*` by 200.
const LAB_L_OFFSET: f64 = 16.0;
const LAB_L_DIVISOR: f64 = 116.0;
const LAB_A_DIVISOR: f64 = 500.0;
const LAB_B_DIVISOR: f64 = 200.0;
/// 8.6.5.4: the slope of `g` below the knee, `108/841`, and its offset,
/// `4/29`.
const LAB_LINEAR_SLOPE: f64 = 108.0 / 841.0;
const LAB_LINEAR_OFFSET: f64 = 4.0 / 29.0;

/// A CIE 1931 XYZ colour.
pub(crate) type Xyz = [f64; 3];

/// CalGray (8.6.5.2): `A` raised to `Gamma`, scaling the white point.
#[derive(Debug, Clone, PartialEq)]
pub struct CalGray {
    pub white_point: Xyz,
    pub black_point: Xyz,
    pub gamma: f64,
}

/// CalRGB (8.6.5.3).
#[derive(Debug, Clone, PartialEq)]
pub struct CalRgb {
    pub white_point: Xyz,
    pub black_point: Xyz,
    pub gamma: [f64; 3],
    /// `[XA YA ZA XB YB ZB XC YC ZC]`.
    pub matrix: [f64; 9],
}

/// Lab (8.6.5.4).
#[derive(Debug, Clone, PartialEq)]
pub struct Lab {
    pub white_point: Xyz,
    pub black_point: Xyz,
    /// `[amin amax bmin bmax]`.
    pub range: [f64; 4],
}

impl CalGray {
    pub(crate) fn to_rgb(&self, a: f64) -> Rgb {
        let a = a.clamp(0.0, 1.0).powf(self.gamma);
        xyz_to_rgb(self.white_point.map(|w| w * a), self.white_point)
    }
}

impl CalRgb {
    pub(crate) fn to_rgb(&self, abc: [f64; 3]) -> Rgb {
        let [a, b, c] = [0, 1, 2].map(|i| abc[i].clamp(0.0, 1.0).powf(self.gamma[i]));
        let m = &self.matrix;
        let xyz = [
            m[0] * a + m[3] * b + m[6] * c,
            m[1] * a + m[4] * b + m[7] * c,
            m[2] * a + m[5] * b + m[8] * c,
        ];
        xyz_to_rgb(xyz, self.white_point)
    }
}

impl Lab {
    /// `L*` is clipped to 0 to 100, `a*` and `b*` to the `Range`.
    pub(crate) fn to_rgb(&self, lab: [f64; 3]) -> Rgb {
        let l = lab[0].clamp(0.0, 100.0);
        let a = lab[1].clamp(self.range[0], self.range[1]);
        let b = lab[2].clamp(self.range[2], self.range[3]);
        let m = (l + LAB_L_OFFSET) / LAB_L_DIVISOR;
        let x = m + a / LAB_A_DIVISOR;
        let z = m - b / LAB_B_DIVISOR;
        let [xw, yw, zw] = self.white_point;
        xyz_to_rgb(
            [xw * lab_g(x), yw * lab_g(m), zw * lab_g(z)],
            self.white_point,
        )
    }
}

fn lab_g(x: f64) -> f64 {
    if x >= LAB_KNEE {
        x * x * x
    } else {
        LAB_LINEAR_SLOPE * (x - LAB_LINEAR_OFFSET)
    }
}

/// XYZ relative to `white` as sRGB, each component in 0 to 1.
pub(crate) fn xyz_to_rgb(xyz: Xyz, white: Xyz) -> Rgb {
    let d65 = mul_vec(&SRGB_TO_XYZ, [1.0, 1.0, 1.0]);
    let adapted = mul_vec(&adaptation(white, d65), xyz);
    let linear = match invert(&SRGB_TO_XYZ) {
        Some(inverse) => mul_vec(&inverse, adapted),
        None => adapted,
    };
    let [r, g, b] = linear.map(encode);
    Rgb { r, g, b }
}

/// The Bradford adaptation from `source` white to `target` white
/// (ICC.1:2010, E.3): `M^-1 * diag(target_cone / source_cone) * M`. A
/// white point with no cone response (all zero) adapts nothing.
fn adaptation(source: Xyz, target: Xyz) -> Matrix {
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (s, t) = (mul_vec(&BRADFORD, source), mul_vec(&BRADFORD, target));
    if s.contains(&0.0) {
        return identity;
    }
    let Some(inverse) = invert(&BRADFORD) else {
        return identity;
    };
    let scale = [
        [t[0] / s[0], 0.0, 0.0],
        [0.0, t[1] / s[1], 0.0],
        [0.0, 0.0, t[2] / s[2]],
    ];
    mul_mat(&inverse, &mul_mat(&scale, &BRADFORD))
}

/// The sRGB transfer function of IEC 61966-2-1, applied to a linear
/// component clipped to 0 to 1.
pub(crate) fn encode(linear: f64) -> f64 {
    let v = linear.clamp(0.0, 1.0);
    if v <= SRGB_LINEAR_LIMIT {
        SRGB_LINEAR_SLOPE * v
    } else {
        SRGB_SCALE * v.powf(1.0 / SRGB_GAMMA) - SRGB_OFFSET
    }
}

fn mul_vec(m: &Matrix, v: [f64; 3]) -> [f64; 3] {
    [0, 1, 2].map(|row| m[row][0] * v[0] + m[row][1] * v[1] + m[row][2] * v[2])
}

fn mul_mat(a: &Matrix, b: &Matrix) -> Matrix {
    [0, 1, 2].map(|row| [0, 1, 2].map(|col| (0..3).map(|k| a[row][k] * b[k][col]).sum()))
}

/// The inverse of a 3 by 3 matrix, `None` when it has none.
fn invert(m: &Matrix) -> Option<Matrix> {
    let cofactor = |r: usize, c: usize| {
        let (r1, r2) = ((r + 1) % 3, (r + 2) % 3);
        let (c1, c2) = ((c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let det = m[0][0] * cofactor(0, 0) + m[0][1] * cofactor(0, 1) + m[0][2] * cofactor(0, 2);
    if det == 0.0 || !det.is_finite() {
        return None;
    }
    Some([0, 1, 2].map(|r| [0, 1, 2].map(|c| cofactor(c, r) / det)))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const D65: Xyz = [0.9505, 1.0, 1.0890];

    fn close(a: Rgb, r: f64, g: f64, b: f64, tolerance: f64) {
        assert!(
            (a.r - r).abs() < tolerance
                && (a.g - g).abs() < tolerance
                && (a.b - b).abs() < tolerance,
            "{a:?} is not ({r}, {g}, {b})"
        );
    }

    #[test]
    fn the_inverse_of_the_srgb_matrix_undoes_it() {
        let inverse = invert(&SRGB_TO_XYZ).unwrap();
        let back = mul_mat(&inverse, &SRGB_TO_XYZ);
        for (r, row) in back.iter().enumerate() {
            for (c, value) in row.iter().enumerate() {
                let expected = if r == c { 1.0 } else { 0.0 };
                assert!((value - expected).abs() < 1e-12);
            }
        }
    }

    #[test]
    fn the_white_point_is_white() {
        close(xyz_to_rgb(D65, D65), 1.0, 1.0, 1.0, 1e-3);
        // A D50 white adapts to the same display white.
        close(
            xyz_to_rgb([0.9642, 1.0, 0.8249], [0.9642, 1.0, 0.8249]),
            1.0,
            1.0,
            1.0,
            1e-3,
        );
    }

    #[test]
    fn lab_extremes() {
        let lab = Lab {
            white_point: [0.9642, 1.0, 0.8249],
            black_point: [0.0; 3],
            range: [-128.0, 127.0, -128.0, 127.0],
        };
        close(lab.to_rgb([100.0, 0.0, 0.0]), 1.0, 1.0, 1.0, 2e-3);
        close(lab.to_rgb([0.0, 0.0, 0.0]), 0.0, 0.0, 0.0, 2e-3);
    }

    #[test]
    fn cal_gray_midpoint_follows_gamma() {
        let gray = CalGray {
            white_point: D65,
            black_point: [0.0; 3],
            gamma: 2.2,
        };
        // 0.5^2.2 of white, sRGB encoded.
        let linear = 0.5f64.powf(2.2);
        let expected = encode(linear);
        close(gray.to_rgb(0.5), expected, expected, expected, 1e-3);
    }
}
