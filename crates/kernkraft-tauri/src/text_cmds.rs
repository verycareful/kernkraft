// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Text files for the editor: open, check against disk, and save.
//!
//! A file is opened with its line ending and UTF-8 byte order mark
//! remembered (Unicode Standard 23.8), and with the reason it must not be
//! saved, if any (see `kernkraft_text::ReadOnly`). Each open and save
//! returns a [`WireStamp`] of the file as it is on disk; a later save with
//! that stamp refuses with `ExternalModification` if anything changed the
//! file meanwhile, and [`Kernkraft::text_check`] tells the editor whether
//! the file it shows is still the one on disk.

use std::io::ErrorKind;
use std::path::PathBuf;
use std::time::{Duration, UNIX_EPOCH};

use kernkraft_fs::{FileStamp, FsError};
use kernkraft_text::{LineEnding, ReadOnly};
use serde::{Deserialize, Serialize};
use tauri::Runtime;

use crate::error::{PluginError, WireError};
use crate::state::Kernkraft;
use crate::wire::WirePath;

/// A file's stamp (`kernkraft_fs::FileStamp`: length, modification time,
/// device, inode and change time), as one opaque string the webview hands
/// back unchanged. It is a string because a JavaScript number cannot hold
/// every 64-bit device or inode number exactly, nor nanoseconds since the
/// epoch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WireStamp(String);

/// Version tag of the [`WireStamp`] text, so a stamp from a different
/// layout is refused rather than misread.
const STAMP_TAG: &str = "s1";

impl WireStamp {
    fn new(s: &FileStamp) -> WireStamp {
        // Nanoseconds from the epoch, negative before it; SystemTime on
        // Linux keeps nanoseconds, so this is lossless.
        let modified: i128 = match s.modified.duration_since(UNIX_EPOCH) {
            Ok(d) => i128::try_from(d.as_nanos()).unwrap_or(i128::MAX),
            Err(e) => i128::try_from(e.duration().as_nanos()).map_or(i128::MIN, |n| -n),
        };
        WireStamp(format!(
            "{STAMP_TAG}:{}:{modified}:{}:{}:{}",
            s.len, s.dev, s.ino, s.changed_ns
        ))
    }

    fn decode(&self) -> Result<FileStamp, WireError> {
        let bad = || {
            WireError::from(PluginError::InvalidArgument {
                what: "file stamp",
                value: self.0.clone(),
            })
        };
        let mut parts = self.0.split(':');
        if parts.next() != Some(STAMP_TAG) {
            return Err(bad());
        }
        let mut next = || parts.next().ok_or_else(bad);
        let len: u64 = next()?.parse().map_err(|_| bad())?;
        let modified: i128 = next()?.parse().map_err(|_| bad())?;
        let dev: u64 = next()?.parse().map_err(|_| bad())?;
        let ino: u64 = next()?.parse().map_err(|_| bad())?;
        let changed_ns: i128 = next()?.parse().map_err(|_| bad())?;
        if parts.next().is_some() {
            return Err(bad());
        }
        let span = |n: u128| {
            u64::try_from(n / 1_000_000_000)
                .ok()
                .map(|secs| Duration::new(secs, (n % 1_000_000_000) as u32))
        };
        let modified = if modified >= 0 {
            span(modified.unsigned_abs()).and_then(|d| UNIX_EPOCH.checked_add(d))
        } else {
            span(modified.unsigned_abs()).and_then(|d| UNIX_EPOCH.checked_sub(d))
        }
        .ok_or_else(bad)?;
        Ok(FileStamp {
            len,
            modified,
            dev,
            ino,
            changed_ns,
        })
    }
}

/// Why a file is shown but must not be saved: `kind` is the name of the
/// `kernkraft_text::ReadOnly` variant (`NotUtf8`, `Utf16`,
/// `MixedLineEndings`, `TooLarge`, `NoWritePermission`), `detail` a sentence
/// for the user.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireReadOnly {
    pub kind: String,
    pub detail: String,
}

impl From<&ReadOnly> for WireReadOnly {
    fn from(r: &ReadOnly) -> WireReadOnly {
        let (kind, detail) = match r {
            ReadOnly::NotUtf8 { offset } => (
                "NotUtf8",
                format!("not valid UTF-8 from byte {offset}; shown with replacement characters"),
            ),
            ReadOnly::Utf16 => (
                "Utf16",
                "UTF-16 text, which is shown but not saved".to_owned(),
            ),
            ReadOnly::MixedLineEndings { lf, crlf, cr } => (
                "MixedLineEndings",
                format!("mixed line endings: {lf} LF, {crlf} CRLF, {cr} CR"),
            ),
            ReadOnly::TooLarge { len, limit } => (
                "TooLarge",
                format!("{len} bytes, above the {limit} byte limit for editing"),
            ),
            ReadOnly::NoWritePermission => (
                "NoWritePermission",
                "you do not have permission to write this file".to_owned(),
            ),
        };
        WireReadOnly {
            kind: kind.to_owned(),
            detail,
        }
    }
}

/// A text file as the editor gets it. `text` uses `\n` only and has no byte
/// order mark; `ending` (`"lf"` or `"crlf"`) and `bom` are what a save puts
/// back. With `read_only` set, `text` is a lossy display copy and the editor
/// must not offer Save.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireText {
    pub path: WirePath,
    pub text: String,
    pub ending: String,
    pub bom: bool,
    pub read_only: Option<WireReadOnly>,
    pub stamp: WireStamp,
}

fn ending_name(e: LineEnding) -> &'static str {
    match e {
        LineEnding::Lf => "lf",
        LineEnding::CrLf => "crlf",
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// Opens `path` as text (symlinks followed). A file above
    /// `kernkraft_text::OPEN_LIMIT` is `TooLargeToOpen`; anything that is
    /// not a regular file is `Io`.
    pub fn text_open(&self, path: WirePath) -> Result<WireText, WireError> {
        let path = PathBuf::try_from(path)?;
        let file = kernkraft_text::open(&path)?;
        Ok(WireText {
            path: WirePath::from(&file.path),
            ending: ending_name(file.decoded.ending).to_owned(),
            bom: file.decoded.bom,
            read_only: file.decoded.read_only.as_ref().map(WireReadOnly::from),
            stamp: WireStamp::new(&file.stamp),
            text: file.decoded.text,
        })
    }

    /// Writes `text` to `path` atomically with line ending `ending`
    /// (`"lf"` or `"crlf"`) and, with `bom`, a UTF-8 byte order mark.
    ///
    /// With `stamp`, the file must still be the one that stamp describes,
    /// else `ExternalModification` and nothing is written. `None` writes
    /// whatever is there: Save As to a new path, or an overwrite the user
    /// chose after being told the file changed. Returns the new stamp.
    ///
    /// In both cases an existing file whose content decodes read-only (not
    /// UTF-8, UTF-16, mixed line endings) is never replaced: the save fails
    /// with `ReadOnly`, whose details name the reason, and the file keeps
    /// its bytes. The text shown for such a file is a lossy copy, and a UI
    /// slip must not write it back over the original. Save As to a path
    /// with nothing there is not affected.
    pub fn text_save(
        &self,
        path: WirePath,
        text: String,
        ending: String,
        bom: bool,
        stamp: Option<WireStamp>,
    ) -> Result<WireStamp, WireError> {
        let path = PathBuf::try_from(path)?;
        let ending = match ending.as_str() {
            "lf" => LineEnding::Lf,
            "crlf" => LineEnding::CrLf,
            other => {
                return Err(WireError::from(PluginError::InvalidArgument {
                    what: "line ending",
                    value: other.to_owned(),
                }))
            }
        };
        let expected = stamp.map(|s| s.decode()).transpose()?;
        let saved = kernkraft_text::save(&path, &text, ending, bom, expected.as_ref())?;
        Ok(WireStamp::new(&saved))
    }

    /// Compares the file at `path` (symlinks followed) with `stamp`:
    /// `"same"`, `"changed"`, or `"deleted"` when nothing is there (no
    /// entry, or a parent that is no longer a folder). The editor calls it
    /// when the folder of an open file changes.
    pub fn text_check(&self, path: WirePath, stamp: WireStamp) -> Result<String, WireError> {
        let path = PathBuf::try_from(path)?;
        let expected = stamp.decode()?;
        let meta = match std::fs::metadata(&path) {
            Ok(meta) => meta,
            Err(e) if matches!(e.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory) => {
                return Ok("deleted".to_owned())
            }
            Err(source) => return Err(WireError::from(FsError::Io { path, source })),
        };
        let now = FileStamp::from_metadata(&path, &meta)?;
        Ok(if now == expected { "same" } else { "changed" }.to_owned())
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::time::{Duration, UNIX_EPOCH};

    use kernkraft_fs::FileStamp;

    use super::WireStamp;

    #[test]
    fn stamp_round_trips_extremes() {
        for (modified, changed_ns) in [
            (UNIX_EPOCH + Duration::new(1_700_000_000, 123_456_789), 5),
            (UNIX_EPOCH - Duration::new(86_400, 1), -86_400_000_000_001),
            (UNIX_EPOCH, 0),
        ] {
            let stamp = FileStamp {
                len: u64::MAX,
                modified,
                dev: u64::MAX,
                ino: u64::MAX - 1,
                changed_ns,
            };
            assert_eq!(WireStamp::new(&stamp).decode().unwrap(), stamp);
        }
    }

    #[test]
    fn malformed_stamps_are_refused() {
        for s in [
            "",
            "s1",
            "s1:1:2:3:4",
            "s1:1:2:3:4:5:6",
            "s0:1:2:3:4:5",
            "s1:x:2:3:4:5",
        ] {
            assert!(WireStamp(s.to_owned()).decode().is_err(), "{s:?}");
        }
    }
}
