// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The buffer every decoder writes into. It refuses to grow past the size
//! the caller of the pipeline allowed, so no encoded input decides how much
//! memory decoding takes.

use crate::PdfError;

/// Decoded bytes, held to a limit.
#[derive(Debug)]
pub(super) struct Output {
    bytes: Vec<u8>,
    /// The most bytes this buffer holds.
    limit: usize,
    /// The cap the caller passed, which the error reports. It is below
    /// `limit` when the buffer also holds bytes a later pass removes.
    cap: usize,
}

impl Output {
    /// A buffer of at most `cap + allowance` bytes, for data that carries
    /// `allowance` bytes which do not reach the result.
    pub(super) fn new(cap: usize, allowance: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit: cap.saturating_add(allowance),
            cap,
        }
    }

    pub(super) fn push(&mut self, byte: u8) -> Result<(), PdfError> {
        self.room_for(1)?;
        self.bytes.push(byte);
        Ok(())
    }

    pub(super) fn extend(&mut self, bytes: &[u8]) -> Result<(), PdfError> {
        self.room_for(bytes.len())?;
        self.bytes.extend_from_slice(bytes);
        Ok(())
    }

    /// Appends `byte` `count` times.
    pub(super) fn repeat(&mut self, byte: u8, count: usize) -> Result<(), PdfError> {
        self.room_for(count)?;
        self.bytes.resize(self.bytes.len() + count, byte);
        Ok(())
    }

    pub(super) fn into_vec(self) -> Vec<u8> {
        self.bytes
    }

    /// Fails, adding nothing, when `more` bytes would pass the limit.
    fn room_for(&self, more: usize) -> Result<(), PdfError> {
        // The length never passes the limit, so the subtraction holds.
        if more > self.limit - self.bytes.len() {
            return Err(too_large(self.cap));
        }
        Ok(())
    }
}

/// The error for decoded data that passes `cap` bytes.
pub(super) fn too_large(cap: usize) -> PdfError {
    PdfError::DecodedTooLarge {
        // A usize is at most 64 bits wide on every supported target.
        cap: cap as u64,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn assert_too_large(result: Result<(), PdfError>, expected_cap: u64) {
        match result {
            Err(PdfError::DecodedTooLarge { cap }) => assert_eq!(cap, expected_cap),
            other => panic!("expected DecodedTooLarge, got {other:?}"),
        }
    }

    #[test]
    fn fills_to_the_cap_and_no_further() {
        let mut out = Output::new(10, 0);
        out.extend(&[7; 10]).unwrap();
        assert_too_large(out.push(1), 10);
        assert_eq!(out.into_vec(), [7; 10]);
    }

    #[test]
    fn extend_past_the_cap_adds_nothing() {
        let mut out = Output::new(4, 0);
        out.extend(b"ab").unwrap();
        assert_too_large(out.extend(b"cde"), 4);
        assert_eq!(out.into_vec(), b"ab");
    }

    #[test]
    fn repeat_past_the_cap_fails_without_growing() {
        let mut out = Output::new(4, 0);
        out.push(9).unwrap();
        assert_too_large(out.repeat(0, usize::MAX), 4);
        out.repeat(5, 3).unwrap();
        assert_eq!(out.into_vec(), [9, 5, 5, 5]);
    }

    #[test]
    fn cap_of_zero_holds_nothing() {
        let mut out = Output::new(0, 0);
        out.extend(&[]).unwrap();
        out.repeat(1, 0).unwrap();
        assert_too_large(out.push(1), 0);
        assert!(out.into_vec().is_empty());
    }

    #[test]
    fn allowance_raises_the_limit_and_not_the_reported_cap() {
        let mut out = Output::new(4, 2);
        out.extend(&[1; 6]).unwrap();
        assert_too_large(out.push(1), 4);
        let mut saturated = Output::new(usize::MAX, 5);
        saturated.push(1).unwrap();
    }
}
