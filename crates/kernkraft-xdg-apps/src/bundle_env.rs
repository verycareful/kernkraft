// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The environment a started program gets when this process runs from an
//! application bundle.
//!
//! An AppImage's launcher points the environment at the bundle so the bundled
//! program finds its own libraries: `LD_LIBRARY_PATH`, `PYTHONHOME`,
//! `PYTHONPATH`, `PERLLIB`, `QT_PLUGIN_PATH`, the GTK, GIO and GStreamer
//! paths, and a bundle entry at the front of `PATH` and `XDG_DATA_DIRS`. Every
//! program started from there inherits them, and a program that embeds Python
//! (kitty) stops at start-up because `PYTHONHOME` names a directory with no
//! standard library.
//!
//! The launcher records the bundle's root in `APPDIR`. When it is set, a
//! started program gets the environment without the bundle: a variable whose
//! entries all lie inside the bundle is removed, and in a list variable
//! (separated by `:`) the entries inside the bundle are dropped and the rest
//! kept in order. Nothing else is touched. Outside a bundle `APPDIR` is unset
//! and the environment passes through as it is.

use std::ffi::{OsStr, OsString};
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::Path;
use std::process::Command;

/// The variable that names the bundle's root.
const APPDIR: &str = "APPDIR";
/// Variables the AppImage runtime sets about itself, never wanted by another
/// program. `APPDIR` is also removed by the rule for entries inside the
/// bundle; it is listed so the intent is plain.
const RUNTIME_VARS: [&str; 4] = [APPDIR, "APPIMAGE", "ARGV0", "OWD"];

/// How one variable changes: set to a value, or removed.
type Change = (OsString, Option<OsString>);

/// Makes `cmd` start with the environment of this process minus the bundle.
pub(crate) fn leave_bundle(cmd: &mut Command) {
    let vars: Vec<(OsString, OsString)> = std::env::vars_os().collect();
    for (key, value) in changes(&vars) {
        match value {
            Some(v) => cmd.env(key, v),
            None => cmd.env_remove(key),
        };
    }
}

/// What changes in `vars` to leave the bundle named by `APPDIR`; empty when
/// `APPDIR` is unset, empty or not absolute.
fn changes(vars: &[(OsString, OsString)]) -> Vec<Change> {
    let appdir = vars
        .iter()
        .find(|(k, _)| k == APPDIR)
        .map(|(_, v)| Path::new(v))
        .filter(|p| p.is_absolute());
    let Some(appdir) = appdir else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (key, value) in vars {
        if RUNTIME_VARS.iter().any(|r| key == r) {
            out.push((key.clone(), None));
            continue;
        }
        if let Some(change) = without_bundle(appdir, value) {
            out.push((key.clone(), change));
        }
    }
    out
}

/// `value` as a `:` separated list with the entries inside `appdir` dropped:
/// `None` when no entry was inside it (nothing to change), `Some(None)` when
/// nothing but empty entries is left (remove the variable), else the shortened
/// list. An entry is inside when its path starts with `appdir` by whole
/// components, so a doubled `/` does not matter.
fn without_bundle(appdir: &Path, value: &OsStr) -> Option<Option<OsString>> {
    let entries: Vec<&[u8]> = value.as_bytes().split(|b| *b == b':').collect();
    let inside = |e: &&[u8]| !e.is_empty() && Path::new(OsStr::from_bytes(e)).starts_with(appdir);
    if !entries.iter().any(inside) {
        return None;
    }
    let kept: Vec<&[u8]> = entries.into_iter().filter(|e| !inside(e)).collect();
    if kept.iter().all(|e| e.is_empty()) {
        return Some(None);
    }
    Some(Some(OsString::from_vec(kept.join(&b':'))))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const APP: &str = "/tmp/.mount_kernAbc";

    fn vars(pairs: &[(&str, &str)]) -> Vec<(OsString, OsString)> {
        pairs
            .iter()
            .map(|(k, v)| (OsString::from(k), OsString::from(v)))
            .collect()
    }

    fn change<'a>(all: &'a [Change], key: &str) -> Option<&'a Option<OsString>> {
        all.iter().find(|(k, _)| k == key).map(|(_, v)| v)
    }

    fn text(v: &Option<OsString>) -> Option<&str> {
        v.as_deref().map(|s| s.to_str().unwrap())
    }

    #[test]
    fn outside_a_bundle_nothing_changes() {
        assert!(changes(&vars(&[("PYTHONHOME", "/tmp/.mount_x/usr"), ("A", "b")])).is_empty());
        assert!(changes(&vars(&[("APPDIR", ""), ("PYTHONHOME", "/x")])).is_empty());
        assert!(changes(&vars(&[("APPDIR", "relative/dir")])).is_empty());
    }

    #[test]
    fn a_variable_that_points_only_into_the_bundle_is_removed() {
        let all = changes(&vars(&[
            ("APPDIR", APP),
            ("PYTHONHOME", "/tmp/.mount_kernAbc/usr/"),
            ("GTK_PATH", "/tmp/.mount_kernAbc//usr/lib/gtk-3.0"),
            ("GTK_DATA_PREFIX", APP),
        ]));
        for key in ["PYTHONHOME", "GTK_PATH", "GTK_DATA_PREFIX"] {
            assert_eq!(change(&all, key), Some(&None), "{key}");
        }
    }

    #[test]
    fn list_variables_lose_only_the_bundle_entries_and_keep_order() {
        let all = changes(&vars(&[
            ("APPDIR", APP),
            (
                "PATH",
                "/tmp/.mount_kernAbc/usr/bin/:/tmp/.mount_kernAbc/bin/:/home/u/.local/bin:/usr/bin",
            ),
            (
                "XDG_DATA_DIRS",
                "/tmp/.mount_kernAbc/usr/share/:/tmp/.mount_kernAbc/usr/share:/usr/share:/usr/local/share:/usr/share",
            ),
        ]));
        assert_eq!(
            text(change(&all, "PATH").unwrap()),
            Some("/home/u/.local/bin:/usr/bin")
        );
        assert_eq!(
            text(change(&all, "XDG_DATA_DIRS").unwrap()),
            Some("/usr/share:/usr/local/share:/usr/share")
        );
    }

    #[test]
    fn a_list_left_with_only_empty_entries_is_removed() {
        let all = changes(&vars(&[
            ("APPDIR", APP),
            (
                "GST_PLUGIN_SYSTEM_PATH",
                "/tmp/.mount_kernAbc/usr/lib/gstreamer:",
            ),
        ]));
        assert_eq!(change(&all, "GST_PLUGIN_SYSTEM_PATH"), Some(&None));
    }

    #[test]
    fn a_path_that_only_shares_the_prefix_is_not_inside() {
        // `/tmp/.mount_kernAbcd` is another directory, not a child of the bundle.
        let all = changes(&vars(&[
            ("APPDIR", APP),
            ("PATH", "/tmp/.mount_kernAbcd/bin:/usr/bin"),
        ]));
        assert_eq!(change(&all, "PATH"), None);
    }

    #[test]
    fn unrelated_variables_and_values_are_left_alone() {
        let all = changes(&vars(&[
            ("APPDIR", APP),
            ("HOME", "/home/u"),
            ("LANG", "en_US.UTF-8"),
            ("GTK_THEME", "Adwaita:dark"),
        ]));
        for key in ["HOME", "LANG", "GTK_THEME"] {
            assert_eq!(change(&all, key), None, "{key}");
        }
    }

    #[test]
    fn the_runtime_variables_are_removed() {
        let all = changes(&vars(&[
            ("APPDIR", APP),
            ("APPIMAGE", "/home/u/Apps/kern.AppImage"),
            ("ARGV0", "kern"),
            ("OWD", "/home/u"),
        ]));
        for key in ["APPDIR", "APPIMAGE", "ARGV0", "OWD"] {
            assert_eq!(change(&all, key), Some(&None), "{key}");
        }
    }

    #[test]
    fn a_non_utf8_entry_outside_the_bundle_is_kept_byte_for_byte() {
        let mut value = OsString::from("/tmp/.mount_kernAbc/usr/bin:/odd/");
        value.push(OsStr::from_bytes(&[0xff, 0xfe]));
        let vars = vec![
            (OsString::from("APPDIR"), OsString::from(APP)),
            (OsString::from("PATH"), value),
        ];
        let all = changes(&vars);
        let kept = change(&all, "PATH").unwrap().as_ref().unwrap();
        assert_eq!(kept.as_bytes(), b"/odd/\xff\xfe");
    }
}
