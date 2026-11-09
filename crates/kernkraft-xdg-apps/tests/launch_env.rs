// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! A program started from a process that runs inside an application bundle
//! gets the environment without the bundle.
//!
//! A test binary of its own with one test: it sets the process environment,
//! which nothing else may read while it changes. The program is a stub that
//! writes its environment to a file.

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use kernkraft_xdg_apps::{launch, parse_desktop_entry, Targets};

/// The `NAME=value` lines of the environment the stub recorded, once the
/// record is complete.
fn recorded(file: &Path) -> Vec<String> {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let text = fs::read_to_string(file).unwrap_or_default();
        if text.lines().last() == Some("RECORD_END=1") || Instant::now() > deadline {
            return text.lines().map(str::to_owned).collect();
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn value<'a>(env: &'a [String], name: &str) -> Option<&'a str> {
    env.iter()
        .find_map(|l| l.strip_prefix(name)?.strip_prefix('='))
}

#[test]
fn a_started_program_does_not_inherit_the_bundle() {
    let work = tempfile::tempdir().unwrap();
    let root = work.path();
    let app = root.join("app");
    let record = root.join("env.txt");
    let stub = root.join("stub");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\n/usr/bin/env > '{0}.part'\necho RECORD_END=1 >> '{0}.part'\nmv '{0}.part' '{0}'\n",
            record.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();

    let keep_bin = root.join("bin");
    std::env::set_var("APPDIR", &app);
    std::env::set_var("APPIMAGE", root.join("kern.AppImage"));
    std::env::set_var("PYTHONHOME", format!("{}/usr/", app.display()));
    std::env::set_var("GTK_PATH", format!("{}//usr/lib/gtk-3.0", app.display()));
    std::env::set_var(
        "PATH",
        format!("{}/usr/bin/:{}:/usr/bin", app.display(), keep_bin.display()),
    );
    std::env::set_var(
        "XDG_DATA_DIRS",
        format!("{}/usr/share:/usr/share", app.display()),
    );
    std::env::set_var("KERN_KEEP_ME", "yes");

    let entry = parse_desktop_entry(
        "stub.desktop",
        Path::new("stub.desktop"),
        &format!(
            "[Desktop Entry]\nType=Application\nName=Stub\nExec={}\n",
            stub.display()
        ),
    )
    .unwrap();
    launch(&entry, Targets::Files(&[]), None).unwrap();

    let env = recorded(&record);
    assert_eq!(value(&env, "RECORD_END"), Some("1"), "{env:?}");
    for gone in ["APPDIR", "APPIMAGE", "PYTHONHOME", "GTK_PATH"] {
        assert_eq!(value(&env, gone), None, "{gone} reached the program");
    }
    assert_eq!(
        value(&env, "PATH"),
        Some(format!("{}:/usr/bin", keep_bin.display()).as_str())
    );
    assert_eq!(value(&env, "XDG_DATA_DIRS"), Some("/usr/share"));
    assert_eq!(value(&env, "KERN_KEEP_ME"), Some("yes"));
}
