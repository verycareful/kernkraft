// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Desktop entry files.
//!
//! Specification: Desktop Entry Specification 1.5.
//! - "Basic format of the file": `[Group]` headers, `Key=Value` lines,
//!   `#` comments and blank lines; spaces around `=` are ignored; only the
//!   `[Desktop Entry]` group is read here (other groups, such as
//!   `[Desktop Action ...]`, are skipped).
//! - "Possible value types": the escapes `\s \n \t \r \\` in a string, `\;`
//!   inside a list, lists ended or separated by `;`, booleans `true` and
//!   `false`.
//! - "Localized values for keys": `Key[locale]` lines are ignored; the
//!   unlocalized key is always used, so the result does not depend on the
//!   environment.
//! - "Recognized desktop entry keys": `Type`, `Name`, `Exec`, `Path`,
//!   `Icon`, `Terminal`, `NoDisplay`, `Hidden`, `MimeType`.
//! - "Desktop File ID": the path below an `applications` directory with `/`
//!   replaced by `-`.

use std::collections::HashSet;
use std::fs;
use std::io::ErrorKind;
use std::path::{Component, Path, PathBuf};

use crate::error::AppsError;

const GROUP: &str = "Desktop Entry";
const TYPE_APPLICATION: &str = "Application";

/// An application's desktop entry, reduced to the keys kernkraft acts on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DesktopEntry {
    /// Desktop file ID, for example `org.gnome.gedit.desktop`.
    pub id: String,
    /// The unlocalized `Name`; the ID when the entry has none.
    pub name: String,
    /// The `Exec` value with string escapes resolved, field codes intact.
    /// See [`crate::expand_exec`].
    pub exec: String,
    pub icon: Option<String>,
    pub mime_types: Vec<String>,
    /// `Terminal=true`: the program runs inside a terminal emulator.
    pub terminal: bool,
    /// `NoDisplay=true`: left out of menus; the application still handles
    /// its types.
    pub no_display: bool,
    /// The `TryExec` key: a program that must exist for the entry to count
    /// as installed.
    pub try_exec: Option<String>,
    /// `Hidden=true`: the entry is treated as deleted.
    pub hidden: bool,
    /// The `Path` key: the working directory to start the program in.
    pub path: Option<PathBuf>,
    /// The file the entry was read from.
    pub source: PathBuf,
}

/// Parses the text of a desktop entry file. An entry with `Hidden=true` is
/// returned as a hidden stub whatever else it lacks.
///
/// `id` is the entry's desktop file ID (see [`desktop_id`]) and `source` the
/// file it came from; neither is read from disk. Fails with
/// [`AppsError::NotAnApplication`] for `Type=Link` and `Type=Directory`, and
/// with [`AppsError::InvalidDesktopEntry`] when the text has no
/// `[Desktop Entry]` group, no `Type`, no `Exec`, a line that is not
/// `Key=Value`, or a boolean that is not `true` or `false`.
pub fn parse_desktop_entry(
    id: &str,
    source: &Path,
    contents: &str,
) -> Result<DesktopEntry, AppsError> {
    let invalid = |reason: String| AppsError::InvalidDesktopEntry {
        path: source.to_path_buf(),
        reason,
    };
    let groups = read_groups(source, contents, &|name| name == GROUP)?;
    let main = groups
        .iter()
        .find(|g| g.name == GROUP)
        .ok_or_else(|| invalid(format!("no [{GROUP}] group")))?;
    let get = |name: &str| main.get(name);

    // `Hidden=true` marks the ID as deleted (Desktop Entry Specification 1.5,
    // "Recognized desktop entry keys"), often in an entry that has no `Type`
    // or `Exec`. It is returned as a stub with `hidden` set so the caller can
    // record the ID as shadowed; [`crate::AppRegistry`] never hands such an
    // entry out, so nothing in it reaches `launch`.
    if get("Hidden") == Some("true") {
        return Ok(DesktopEntry {
            id: id.to_string(),
            name: get("Name").map(unescape).unwrap_or_else(|| id.to_string()),
            exec: get("Exec").map(unescape).unwrap_or_default(),
            icon: None,
            mime_types: Vec::new(),
            terminal: false,
            no_display: false,
            try_exec: None,
            hidden: true,
            path: None,
            source: source.to_path_buf(),
        });
    }
    let kind = get("Type").ok_or_else(|| invalid("no Type".to_string()))?;
    if kind != TYPE_APPLICATION {
        return Err(AppsError::NotAnApplication {
            path: source.to_path_buf(),
            kind: kind.to_string(),
        });
    }
    let exec = unescape(get("Exec").ok_or_else(|| invalid("no Exec".to_string()))?);
    let flag = |name: &str| -> Result<bool, AppsError> {
        match get(name) {
            None | Some("false") => Ok(false),
            Some("true") => Ok(true),
            Some(other) => Err(invalid(format!("{name}={other:?} is not true or false"))),
        }
    };
    Ok(DesktopEntry {
        id: id.to_string(),
        name: get("Name")
            .map(unescape)
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| id.to_string()),
        exec,
        icon: get("Icon").map(unescape).filter(|i| !i.is_empty()),
        mime_types: get("MimeType").map(split_list).unwrap_or_default(),
        terminal: flag("Terminal")?,
        no_display: flag("NoDisplay")?,
        try_exec: get("TryExec").map(unescape).filter(|t| !t.is_empty()),
        hidden: flag("Hidden")?,
        path: get("Path")
            .map(unescape)
            .filter(|p| !p.is_empty())
            .map(PathBuf::from),
        source: source.to_path_buf(),
    })
}

/// The `Key=Value` pairs of one group of a desktop entry file.
pub(crate) struct Group<'a> {
    pub name: &'a str,
    keys: Vec<(&'a str, &'a str)>,
}

impl<'a> Group<'a> {
    /// The raw value of `key`. A key listed twice keeps its first value, and
    /// a localized `Key[locale]` line never stands in for the unlocalized
    /// key.
    pub fn get(&self, key: &str) -> Option<&'a str> {
        self.keys.iter().find(|(k, _)| *k == key).map(|(_, v)| *v)
    }
}

/// Reads `contents` line by line and returns the groups for which `wanted`
/// is true, in file order; a group that appears twice is one group.
///
/// Every line is checked for being a comment, blank, a group header, or (in
/// a wanted group) `Key=Value`; a line of an unwanted group is not looked at,
/// so a malformed line there does not fail the file. Fails with
/// [`AppsError::InvalidDesktopEntry`] for an unterminated group header, a key
/// before the first group, or a line of a wanted group that is not
/// `Key=Value`.
pub(crate) fn read_groups<'a>(
    source: &Path,
    contents: &'a str,
    wanted: &dyn Fn(&str) -> bool,
) -> Result<Vec<Group<'a>>, AppsError> {
    let invalid = |reason: String| AppsError::InvalidDesktopEntry {
        path: source.to_path_buf(),
        reason,
    };
    let mut groups: Vec<Group<'a>> = Vec::new();
    let mut any_group = false;
    let mut current: Option<usize> = None;
    for (index, raw) in contents.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('[') {
            let name = rest
                .strip_suffix(']')
                .ok_or_else(|| invalid(format!("line {}: unterminated group header", index + 1)))?;
            any_group = true;
            current = wanted(name).then(|| {
                groups
                    .iter()
                    .position(|g| g.name == name)
                    .unwrap_or_else(|| {
                        groups.push(Group {
                            name,
                            keys: Vec::new(),
                        });
                        groups.len() - 1
                    })
            });
            continue;
        }
        if !any_group {
            return Err(invalid(format!(
                "line {}: key before the first group",
                index + 1
            )));
        }
        let Some(at) = current else { continue };
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid(format!("line {}: not Key=Value", index + 1)))?;
        let (key, value) = (key.trim(), value.trim());
        if key.contains('[') {
            continue;
        }
        groups[at].keys.push((key, value));
    }
    Ok(groups)
}

/// What [`walk_applications`] found.
pub(crate) enum Walked {
    /// A `.desktop` file and its desktop file ID.
    File { id: String, path: PathBuf },
    /// A path that could not be used: a dangling symlink or an unreadable
    /// entry.
    Skipped(AppsError),
}

/// Calls `visit` for every `.desktop` file below `dir`, which is `root` or
/// a directory inside it, in path order, descending into subdirectories
/// (Desktop Entry Specification 1.5, "Desktop File ID").
///
/// A missing directory contributes nothing; one that exists but cannot be
/// listed is an error. A symlink loop is not followed twice, through
/// `visited`.
pub(crate) fn walk_applications(
    root: &Path,
    dir: &Path,
    visited: &mut HashSet<PathBuf>,
    visit: &mut dyn FnMut(Walked),
) -> Result<(), AppsError> {
    let listing = match fs::read_dir(dir) {
        Ok(l) => l,
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(AppsError::io(dir, e)),
    };
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
                visit(Walked::Skipped(AppsError::io(&path, e)));
                continue;
            }
        };
        if meta.is_dir() {
            walk_applications(root, &path, visited, visit)?;
        } else if path.extension().is_some_and(|e| e == "desktop") {
            visit(Walked::File {
                id: desktop_id(root, &path),
                path,
            });
        }
    }
    Ok(())
}

/// The desktop file ID of `file`, a file below `applications_dir`: the
/// relative path with every `/` replaced by `-`
/// (`applications/kde/foo.desktop` is `kde-foo.desktop`). A file that is not
/// below `applications_dir` gets its file name.
pub fn desktop_id(applications_dir: &Path, file: &Path) -> String {
    let relative = file.strip_prefix(applications_dir).unwrap_or(file);
    let parts: Vec<String> = relative
        .components()
        .filter_map(|c| match c {
            Component::Normal(part) => Some(part.to_string_lossy().into_owned()),
            _ => None,
        })
        .collect();
    if file.starts_with(applications_dir) {
        parts.join("-")
    } else {
        file.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }
}

/// Resolves the string escapes `\s \n \t \r \\`. Any other backslash
/// sequence is outside the specification and is kept as written.
pub(crate) fn unescape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('s') => out.push(' '),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\\') => out.push('\\'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    out
}

/// Splits a `;` separated list, resolving the escapes (`\;` is a literal
/// semicolon). Empty elements, including the one after a trailing `;`, are
/// dropped.
pub(crate) fn split_list(value: &str) -> Vec<String> {
    let mut items = Vec::new();
    let mut current = String::new();
    let mut chars = value.chars();
    while let Some(c) = chars.next() {
        match c {
            ';' => {
                if !current.is_empty() {
                    items.push(std::mem::take(&mut current));
                }
            }
            '\\' => match chars.next() {
                Some(';') => current.push(';'),
                Some('s') => current.push(' '),
                Some('n') => current.push('\n'),
                Some('t') => current.push('\t'),
                Some('r') => current.push('\r'),
                Some('\\') => current.push('\\'),
                Some(other) => {
                    current.push('\\');
                    current.push(other);
                }
                None => current.push('\\'),
            },
            other => current.push(other),
        }
    }
    if !current.is_empty() {
        items.push(current);
    }
    items
}
