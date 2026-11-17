// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A map from inclusive ranges of keys to values, in which a key has one
//! value: the first range given to cover it.
//!
//! The width arrays `W` and `W2` of a CIDFont (ISO 32000-1:2008, 9.7.4.3)
//! and the ranges of a CMap (9.7.5) say a value for a range, and a file may
//! give overlapping ranges. A range is held as it is and never expanded to
//! its keys, so `0 4294967295 w` costs one entry.

use std::collections::BTreeMap;

#[derive(Debug, Clone)]
pub(crate) struct RangeMap<V> {
    segments: BTreeMap<u64, (u64, V)>,
}

impl<V> Default for RangeMap<V> {
    fn default() -> Self {
        Self {
            segments: BTreeMap::new(),
        }
    }
}

impl<V: Clone> RangeMap<V> {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    /// Gives the keys `first` to `last` that have no value yet `value`.
    /// True when some key of the range had one already, which is kept.
    pub(crate) fn insert(&mut self, first: u64, last: u64, value: V) -> bool {
        if first > last {
            return false;
        }
        let mut covered: Vec<(u64, u64)> = self
            .segments
            .range(..=last)
            .rev()
            .take_while(|(_, (end, _))| *end >= first)
            .map(|(start, (end, _))| (*start, *end))
            .collect();
        covered.reverse();
        let overlapped = !covered.is_empty();
        // The first key not yet known to have a value, or none once a
        // segment reaches the largest key.
        let mut next = Some(first);
        let mut gaps = Vec::new();
        for (start, end) in covered {
            if let Some(from) = next.filter(|from| start > *from) {
                gaps.push((from, start - 1));
            }
            next = next.and_then(|from| end.checked_add(1).map(|after| from.max(after)));
        }
        if let Some(from) = next.filter(|from| *from <= last) {
            gaps.push((from, last));
        }
        for (start, end) in gaps {
            self.segments.insert(start, (end, value.clone()));
        }
        overlapped
    }

    /// The value of `key` and the first key of the segment it is in.
    pub(crate) fn get(&self, key: u64) -> Option<(u64, &V)> {
        let (start, (end, value)) = self.segments.range(..=key).next_back()?;
        (*end >= key).then_some((*start, value))
    }

    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.segments.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(map: &RangeMap<u32>, key: u64) -> Option<u32> {
        map.get(key).map(|(_, v)| *v)
    }

    #[test]
    fn a_range_is_one_entry() {
        let mut map = RangeMap::new();
        assert!(!map.insert(0, u64::from(u32::MAX), 7));
        assert_eq!(map.len(), 1);
        assert_eq!(value(&map, 0), Some(7));
        assert_eq!(value(&map, u64::from(u32::MAX)), Some(7));
        assert_eq!(value(&map, u64::from(u32::MAX) + 1), None);
    }

    #[test]
    fn the_first_range_given_keeps_its_keys() {
        let mut map = RangeMap::new();
        assert!(!map.insert(10, 20, 1));
        assert!(map.insert(5, 25, 2));
        for key in 5..=9 {
            assert_eq!(value(&map, key), Some(2));
        }
        for key in 10..=20 {
            assert_eq!(value(&map, key), Some(1));
        }
        for key in 21..=25 {
            assert_eq!(value(&map, key), Some(2));
        }
        assert_eq!(value(&map, 4), None);
        assert_eq!(value(&map, 26), None);
    }

    #[test]
    fn a_range_inside_another_changes_nothing() {
        let mut map = RangeMap::new();
        map.insert(0, 100, 1);
        assert!(map.insert(40, 60, 2));
        assert_eq!(map.len(), 1);
        assert_eq!(value(&map, 50), Some(1));
    }

    #[test]
    fn the_gap_between_two_ranges_is_filled() {
        let mut map = RangeMap::new();
        map.insert(0, 4, 1);
        map.insert(10, 14, 2);
        assert!(map.insert(0, 14, 3));
        assert_eq!(value(&map, 2), Some(1));
        assert_eq!(value(&map, 7), Some(3));
        assert_eq!(value(&map, 12), Some(2));
    }

    #[test]
    fn a_range_to_the_largest_key_is_kept_whole() {
        let mut map = RangeMap::new();
        map.insert(5, u64::MAX, 1);
        assert!(map.insert(0, u64::MAX, 2));
        assert_eq!(value(&map, 3), Some(2));
        assert_eq!(value(&map, u64::MAX), Some(1));
    }

    #[test]
    fn the_segment_start_is_reported() {
        let mut map = RangeMap::new();
        map.insert(10, 20, 1);
        assert_eq!(map.get(15).map(|(start, _)| start), Some(10));
    }
}
