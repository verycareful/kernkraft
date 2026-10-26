// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Desktop Entry Specification 1.5, `TryExec`: an entry whose program is
//! missing or not executable is not installed. One test only, because it
//! sets `PATH` for the whole process.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;

use kernkraft_fs::base_dirs_from;
use kernkraft_xdg_apps::{AppRegistry, MimeDb};

#[test]
fn try_exec_decides_whether_an_entry_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    for (name, mode) in [("present", 0o755), ("plain-file", 0o644)] {
        fs::write(bin.join(name), "#!/bin/sh\n").unwrap();
        fs::set_permissions(bin.join(name), fs::Permissions::from_mode(mode)).unwrap();
    }
    let apps = root.join("data/applications");
    fs::create_dir_all(&apps).unwrap();
    // In listing order (by ID): a, b, c, d, e.
    let entries = [
        ("a-absent.desktop", "TryExec=absent-program\n"),
        ("b-plain.desktop", "TryExec=plain-file\n"),
        ("c-present.desktop", "TryExec=present\n"),
        (
            "d-path.desktop",
            &format!("TryExec={}/present\n", bin.display()),
        ),
        ("e-none.desktop", ""),
    ];
    for (id, try_exec) in &entries {
        fs::write(
            apps.join(id),
            format!(
                "[Desktop Entry]\nType=Application\nName=n\nExec=true\n{try_exec}MimeType=image/png;\n"
            ),
        )
        .unwrap();
    }
    std::env::set_var("PATH", &bin);
    let mut vars: HashMap<&str, OsString> = HashMap::new();
    vars.insert("HOME", root.into());
    vars.insert("XDG_CONFIG_HOME", root.join("config").into());
    vars.insert("XDG_DATA_HOME", root.join("data").into());
    vars.insert("XDG_CONFIG_DIRS", root.join("none").into());
    vars.insert("XDG_DATA_DIRS", root.join("none").into());
    let base = base_dirs_from(&|k| vars.get(k).cloned()).unwrap();
    let r = AppRegistry::load(&base).unwrap();
    let db = MimeDb::default();
    let listed: Vec<&str> = r
        .apps_for("image/png", &db)
        .iter()
        .map(|e| e.id.as_str())
        .collect();
    // absent and non-executable are skipped; the bare name is found on PATH.
    assert_eq!(
        listed,
        ["c-present.desktop", "d-path.desktop", "e-none.desktop"]
    );
    assert_eq!(
        r.default_for("image/png", &db).unwrap().id,
        "c-present.desktop"
    );
    // An explicit default with a missing program is skipped too.
    fs::create_dir_all(root.join("config")).unwrap();
    fs::write(
        root.join("config/mimeapps.list"),
        "[Default Applications]\nimage/png=a-absent.desktop;e-none.desktop;\n",
    )
    .unwrap();
    let r = AppRegistry::load(&base).unwrap();
    assert_eq!(
        r.default_for("image/png", &db).unwrap().id,
        "e-none.desktop"
    );
}
