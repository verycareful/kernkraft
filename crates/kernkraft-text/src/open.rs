// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Open a file as text and save it back.

use std::fs::{self, OpenOptions};
use std::io::{self, ErrorKind, Read};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use kernkraft_fs::{write_bytes_atomic, FileStamp, FsError};
use rustix::fs::{access, fcntl_getfl, fcntl_setfl, Access, OFlags};

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
/// as an [`FsError::Io`] of kind `InvalidInput`: no existing [`FsError`]
/// variant means "wrong file type". The file's metadata is checked first, so
/// a FIFO or device in plain sight is never opened. The file is then opened
/// non-blocking and its type checked again on the handle (`fstat`), which is
/// the check that counts: a FIFO or device swapped in between the two is
/// refused without blocking and without being read. Opening a device node
/// can itself have effects some drivers define; that is the one thing the
/// handle check cannot undo, and the first check makes it need a swap within
/// that window.
///
/// A file above [`OPEN_LIMIT`] is refused without being read. The result's
/// `read_only` reports one reason; when several apply the order is: size
/// above [`EDITABLE_LIMIT`] ([`ReadOnly::TooLarge`]), then no write
/// permission for the current user ([`ReadOnly::NoWritePermission`], any
/// failure of the `access(2)` check counting as none), then what
/// [`decode`] found (UTF-16, not UTF-8, mixed line endings).
///
/// The stamp is taken from the open handle before the read, so it describes
/// the file actually read, and a change during the read is caught by the
/// next [`save`].
pub fn open(path: &Path) -> Result<TextFile, TextError> {
    let meta = fs::metadata(path).map_err(|source| not_found_or_io(path, source))?;
    if !meta.is_file() {
        return Err(not_regular(path).into());
    }
    let (bytes, stamp) = read_regular(path)?;

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

fn not_found_or_io(path: &Path, source: io::Error) -> FsError {
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
}

fn not_regular(path: &Path) -> FsError {
    FsError::Io {
        path: path.to_path_buf(),
        source: io::Error::new(ErrorKind::InvalidInput, "not a regular file"),
    }
}

/// Opens `path` without blocking (`O_NONBLOCK`: opening a FIFO with no
/// writer returns at once instead of waiting; `O_NOCTTY`: a terminal device
/// does not become the controlling terminal), refuses it unless the handle
/// is a regular file, then clears `O_NONBLOCK` and reads it whole. Returns
/// the bytes and the stamp of the handle.
fn read_regular(path: &Path) -> Result<(Vec<u8>, FileStamp), TextError> {
    let io = |source| FsError::Io {
        path: path.to_path_buf(),
        source,
    };
    let file = OpenOptions::new()
        .read(true)
        .custom_flags((OFlags::NONBLOCK | OFlags::NOCTTY).bits() as i32)
        .open(path)
        .map_err(|source| not_found_or_io(path, source))?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(not_regular(path).into());
    }
    let stamp = FileStamp::from_metadata(path, &meta)?;
    if stamp.len > OPEN_LIMIT {
        return Err(TextError::TooLargeToOpen {
            len: stamp.len,
            limit: OPEN_LIMIT,
        });
    }
    // Reads of a regular file never return EAGAIN, but blocking mode is
    // what every reader of the handle expects.
    let flags = fcntl_getfl(&file).map_err(|e| io(e.into()))?;
    fcntl_setfl(&file, flags - OFlags::NONBLOCK).map_err(|e| io(e.into()))?;
    let mut bytes = Vec::new();
    (&file)
        .take(OPEN_LIMIT + 1)
        .read_to_end(&mut bytes)
        .map_err(io)?;
    if bytes.len() as u64 > OPEN_LIMIT {
        return Err(TextError::TooLargeToOpen {
            len: bytes.len() as u64,
            limit: OPEN_LIMIT,
        });
    }
    Ok((bytes, stamp))
}

/// Encodes `text` and writes it to `path` atomically; see
/// [`kernkraft_fs::write_atomic`]. Returns the stamp of the file now on disk.
///
/// With `expected` the file must still match that stamp, else
/// [`FsError::ExternalModification`] and nothing is written (a file deleted
/// since counts as changed). Without it, whatever is at `path` is replaced:
/// Save As to a new path, or an overwrite the user chose.
///
/// Either way, an existing file is read and decoded first (through the
/// same handle-checked read as [`open`]), and one whose content decodes
/// read-only for a content reason ([`ReadOnly::is_content`]: not UTF-8,
/// UTF-16, mixed line endings) is refused with [`TextError::ReadOnly`]:
/// the text the caller holds for it is a lossy copy, and writing it would
/// replace the original bytes. With `expected`, the stamp of that read must
/// match, and the write checks the stamp again, so a change between the
/// read and the write is caught too. An existing file above
/// [`OPEN_LIMIT`] cannot be checked and is refused with
/// [`TextError::TooLargeToOpen`]; anything that is not a regular file is
/// refused as in [`open`]. [`ReadOnly::TooLarge`] and
/// [`ReadOnly::NoWritePermission`] are not checked here: the text of such a
/// file is exact, and a write the user may not make fails on its own.
pub fn save(
    path: &Path,
    text: &str,
    ending: LineEnding,
    bom: bool,
    expected: Option<&FileStamp>,
) -> Result<FileStamp, TextError> {
    match read_regular(path) {
        Ok((bytes, stamp)) => {
            if expected.is_some_and(|e| *e != stamp) {
                return Err(FsError::ExternalModification {
                    path: path.to_path_buf(),
                }
                .into());
            }
            if let Some(reason) = decode(&bytes).read_only.filter(ReadOnly::is_content) {
                return Err(TextError::ReadOnly {
                    path: path.to_path_buf(),
                    reason,
                });
            }
        }
        // Nothing there: a new file, or (with `expected`) one deleted
        // since, which the write reports as changed.
        Err(TextError::Fs(FsError::NotFound { .. })) => {}
        Err(e) => return Err(e),
    }
    Ok(write_bytes_atomic(
        path,
        expected,
        &encode(text, ending, bom),
    )?)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::io::ErrorKind;
    use std::sync::mpsc;
    use std::time::Duration;

    use kernkraft_fs::FsError;

    use super::read_regular;
    use crate::TextError;

    /// A FIFO that took the file's place after the metadata check: the
    /// read through the handle refuses it, and does not block waiting for a
    /// writer.
    #[test]
    fn handle_check_refuses_a_fifo_swapped_in_after_the_stat() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("swapped");
        rustix::fs::mknodat(
            rustix::fs::CWD,
            &path,
            rustix::fs::FileType::Fifo,
            rustix::fs::Mode::from_raw_mode(0o600),
            0,
        )
        .unwrap();
        let (tx, rx) = mpsc::channel();
        let thread_path = path.clone();
        std::thread::spawn(move || {
            let _ = tx.send(read_regular(&thread_path).map(|_| ()));
        });
        let result = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("read_regular blocked on a FIFO");
        match result.unwrap_err() {
            TextError::Fs(FsError::Io { path: p, source }) => {
                assert_eq!(p, path);
                assert_eq!(source.kind(), ErrorKind::InvalidInput);
            }
            other => panic!("{other:?}"),
        }
    }

    /// A character device in the file's place is refused from the handle.
    #[test]
    fn handle_check_refuses_a_device() {
        let err = read_regular(std::path::Path::new("/dev/null"))
            .map(|_| ())
            .unwrap_err();
        assert!(
            matches!(&err, TextError::Fs(FsError::Io { source, .. }) if source.kind() == ErrorKind::InvalidInput),
            "{err:?}"
        );
    }

    /// A regular file reads in full with the stamp of the handle it was
    /// read through.
    #[test]
    fn regular_file_reads_with_its_stamp() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        std::fs::write(&path, "abc").unwrap();
        let (bytes, stamp) = read_regular(&path).unwrap();
        assert_eq!(bytes, b"abc");
        assert_eq!(stamp, kernkraft_fs::FileStamp::of(&path).unwrap());
    }
}
