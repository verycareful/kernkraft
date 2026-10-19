// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! XDG base directories and user directories.
//!
//! Base directories follow the XDG Base Directory Specification 0.8: an
//! environment value is used only when it is set, non-empty and absolute;
//! otherwise the specification's default under `$HOME` applies.
//!
//! User directories follow xdg-user-dirs: `$XDG_CONFIG_HOME/user-dirs.dirs`
//! holds shell-style lines `XDG_<NAME>_DIR="<value>"`, where the value is
//! either `$HOME/<path>`, `$HOME` alone, or an absolute path.

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use crate::FsError;

const USER_DIRS_FILE: &str = "user-dirs.dirs";
const HOME_VAR: &str = "$HOME";

/// The XDG base directories, all absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BaseDirs {
    pub home: PathBuf,
    pub config: PathBuf,
    pub cache: PathBuf,
    pub state: PathBuf,
    pub data: PathBuf,
    /// `$XDG_CONFIG_DIRS`, most important first; `/etc/xdg` when unset.
    pub config_dirs: Vec<PathBuf>,
    /// `$XDG_DATA_DIRS`, most important first; `/usr/local/share:/usr/share`
    /// when unset.
    pub data_dirs: Vec<PathBuf>,
    /// `$XDG_CURRENT_DESKTOP` (desktop-entry spec, "Recognized desktop entry
    /// keys", `OnlyShowIn`): the colon-separated names, in order, as set.
    /// Empty when unset. Not a base directory; it sits here because the
    /// desktop-specific `mimeapps.list` lookup needs it beside the
    /// directories.
    pub current_desktop: Vec<String>,
}

/// The user directories kernkraft consumers show, all absolute.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UserDirs {
    pub desktop: PathBuf,
    pub documents: PathBuf,
    pub download: PathBuf,
    pub music: PathBuf,
    pub pictures: PathBuf,
    pub videos: PathBuf,
}

/// XDG Base Directory Specification 0.8, `$XDG_DATA_DIRS` default.
const DEFAULT_DATA_DIRS: &str = "/usr/local/share:/usr/share";
/// XDG Base Directory Specification 0.8, `$XDG_CONFIG_DIRS` default.
const DEFAULT_CONFIG_DIRS: &str = "/etc/xdg";

/// Resolves the base directories from `env`. `None` when `HOME` is unset,
/// empty or relative, since every default is defined relative to it.
pub fn base_dirs_from(env: &dyn Fn(&str) -> Option<OsString>) -> Option<BaseDirs> {
    let home = absolute(env("HOME"))?;
    let pick = |var: &str, default: &[&str]| {
        absolute(env(var)).unwrap_or_else(|| default.iter().fold(home.clone(), |p, c| p.join(c)))
    };
    let dir_list = |var: &str, default: &str| {
        let value = env(var)
            .filter(|v| !v.is_empty())
            .unwrap_or_else(|| OsString::from(default));
        // Relative entries are invalid and ignored (XDG Base Directory 0.8).
        std::env::split_paths(&value)
            .filter(|p| p.is_absolute())
            .collect::<Vec<_>>()
    };
    let current_desktop = env("XDG_CURRENT_DESKTOP")
        .map(|v| {
            v.to_string_lossy()
                .split(':')
                .filter(|n| !n.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Some(BaseDirs {
        config_dirs: dir_list("XDG_CONFIG_DIRS", DEFAULT_CONFIG_DIRS),
        data_dirs: dir_list("XDG_DATA_DIRS", DEFAULT_DATA_DIRS),
        current_desktop,
        config: pick("XDG_CONFIG_HOME", &[".config"]),
        cache: pick("XDG_CACHE_HOME", &[".cache"]),
        state: pick("XDG_STATE_HOME", &[".local", "state"]),
        data: pick("XDG_DATA_HOME", &[".local", "share"]),
        home,
    })
}

/// [`base_dirs_from`] over the process environment.
pub fn base_dirs() -> Option<BaseDirs> {
    base_dirs_from(&|key| std::env::var_os(key))
}

fn absolute(value: Option<OsString>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    path.is_absolute().then_some(path)
}

/// Parses the contents of `user-dirs.dirs` into `NAME -> path` (for
/// `XDG_DOCUMENTS_DIR`, the key is `DOCUMENTS`). Comment lines, malformed
/// lines and values that are not absolute after `$HOME` expansion are left
/// out. `$HOME` is expanded only as the whole value or a leading `$HOME/`,
/// as xdg-user-dirs writes it.
pub fn parse_user_dirs(contents: &str, home: &Path) -> HashMap<String, PathBuf> {
    contents
        .lines()
        .filter_map(|line| parse_line(line.trim(), home))
        .collect()
}

fn parse_line(line: &str, home: &Path) -> Option<(String, PathBuf)> {
    if line.starts_with('#') {
        return None;
    }
    let (key, value) = line.split_once('=')?;
    let name = key.strip_prefix("XDG_")?.strip_suffix("_DIR")?;
    if name.is_empty() {
        return None;
    }
    let value = unquote(value)?;
    let path = if value == HOME_VAR {
        home.to_path_buf()
    } else if let Some(rest) = value.strip_prefix("$HOME/") {
        home.join(rest)
    } else {
        PathBuf::from(value)
    };
    path.is_absolute().then(|| (name.to_string(), path))
}

/// Strips the surrounding double quotes and resolves the backslash escapes
/// a shell honours inside them (`\"`, `\\`, `\$`, `` \` ``). Anything else
/// (no quotes, text after the closing quote) is malformed.
fn unquote(value: &str) -> Option<String> {
    let inner = value.strip_prefix('"')?;
    let mut out = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        match c {
            '"' => return chars.as_str().is_empty().then_some(out),
            '\\' => match chars.next()? {
                e @ ('"' | '\\' | '$' | '`') => out.push(e),
                other => {
                    out.push('\\');
                    out.push(other);
                }
            },
            c => out.push(c),
        }
    }
    None
}

/// Reads `<config>/user-dirs.dirs` and fills every directory it does not
/// name with the English default under `home`. A missing file means every
/// directory takes its default; a file that exists but cannot be read, or is
/// not UTF-8, is an error.
pub fn user_dirs(base: &BaseDirs) -> Result<UserDirs, FsError> {
    let path = base.config.join(USER_DIRS_FILE);
    let parsed = match std::fs::read(&path) {
        Ok(bytes) => {
            let text = String::from_utf8(bytes).map_err(|e| FsError::Io {
                path: path.clone(),
                source: std::io::Error::new(ErrorKind::InvalidData, e),
            })?;
            parse_user_dirs(&text, &base.home)
        }
        Err(e) if e.kind() == ErrorKind::NotFound => HashMap::new(),
        Err(source) => return Err(FsError::Io { path, source }),
    };
    let get = |name: &str, default: &str| {
        parsed
            .get(name)
            .cloned()
            .unwrap_or_else(|| base.home.join(default))
    };
    Ok(UserDirs {
        desktop: get("DESKTOP", "Desktop"),
        documents: get("DOCUMENTS", "Documents"),
        download: get("DOWNLOAD", "Downloads"),
        music: get("MUSIC", "Music"),
        pictures: get("PICTURES", "Pictures"),
        videos: get("VIDEOS", "Videos"),
    })
}

#[cfg(test)]
mod tests {
    use super::unquote;

    #[test]
    fn unquote_resolves_shell_escapes() {
        assert_eq!(
            unquote(r#""/a \"b\" \$c""#).as_deref(),
            Some(r#"/a "b" $c"#)
        );
    }

    #[test]
    fn unquote_rejects_trailing_text_and_open_quote() {
        assert_eq!(unquote(r#""/a" x"#), None);
        assert_eq!(unquote(r#""/a"#), None);
        assert_eq!(unquote("/a"), None);
    }
}
