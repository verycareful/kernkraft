// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Searching the bytes of a source for a keyword. This is below the format
//! and implements no clause of ISO 32000.

use crate::{PdfError, Source};

/// The bytes one read of a forward search fetches. No specification sets
/// it: searching in pieces keeps a large file out of memory, and a piece
/// this size costs few reads for each megabyte searched.
const SEARCH_CHUNK_BYTES: usize = 1 << 16;

/// The first position at or after `from` where `needle` starts in `bytes`.
pub(crate) fn find_in(bytes: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    bytes
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

/// The last position where `needle` starts in `bytes`.
pub(crate) fn find_last_in(bytes: &[u8], needle: &[u8]) -> Option<usize> {
    bytes
        .windows(needle.len())
        .rposition(|window| window == needle)
}

/// The first offset at or after `from` where `needle` starts in `source`.
pub(crate) fn find_forward(
    source: &dyn Source,
    needle: &[u8],
    from: u64,
) -> Result<Option<u64>, PdfError> {
    let size = source.len();
    // A match may straddle two reads, so each read starts this many bytes
    // before the end of the one before it.
    let overlap = needle.len().saturating_sub(1) as u64;
    let mut offset = from;
    while offset < size {
        let len = usize::try_from(size - offset)
            .map_or(SEARCH_CHUNK_BYTES, |rest| rest.min(SEARCH_CHUNK_BYTES));
        let chunk = source.read(offset, len)?;
        if let Some(position) = find_in(&chunk, needle, 0) {
            return Ok(Some(offset + position as u64));
        }
        let read = len as u64;
        if read <= overlap {
            break;
        }
        offset += read - overlap;
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::MemorySource;

    const NEEDLE: &[u8] = b"endstream";

    /// `len` bytes of filler with the needle written at `at`.
    fn haystack(len: usize, at: usize) -> MemorySource {
        let mut bytes = vec![b'.'; len];
        bytes[at..at + NEEDLE.len()].copy_from_slice(NEEDLE);
        MemorySource::new(bytes)
    }

    #[test]
    fn finds_a_needle_at_every_position_around_a_chunk_boundary() {
        let len = 2 * SEARCH_CHUNK_BYTES + 40;
        for at in SEARCH_CHUNK_BYTES - 2 * NEEDLE.len()..SEARCH_CHUNK_BYTES + NEEDLE.len() {
            let source = haystack(len, at);
            assert_eq!(
                find_forward(&source, NEEDLE, 0).unwrap(),
                Some(at as u64),
                "at {at}"
            );
        }
    }

    #[test]
    fn finds_from_an_offset_and_not_before_it() {
        let source = haystack(100, 20);
        assert_eq!(find_forward(&source, NEEDLE, 20).unwrap(), Some(20));
        assert_eq!(find_forward(&source, NEEDLE, 21).unwrap(), None);
        assert_eq!(find_forward(&source, NEEDLE, 100).unwrap(), None);
        assert_eq!(find_forward(&source, NEEDLE, 1000).unwrap(), None);
    }

    #[test]
    fn finds_at_the_very_end_and_misses_a_cut_needle() {
        let len = SEARCH_CHUNK_BYTES + 3;
        let source = haystack(len, len - NEEDLE.len());
        assert_eq!(
            find_forward(&source, NEEDLE, 0).unwrap(),
            Some((len - NEEDLE.len()) as u64)
        );
        let cut = MemorySource::new(b"....endstrea".to_vec());
        assert_eq!(find_forward(&cut, NEEDLE, 0).unwrap(), None);
    }

    #[test]
    fn slice_searches() {
        assert_eq!(find_in(b"abcabc", b"bc", 0), Some(1));
        assert_eq!(find_in(b"abcabc", b"bc", 2), Some(4));
        assert_eq!(find_in(b"abcabc", b"bc", 5), None);
        assert_eq!(find_in(b"abcabc", b"bc", 7), None);
        assert_eq!(find_last_in(b"abcabc", b"bc"), Some(4));
        assert_eq!(find_last_in(b"abcabc", b"x"), None);
        assert_eq!(find_last_in(b"b", b"bc"), None);
    }
}
