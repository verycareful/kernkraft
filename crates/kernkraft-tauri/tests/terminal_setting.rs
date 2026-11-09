// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A `Terminal=true` application runs in the terminal the user's setting
//! names, even when another terminal is installed.
//!
//! A test binary of its own: `PATH` starts with one stub, `my-term`, and the
//! test app has no terminal entry; the test adds one (and its stub) later.
//! The stubs record their arguments and exit; nothing real is started, and the XDG directories
//! are the test app's temp ones.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::time::{Duration, Instant};

use common::{env, w, EVENT_WAIT};

#[test]
fn terminal_apps_use_the_terminal_setting() {
    let stubs = tempfile::tempdir().unwrap();
    let bin = stubs.path().join("bin");
    let log = stubs.path().join("log");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&log).unwrap();
    let script = format!(
        "#!/bin/sh\nprintf '%s\\n' \"$@\" END > '{}/my-term.'$$\n",
        log.display()
    );
    let term = bin.join("my-term");
    fs::write(&term, script).unwrap();
    fs::set_permissions(&term, fs::Permissions::from_mode(0o755)).unwrap();
    // The only test in this binary, so nothing else reads the environment
    // while it changes.
    std::env::set_var("PATH", &bin);
    std::env::remove_var("TERMINAL");

    let e = env();
    let apps = e.xdg.data.join("applications");
    fs::create_dir_all(&apps).unwrap();
    fs::write(
        apps.join("top.desktop"),
        "[Desktop Entry]\nType=Application\nName=Top\nExec=htop %f\nTerminal=true\nMimeType=text/plain;\n",
    )
    .unwrap();
    fs::create_dir_all(&e.xdg.config).unwrap();
    fs::write(
        e.xdg.config.join("mimeapps.list"),
        "[Default Applications]\ntext/plain=top.desktop\n",
    )
    .unwrap();
    let file = e.dir("ts").join("a.txt");
    fs::write(&file, "x").unwrap();
    let file_arg = file.to_string_lossy().into_owned();

    e.kk.launch_app(
        "top.desktop".to_owned(),
        vec![w(&file)],
        Some("my-term".to_owned()),
    )
    .unwrap();
    e.kk.open_default(vec![w(&file)], Some("my-term".to_owned()))
        .unwrap();

    // Both launches: the setting's terminal runs `-e <Exec argv>`.
    let records = wait_for_records(&log, 2);
    assert_eq!(records.len(), 2, "{records:?}");
    for r in &records {
        assert_eq!(r, &["-e".to_owned(), "htop".to_owned(), file_arg.clone()]);
    }

    // Without the setting there is no terminal to use.
    let err =
        e.kk.launch_app("top.desktop".to_owned(), vec![w(&file)], None)
            .unwrap_err();
    assert_eq!(err.kind, "NoTerminal");

    // With another terminal installed the setting still decides: the launch
    // reaches `my-term` and never the installed terminal's stub.
    let other_log = stubs.path().join("other-log");
    fs::create_dir_all(&other_log).unwrap();
    let other = bin.join("other-term");
    fs::write(
        &other,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" END > '{}/other.'$$\n",
            other_log.display()
        ),
    )
    .unwrap();
    fs::set_permissions(&other, fs::Permissions::from_mode(0o755)).unwrap();
    fs::write(
        apps.join("other-term.desktop"),
        "[Desktop Entry]\nType=Application\nName=Other\nExec=other-term\nCategories=System;TerminalEmulator;\n",
    )
    .unwrap();
    e.kk.launch_app(
        "top.desktop".to_owned(),
        vec![w(&file)],
        Some("my-term".to_owned()),
    )
    .unwrap();
    let records = wait_for_records(&log, 3);
    assert_eq!(records.len(), 3, "{records:?}");
    assert_eq!(fs::read_dir(&other_log).unwrap().count(), 0);
}

/// The argument lists recorded by the `my-term` stub in `log` (without the
/// `END` marker), once there are `count` of them or `EVENT_WAIT` has passed.
fn wait_for_records(log: &std::path::Path, count: usize) -> Vec<Vec<String>> {
    let deadline = Instant::now() + EVENT_WAIT;
    loop {
        let records: Vec<Vec<String>> = fs::read_dir(log)
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap_or_default())
            .map(|t| t.lines().map(str::to_owned).collect::<Vec<_>>())
            .filter(|l| l.last().map(String::as_str) == Some("END"))
            .map(|l| l[..l.len() - 1].to_vec())
            .collect();
        if records.len() >= count || Instant::now() > deadline {
            return records;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
