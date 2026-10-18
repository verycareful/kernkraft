// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Directory listing and recursive scanning.
//!
//! A scan follows symlinks to directories, so a library reached through a
//! link is found, and keeps a set of the `(device, inode)` pairs of every
//! directory it has entered, so a link loop or a directory reachable twice
//! is walked once.

use std::collections::HashSet;
use std::fs::{self, Metadata};
use std::io::ErrorKind;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use crate::{FsError, Progress};

/// Depth a scan descends to unless told otherwise: deep enough for real
/// document trees, shallow enough to stay quick on a home directory.
pub const DEFAULT_MAX_DEPTH: u32 = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    /// FIFO, socket or device.
    Other,
}

/// One directory entry. For a symlink, `kind` is `Symlink`, `len` and
/// `modified` describe the link itself, and `target_is_dir` tells whether
/// it resolves to a directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    pub path: PathBuf,
    /// The file name for display; a name that is not UTF-8 is shown with
    /// replacement characters, while `path` keeps the real bytes.
    pub name: String,
    pub kind: EntryKind,
    pub target_is_dir: bool,
    pub len: u64,
    pub modified: Option<SystemTime>,
    /// The name starts with `.`.
    pub hidden: bool,
}

impl Entry {
    fn new(path: PathBuf, meta: &Metadata) -> Entry {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let file_type = meta.file_type();
        let kind = if file_type.is_symlink() {
            EntryKind::Symlink
        } else if file_type.is_dir() {
            EntryKind::Dir
        } else if file_type.is_file() {
            EntryKind::File
        } else {
            EntryKind::Other
        };
        let target_is_dir = match kind {
            EntryKind::Dir => true,
            EntryKind::Symlink => fs::metadata(&path).is_ok_and(|m| m.is_dir()),
            _ => false,
        };
        Entry {
            hidden: name.starts_with('.'),
            name,
            kind,
            target_is_dir,
            len: meta.len(),
            modified: meta.modified().ok(),
            path,
        }
    }
}

/// The entries of `dir`, sorted by name, dotfiles left out unless
/// `include_hidden`. An entry removed while the listing runs is left out.
pub fn list_dir(dir: &Path, include_hidden: bool) -> Result<Vec<Entry>, FsError> {
    let mut entries = Vec::new();
    for dirent in fs::read_dir(dir).map_err(|e| FsError::not_found_or_io(dir, e))? {
        let path = dirent.map_err(|e| FsError::io(dir, e))?.path();
        let meta = match fs::symlink_metadata(&path) {
            Ok(m) => m,
            Err(e) if e.kind() == ErrorKind::NotFound => continue,
            Err(e) => return Err(FsError::io(&path, e)),
        };
        let entry = Entry::new(path, &meta);
        if include_hidden || !entry.hidden {
            entries.push(entry);
        }
    }
    entries.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(entries)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScanOptions {
    /// Entries directly inside a root are at depth 1; directories at
    /// `max_depth` are returned but not entered.
    pub max_depth: u32,
    /// Return and enter dotfiles and dot-directories.
    pub include_hidden: bool,
}

impl Default for ScanOptions {
    fn default() -> Self {
        ScanOptions {
            max_depth: DEFAULT_MAX_DEPTH,
            include_hidden: false,
        }
    }
}

#[derive(Debug, Default)]
pub struct ScanReport {
    /// Entries `keep` accepted, in walk order.
    pub entries: Vec<Entry>,
    /// Directories that could not be read; the scan went on without them.
    pub unreadable: Vec<PathBuf>,
    pub cancelled: bool,
}

/// Walks `roots` depth first. `keep` decides which entries are returned;
/// directories are entered whether or not they are kept. `progress`
/// receives the count of entries seen (total unknown, reported as 0) with
/// the directory being read, and is polled for cancellation before each
/// directory.
pub fn scan(
    roots: &[PathBuf],
    opts: &ScanOptions,
    keep: &dyn Fn(&Entry) -> bool,
    progress: &dyn Progress,
) -> ScanReport {
    let mut report = ScanReport::default();
    let mut visited: HashSet<(u64, u64)> = HashSet::new();
    let mut stack: Vec<(PathBuf, u32)> = Vec::new();
    for root in roots.iter().rev() {
        match fs::metadata(root) {
            Ok(m) if m.is_dir() => {
                if visited.insert((m.dev(), m.ino())) {
                    stack.push((root.clone(), 0));
                }
            }
            _ => report.unreadable.push(root.clone()),
        }
    }

    let mut seen: u64 = 0;
    while let Some((dir, depth)) = stack.pop() {
        if progress.is_cancelled() {
            report.cancelled = true;
            break;
        }
        progress.report(seen, 0, &dir.to_string_lossy());
        let entries = match list_dir(&dir, opts.include_hidden) {
            Ok(entries) => entries,
            Err(_) => {
                report.unreadable.push(dir);
                continue;
            }
        };
        let depth = depth + 1;
        let mut subdirs = Vec::new();
        for entry in entries {
            seen += 1;
            if entry.target_is_dir && depth < opts.max_depth {
                let target = fs::metadata(&entry.path);
                if let Ok(m) = target {
                    if visited.insert((m.dev(), m.ino())) {
                        subdirs.push((entry.path.clone(), depth));
                    }
                }
            }
            if keep(&entry) {
                report.entries.push(entry);
            }
        }
        // Reversed so the first subdirectory by name is walked first.
        stack.extend(subdirs.into_iter().rev());
    }
    report
}
