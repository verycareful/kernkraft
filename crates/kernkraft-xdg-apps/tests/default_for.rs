// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! `AppRegistry::default_for` against trees built per test, following MIME
//! Applications Associations 1.0.1 section 4 ("Default application") with
//! the section 3 associations as the fallback. Every expectation derives
//! from how the tree is built.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;

use kernkraft_fs::base_dirs_from;
use kernkraft_xdg_apps::{AppRegistry, MimeDb};

/// One temp tree: `config` is `$XDG_CONFIG_HOME`, `data` is
/// `$XDG_DATA_HOME`, `sys/data` is the one `$XDG_DATA_DIRS` entry before
/// `sys2/data`, and `sys/config` is `$XDG_CONFIG_DIRS`.
struct Tree {
    dir: tempfile::TempDir,
}

impl Tree {
    fn new() -> Tree {
        Tree {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.path().join(rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    }

    /// An installed application registering `mimes` (may be empty).
    fn app(&self, dir: &str, id: &str, mimes: &[&str]) {
        let mime = if mimes.is_empty() {
            String::new()
        } else {
            format!("MimeType={};\n", mimes.join(";"))
        };
        self.write(
            &format!("{dir}/applications/{id}"),
            &format!("[Desktop Entry]\nType=Application\nName={id}\nExec=true %f\n{mime}"),
        );
    }

    fn registry(&self, desktop: Option<&str>) -> AppRegistry {
        let root = self.dir.path();
        let data_dirs = format!(
            "{}:{}",
            root.join("sys/data").display(),
            root.join("sys2/data").display()
        );
        let mut vars: HashMap<&str, OsString> = HashMap::new();
        vars.insert("HOME", root.into());
        vars.insert("XDG_CONFIG_HOME", root.join("config").into());
        vars.insert("XDG_DATA_HOME", root.join("data").into());
        vars.insert("XDG_CONFIG_DIRS", root.join("sys/config").into());
        vars.insert("XDG_DATA_DIRS", data_dirs.into());
        if let Some(d) = desktop {
            vars.insert("XDG_CURRENT_DESKTOP", d.into());
        }
        let base = base_dirs_from(&|k| vars.get(k).cloned()).unwrap();
        AppRegistry::load(&base).unwrap()
    }
}

fn default_id(r: &AppRegistry, mime: &str) -> Option<String> {
    r.default_for(mime, &MimeDb::default())
        .map(|e| e.id.clone())
}

fn listed(r: &AppRegistry, mime: &str) -> Vec<String> {
    r.apps_for(mime, &MimeDb::default())
        .iter()
        .map(|e| e.id.clone())
        .collect()
}

// Section 4 names no explicit default here, so the most-preferred
// association is the default: `b.desktop` and `c.desktop` register the type
// in the system directory, `z.desktop` in the user directory. The user
// directory comes first in the data directory order, then by ID.
#[test]
fn default_falls_back_to_mimeinfo_cache_order() {
    let t = Tree::new();
    t.app("sys/data", "c.desktop", &["image/png"]);
    t.app("sys/data", "b.desktop", &["image/png", "image/gif"]);
    t.app("data", "z.desktop", &["image/png"]);
    t.app("sys2/data", "a.desktop", &["image/png"]);
    let r = t.registry(None);
    assert_eq!(
        listed(&r, "image/png"),
        ["z.desktop", "b.desktop", "c.desktop", "a.desktop"]
    );
    assert_eq!(default_id(&r, "image/png").unwrap(), "z.desktop");
    assert_eq!(default_id(&r, "image/gif").unwrap(), "b.desktop");
    // Nothing registers or adds this type: the spec yields nothing.
    assert_eq!(default_id(&r, "video/mp4"), None);
}

// An Added Association ranks before every registration, wherever the
// registering entry lives.
#[test]
fn added_association_beats_cache_order() {
    let t = Tree::new();
    t.app("data", "first.desktop", &["image/png"]);
    t.app("sys/data", "added.desktop", &[]);
    t.write(
        "config/mimeapps.list",
        "[Added Associations]\nimage/png=added.desktop;\n",
    );
    let r = t.registry(None);
    assert_eq!(default_id(&r, "image/png").unwrap(), "added.desktop");
    assert_eq!(listed(&r, "image/png"), ["added.desktop", "first.desktop"]);
}

// `first.desktop` would head the registrations; the Removed line takes it
// out, so the next one is the default. A type whose only registration is
// removed has no default.
#[test]
fn removed_association_is_skipped() {
    let t = Tree::new();
    t.app("data", "first.desktop", &["image/png", "image/bmp"]);
    t.app("data", "second.desktop", &["image/png"]);
    t.write(
        "config/mimeapps.list",
        "[Removed Associations]\nimage/png=first.desktop;\nimage/bmp=first.desktop;\n",
    );
    let r = t.registry(None);
    assert_eq!(default_id(&r, "image/png").unwrap(), "second.desktop");
    assert_eq!(default_id(&r, "image/bmp"), None);
}

// An entry under [Default Applications] beats both an added association and
// a registration, and the system list loses to the user list.
#[test]
fn explicit_default_still_wins() {
    let t = Tree::new();
    t.app("data", "reg.desktop", &["image/png"]);
    t.app("data", "added.desktop", &[]);
    t.app("data", "user.desktop", &[]);
    t.app("data", "sys.desktop", &[]);
    t.write(
        "config/mimeapps.list",
        "[Default Applications]\nimage/png=user.desktop;\n[Added Associations]\nimage/png=added.desktop;\n",
    );
    t.write(
        "sys/config/mimeapps.list",
        "[Default Applications]\nimage/png=sys.desktop;\n",
    );
    let r = t.registry(None);
    assert_eq!(default_id(&r, "image/png").unwrap(), "user.desktop");
}

// The explicit default names an application that is not installed, so the
// second name is used; with no second usable name the association order
// decides.
#[test]
fn uninstalled_explicit_default_is_skipped() {
    let t = Tree::new();
    t.app("data", "reg.desktop", &["image/png", "image/gif"]);
    t.app("data", "listed.desktop", &[]);
    t.write(
        "config/mimeapps.list",
        "[Default Applications]\nimage/png=gone.desktop;listed.desktop;\nimage/gif=gone.desktop;\n",
    );
    let r = t.registry(None);
    assert_eq!(default_id(&r, "image/png").unwrap(), "listed.desktop");
    assert_eq!(default_id(&r, "image/gif").unwrap(), "reg.desktop");
}

// `hyprland-mimeapps.list` (the lowercased XDG_CURRENT_DESKTOP) in the same
// directory outranks `mimeapps.list`; with another desktop it is not read.
#[test]
fn desktop_specific_mimeapps_is_read() {
    let t = Tree::new();
    t.app("data", "plain.desktop", &[]);
    t.app("data", "hypr.desktop", &[]);
    t.app("data", "reg.desktop", &["image/png"]);
    t.write(
        "config/mimeapps.list",
        "[Default Applications]\nimage/png=plain.desktop;\n",
    );
    t.write(
        "config/hyprland-mimeapps.list",
        "[Default Applications]\nimage/png=hypr.desktop;\n",
    );
    assert_eq!(
        default_id(&t.registry(Some("Hyprland")), "image/png").unwrap(),
        "hypr.desktop"
    );
    assert_eq!(
        default_id(&t.registry(Some("KDE")), "image/png").unwrap(),
        "plain.desktop"
    );
    // A desktop-specific file that only adds still counts as an association.
    t.write(
        "config/hyprland-mimeapps.list",
        "[Added Associations]\nimage/gif=hypr.desktop;\n",
    );
    assert_eq!(
        default_id(&t.registry(Some("Hyprland")), "image/gif").unwrap(),
        "hypr.desktop"
    );
}

// Whatever the default is, it is the head of the list offered, unless an
// explicit default names a different entry.
#[test]
fn fallback_default_is_the_head_of_apps_for() {
    let t = Tree::new();
    t.app("sys/data", "m.desktop", &["text/plain"]);
    t.app("sys/data", "k.desktop", &["text/plain"]);
    let r = t.registry(None);
    let list = listed(&r, "text/plain");
    assert_eq!(default_id(&r, "text/plain").unwrap(), list[0]);
}

// A NoDisplay entry is not an association, so it is not a fallback default.
#[test]
fn no_display_entry_is_not_a_fallback() {
    let t = Tree::new();
    t.write(
        "data/applications/hidden-ui.desktop",
        "[Desktop Entry]\nType=Application\nName=x\nExec=true\nNoDisplay=true\nMimeType=image/png;\n",
    );
    let r = t.registry(None);
    assert_eq!(default_id(&r, "image/png"), None);
}
