// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Byte-at-a-time reading of a [`Source`], with seeking.
//!
//! The cursor holds one window of the source at a time, so reading a file
//! costs one read per window and never holds the file whole. This layer is
//! below the format and implements no clause of ISO 32000.

use std::borrow::Cow;

use crate::{PdfError, Source};

/// The bytes one read of the source fetches. No specification sets it: it
/// is large enough that a typical object is covered by one read, and small
/// enough that loading one object from a file reads little past it.
const WINDOW_BYTES: usize = 4096;

/// A position in a [`Source`] and a window of its bytes around it.
pub struct Cursor<'s> {
    source: &'s dyn Source,
    size: u64,
    window: Cow<'s, [u8]>,
    window_start: u64,
    position: u64,
}

impl<'s> Cursor<'s> {
    /// A cursor at the start of `source`.
    pub fn new(source: &'s dyn Source) -> Self {
        Self {
            source,
            size: source.len(),
            window: Cow::Borrowed(&[]),
            window_start: 0,
            position: 0,
        }
    }

    /// The offset of the next byte to be read.
    pub fn position(&self) -> u64 {
        self.position
    }

    /// The size of the source in bytes.
    pub fn len(&self) -> u64 {
        self.size
    }

    /// Whether the source holds no bytes.
    pub fn is_empty(&self) -> bool {
        self.size == 0
    }

    /// Moves `count` bytes on, stopping at the end of the source.
    pub(crate) fn advance(&mut self, count: u64) {
        self.position = self.position.saturating_add(count).min(self.size);
    }

    /// Moves to `offset`, which may be the end of the source. An offset past
    /// the end is [`PdfError::OutOfBounds`].
    pub fn seek(&mut self, offset: u64) -> Result<(), PdfError> {
        if offset > self.size {
            return Err(PdfError::OutOfBounds {
                offset,
                len: 0,
                size: self.size,
            });
        }
        self.position = offset;
        Ok(())
    }

    /// The byte at the position, or `None` at the end of the source.
    pub fn peek(&mut self) -> Result<Option<u8>, PdfError> {
        self.byte_at(self.position)
    }

    /// The byte `n` after the position, or `None` when that is at or past
    /// the end of the source.
    pub fn peek_ahead(&mut self, n: u64) -> Result<Option<u8>, PdfError> {
        match self.position.checked_add(n) {
            Some(offset) => self.byte_at(offset),
            None => Ok(None),
        }
    }

    /// The byte at the position, moving past it; `None` at the end of the
    /// source.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<u8>, PdfError> {
        let byte = self.peek()?;
        if byte.is_some() {
            self.position += 1;
        }
        Ok(byte)
    }

    /// The `len` bytes at the position, read straight from the source, and
    /// moves past them.
    pub fn read_exact(&mut self, len: usize) -> Result<Cow<'s, [u8]>, PdfError> {
        let bytes = self.source.read(self.position, len)?;
        // The read succeeded, so the range lies inside the source and its
        // end fits in a u64.
        self.position += len as u64;
        Ok(bytes)
    }

    fn byte_at(&mut self, offset: u64) -> Result<Option<u8>, PdfError> {
        if offset >= self.size {
            return Ok(None);
        }
        if let Some(byte) = self.in_window(offset) {
            return Ok(Some(byte));
        }
        // A window that starts at the position also covers a byte a little
        // ahead of it, so looking ahead and then reading on costs one read.
        let start = if offset >= self.position && offset - self.position < WINDOW_BYTES as u64 {
            self.position
        } else {
            offset
        };
        let len =
            usize::try_from(self.size - start).map_or(WINDOW_BYTES, |rest| rest.min(WINDOW_BYTES));
        self.window = self.source.read(start, len)?;
        self.window_start = start;
        Ok(self.in_window(offset))
    }

    fn in_window(&self, offset: u64) -> Option<u8> {
        let index = usize::try_from(offset.checked_sub(self.window_start)?).ok()?;
        self.window.get(index).copied()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::syntax::{Lexer, Span, Token, TokenKind};
    use crate::{Diagnostics, FileSource, MemorySource};

    /// Three full windows and a tail shorter than one.
    const SIZE: usize = 3 * WINDOW_BYTES + 5;
    /// A prime, so the pattern does not repeat on a window boundary.
    const PATTERN_PERIOD: usize = 251;

    fn pattern() -> Vec<u8> {
        (0..SIZE).map(|i| (i % PATTERN_PERIOD) as u8).collect()
    }

    /// Counts the reads made of a memory source and checks their length.
    struct Counting {
        inner: MemorySource,
        reads: AtomicUsize,
    }

    impl Source for Counting {
        fn len(&self) -> u64 {
            self.inner.len()
        }

        fn read(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>, PdfError> {
            assert!(len <= WINDOW_BYTES, "read of {len} bytes");
            self.reads.fetch_add(1, Ordering::Relaxed);
            self.inner.read(offset, len)
        }
    }

    #[test]
    fn next_walks_every_byte_across_windows() {
        let source = MemorySource::new(pattern());
        let mut cursor = Cursor::new(&source);
        let mut walked = Vec::new();
        while let Some(byte) = cursor.next().unwrap() {
            walked.push(byte);
        }
        assert_eq!(walked, pattern());
        assert_eq!(cursor.position(), SIZE as u64);
        assert_eq!(cursor.next().unwrap(), None);
        assert_eq!(cursor.position(), SIZE as u64);
    }

    #[test]
    fn walking_reads_one_window_at_a_time() {
        let source = Counting {
            inner: MemorySource::new(pattern()),
            reads: AtomicUsize::new(0),
        };
        let mut cursor = Cursor::new(&source);
        while cursor.next().unwrap().is_some() {}
        assert_eq!(
            source.reads.load(Ordering::Relaxed),
            SIZE.div_ceil(WINDOW_BYTES)
        );
    }

    #[test]
    fn peek_does_not_advance() {
        let source = MemorySource::new(pattern());
        let mut cursor = Cursor::new(&source);
        cursor.seek(7).unwrap();
        assert_eq!(cursor.peek().unwrap(), Some(pattern()[7]));
        assert_eq!(cursor.peek().unwrap(), Some(pattern()[7]));
        assert_eq!(cursor.position(), 7);
    }

    #[test]
    fn peek_ahead_crosses_a_window() {
        let source = Counting {
            inner: MemorySource::new(pattern()),
            reads: AtomicUsize::new(0),
        };
        let mut cursor = Cursor::new(&source);
        let last_of_first_window = WINDOW_BYTES - 1;
        cursor.seek(last_of_first_window as u64).unwrap();
        assert_eq!(
            cursor.peek().unwrap(),
            Some(pattern()[last_of_first_window])
        );
        assert_eq!(cursor.peek_ahead(1).unwrap(), Some(pattern()[WINDOW_BYTES]));
        assert_eq!(
            cursor.peek_ahead(2).unwrap(),
            Some(pattern()[WINDOW_BYTES + 1])
        );
        assert_eq!(cursor.position(), last_of_first_window as u64);
        // Looking ahead moved the window to the position, so reading on
        // through what was looked at costs no further read.
        let reads = source.reads.load(Ordering::Relaxed);
        for expected in &pattern()[last_of_first_window..=WINDOW_BYTES + 1] {
            assert_eq!(cursor.next().unwrap(), Some(*expected));
        }
        assert_eq!(source.reads.load(Ordering::Relaxed), reads);
    }

    #[test]
    fn peek_ahead_past_the_end_is_none() {
        let source = MemorySource::new(pattern());
        let mut cursor = Cursor::new(&source);
        cursor.seek(SIZE as u64 - 1).unwrap();
        assert_eq!(cursor.peek_ahead(0).unwrap(), Some(pattern()[SIZE - 1]));
        assert_eq!(cursor.peek_ahead(1).unwrap(), None);
        assert_eq!(cursor.peek_ahead(u64::MAX).unwrap(), None);
    }

    #[test]
    fn seek_backwards_and_forwards() {
        let source = MemorySource::new(pattern());
        let mut cursor = Cursor::new(&source);
        for offset in [2 * WINDOW_BYTES + 1, 1, SIZE - 1] {
            cursor.seek(offset as u64).unwrap();
            assert_eq!(cursor.peek().unwrap(), Some(pattern()[offset]));
        }
        cursor.seek(SIZE as u64).unwrap();
        assert_eq!(cursor.peek().unwrap(), None);
    }

    fn tokens(source: &dyn Source) -> Vec<Token> {
        let mut lexer = Lexer::new(source);
        let mut diagnostics = Diagnostics::new();
        let mut tokens = Vec::new();
        loop {
            let token = lexer.next_token(&mut diagnostics).unwrap();
            let done = token.kind == TokenKind::End;
            tokens.push(token);
            if done {
                assert!(diagnostics.is_empty());
                return tokens;
            }
        }
    }

    #[test]
    fn tokens_straddling_a_window_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("straddle.txt");
        for token in [
            &b"/Name#20X"[..],
            br"(a\053b)",
            b"(a\r\nb)",
            b"<4E6F>",
            b"-12.5",
            b">>",
            b"<<",
            b"endobj",
        ] {
            let mut alone = token.to_vec();
            alone.extend_from_slice(b" 7");
            let expected = tokens(&MemorySource::new(alone.clone()));

            for bytes_before_boundary in 1..token.len() {
                let padding = WINDOW_BYTES - bytes_before_boundary;
                let mut input = vec![b' '; padding];
                input.extend_from_slice(&alone);
                let shifted: Vec<Token> = expected
                    .iter()
                    .map(|token| Token {
                        kind: token.kind.clone(),
                        span: Span {
                            start: token.span.start + padding as u64,
                            end: token.span.end + padding as u64,
                        },
                    })
                    .collect();

                std::fs::write(&path, &input).unwrap();
                let file = FileSource::open(&path).unwrap();
                assert_eq!(tokens(&file), shifted, "file, {bytes_before_boundary}");
                let memory = MemorySource::new(input);
                assert_eq!(tokens(&memory), shifted, "memory, {bytes_before_boundary}");
            }
        }
    }
}
