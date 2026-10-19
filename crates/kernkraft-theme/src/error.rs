// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

use std::path::PathBuf;

/// Every failure `kernkraft-theme` reports. Parsing a theme source never
/// fails (bad values are ignored and listed); only reading, asking the portal
/// and watching can.
#[derive(Debug, thiserror::Error)]
pub enum ThemeError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} is larger than the {limit} byte limit for a theme file")]
    TooLarge { path: PathBuf, limit: u64 },
    /// The session bus or `org.freedesktop.portal.Desktop` answered with an
    /// error other than "no such service", or with a value of the wrong type.
    #[error("xdg-desktop-portal: {0}")]
    Portal(String),
    #[error("watching {path}: {source}")]
    Watch {
        path: PathBuf,
        #[source]
        source: notify::Error,
    },
}
