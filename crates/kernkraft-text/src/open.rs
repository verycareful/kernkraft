// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Open a file as text and save it back.

use std::fs::{self, File};
use std::io::{self, ErrorKind, Read};
use std::path::{Path, PathBuf};

use kernkraft_fs::{write_bytes_atomic, FileStamp, FsError};
use rustix::fs::{access, Access};

use crate::decode::{decode, Decoded, LineEnding, ReadOnly};
use crate::encode::encode;
use crate::TextError;

/// Largest file opened for editing. Editing stays responsive in CodeMirror
/// well past this; above it a file is a log or a dump, opened to read.
pub const EDITABLE_LIMIT: u64 = 10 * 1024 * 1024;

/// Largest file opened at all. Larger files would hold several times their
/// size in the webview; they are refused with [`TextError::TooLargeToOpen`]
/// and the UI offers the default app.
pub const OPEN_LIMIT: u64 = 256 * 1024 * 1024;

/// A file read as text, with the stamp a later [`save`] checks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextFile {
    pub path: PathBuf,
    pub decoded: Decoded,
    pub stamp: FileStamp,
}

/// Reads the file at `path` (symlinks followed).
///
/// Anything that is not a regular file (after following symlinks) is refused
/// from its metadata, before it is opened, as an [`FsError::Io`] of kind
/// `InvalidInput`: no existing [`FsError`] variant means "wrong file type".
/// A file above [`OPEN_LIMIT`] is refused without being read. The result's
/// `read_only` reports one reason; when several apply the order is: size
/// above [`EDITABLE_LIMIT`] ([`ReadOnly::TooLarge`]), then no write
/// permission for the current user ([`ReadOnly::NoWritePermission`], any
/// failure of the `access(2)` check counting as none), then what
/// [`decode`] found (UTF-16, not UTF-8, mixed line endings).
///
/// The stamp is taken before the read, so a change between the two is caught
/// by the next [`save`].
pub fn open(path: &Path) -> Result<TextFile, TextError> {
    let meta = fs::metadata(path).map_err(|source| {
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
    })?;
    if !meta.is_file() {
        // Opening a FIFO with no writer, or reading a device, can block
        // forever; the check is on the metadata so nothing is opened.
        return Err(FsError::Io {
            path: path.to_path_buf(),
            source: io::Error::new(ErrorKind::InvalidInput, "not a regular file"),
        }
        .into());
    }
    let stamp = FileStamp::of(path)?;
    if stamp.len > OPEN_LIMIT {
        return Err(TextError::TooLargeToOpen {
            len: stamp.len,
            limit: OPEN_LIMIT,
        });
    }
    let io = |source| FsError::Io {
        path: path.to_path_buf(),
        source,
    };
    let mut bytes = Vec::new();
    File::open(path)
        .and_then(|f| f.take(OPEN_LIMIT + 1).read_to_end(&mut bytes))
        .map_err(io)?;
    if bytes.len() as u64 > OPEN_LIMIT {
        return Err(TextError::TooLargeToOpen {
            len: bytes.len() as u64,
            limit: OPEN_LIMIT,
        });
    }

    let mut decoded = decode(&bytes);
    if stamp.len > EDITABLE_LIMIT {
        decoded.read_only = Some(ReadOnly::TooLarge {
            len: stamp.len,
            limit: EDITABLE_LIMIT,
        });
    } else if access(path, Access::WRITE_OK).is_err() {
        decoded.read_only = Some(ReadOnly::NoWritePermission);
    }
    Ok(TextFile {
        path: path.to_path_buf(),
        decoded,
        stamp,
    })
}

/// Encodes `text` and writes it to `path` atomically; see
/// [`kernkraft_fs::write_atomic`]. With `expected` the file must still
/// match that stamp, else [`FsError::ExternalModification`] and nothing is
/// written. Returns the stamp of the file now on disk.
///
/// This does not look at [`ReadOnly`]: not offering Save for read-only
/// content is the caller's job.
pub fn save(
    path: &Path,
    text: &str,
    ending: LineEnding,
    bom: bool,
    expected: Option<&FileStamp>,
) -> Result<FileStamp, TextError> {
    Ok(write_bytes_atomic(
        path,
        expected,
        &encode(text, ending, bom),
    )?)
}
