// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::fs::OpenOptions;

use kernkraft_fs::FsError;
use kernkraft_pdf::syntax::Cursor;
use kernkraft_pdf::{FileSource, PdfError};

use crate::{both, pattern};

const SIZE: usize = 1000;
const SIZE_U64: u64 = 1000;

#[test]
fn seek_past_the_end_is_out_of_bounds() {
    let sources = both(&pattern(SIZE));
    for (kind, source) in &sources.all {
        let mut cursor = Cursor::new(source.as_ref());
        assert!(cursor.seek(SIZE_U64).is_ok(), "{kind}");
        assert_eq!(cursor.peek().unwrap(), None, "{kind}");
        match cursor.seek(SIZE_U64 + 1) {
            Err(PdfError::OutOfBounds { offset, len, size }) => {
                assert_eq!((offset, len, size), (SIZE_U64 + 1, 0, SIZE_U64), "{kind}");
            }
            other => panic!("{kind}: expected OutOfBounds, got {other:?}"),
        }
    }
}

#[test]
fn read_exact_returns_the_range_and_advances() {
    let bytes = pattern(SIZE);
    let sources = both(&bytes);
    for (kind, source) in &sources.all {
        let mut cursor = Cursor::new(source.as_ref());
        cursor.seek(300).unwrap();
        let range = cursor.read_exact(50).unwrap();
        assert_eq!(&range[..], &bytes[300..350], "{kind}");
        assert_eq!(cursor.position(), 350, "{kind}");
        assert_eq!(cursor.next().unwrap(), Some(bytes[350]), "{kind}");
        assert_eq!(cursor.position(), 351, "{kind}");
    }
}

#[test]
fn empty_source_is_at_its_end() {
    let sources = both(&[]);
    for (kind, source) in &sources.all {
        let mut cursor = Cursor::new(source.as_ref());
        assert_eq!(cursor.len(), 0, "{kind}");
        assert_eq!(cursor.peek().unwrap(), None, "{kind}");
        assert_eq!(cursor.next().unwrap(), None, "{kind}");
        assert_eq!(cursor.position(), 0, "{kind}");
    }
}

#[test]
fn a_file_that_shrank_fails_the_next_read() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shrinks.bin");
    std::fs::write(&path, pattern(SIZE)).unwrap();
    let source = FileSource::open(&path).unwrap();
    OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(100)
        .unwrap();

    let mut cursor = Cursor::new(&source);
    cursor.seek(500).unwrap();
    match cursor.peek() {
        Err(PdfError::Fs(FsError::ExternalModification { path: reported })) => {
            assert_eq!(reported, path);
        }
        other => panic!("expected ExternalModification, got {other:?}"),
    }
}
