// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! MIME Applications Associations 1.0.1 sections 2 to 4 against the fixture
//! tree `tests/data/apps/{user,sys}`. `user/` plays `$XDG_CONFIG_HOME` and
//! `$XDG_DATA_HOME`, `sys/` plays `$XDG_CONFIG_DIRS` and `$XDG_DATA_DIRS`.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use kernkraft_fs::base_dirs_from;
use kernkraft_xdg_apps::{AppRegistry, MimeDb};

fn data() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data")
}

fn registry(desktop: Option<&str>) -> AppRegistry {
    let apps = data().join("apps");
    let mut vars: HashMap<&str, OsString> = HashMap::new();
    vars.insert("HOME", data().join("apps/home").into());
    vars.insert("XDG_CONFIG_HOME", apps.join("user/config").into());
    vars.insert("XDG_DATA_HOME", apps.join("user/data").into());
    vars.insert("XDG_CONFIG_DIRS", apps.join("sys/config").into());
    vars.insert("XDG_DATA_DIRS", apps.join("sys/data").into());
    if let Some(d) = desktop {
        vars.insert("XDG_CURRENT_DESKTOP", d.into());
    }
    let base = base_dirs_from(&|k| vars.get(k).cloned()).unwrap();
    AppRegistry::load(&base).unwrap()
}

fn db() -> MimeDb {
    MimeDb::load(&[data()]).unwrap()
}

fn ids<'a>(apps: &[&'a kernkraft_xdg_apps::DesktopEntry]) -> Vec<&'a str> {
    apps.iter().map(|e| e.id.as_str()).collect()
}

// Relies on user/config/mimeapps.list `image/png=mine.desktop` and
// sys/config/mimeapps.list `image/png=viewer.desktop`.
#[test]
fn default_from_user_mimeapps_wins() {
    let r = registry(None);
    assert_eq!(
        r.default_for("image/png", &db()).unwrap().id,
        "mine.desktop"
    );
    // Only the system list names image/jpeg.
    assert_eq!(
        r.default_for("image/jpeg", &db()).unwrap().id,
        "viewer.desktop"
    );
    // `image/gif=missing.desktop;viewer.desktop;`: the first is not installed.
    assert_eq!(
        r.default_for("image/gif", &db()).unwrap().id,
        "viewer.desktop"
    );
    assert!(r.default_for("video/mp4", &db()).is_none());
}

// Relies on user/config/hyprland-mimeapps.list `application/pdf=hypr.desktop`
// and mimeapps.list `application/pdf=viewer.desktop`.
#[test]
fn desktop_specific_list_first() {
    let r = registry(Some("Hyprland"));
    assert_eq!(
        r.default_for("application/pdf", &db()).unwrap().id,
        "hypr.desktop"
    );
    let r = registry(Some("GNOME:Hyprland"));
    assert_eq!(
        r.default_for("application/pdf", &db()).unwrap().id,
        "hypr.desktop"
    );
    let r = registry(None);
    assert_eq!(
        r.default_for("application/pdf", &db()).unwrap().id,
        "viewer.desktop"
    );
}

// Relies on user mimeapps.list: Added `image/png=editor.desktop`, Removed
// `image/png=unwanted.desktop`. unwanted.desktop is neither Hidden nor
// NoDisplay and registers image/png, so only the Removed line excludes it;
// gimp (user entry Hidden) and hidden are excluded by Hidden=true; nodisplay
// stays, as NoDisplay only keeps an entry out of menus.
#[test]
fn removed_association_excluded() {
    let r = registry(None);
    let apps = r.apps_for("image/png", &db());
    // Added first, then registrations by ID.
    assert_eq!(
        ids(&apps),
        [
            "editor.desktop",
            "mine.desktop",
            "nodisplay.desktop",
            "viewer.desktop"
        ]
    );
}

// Relies on editor.desktop `MimeType=text/plain` and the default
// `text/plain=editor.desktop`; text/x-c++src is not in the fixture database
// and is a text type, so text/plain is its parent (Shared MIME-info 2.5).
#[test]
fn falls_back_to_parent_type() {
    let r = registry(None);
    assert_eq!(ids(&r.apps_for("text/x-c++src", &db())), ["editor.desktop"]);
    assert_eq!(
        r.default_for("text/x-c++src", &db()).unwrap().id,
        "editor.desktop"
    );
}

// Relies on tests/data/mime/subclasses `application/json application/javascript`
// and `application/javascript text/plain`, and aliases `application/x-javascript
// application/javascript`.
#[test]
fn parents_and_aliases_are_followed() {
    let r = registry(None);
    assert_eq!(
        ids(&r.apps_for("application/json", &db())),
        ["editor.desktop"]
    );
    assert_eq!(
        ids(&r.apps_for("application/x-javascript", &db())),
        ["editor.desktop"]
    );
}

#[test]
fn ids_come_from_the_applications_tree() {
    let r = registry(None);
    assert_eq!(r.get("kde-foo.desktop").unwrap().name, "Foo");
    // The user's Hidden entry hides the system one with the same ID.
    assert!(r.get("gimp.desktop").is_none());
    assert_eq!(r.get("mine.desktop").unwrap().name, "Mine");
    // One entry per distinct ID among the fixture files, less those that
    // are not applications (a Link, a file that is not an entry) or hidden.
    let mut fixture_ids = std::collections::BTreeSet::new();
    for root in ["user/data/applications", "sys/data/applications"] {
        let root = data().join("apps").join(root);
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            for item in std::fs::read_dir(&dir).unwrap() {
                let path = item.unwrap().path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|e| e == "desktop") {
                    fixture_ids.insert(kernkraft_xdg_apps::desktop_id(&root, &path));
                }
            }
        }
    }
    // Hidden entries are never listed (user gimp, hidden, shadowed).
    for not_an_app in [
        "site.desktop",
        "broken.desktop",
        "gimp.desktop",
        "hidden.desktop",
        "shadowed.desktop",
    ] {
        assert!(fixture_ids.remove(not_an_app));
    }
    assert_eq!(r.all().len(), fixture_ids.len());
    // A Link is not an application and is not an error; the broken file is
    // kept as a reported error, not dropped silently.
    assert!(r.get("site.desktop").is_none());
    assert_eq!(r.skipped().len(), 1);
}

// Relies on user/config/mimeapps.list Removed `image/webp=lowadd.desktop`
// (higher priority) and sys/config/mimeapps.list Added
// `image/webp=lowadd.desktop` (lower); lowadd.desktop has no MimeType.
// MIME Applications Associations 1.0.1 section 3.
#[test]
fn removal_in_higher_file_hides_lower_add() {
    let r = registry(None);
    assert!(r.get("lowadd.desktop").is_some());
    assert!(r.apps_for("image/webp", &db()).is_empty());
}

// Relies on user/config/mimeapps.list listing `image/bmp=both.desktop` under
// Added and under Removed, and both.desktop registering image/bmp: the
// removal is in the same file, so it does not cancel that file's addition.
#[test]
fn same_file_add_survives_removal() {
    let r = registry(None);
    assert_eq!(ids(&r.apps_for("image/bmp", &db())), ["both.desktop"]);
}

// Relies on user/data/applications/shadowed.desktop holding only
// `[Desktop Entry]` and `Hidden=true`, and the system shadowed.desktop being
// a full application registering image/tiff.
#[test]
fn hidden_stub_shadows_system_entry() {
    let r = registry(None);
    assert!(r.get("shadowed.desktop").is_none());
    assert!(r.apps_for("image/tiff", &db()).is_empty());
    assert!(r.all().iter().all(|e| e.id != "shadowed.desktop"));
    // The stub is not an error: only broken.desktop is reported.
    assert_eq!(r.skipped().len(), 1);
}

// Relies on sys hidden.desktop (Hidden=true with Type, Exec and MimeType
// image/png) and the user's Hidden gimp.desktop over the system gimp.
#[test]
fn hidden_entry_not_returned_by_get_or_all() {
    let r = registry(None);
    for id in ["hidden.desktop", "gimp.desktop"] {
        assert!(r.get(id).is_none(), "{id}");
        assert!(r.all().iter().all(|e| e.id != id), "{id}");
        assert!(
            r.apps_for("image/png", &db()).iter().all(|e| e.id != id),
            "{id}"
        );
    }
}

#[test]
fn dangling_symlink_is_reported_not_dropped() {
    let work = tempfile::tempdir().unwrap();
    let apps = work.path().join("data/applications");
    std::fs::create_dir_all(&apps).unwrap();
    std::os::unix::fs::symlink(work.path().join("gone"), apps.join("dangling.desktop")).unwrap();
    let mut vars: HashMap<&str, OsString> = HashMap::new();
    vars.insert("HOME", work.path().into());
    vars.insert("XDG_CONFIG_HOME", work.path().join("config").into());
    vars.insert("XDG_DATA_HOME", work.path().join("data").into());
    vars.insert("XDG_CONFIG_DIRS", work.path().join("none").into());
    vars.insert("XDG_DATA_DIRS", work.path().join("none").into());
    let base = base_dirs_from(&|k| vars.get(k).cloned()).unwrap();
    let r = AppRegistry::load(&base).unwrap();
    assert!(r.all().is_empty());
    assert_eq!(r.skipped().len(), 1);
}
