// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The set of characters a font has glyphs for, as sorted ranges.

/// Unicode code points, held as sorted, disjoint, non-adjacent inclusive
/// ranges. A CJK font covers tens of thousands of characters in a few
/// hundred ranges.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Coverage {
    ranges: Vec<(u32, u32)>,
}

impl Coverage {
    /// The coverage of `points`, which may be in any order and repeat.
    pub fn from_code_points(mut points: Vec<u32>) -> Coverage {
        points.sort_unstable();
        points.dedup();
        let mut ranges: Vec<(u32, u32)> = Vec::new();
        for p in points {
            match ranges.last_mut() {
                Some((_, end)) if *end + 1 == p => *end = p,
                _ => ranges.push((p, p)),
            }
        }
        Coverage { ranges }
    }

    pub fn contains(&self, c: char) -> bool {
        let c = u32::from(c);
        let i = self.ranges.partition_point(|&(_, end)| end < c);
        self.ranges.get(i).is_some_and(|&(start, _)| start <= c)
    }

    /// Whether every character of `text` is covered.
    pub fn covers_all(&self, text: &str) -> bool {
        text.chars().all(|c| self.contains(c))
    }

    /// The ranges, ascending, as inclusive `(first, last)` pairs.
    pub fn ranges(&self) -> &[(u32, u32)] {
        &self.ranges
    }

    /// How many characters are covered.
    pub fn len(&self) -> usize {
        self.ranges.iter().map(|&(a, b)| (b - a) as usize + 1).sum()
    }

    pub fn is_empty(&self) -> bool {
        self.ranges.is_empty()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn adjacent_points_merge_and_duplicates_vanish() {
        let c = Coverage::from_code_points(vec![5, 3, 4, 4, 10, 12, 11, 20]);
        assert_eq!(c.ranges(), &[(3, 5), (10, 12), (20, 20)]);
        assert_eq!(c.len(), 7);
    }

    #[test]
    fn membership_at_the_edges() {
        let c = Coverage::from_code_points(vec![0x41, 0x42, 0x43, 0x10FFFF]);
        assert!(c.contains('A') && c.contains('C') && c.contains('\u{10FFFF}'));
        assert!(!c.contains('@') && !c.contains('D') && !c.contains('\0'));
        assert!(c.covers_all("ABCA"));
        assert!(!c.covers_all("ABD"));
        assert!(c.covers_all(""));
    }

    #[test]
    fn empty_covers_only_the_empty_text() {
        let c = Coverage::default();
        assert!(c.is_empty());
        assert!(!c.contains('A'));
        assert!(c.covers_all(""));
    }
}
