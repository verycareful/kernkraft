// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::Arc;

use kernkraft_fs::FsError;
use kernkraft_pdf::{FileSource, MemorySource, PdfError, Source};

const SIZE: usize = 1000;
const SIZE_U64: u64 = 1000;
/// A prime, so the pattern does not repeat on a power-of-two boundary.
const PATTERN_PERIOD: usize = 251;

/// `n` bytes where byte `i` is `i % PATTERN_PERIOD`.
fn pattern(n: usize) -> Vec<u8> {
    (0..n).map(|i| (i % PATTERN_PERIOD) as u8).collect()
}

struct Sources {
    // Keeps the file alive for as long as the sources are used.
    _dir: tempfile::TempDir,
    path: PathBuf,
    all: Vec<(&'static str, Arc<dyn Source>)>,
}

/// A memory source and a file source over the same `pattern(n)` bytes.
fn both(n: usize) -> Sources {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.bin");
    std::fs::write(&path, pattern(n)).unwrap();
    let all: Vec<(&'static str, Arc<dyn Source>)> = vec![
        ("memory", Arc::new(MemorySource::new(pattern(n)))),
        ("file", Arc::new(FileSource::open(&path).unwrap())),
    ];
    Sources {
        _dir: dir,
        path,
        all,
    }
}

fn assert_out_of_bounds(
    result: Result<Cow<'_, [u8]>, PdfError>,
    offset: u64,
    len: u64,
    name: &str,
) {
    match result {
        Err(PdfError::OutOfBounds {
            offset: o,
            len: l,
            size,
        }) => assert_eq!((o, l, size), (offset, len, SIZE_U64), "{name}"),
        other => panic!("{name}: {other:?}"),
    }
}

#[test]
fn len_is_the_byte_count() {
    for (name, source) in both(SIZE).all {
        assert_eq!(source.len(), SIZE_U64, "{name}");
        assert!(!source.is_empty(), "{name}");
    }
}

#[test]
fn reads_a_range_from_the_middle() {
    for (name, source) in both(SIZE).all {
        let bytes = source.read(300, 50).unwrap();
        assert_eq!(&bytes[..], &pattern(SIZE)[300..350], "{name}");
    }
}

#[test]
fn reads_the_whole_source() {
    for (name, source) in both(SIZE).all {
        let bytes = source.read(0, SIZE).unwrap();
        assert_eq!(&bytes[..], &pattern(SIZE)[..], "{name}");
    }
}

#[test]
fn zero_length_read_at_the_end_is_empty() {
    for (name, source) in both(SIZE).all {
        assert!(source.read(SIZE_U64, 0).unwrap().is_empty(), "{name}");
    }
}

#[test]
fn empty_source_reads_empty() {
    for (name, source) in both(0).all {
        assert_eq!(source.len(), 0, "{name}");
        assert!(source.is_empty(), "{name}");
        assert!(source.read(0, 0).unwrap().is_empty(), "{name}");
    }
}

#[test]
fn read_past_the_end_is_out_of_bounds() {
    for (name, source) in both(SIZE).all {
        assert_out_of_bounds(source.read(990, 11), 990, 11, name);
    }
}

#[test]
fn offset_past_the_end_is_out_of_bounds() {
    for (name, source) in both(SIZE).all {
        assert_out_of_bounds(source.read(1001, 0), 1001, 0, name);
    }
}

#[test]
fn overflowing_range_is_out_of_bounds() {
    for (name, source) in both(SIZE).all {
        assert_out_of_bounds(source.read(u64::MAX, 2), u64::MAX, 2, name);
    }
}

#[test]
fn huge_length_does_not_allocate() {
    for (name, source) in both(SIZE).all {
        // Returning at all is the proof: an allocation of this size aborts.
        assert_out_of_bounds(source.read(0, usize::MAX), 0, usize::MAX as u64, name);
    }
}

#[test]
fn memory_source_borrows() {
    let source = MemorySource::new(pattern(SIZE));
    assert!(matches!(source.read(10, 10).unwrap(), Cow::Borrowed(_)));
}

#[test]
fn file_source_knows_its_path() {
    let sources = both(SIZE);
    let source = FileSource::open(&sources.path).unwrap();
    assert_eq!(source.path(), sources.path);
}

#[test]
fn missing_file_is_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let missing = dir.path().join("none.pdf");
    match FileSource::open(&missing) {
        Err(PdfError::Fs(FsError::NotFound { path })) => assert_eq!(path, missing),
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_directory_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    match FileSource::open(dir.path()) {
        Err(PdfError::Fs(FsError::Io { path, source })) => {
            assert_eq!(path, dir.path());
            assert_eq!(source.kind(), std::io::ErrorKind::IsADirectory);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_pipe_is_an_io_error() {
    let dir = tempfile::tempdir().unwrap();
    let fifo = dir.path().join("pipe");
    let status = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .unwrap();
    assert!(status.success());
    // A reader blocks in open(2) until a writer arrives, so one is held
    // open (read and write, which never blocks on Linux) for the call.
    let _writer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&fifo)
        .unwrap();
    match FileSource::open(&fifo) {
        Err(PdfError::Fs(FsError::Io { path, source })) => {
            assert_eq!(path, fifo);
            assert_eq!(source.kind(), std::io::ErrorKind::InvalidInput);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn a_file_that_shrank_is_reported() {
    let sources = both(SIZE);
    let source = FileSource::open(&sources.path).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&sources.path)
        .unwrap()
        .set_len(100)
        .unwrap();
    assert_eq!(source.len(), SIZE_U64);
    match source.read(500, 10) {
        Err(PdfError::Fs(FsError::ExternalModification { path })) => {
            assert_eq!(path, sources.path)
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn threads_read_their_own_ranges() {
    const THREADS: usize = 8;
    const CHUNK: usize = 100;
    const ROUNDS: usize = 200;
    for (name, source) in both(SIZE).all {
        let handles: Vec<_> = (0..THREADS)
            .map(|t| {
                let source = Arc::clone(&source);
                std::thread::spawn(move || {
                    let start = t * CHUNK;
                    let expected = &pattern(SIZE)[start..start + CHUNK];
                    for _ in 0..ROUNDS {
                        let bytes = source.read(start as u64, CHUNK).unwrap();
                        assert_eq!(&bytes[..], expected);
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap_or_else(|_| panic!("{name}"));
        }
    }
}

#[test]
fn source_is_object_safe_and_shareable() {
    fn assert_send_sync<T: Send + Sync + ?Sized>() {}
    assert_send_sync::<dyn Source>();
}
