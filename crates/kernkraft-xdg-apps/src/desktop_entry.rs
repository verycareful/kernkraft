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
    let mut any_group = false;
    let mut in_main = false;
    let mut found_main = false;
    let mut keys: Vec<(&str, &str)> = Vec::new();
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
            in_main = name == GROUP;
            found_main |= in_main;
            continue;
        }
        if !any_group {
            return Err(invalid(format!(
                "line {}: key before the first group",
                index + 1
            )));
        }
        if !in_main {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| invalid(format!("line {}: not Key=Value", index + 1)))?;
        let (key, value) = (key.trim(), value.trim());
        // A localized key never replaces the unlocalized one.
        if key.contains('[') {
            continue;
        }
        keys.push((key, value));
    }
    if !found_main {
        return Err(invalid(format!("no [{GROUP}] group")));
    }
    // A key listed twice keeps its first value.
    let get = |name: &str| keys.iter().find(|(k, _)| *k == name).map(|(_, v)| *v);

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
