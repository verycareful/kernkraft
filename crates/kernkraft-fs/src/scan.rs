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
    /// The entry for `path` itself, as [`list_dir`] would list it in its
    /// directory. A symlink is described as itself, never followed.
    pub fn of(path: &Path) -> Result<Entry, FsError> {
        let meta = fs::symlink_metadata(path).map_err(|e| FsError::not_found_or_io(path, e))?;
        Ok(Entry::new(path.to_path_buf(), &meta))
    }

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

/// Totals of [`measure`].
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Measure {
    /// Every entry that is not a directory: regular files, symlinks and
    /// special files.
    pub files: u64,
    pub dirs: u64,
    /// Sum of the lengths of the regular files, each file with several hard
    /// links counted once.
    pub bytes: u64,
    /// Paths that could not be read: a root that is missing, or a directory
    /// whose entries could not be listed. Their contents are not counted.
    pub unreadable: Vec<PathBuf>,
    pub cancelled: bool,
}

/// Counts the files, directories and bytes in `paths` and everything under
/// them, as a file manager's properties view shows them.
///
/// No symlink is followed, at the top or inside: a link counts as one file
/// of no bytes, so a link to a large folder, or a link loop, adds nothing.
/// A directory reached twice (through a bind mount) is walked once, keyed by
/// `(device, inode)`, and so is a file with several hard links counted for
/// its bytes once, as `du(1)` does. Roots are walked in order, each depth
/// first. `progress` receives the count of entries seen (total unknown,
/// reported as 0) with the directory being read, and is polled for
/// cancellation before each directory.
pub fn measure(paths: &[PathBuf], progress: &dyn Progress) -> Measure {
    let mut m = Measure::default();
    let mut dirs_seen: HashSet<(u64, u64)> = HashSet::new();
    let mut linked_seen: HashSet<(u64, u64)> = HashSet::new();
    let mut seen: u64 = 0;
    for root in paths {
        let mut stack = vec![root.clone()];
        while let Some(path) = stack.pop() {
            let meta = match fs::symlink_metadata(&path) {
                Ok(meta) => meta,
                // An entry removed while the walk runs is not counted; only
                // a root the caller named is reported missing.
                Err(e) if e.kind() == ErrorKind::NotFound && &path != root => continue,
                Err(_) => {
                    m.unreadable.push(path);
                    continue;
                }
            };
            seen += 1;
            if !meta.is_dir() {
                m.files += 1;
                let counted = meta.nlink() <= 1 || linked_seen.insert((meta.dev(), meta.ino()));
                if meta.is_file() && counted {
                    m.bytes += meta.len();
                }
                continue;
            }
            if !dirs_seen.insert((meta.dev(), meta.ino())) {
                continue;
            }
            m.dirs += 1;
            if progress.is_cancelled() {
                m.cancelled = true;
                return m;
            }
            progress.report(seen, 0, &path.to_string_lossy());
            let children = fs::read_dir(&path).and_then(|entries| {
                entries
                    .map(|e| e.map(|e| e.path()))
                    .collect::<std::io::Result<Vec<_>>>()
            });
            match children {
                Ok(mut children) => {
                    // Reversed so the first child by name is walked first.
                    children.sort_by(|a, b| b.cmp(a));
                    stack.extend(children);
                }
                Err(_) => m.unreadable.push(path),
            }
        }
    }
    m
}
