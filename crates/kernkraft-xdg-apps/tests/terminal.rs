// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Terminal discovery. Every lookup runs against a `PATH` of stub scripts in
//! a temp directory; no real terminal is started.

use std::collections::HashMap;
use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use kernkraft_xdg_apps::{terminal_command_for, AppsError};

fn stub(dir: &Path, name: &str) {
    let path = dir.join(name);
    fs::write(&path, "#!/bin/sh\nexit 0\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn env(path: &Path, terminal: Option<&str>) -> impl Fn(&str) -> Option<OsString> {
    let mut vars: HashMap<String, OsString> = HashMap::new();
    vars.insert("PATH".into(), path.into());
    if let Some(t) = terminal {
        vars.insert("TERMINAL".into(), t.into());
    }
    move |k| vars.get(k).cloned()
}

fn program(cmd: &std::process::Command) -> String {
    Path::new(cmd.get_program())
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .to_string()
}

#[test]
fn terminal_order() {
    let work = tempfile::tempdir().unwrap();
    let bin = work.path().join("bin");
    fs::create_dir(&bin).unwrap();
    stub(&bin, "from-env");
    stub(&bin, "from-setting");
    let argv = [OsString::from("htop"), OsString::from("-d")];

    // Nothing at all: all three sources are named.
    let e = env(&bin, None);
    let err = terminal_command_for(&e, work.path(), None, &[]).unwrap_err();
    let AppsError::NoTerminal { tried } = err else {
        panic!("{err:?}")
    };
    assert_eq!(tried.len(), 3);
    assert!(tried[0].contains("xdg-terminal-exec"));
    assert!(tried[1].contains("$TERMINAL"));
    assert!(tried[2].contains("setting"));

    // The setting alone.
    let cmd = terminal_command_for(&e, work.path(), Some("from-setting"), &argv).unwrap();
    assert_eq!(program(&cmd), "from-setting");
    let args: Vec<_> = cmd.get_args().collect();
    assert_eq!(args, ["-e", "htop", "-d"]);
    assert_eq!(cmd.get_current_dir(), Some(work.path()));

    // $TERMINAL beats the setting.
    let e = env(&bin, Some("from-env"));
    let cmd = terminal_command_for(&e, work.path(), Some("from-setting"), &[]).unwrap();
    assert_eq!(program(&cmd), "from-env");
    assert_eq!(cmd.get_args().count(), 0, "no -e without a program to run");

    // $TERMINAL naming something absent is skipped, the setting is used.
    let e = env(&bin, Some("no-such-terminal"));
    let cmd = terminal_command_for(&e, work.path(), Some("from-setting"), &[]).unwrap();
    assert_eq!(program(&cmd), "from-setting");

    // xdg-terminal-exec on PATH beats both and takes the program directly.
    stub(&bin, "xdg-terminal-exec");
    let e = env(&bin, Some("from-env"));
    let cmd = terminal_command_for(&e, work.path(), Some("from-setting"), &argv).unwrap();
    assert_eq!(program(&cmd), "xdg-terminal-exec");
    let args: Vec<_> = cmd.get_args().collect();
    assert_eq!(args, ["htop", "-d"]);
}

#[test]
fn non_executable_stub_does_not_count() {
    let work = tempfile::tempdir().unwrap();
    fs::write(work.path().join("xdg-terminal-exec"), "x").unwrap();
    let e = env(work.path(), None);
    assert!(matches!(
        terminal_command_for(&e, work.path(), None, &[]),
        Err(AppsError::NoTerminal { .. })
    ));
}
