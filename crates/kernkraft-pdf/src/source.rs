// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Random access to the bytes of one PDF file.
//!
//! A source has a size, fixed when it is created, and hands out any range of
//! its bytes. A reader asks for the ranges it needs, so a file is never held
//! in memory whole unless the caller chose [`MemorySource`].

use std::borrow::Cow;
use std::fs::File;
use std::io::ErrorKind;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use kernkraft_fs::FsError;

use crate::PdfError;

/// Random access to the bytes of one PDF file.
pub trait Source: Send + Sync {
    /// The size in bytes, fixed when the source was created.
    fn len(&self) -> u64;

    /// Whether the source holds no bytes.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The `len` bytes starting at `offset`: borrowed when the source holds
    /// them in memory, owned when they are read from a file.
    ///
    /// A range that does not lie inside the source is
    /// [`PdfError::OutOfBounds`], decided before anything is allocated or
    /// read. A zero-length read at the end of the source is an empty slice.
    fn read(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>, PdfError>;
}

/// The end of the range `offset..offset + len`, when the range lies inside
/// a source of `size` bytes.
fn range_end(offset: u64, len: usize, size: u64) -> Result<u64, PdfError> {
    // A usize wider than 64 bits cannot name a range inside a u64-sized
    // source, so the saturated value fails the same check.
    let len = u64::try_from(len).unwrap_or(u64::MAX);
    match offset.checked_add(len) {
        Some(end) if end <= size => Ok(end),
        _ => Err(PdfError::OutOfBounds { offset, len, size }),
    }
}

/// A source whose bytes are held in memory. Clones share the bytes.
#[derive(Clone, Debug)]
pub struct MemorySource {
    bytes: Arc<[u8]>,
}

impl MemorySource {
    pub fn new(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }
}

impl Source for MemorySource {
    fn len(&self) -> u64 {
        // A slice is at most isize::MAX bytes, which fits in a u64.
        self.bytes.len() as u64
    }

    fn read(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>, PdfError> {
        let size = self.len();
        let end = range_end(offset, len, size)?;
        let out_of_bounds = || PdfError::OutOfBounds {
            offset,
            len: end - offset,
            size,
        };
        let start = usize::try_from(offset).map_err(|_| out_of_bounds())?;
        let end = usize::try_from(end).map_err(|_| out_of_bounds())?;
        self.bytes
            .get(start..end)
            .map(Cow::Borrowed)
            .ok_or_else(out_of_bounds)
    }
}

/// A source that reads a file by offset. Reads take `&self` and carry their
/// own offset, so threads sharing one source do not disturb each other.
#[derive(Debug)]
pub struct FileSource {
    file: File,
    path: PathBuf,
    size: u64,
}

impl FileSource {
    /// Opens `path` for reading and records its size. A missing file is
    /// [`FsError::NotFound`]; any other failure is [`FsError::Io`], and so
    /// is a path that is not a regular file.
    pub fn open(path: &Path) -> Result<Self, PdfError> {
        let io_error = |source: std::io::Error| {
            if source.kind() == ErrorKind::NotFound {
                FsError::NotFound {
                    path: path.to_path_buf(),
                }
            } else {
                FsError::Io {
                    path: path.to_path_buf(),
                    source,
                }
            }
        };
        let file = File::open(path).map_err(io_error)?;
        let metadata = file.metadata().map_err(io_error)?;
        // A directory opens read-only and a pipe or device reports no size,
        // so each would pass for an empty file.
        if metadata.is_dir() {
            return Err(io_error(ErrorKind::IsADirectory.into()).into());
        }
        if !metadata.is_file() {
            return Err(io_error(std::io::Error::new(
                ErrorKind::InvalidInput,
                "not a regular file",
            ))
            .into());
        }
        let size = metadata.len();
        Ok(Self {
            file,
            path: path.to_path_buf(),
            size,
        })
    }

    /// The path the source was opened from.
    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Source for FileSource {
    fn len(&self) -> u64 {
        self.size
    }

    /// A file that has become shorter than it was when opened is
    /// [`FsError::ExternalModification`]: the bytes asked for are gone.
    fn read(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>, PdfError> {
        range_end(offset, len, self.size)?;
        let mut buffer = vec![0; len];
        self.file
            .read_exact_at(&mut buffer, offset)
            .map_err(|source| {
                if source.kind() == ErrorKind::UnexpectedEof {
                    FsError::ExternalModification {
                        path: self.path.clone(),
                    }
                } else {
                    FsError::Io {
                        path: self.path.clone(),
                        source,
                    }
                }
            })?;
        Ok(Cow::Owned(buffer))
    }
}
