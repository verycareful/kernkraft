// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Choosing a face from a [`FontList`]: by family name, then by the closest
//! width, slant and weight, in the order of the font matching algorithm of
//! CSS Fonts Level 4 (section 5.2): width first, then style, then weight;
//! and by the characters a face covers.

use std::cmp::Ordering;

use crate::list::{FaceInfo, FontList};

/// What is wanted.
#[derive(Debug, Clone)]
pub struct Query<'a> {
    /// Family names in order of preference. Names compare without regard to
    /// case. Aliases (Helvetica for Arial) are the caller's business.
    pub families: &'a [&'a str],
    /// 1 to 1000; 400 Regular, 700 Bold.
    pub weight: u16,
    /// 1 to 9; 5 is normal.
    pub width: u8,
    pub italic: bool,
}

impl Default for Query<'_> {
    fn default() -> Self {
        Query {
            families: &[],
            weight: 400,
            width: 5,
            italic: false,
        }
    }
}

/// Where a candidate falls in the search order away from the wanted value:
/// `(group, distance)`, smaller first.
type Key = (u8, u32);

/// Width: if the wanted width is normal or narrower, narrower faces come
/// first, closest first, then wider ones; otherwise wider first.
fn width_key(wanted: u8, have: u8) -> Key {
    let (w, h) = (u32::from(wanted), u32::from(have));
    if wanted <= 5 {
        if h <= w {
            (0, w - h)
        } else {
            (1, h - w)
        }
    } else if h >= w {
        (0, h - w)
    } else {
        (1, w - h)
    }
}

/// Weight (section 5.2 of the same module): a wanted weight from 400 to 500
/// tries weights from it up to 500 first, then lighter ones descending, then
/// heavier than 500 ascending; below 400 tries lighter ones descending then
/// heavier ascending; above 500 tries heavier ascending then lighter
/// descending.
fn weight_key(wanted: u16, have: u16) -> Key {
    let (w, h) = (u32::from(wanted), u32::from(have));
    if (400..=500).contains(&wanted) {
        if h >= w && h <= 500 {
            (0, h - w)
        } else if h < w {
            (1, w - h)
        } else {
            (2, h - 500)
        }
    } else if wanted < 400 {
        if h <= w {
            (0, w - h)
        } else {
            (1, h - w)
        }
    } else if h >= w {
        (0, h - w)
    } else {
        (1, w - h)
    }
}

fn style_order(q: &Query<'_>, a: &FaceInfo, b: &FaceInfo) -> Ordering {
    let key = |f: &FaceInfo| {
        (
            width_key(q.width, f.width),
            f.italic != q.italic,
            weight_key(q.weight, f.weight),
            f.path.clone(),
            f.index,
        )
    };
    key(a).cmp(&key(b))
}

fn has_family(face: &FaceInfo, family: &str) -> bool {
    let want = family.to_lowercase();
    face.families.iter().any(|f| f.to_lowercase() == want)
}

impl FontList {
    /// The faces of the families in `q`, in the order of the families, each
    /// family's faces from the closest style to the farthest. A face
    /// appears once.
    pub fn by_family<'l>(&'l self, q: &Query<'_>) -> Vec<&'l FaceInfo> {
        let mut out: Vec<&FaceInfo> = Vec::new();
        for family in q.families {
            let mut faces: Vec<&FaceInfo> = self
                .faces
                .iter()
                .filter(|f| has_family(f, family) && !out.contains(f))
                .collect();
            faces.sort_by(|a, b| style_order(q, a, b));
            out.extend(faces);
        }
        out
    }

    /// The closest face of the first family in `q` that the list has.
    pub fn best<'l>(&'l self, q: &Query<'_>) -> Option<&'l FaceInfo> {
        q.families.iter().find_map(|family| {
            self.faces
                .iter()
                .filter(|f| has_family(f, family))
                .min_by(|a, b| style_order(q, a, b))
        })
    }

    /// A face with a glyph for `c`: from the families of `q` when one has it
    /// (the closest style), else from any face of the list by closeness of
    /// style. Faces whose coverage is unknown are never chosen.
    pub fn covering<'l>(&'l self, c: char, q: &Query<'_>) -> Option<&'l FaceInfo> {
        if let Some(face) = self.by_family(q).into_iter().find(|f| f.covers(c)) {
            return Some(face);
        }
        self.faces
            .iter()
            .filter(|f| f.covers(c))
            .min_by(|a, b| style_order(q, a, b))
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn weights(wanted: u16, have: &[u16]) -> Vec<u16> {
        let mut v = have.to_vec();
        v.sort_by_key(|&h| weight_key(wanted, h));
        v
    }

    #[test]
    fn weight_order_for_a_wanted_400() {
        // 400 first, then 500 (between 400 and 500 ascending), then lighter
        // descending, then heavier than 500 ascending.
        assert_eq!(
            weights(400, &[900, 700, 600, 500, 400, 300, 100]),
            vec![400, 500, 300, 100, 600, 700, 900]
        );
    }

    #[test]
    fn weight_order_for_a_wanted_500() {
        assert_eq!(
            weights(500, &[900, 700, 600, 500, 400, 300]),
            vec![500, 400, 300, 600, 700, 900]
        );
    }

    #[test]
    fn weight_order_for_a_wanted_below_400() {
        assert_eq!(
            weights(300, &[100, 200, 300, 400, 700]),
            vec![300, 200, 100, 400, 700]
        );
    }

    #[test]
    fn weight_order_for_a_wanted_above_500() {
        assert_eq!(
            weights(700, &[100, 400, 600, 700, 800, 900]),
            vec![700, 800, 900, 600, 400, 100]
        );
    }

    #[test]
    fn width_order() {
        let order = |wanted: u8, have: &[u8]| {
            let mut v = have.to_vec();
            v.sort_by_key(|&h| width_key(wanted, h));
            v
        };
        // Normal or narrower wanted: narrower first, then wider.
        assert_eq!(order(5, &[9, 7, 5, 4, 2]), vec![5, 4, 2, 7, 9]);
        assert_eq!(order(3, &[1, 2, 3, 4, 8]), vec![3, 2, 1, 4, 8]);
        // Wider than normal wanted: wider first, then narrower.
        assert_eq!(order(7, &[9, 7, 6, 5, 2]), vec![7, 9, 6, 5, 2]);
    }
}
