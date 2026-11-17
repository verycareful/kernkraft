// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Transformation matrices: ISO 32000-1:2008, 8.3.3 (common transformations)
//! and 8.3.4 (transformation matrices).

/// A matrix `[a b c d e f]` (8.3.4), the third column of the 3 by 3 matrix
/// `[a b 0; c d 0; e f 1]` being fixed. A point `(x, y)` maps to
/// `(a*x + c*y + e, b*x + d*y + f)`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Matrix {
    pub a: f64,
    pub b: f64,
    pub c: f64,
    pub d: f64,
    pub e: f64,
    pub f: f64,
}

impl Matrix {
    /// The identity: nothing moves.
    pub const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    pub const fn new(a: f64, b: f64, c: f64, d: f64, e: f64, f: f64) -> Self {
        Self { a, b, c, d, e, f }
    }

    /// A translation by `(tx, ty)` (8.3.3).
    pub const fn translation(tx: f64, ty: f64) -> Self {
        Self::new(1.0, 0.0, 0.0, 1.0, tx, ty)
    }

    /// A scaling by `sx` and `sy` (8.3.3).
    pub const fn scaling(sx: f64, sy: f64) -> Self {
        Self::new(sx, 0.0, 0.0, sy, 0.0, 0.0)
    }

    /// The matrix that applies `self` first and `next` after it: the
    /// product `self x next` of 8.3.4, which is how `cm` and `Tm` combine
    /// a new matrix with the current one (`new x current`).
    pub fn then(self, next: Matrix) -> Matrix {
        Matrix {
            a: self.a * next.a + self.b * next.c,
            b: self.a * next.b + self.b * next.d,
            c: self.c * next.a + self.d * next.c,
            d: self.c * next.b + self.d * next.d,
            e: self.e * next.a + self.f * next.c + next.e,
            f: self.e * next.b + self.f * next.d + next.f,
        }
    }

    /// The point `(x, y)` carried through the matrix.
    pub fn apply(&self, x: f64, y: f64) -> (f64, f64) {
        (
            self.a * x + self.c * y + self.e,
            self.b * x + self.d * y + self.f,
        )
    }

    /// The determinant `a*d - b*c`: zero when the matrix flattens the plane.
    pub fn determinant(&self) -> f64 {
        self.a * self.d - self.b * self.c
    }

    /// The matrix that undoes this one, or `None` when it is singular
    /// (or so close to it that the inverse is not finite).
    pub fn inverse(&self) -> Option<Matrix> {
        let det = self.determinant();
        if det == 0.0 || !det.is_finite() {
            return None;
        }
        let inverse = Matrix {
            a: self.d / det,
            b: -self.b / det,
            c: -self.c / det,
            d: self.a / det,
            e: (self.c * self.f - self.d * self.e) / det,
            f: (self.b * self.e - self.a * self.f) / det,
        };
        let finite = [
            inverse.a, inverse.b, inverse.c, inverse.d, inverse.e, inverse.f,
        ]
        .iter()
        .all(|value| value.is_finite());
        finite.then_some(inverse)
    }

    /// The six numbers in the order a PDF array writes them.
    pub fn to_array(&self) -> [f64; 6] {
        [self.a, self.b, self.c, self.d, self.e, self.f]
    }

    /// The matrix an array of six numbers gives.
    pub fn from_array(values: [f64; 6]) -> Self {
        let [a, b, c, d, e, f] = values;
        Self { a, b, c, d, e, f }
    }
}

impl Default for Matrix {
    fn default() -> Self {
        Self::IDENTITY
    }
}
