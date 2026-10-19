// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::{Path, PathBuf};
use std::process::ExitStatus;

/// Every failure `kernkraft-xdg-apps` reports.
#[derive(Debug, thiserror::Error)]
pub enum AppsError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// A line of a MIME database file that does not have the documented
    /// shape. `line` is 1-based.
    #[error("{path}:{line}: malformed MIME database line")]
    MalformedMimeDb { path: PathBuf, line: usize },
    /// A desktop entry that cannot be used: no `[Desktop Entry]` group, no
    /// `Type` or `Exec`, a line that is not `Key=Value`, or a value that is
    /// not of the key's type.
    #[error("{path}: invalid desktop entry: {reason}")]
    InvalidDesktopEntry { path: PathBuf, reason: String },
    /// A desktop entry whose `Type` is not `Application` (a `Link` or a
    /// `Directory`): well formed, but not something to launch.
    #[error("{path}: desktop entry of type {kind}, not Application")]
    NotAnApplication { path: PathBuf, kind: String },
    /// A line of a `mimeapps.list` that is neither a group header, a comment
    /// nor `type=ids`. `line` is 1-based.
    #[error("{path}:{line}: malformed mimeapps.list line")]
    MalformedMimeApps { path: PathBuf, line: usize },
    /// An `Exec` value that the Desktop Entry Specification does not allow
    /// (unterminated quote, unknown field code, a list code mixed into a
    /// longer argument, nothing to run).
    #[error("{id}: invalid Exec: {reason}")]
    InvalidExec { id: String, reason: String },
    /// Files were given to an application whose `Exec` has no file field
    /// code; such an application takes none.
    #[error("{id}: Exec has no file field code, so it cannot be given files")]
    NoFileArgument { id: String },
    /// A program could not be started. `id` is the desktop file ID, or the
    /// program's name for a terminal or `xdg-mime`.
    #[error("{id}: cannot start: {source}")]
    Launch {
        id: String,
        #[source]
        source: std::io::Error,
    },
    /// `xdg-mime` ran and exited unsuccessfully.
    #[error("xdg-mime failed ({status}): {stderr}")]
    XdgMime { status: ExitStatus, stderr: String },
    /// No terminal emulator could be found. `tried` names every source
    /// consulted, in order, with why each was not usable.
    #[error("no terminal found; tried {}", tried.join(", "))]
    NoTerminal { tried: Vec<String> },
    /// An argument that would be read as an option or is not of the expected
    /// shape.
    #[error("invalid {what}: {value:?}")]
    InvalidArgument { what: &'static str, value: String },
}

impl AppsError {
    /// An I/O failure at `path`.
    pub(crate) fn io(path: &Path, source: std::io::Error) -> AppsError {
        AppsError::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}
