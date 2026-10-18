// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Name rules for entries kernkraft creates or renames.

use std::fs;
use std::path::Path;

use crate::naming::numbered;
use crate::FsError;

/// Checks that `name` is a single path component: non-empty, no `/`, no NUL,
/// and not `.` or `..`.
pub fn validate_name(name: &str) -> Result<(), FsError> {
    let bad = name.is_empty() || name == "." || name == ".." || name.contains(['/', '\0']);
    if bad {
        Err(FsError::InvalidName {
            name: name.to_string(),
        })
    } else {
        Ok(())
    }
}

/// `name` if nothing in `dir` has it, else the first free `name (n)`,
/// n = 2, 3, ... The suffix goes before the extension of a file
/// (`a (2).txt`) and at the end of a directory or dotfile name. Whether
/// `name` is a directory is read from the entry already in `dir`.
pub fn unique_name(dir: &Path, name: &str) -> String {
    let is_dir = fs::symlink_metadata(dir.join(name))
        .map(|m| m.is_dir())
        .unwrap_or(false);
    unique_name_for(dir, name, is_dir)
}

/// [`unique_name`] for an entry whose kind the caller knows.
///
/// A name counts as taken when anything is there, a dangling symlink
/// included, or when its status cannot be read: a name is only offered when
/// it is certain not to replace anything.
pub(crate) fn unique_name_for(dir: &Path, name: &str, is_dir: bool) -> String {
    let free = |candidate: &str| {
        matches!(
            fs::symlink_metadata(dir.join(candidate)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound
        )
    };
    if free(name) {
        return name.to_string();
    }
    (2..)
        .map(|n| numbered(name, n, !is_dir))
        .find(|candidate| free(candidate))
        .unwrap_or_default()
}
