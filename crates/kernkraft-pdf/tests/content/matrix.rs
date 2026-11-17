// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `Matrix`: ISO 32000-1, 8.3.4.

use kernkraft_pdf::content::Matrix;

use super::{assert_matrix, near};

#[test]
fn identity_leaves_a_point_where_it_is() {
    assert_eq!(Matrix::IDENTITY.apply(3.0, -4.0), (3.0, -4.0));
}

#[test]
fn a_point_is_carried_through_the_six_numbers() {
    // x' = a x + c y + e, y' = b x + d y + f.
    let m = Matrix::new(2.0, 3.0, 5.0, 7.0, 11.0, 13.0);
    assert_eq!(m.apply(1.0, 1.0), (2.0 + 5.0 + 11.0, 3.0 + 7.0 + 13.0));
}

#[test]
fn then_applies_self_first() {
    // Scale by 2, then move by (10, 20): (1, 1) -> (2, 2) -> (12, 22).
    let m = Matrix::scaling(2.0, 2.0).then(Matrix::translation(10.0, 20.0));
    assert_eq!(m.apply(1.0, 1.0), (12.0, 22.0));
    // The other order: move first, then scale: (1, 1) -> (11, 21) -> (22, 42).
    let m = Matrix::translation(10.0, 20.0).then(Matrix::scaling(2.0, 2.0));
    assert_eq!(m.apply(1.0, 1.0), (22.0, 42.0));
}

#[test]
fn then_matches_the_row_vector_product() {
    let a = Matrix::new(1.0, 2.0, 3.0, 4.0, 5.0, 6.0);
    let b = Matrix::new(7.0, 8.0, 9.0, 10.0, 11.0, 12.0);
    // [a b 0; c d 0; e f 1] x [a' b' 0; c' d' 0; e' f' 1], by hand.
    assert_matrix(
        a.then(b),
        [
            1.0 * 7.0 + 2.0 * 9.0,
            1.0 * 8.0 + 2.0 * 10.0,
            3.0 * 7.0 + 4.0 * 9.0,
            3.0 * 8.0 + 4.0 * 10.0,
            5.0 * 7.0 + 6.0 * 9.0 + 11.0,
            5.0 * 8.0 + 6.0 * 10.0 + 12.0,
        ],
    );
}

#[test]
fn the_inverse_undoes_the_matrix() {
    let m = Matrix::new(2.0, 1.0, -1.0, 3.0, 4.0, 5.0);
    let inverse = m.inverse().unwrap();
    assert_matrix(m.then(inverse), Matrix::IDENTITY.to_array());
}

#[test]
fn a_flattening_matrix_has_no_inverse() {
    assert!(Matrix::new(1.0, 2.0, 2.0, 4.0, 0.0, 0.0)
        .inverse()
        .is_none());
    assert!(Matrix::scaling(0.0, 1.0).inverse().is_none());
}

#[test]
fn the_determinant_is_ad_minus_bc() {
    assert!(near(
        Matrix::new(2.0, 3.0, 5.0, 7.0, 0.0, 0.0).determinant(),
        2.0 * 7.0 - 3.0 * 5.0
    ));
}

#[test]
fn an_array_round_trips() {
    let values = [1.0, 2.0, 3.0, 4.0, 5.0, 6.0];
    assert_eq!(Matrix::from_array(values).to_array(), values);
}
