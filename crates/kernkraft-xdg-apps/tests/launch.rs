// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Launching: a stub script stands in for the application and records where
//! it ran and what it was given.

use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kernkraft_xdg_apps::{launch, AppsError, DesktopEntry};

fn entry(exec: String) -> DesktopEntry {
    DesktopEntry {
        id: "stub.desktop".to_string(),
        name: "Stub".to_string(),
        exec,
        icon: None,
        mime_types: Vec::new(),
        terminal: false,
        no_display: false,
        hidden: false,
        path: None,
        source: PathBuf::from("/stub.desktop"),
    }
}

#[test]
fn launch_runs_detached_in_cwd_with_files() {
    let work = tempfile::tempdir().unwrap();
    let script = work.path().join("stub.sh");
    fs::write(
        &script,
        "#!/bin/sh\npwd > out.txt\nprintf '%s\\n' \"$@\" >> out.txt\n",
    )
    .unwrap();
    let e = entry(format!("/bin/sh {} %F", script.display()));
    let file = work.path().join("f.txt");
    launch(&e, std::slice::from_ref(&file), Some(work.path())).unwrap();

    let out = work.path().join("out.txt");
    let deadline = Instant::now() + Duration::from_secs(5);
    let text = loop {
        if let Ok(t) = fs::read_to_string(&out) {
            if t.lines().count() >= 2 {
                break t;
            }
        }
        assert!(Instant::now() < deadline, "the stub never ran");
        std::thread::sleep(Duration::from_millis(20));
    };
    let mut lines = text.lines();
    assert_eq!(
        fs::canonicalize(lines.next().unwrap()).unwrap(),
        fs::canonicalize(work.path()).unwrap()
    );
    assert_eq!(lines.next().unwrap(), file.to_str().unwrap());
}

#[test]
fn spawn_failure_is_launch_error() {
    let e = entry("/definitely/not/a/program".to_string());
    let err = launch(&e, &[], None).unwrap_err();
    assert!(matches!(err, AppsError::Launch { ref id, .. } if id == "stub.desktop"));
}
