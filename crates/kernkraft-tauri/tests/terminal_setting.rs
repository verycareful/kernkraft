// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A `Terminal=true` application runs in the terminal the user's setting
//! names, when neither `xdg-terminal-exec` nor `$TERMINAL` is there.
//!
//! A test binary of its own: `PATH` holds one stub, `my-term`, and no
//! `xdg-terminal-exec` (which would be chosen first). The stub records its
//! arguments and exits; nothing real is started, and the XDG directories
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
    let deadline = Instant::now() + EVENT_WAIT;
    let records = loop {
        let records: Vec<Vec<String>> = fs::read_dir(&log)
            .unwrap()
            .map(|e| fs::read_to_string(e.unwrap().path()).unwrap_or_default())
            .map(|t| t.lines().map(str::to_owned).collect::<Vec<_>>())
            .filter(|l| l.last().map(String::as_str) == Some("END"))
            .map(|l| l[..l.len() - 1].to_vec())
            .collect();
        if records.len() >= 2 || Instant::now() > deadline {
            break records;
        }
        std::thread::sleep(Duration::from_millis(20));
    };
    assert_eq!(records.len(), 2, "{records:?}");
    for r in &records {
        assert_eq!(r, &["-e".to_owned(), "htop".to_owned(), file_arg.clone()]);
    }

    // Without the setting there is no terminal to use.
    let err =
        e.kk.launch_app("top.desktop".to_owned(), vec![w(&file)], None)
            .unwrap_err();
    assert_eq!(err.kind, "NoTerminal");
}
