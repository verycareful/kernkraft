// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `mimeapps.list` files and the registry of installed applications.
//!
//! Specification: MIME Applications Associations 1.0.1.
//! - Section 2, "File locations": `mimeapps.list` and
//!   `$desktop-mimeapps.list` in `$XDG_CONFIG_HOME`, each of
//!   `$XDG_CONFIG_DIRS`, `$XDG_DATA_HOME/applications` and each of
//!   `$XDG_DATA_DIRS/applications` (the last two are deprecated locations,
//!   still read), in that order; within one directory the desktop-specific
//!   file, for each name in `$XDG_CURRENT_DESKTOP` lowercased, comes before
//!   the plain one. The first file in this order has the highest priority.
//! - Section 3, "Adding/removing associations": groups `[Added
//!   Associations]` and `[Removed Associations]`, each line `mime=id;id;`.
//!   An application listed under `[Added Associations]` handles the type
//!   even without a `MimeType=` registration; one under `[Removed
//!   Associations]` does not, for registrations and for additions in files
//!   of lower priority.
//! - Section 4, "Default application": group `[Default Applications]`; the
//!   first listed desktop file ID that is installed wins, file by file in
//!   priority order.
//!
//! Desktop entries are read from `applications` below `$XDG_DATA_HOME` and
//! each of `$XDG_DATA_DIRS` (Desktop Entry Specification 1.5, "Desktop File
//! ID"); an entry in an earlier directory hides one with the same ID in a
//! later one.
//!
//! Types are tried in the order [`MimeDb::names`] gives (the type, its
//! canonical form, its other aliases), then the same for each of
//! [`MimeDb::ancestors`], so a `text/x-c++src` file reaches `text/plain`
//! handlers.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use kernkraft_fs::BaseDirs;

use crate::desktop_entry::{desktop_id, parse_desktop_entry, split_list, DesktopEntry};
use crate::error::AppsError;
use crate::mime::MimeDb;

const ADDED: &str = "Added Associations";
const REMOVED: &str = "Removed Associations";
const DEFAULTS: &str = "Default Applications";
const MIMEAPPS: &str = "mimeapps.list";

/// One parsed `mimeapps.list`.
#[derive(Debug, Default)]
pub(crate) struct MimeApps {
    added: HashMap<String, Vec<String>>,
    removed: HashMap<String, Vec<String>>,
    defaults: HashMap<String, Vec<String>>,
}

/// Parses the text of a `mimeapps.list`. Groups other than the three above
/// are skipped; a line in a known group that is not `type=ids` fails with
/// [`AppsError::MalformedMimeApps`]. A line before any group does too.
pub(crate) fn parse_mimeapps(path: &Path, contents: &str) -> Result<MimeApps, AppsError> {
    let mut out = MimeApps::default();
    let mut group: Option<&str> = None;
    let mut known = false;
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let bad = || AppsError::MalformedMimeApps {
            path: path.to_path_buf(),
            line: index + 1,
        };
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest.strip_suffix(']').ok_or_else(bad)?;
            group = Some(name);
            known = matches!(name, ADDED | REMOVED | DEFAULTS);
            continue;
        }
        if group.is_none() {
            return Err(bad());
        }
        if !known {
            continue;
        }
        let (mime, ids) = line.split_once('=').ok_or_else(bad)?;
        let mime = mime.trim();
        if mime.is_empty() {
            return Err(bad());
        }
        let target = match group {
            Some(ADDED) => &mut out.added,
            Some(REMOVED) => &mut out.removed,
            _ => &mut out.defaults,
        };
        target
            .entry(mime.to_string())
            .or_default()
            .extend(split_list(ids));
    }
    Ok(out)
}

/// Every installed application, and the association files that rank them.
#[derive(Debug)]
pub struct AppRegistry {
    entries: BTreeMap<String, DesktopEntry>,
    /// Association files in priority order, highest first.
    lists: Vec<MimeApps>,
    skipped: Vec<AppsError>,
}

impl AppRegistry {
    /// Reads the desktop entries and association files that exist under
    /// `base`.
    ///
    /// A missing directory or file is normal and contributes nothing. A
    /// directory that exists but cannot be listed fails the load. A single
    /// file that cannot be used (unreadable, not UTF-8, malformed) does not
    /// hide every other application: it is left out and its error is kept
    /// for [`AppRegistry::skipped`]. A `Type=Link` or `Type=Directory` entry
    /// is not an application and is left out without an error.
    pub fn load(base: &BaseDirs) -> Result<AppRegistry, AppsError> {
        let mut registry = AppRegistry {
            entries: BTreeMap::new(),
            lists: Vec::new(),
            skipped: Vec::new(),
        };
        let mut app_dirs = vec![base.data.join("applications")];
        app_dirs.extend(base.data_dirs.iter().map(|d| d.join("applications")));
        for dir in &app_dirs {
            let mut visited = HashSet::new();
            registry.scan(dir, dir, &mut visited)?;
        }
        for dir in list_dirs(base, &app_dirs) {
            for name in list_names(base) {
                let path = dir.join(name);
                match fs::read_to_string(&path) {
                    Ok(text) => match parse_mimeapps(&path, &text) {
                        Ok(list) => registry.lists.push(list),
                        Err(e) => registry.skipped.push(e),
                    },
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => registry.skipped.push(AppsError::io(&path, e)),
                }
            }
        }
        Ok(registry)
    }

    fn scan(
        &mut self,
        root: &Path,
        dir: &Path,
        visited: &mut HashSet<PathBuf>,
    ) -> Result<(), AppsError> {
        let listing = match fs::read_dir(dir) {
            Ok(l) => l,
            Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(AppsError::io(dir, e)),
        };
        // A symlink loop must not recurse forever.
        if let Ok(real) = fs::canonicalize(dir) {
            if !visited.insert(real) {
                return Ok(());
            }
        }
        let mut paths = Vec::new();
        for item in listing {
            paths.push(item.map_err(|e| AppsError::io(dir, e))?.path());
        }
        paths.sort();
        for path in paths {
            let meta = match fs::metadata(&path) {
                Ok(m) => m,
                Err(e) => {
                    // A dangling symlink or an unreadable entry.
                    self.skipped.push(AppsError::io(&path, e));
                    continue;
                }
            };
            if meta.is_dir() {
                self.scan(root, &path, visited)?;
            } else if path.extension().is_some_and(|e| e == "desktop") {
                let id = desktop_id(root, &path);
                if self.entries.contains_key(&id) {
                    continue;
                }
                match fs::read_to_string(&path) {
                    Ok(text) => match parse_desktop_entry(&id, &path, &text) {
                        Ok(entry) => {
                            self.entries.insert(id, entry);
                        }
                        Err(AppsError::NotAnApplication { .. }) => {}
                        Err(e) => self.skipped.push(e),
                    },
                    Err(e) => self.skipped.push(AppsError::io(&path, e)),
                }
            }
        }
        Ok(())
    }

    /// The entry with desktop file ID `id`, hidden ones included.
    pub fn get(&self, id: &str) -> Option<&DesktopEntry> {
        self.entries.get(id)
    }

    /// Every entry, by ID.
    pub fn all(&self) -> Vec<&DesktopEntry> {
        self.entries.values().collect()
    }

    /// The files and entries [`AppRegistry::load`] could not use, with why.
    pub fn skipped(&self) -> &[AppsError] {
        &self.skipped
    }

    /// Applications that can open `mime`, best first: the associations
    /// added in `mimeapps.list` files (higher priority file first), then
    /// the entries whose `MimeType=` names the type (by ID). Removed
    /// associations, `Hidden` and `NoDisplay` entries are left out. Then the
    /// same for each parent type, nearest first; nothing appears twice.
    pub fn apps_for(&self, mime: &str, db: &MimeDb) -> Vec<&DesktopEntry> {
        let mut out: Vec<&DesktopEntry> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        for level in levels(mime, db) {
            let mut removed: HashSet<&str> = HashSet::new();
            for list in &self.lists {
                for name in &level {
                    for id in list.added.get(name).into_iter().flatten() {
                        if !removed.contains(id.as_str()) {
                            self.push_listed(id, &mut seen, &mut out);
                        }
                    }
                }
                for name in &level {
                    removed.extend(
                        list.removed
                            .get(name)
                            .into_iter()
                            .flatten()
                            .map(String::as_str),
                    );
                }
            }
            for entry in self.entries.values() {
                if !removed.contains(entry.id.as_str())
                    && entry.mime_types.iter().any(|m| level.contains(m))
                {
                    self.push_listed(&entry.id, &mut seen, &mut out);
                }
            }
        }
        out
    }

    fn push_listed<'a>(
        &'a self,
        id: &str,
        seen: &mut HashSet<&'a str>,
        out: &mut Vec<&'a DesktopEntry>,
    ) {
        if let Some(entry) = self.entries.get(id) {
            if !entry.hidden && !entry.no_display && seen.insert(entry.id.as_str()) {
                out.push(entry);
            }
        }
    }

    /// The default application for `mime`: the first desktop file ID under
    /// `[Default Applications]` that is installed and not `Hidden`, taking
    /// the files in priority order. A type with no such ID falls through to
    /// its parent types. There is no default when no file names one;
    /// [`AppRegistry::apps_for`] is the list to offer then.
    pub fn default_for(&self, mime: &str, db: &MimeDb) -> Option<&DesktopEntry> {
        for level in levels(mime, db) {
            for list in &self.lists {
                for name in &level {
                    for id in list.defaults.get(name).into_iter().flatten() {
                        if let Some(entry) = self.entries.get(id).filter(|e| !e.hidden) {
                            return Some(entry);
                        }
                    }
                }
            }
        }
        None
    }
}

/// `mime` and its parents, nearest first; each level holds the names that
/// level goes by.
fn levels(mime: &str, db: &MimeDb) -> Vec<Vec<String>> {
    let mut out = vec![db.names(mime)];
    out.extend(db.ancestors(mime).iter().map(|a| db.names(a)));
    out
}

/// Section 2 directories in priority order.
fn list_dirs(base: &BaseDirs, app_dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut dirs = vec![base.config.clone()];
    dirs.extend(base.config_dirs.iter().cloned());
    dirs.extend(app_dirs.iter().cloned());
    dirs
}

/// Section 2 file names within one directory in priority order.
fn list_names(base: &BaseDirs) -> Vec<String> {
    let mut names: Vec<String> = base
        .current_desktop
        .iter()
        .map(|d| format!("{}-{MIMEAPPS}", d.to_lowercase()))
        .collect();
    names.push(MIMEAPPS.to_string());
    names
}
