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
//!   The associations of a type are built from the section 2 files in
//!   order. For each `mimeapps.list`: its `[Added Associations]`, minus the
//!   blacklist; its `[Removed Associations]` join the blacklist; then the
//!   desktop files of that same directory whose `MimeType=` lists the type,
//!   minus the blacklist; then all of that directory's desktop files join
//!   the blacklist. A removal therefore applies to its own directory's
//!   desktop files and to everything below, never to what an earlier
//!   directory already added: a desktop file in `$XDG_DATA_HOME` outranks
//!   an addition in `/usr/share/applications/mimeapps.list`, and a removal
//!   there does not remove it. A configuration directory has no desktop
//!   files of its own and contributes only its lists.
//! - Section 4, "Default application": group `[Default Applications]`; the
//!   first listed desktop file ID that is installed wins, file by file in
//!   priority order. When no file names a usable one, the most-preferred
//!   association is the default: the first of [`AppRegistry::apps_for`].
//!
//! The specification gives no order among the desktop files of one
//! directory; they are taken in desktop file ID order, which is also the
//! order `update-desktop-database` writes `mimeinfo.cache` in practice. The
//! registrations come from the desktop files' own `MimeType=`, not from
//! `mimeinfo.cache`.
//!
//! An entry whose `TryExec` names a program that is not an executable file
//! (a path as given, a bare name looked up on `PATH`) is not installed
//! (Desktop Entry Specification 1.5, `TryExec`) and is neither an
//! association nor a default. `NoDisplay` hides an entry from menus only
//! (same specification); such an entry is still an association, so a
//! custom command with `NoDisplay=true` can be the default.
//!
//! Desktop entries are read from `applications` below `$XDG_DATA_HOME` and
//! each of `$XDG_DATA_DIRS` (Desktop Entry Specification 1.5, "Desktop File
//! ID"); an entry in an earlier directory hides one with the same ID in a
//! later one.
//!
//! Types are tried in the order [`MimeDb::names`] gives (the type, its
//! canonical form, its other aliases), then the same for each of
//! [`MimeDb::ancestors`], so a `text/x-c++src` file reaches `text/plain`
//! handlers (Shared MIME-info Database 0.21, "Subclassing"). Parents are not
//! in the MIME Applications specification; the rule used is that of GLib's
//! `gio`: the child type is answered first, an association of the child
//! beats a `[Default Applications]` entry of a parent, and the blacklist
//! carries over to the parents, so `text/x-csrc=gedit.desktop` under
//! `[Removed Associations]` keeps `gedit` from being offered for C sources
//! through `text/plain`.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::ErrorKind;
use std::os::unix::fs::PermissionsExt;
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

/// One stop in the section 3 walk over the search directories.
#[derive(Debug)]
enum Step {
    /// One `mimeapps.list` file. `desktop` is set on the last file of an
    /// applications directory: the desktop files of that directory (its
    /// rank) are walked after the file's additions and removals.
    List {
        list: MimeApps,
        desktop: Option<usize>,
    },
    /// The desktop files of an applications directory that has no
    /// `mimeapps.list`, by rank.
    Desktop(usize),
}

/// Every installed application, and the association files that rank them.
#[derive(Debug)]
pub struct AppRegistry {
    /// Usable applications only: no `Hidden` entry is ever in here.
    entries: BTreeMap<String, DesktopEntry>,
    /// For each ID in `entries`, the index of the `applications` directory
    /// it was read from, `$XDG_DATA_HOME` first.
    rank: HashMap<String, usize>,
    /// IDs of `Hidden=true` entries. They are not returned, but they hide
    /// every entry of the same ID in a lower-priority directory.
    shadowed: HashSet<String>,
    /// The association files and the applications directories in the section
    /// 3 order, highest priority first.
    steps: Vec<Step>,
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
            rank: HashMap::new(),
            shadowed: HashSet::new(),
            steps: Vec::new(),
            skipped: Vec::new(),
        };
        let mut app_dirs = vec![base.data.join("applications")];
        app_dirs.extend(base.data_dirs.iter().map(|d| d.join("applications")));
        for (rank, dir) in app_dirs.iter().enumerate() {
            let mut visited = HashSet::new();
            registry.scan(dir, dir, rank, &mut visited)?;
        }
        let first_app_dir = 1 + base.config_dirs.len();
        for (index, dir) in list_dirs(base, &app_dirs).into_iter().enumerate() {
            let start = registry.steps.len();
            for name in list_names(base) {
                let path = dir.join(name);
                match fs::read_to_string(&path) {
                    Ok(text) => match parse_mimeapps(&path, &text) {
                        Ok(list) => registry.steps.push(Step::List {
                            list,
                            desktop: None,
                        }),
                        Err(e) => registry.skipped.push(e),
                    },
                    Err(e) if e.kind() == ErrorKind::NotFound => {}
                    Err(e) => registry.skipped.push(AppsError::io(&path, e)),
                }
            }
            if index >= first_app_dir {
                let rank = index - first_app_dir;
                let has_list = registry.steps.len() > start;
                match registry.steps.last_mut() {
                    Some(Step::List { desktop, .. }) if has_list => {
                        *desktop = Some(rank);
                    }
                    _ => registry.steps.push(Step::Desktop(rank)),
                }
            }
        }
        Ok(registry)
    }

    fn scan(
        &mut self,
        root: &Path,
        dir: &Path,
        rank: usize,
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
                self.scan(root, &path, rank, visited)?;
            } else if path.extension().is_some_and(|e| e == "desktop") {
                let id = desktop_id(root, &path);
                if self.entries.contains_key(&id) || self.shadowed.contains(&id) {
                    continue;
                }
                match fs::read_to_string(&path) {
                    Ok(text) => match parse_desktop_entry(&id, &path, &text) {
                        Ok(entry) if entry.hidden => {
                            self.shadowed.insert(id);
                        }
                        Ok(entry) => {
                            self.rank.insert(id.clone(), rank);
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

    /// The entry with desktop file ID `id`. A `Hidden` entry is never returned.
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

    /// Applications that can open `mime`, best first, built directory by
    /// directory as the module documentation describes: additions, then the
    /// directory's desktop files registering the type, minus the removed
    /// ones. `Hidden` entries and entries whose `TryExec` program is missing
    /// are left out; `NoDisplay` ones are kept (see the `no_display` field
    /// for a menu). Then the same for each parent type, nearest first, with
    /// the removals carried over; nothing appears twice.
    pub fn apps_for(&self, mime: &str, db: &MimeDb) -> Vec<&DesktopEntry> {
        let mut out: Vec<&DesktopEntry> = Vec::new();
        let mut seen: HashSet<&str> = HashSet::new();
        let mut removed: HashSet<&str> = HashSet::new();
        for level in levels(mime, db) {
            self.associations(&level, &mut removed, &mut seen, &mut out, false);
        }
        out
    }

    /// Appends the associations of one level (the names one type goes by)
    /// that are not in `seen`, best first, and adds the level's removals to
    /// `removed`. With `first_only` it stops at the first one found.
    fn associations<'a>(
        &'a self,
        level: &[String],
        removed: &mut HashSet<&'a str>,
        seen: &mut HashSet<&'a str>,
        out: &mut Vec<&'a DesktopEntry>,
        first_only: bool,
    ) {
        // Entries of the directories passed so far. Unlike `removed`, which
        // follows the type up to its parents, this holds for one level.
        let mut passed: HashSet<&'a str> = HashSet::new();
        for step in &self.steps {
            match step {
                Step::List { list, desktop } => {
                    for name in level {
                        for id in list.added.get(name).into_iter().flatten() {
                            if !removed.contains(id.as_str()) && !passed.contains(id.as_str()) {
                                self.push_listed(id, seen, out);
                                if first_only && !out.is_empty() {
                                    return;
                                }
                            }
                        }
                    }
                    for name in level {
                        removed.extend(
                            list.removed
                                .get(name)
                                .into_iter()
                                .flatten()
                                .map(String::as_str),
                        );
                    }
                    if let Some(rank) = desktop {
                        self.push_registered(*rank, level, removed, &mut passed, seen, out);
                        if first_only && !out.is_empty() {
                            return;
                        }
                    }
                }
                Step::Desktop(rank) => {
                    self.push_registered(*rank, level, removed, &mut passed, seen, out);
                    if first_only && !out.is_empty() {
                        return;
                    }
                }
            }
        }
    }

    /// Appends the entries of applications directory `rank` whose
    /// `MimeType=` lists a name of `level`, by ID, minus `removed`; then
    /// every entry of the directory joins `passed`.
    fn push_registered<'a>(
        &'a self,
        rank: usize,
        level: &[String],
        removed: &HashSet<&'a str>,
        passed: &mut HashSet<&'a str>,
        seen: &mut HashSet<&'a str>,
        out: &mut Vec<&'a DesktopEntry>,
    ) {
        for entry in self.entries.values() {
            if self.rank.get(&entry.id) == Some(&rank)
                && !removed.contains(entry.id.as_str())
                && entry.mime_types.iter().any(|m| level.contains(m))
            {
                self.push_listed(&entry.id, seen, out);
            }
        }
        passed.extend(
            self.entries
                .values()
                .filter(|e| self.rank.get(&e.id) == Some(&rank))
                .map(|e| e.id.as_str()),
        );
    }

    fn push_listed<'a>(
        &'a self,
        id: &str,
        seen: &mut HashSet<&'a str>,
        out: &mut Vec<&'a DesktopEntry>,
    ) {
        if let Some(entry) = self.entries.get(id) {
            if self.installed(entry) && seen.insert(entry.id.as_str()) {
                out.push(entry);
            }
        }
    }

    /// Whether `entry` can be offered: not `Hidden`, and its `TryExec`
    /// program, if it names one, is an executable file.
    fn installed(&self, entry: &DesktopEntry) -> bool {
        !entry.hidden
            && entry
                .try_exec
                .as_deref()
                .is_none_or(|t| program_exists(t, std::env::var_os("PATH").as_deref()))
    }

    /// The default application for `mime`, level by level: the type, then
    /// each parent type, nearest first. At each level, MIME Applications
    /// Associations 1.0.1 section 4: the first desktop file ID under
    /// `[Default Applications]` that is installed, taking the files in
    /// priority order. When none is usable, the most-preferred association
    /// of that level, which is the first entry [`AppRegistry::apps_for`]
    /// would give for it. The result is therefore either an explicit
    /// default or the head of the list `apps_for` offers. There is no
    /// default when neither exists at any level.
    pub fn default_for(&self, mime: &str, db: &MimeDb) -> Option<&DesktopEntry> {
        let mut removed: HashSet<&str> = HashSet::new();
        for level in levels(mime, db) {
            for step in &self.steps {
                let Step::List { list, .. } = step else {
                    continue;
                };
                for name in &level {
                    for id in list.defaults.get(name).into_iter().flatten() {
                        if let Some(entry) = self.entries.get(id).filter(|e| self.installed(e)) {
                            return Some(entry);
                        }
                    }
                }
            }
            let mut out = Vec::new();
            self.associations(&level, &mut removed, &mut HashSet::new(), &mut out, true);
            if let Some(first) = out.first() {
                return Some(first);
            }
        }
        None
    }
}

/// `name` as a path when it has a separator, else the first executable
/// file of that name in `path`.
fn program_exists(name: &str, path: Option<&std::ffi::OsStr>) -> bool {
    let is_exec = |p: &Path| {
        fs::metadata(p).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    };
    if name.contains('/') {
        return is_exec(Path::new(name));
    }
    path.is_some_and(|p| {
        std::env::split_paths(p)
            .filter(|d| !d.as_os_str().is_empty())
            .any(|d| is_exec(&d.join(name)))
    })
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
