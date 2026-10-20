// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Applications: lookup, default apps, launching and the terminal.
//!
//! Nothing real is ever started. `PATH` holds only a directory of stub
//! shell scripts made once for this test binary, so every program the
//! plugin runs (the applications' `Exec` programs, `xdg-mime`,
//! `xdg-terminal-exec`) is a stub that records its working directory and
//! arguments and exits. The XDG directories are the test app's temp ones,
//! so no real `mimeapps.list` is read or written.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::ffi::OsStr;
use std::fs;
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use common::{env, w, Env, EVENT_WAIT};

/// Programs the stubs stand in for. Each records into `<log>/<name>.<pid>`.
const STUBS: &[&str] = &[
    "rec-open",
    "rec-launch",
    "rec-terminal-app",
    "xdg-mime",
    "xdg-terminal-exec",
];

struct Stubs {
    log: PathBuf,
    _dir: tempfile::TempDir,
}

/// Writes the stubs and points `PATH` at them, once per test binary,
/// before any test starts a program. Every test calls it first, so no
/// program is being started while the scripts are written (which could
/// fail with ETXTBSY) or while the environment changes.
fn stubs() -> &'static Stubs {
    static STUBS_ONCE: OnceLock<Stubs> = OnceLock::new();
    STUBS_ONCE.get_or_init(|| {
        let dir = tempfile::tempdir().unwrap();
        let bin = dir.path().join("bin");
        let log = dir.path().join("log");
        fs::create_dir_all(&bin).unwrap();
        fs::create_dir_all(&log).unwrap();
        for name in STUBS {
            // Builtins only: PATH holds nothing but these stubs. The last
            // line marks the record complete.
            let script = format!(
                "#!/bin/sh\nprintf '%s\\n' \"$PWD\" \"$@\" END > '{}/{name}.'$$\n",
                log.display()
            );
            let p = bin.join(name);
            fs::write(&p, script).unwrap();
            fs::set_permissions(&p, fs::Permissions::from_mode(0o755)).unwrap();
        }
        std::env::set_var("PATH", &bin);
        std::env::remove_var("TERMINAL");
        Stubs { log, _dir: dir }
    })
}

/// The records of stub `name` whose arguments mention `marker`, each as its
/// lines without the end mark, waiting until `count` are complete.
fn records(name: &str, marker: &str, count: usize) -> Vec<Vec<String>> {
    let deadline = Instant::now() + EVENT_WAIT;
    loop {
        let mut found = Vec::new();
        for entry in fs::read_dir(&stubs().log).unwrap() {
            let entry = entry.unwrap();
            let file = entry.file_name().to_string_lossy().into_owned();
            if !file.starts_with(&format!("{name}.")) {
                continue;
            }
            let text = fs::read_to_string(entry.path()).unwrap_or_default();
            let lines: Vec<String> = text.lines().map(str::to_owned).collect();
            if lines.last().map(String::as_str) == Some("END")
                && lines.iter().any(|l| l.contains(marker))
            {
                found.push(lines[..lines.len() - 1].to_vec());
            }
        }
        if found.len() >= count || Instant::now() > deadline {
            found.sort();
            return found;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// No record of stub `name` mentions `marker`.
fn no_records(name: &str, marker: &str) -> bool {
    // A launch is started before the command returns; give the stub time
    // to write, then look.
    std::thread::sleep(Duration::from_millis(300));
    records(name, marker, 0).is_empty()
}

fn desktop(e: &Env, id: &str, body: &str) {
    let dir = e.xdg.data.join("applications");
    fs::create_dir_all(&dir).unwrap();
    fs::write(dir.join(id), body).unwrap();
}

fn mimeapps(e: &Env, body: &str) {
    fs::create_dir_all(&e.xdg.config).unwrap();
    fs::write(e.xdg.config.join("mimeapps.list"), body).unwrap();
}

fn viewer(e: &Env) {
    desktop(
        e,
        "viewer.desktop",
        "[Desktop Entry]\nType=Application\nName=Viewer\nExec=rec-open %F\nMimeType=text/plain;\n",
    );
    mimeapps(e, "[Default Applications]\ntext/plain=viewer.desktop\n");
}

fn file(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
    let p = dir.join(name);
    fs::write(&p, bytes).unwrap();
    p
}

#[test]
fn open_default_reports_missing_handler() {
    stubs();
    let e = env();
    viewer(&e);
    let dir = e.dir("odrm");
    let text = file(&dir, "a.txt", b"hello");
    // No name match, and NUL bytes: application/octet-stream, which no
    // application is the default for.
    let blob = file(&dir, "blob", b"\0\0\0");

    let err =
        e.kk.open_default(vec![w(&text), w(&blob)], None)
            .unwrap_err();
    assert_eq!(err.kind, "NoDefaultApp");
    assert_eq!(err.path, Some(w(&blob)));
    // Nothing is opened when any path has no default.
    assert!(no_records("rec-open", "odrm"));

    e.kk.open_default(vec![w(&text)], None).unwrap();
    let got = records("rec-open", "odrm", 1);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0][1..], [text.to_string_lossy().into_owned()]);
}

#[test]
fn open_default_groups_paths_by_app() {
    stubs();
    let e = env();
    viewer(&e);
    let dir = e.dir("group");
    let a = file(&dir, "a.txt", b"a");
    let b = file(&dir, "b.txt", b"b");
    e.kk.open_default(vec![w(&a), w(&b)], None).unwrap();
    // %F: one launch with both files.
    let got = records("rec-open", "group", 1);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0][1..],
        [
            a.to_string_lossy().into_owned(),
            b.to_string_lossy().into_owned()
        ]
    );
}

#[test]
fn launch_app_runs_exec_with_files() {
    stubs();
    let e = env();
    desktop(
        &e,
        "editor.desktop",
        "[Desktop Entry]\nType=Application\nName=Editor\nExec=rec-launch --new %f\n",
    );
    let dir = e.dir("launch");
    let a = file(&dir, "x.md", b"#");
    let b = file(&dir, "y.md", b"#");
    e.kk.launch_app("editor.desktop".to_owned(), vec![w(&a), w(&b)], None)
        .unwrap();
    // %f: one launch per file.
    let got = records("rec-launch", "launch", 2);
    let args: Vec<&[String]> = got.iter().map(|r| &r[1..]).collect();
    assert_eq!(
        args,
        [
            &["--new".to_owned(), a.to_string_lossy().into_owned()][..],
            &["--new".to_owned(), b.to_string_lossy().into_owned()][..]
        ]
    );

    let err =
        e.kk.launch_app("missing.desktop".to_owned(), vec![w(&a)], None)
            .unwrap_err();
    assert_eq!(err.kind, "UnknownApp");
}

#[test]
fn set_default_app_runs_xdg_mime() {
    stubs();
    let e = env();
    desktop(
        &e,
        "sda-viewer.desktop",
        "[Desktop Entry]\nType=Application\nName=V\nExec=rec-open %F\n",
    );
    e.kk.set_default_app("sda-viewer.desktop".to_owned(), "text/markdown".to_owned())
        .unwrap();
    let got = records("xdg-mime", "sda-viewer", 1);
    assert_eq!(got.len(), 1);
    assert_eq!(
        got[0][1..],
        ["default", "sda-viewer.desktop", "text/markdown"]
    );

    let err =
        e.kk.set_default_app("sda-nope.desktop".to_owned(), "text/plain".to_owned())
            .unwrap_err();
    assert_eq!(err.kind, "UnknownApp");
    assert!(no_records("xdg-mime", "sda-nope"));
}

#[test]
fn open_terminal_starts_in_dir() {
    stubs();
    let e = env();
    let dir = e.dir("term-here");
    e.kk.open_terminal(w(&dir), None).unwrap();
    let got = records("xdg-terminal-exec", "term-here", 1);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0], [dir.to_string_lossy().into_owned()]);

    let missing = e.base.join("term-missing");
    let err = e.kk.open_terminal(w(&missing), None).unwrap_err();
    assert_eq!(err.kind, "NotFound");
    assert!(no_records("xdg-terminal-exec", "term-missing"));
}

#[test]
fn apps_for_reports_skipped_entries() {
    stubs();
    let e = env();
    viewer(&e);
    desktop(
        &e,
        "notes.desktop",
        "[Desktop Entry]\nType=Application\nName=Notes\nExec=rec-open %f\nMimeType=text/markdown;text/plain;\n",
    );
    // No [Desktop Entry] group: unusable, and reported.
    desktop(&e, "broken.desktop", "Name=Broken\n");
    let doc = file(&e.dir("af"), "a.txt", b"x");

    let list = e.kk.apps_for(w(&doc)).unwrap();
    let ids: Vec<&str> = list.apps.iter().map(|a| a.id.as_str()).collect();
    // Desktop entries naming the type, by ID.
    assert_eq!(ids, ["notes.desktop", "viewer.desktop"]);
    assert_eq!(list.problems.len(), 1);
    assert_eq!(list.problems[0].kind, "InvalidDesktopEntry");

    let all = e.kk.all_apps().unwrap();
    assert_eq!(all.apps.len(), 2);
    assert_eq!(all.problems.len(), 1);

    let default = e.kk.app_default(w(&doc)).unwrap();
    assert_eq!(default.app.unwrap().id, "viewer.desktop");
    assert_eq!(default.problems.len(), 1);

    let mime = e.kk.mime_of(w(&doc)).unwrap();
    assert_eq!(mime.mime, "text/plain");
    assert!(mime.is_text);
}

/// The complete records of stub `name` whose bytes contain `marker`, as
/// raw lines, waiting until one is there. For arguments that are not UTF-8.
fn raw_records(name: &str, marker: &[u8]) -> Vec<Vec<Vec<u8>>> {
    let deadline = Instant::now() + EVENT_WAIT;
    loop {
        let mut found = Vec::new();
        for entry in fs::read_dir(&stubs().log).unwrap() {
            let entry = entry.unwrap();
            if !entry
                .file_name()
                .as_bytes()
                .starts_with(format!("{name}.").as_bytes())
            {
                continue;
            }
            let bytes = fs::read(entry.path()).unwrap_or_default();
            let lines: Vec<Vec<u8>> = bytes
                .split(|b| *b == b'\n')
                .filter(|l| !l.is_empty())
                .map(<[u8]>::to_vec)
                .collect();
            if lines.last().map(Vec::as_slice) == Some(b"END".as_slice())
                && bytes.windows(marker.len()).any(|w| w == marker)
            {
                found.push(lines[..lines.len() - 1].to_vec());
            }
        }
        if !found.is_empty() || Instant::now() > deadline {
            return found;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn launch_app_passes_non_utf8_names_as_their_bytes() {
    stubs();
    let e = env();
    desktop(
        &e,
        "bytes.desktop",
        "[Desktop Entry]\nType=Application\nName=Bytes\nExec=rec-launch %F\n",
    );
    let dir = e.dir("nonutf8-launch");
    // 0xFF never occurs in UTF-8 (RFC 3629 section 1).
    let odd = dir.join(OsStr::from_bytes(b"caf\xFF.md"));
    fs::write(&odd, "#").unwrap();
    e.kk.launch_app("bytes.desktop".to_owned(), vec![w(&odd)], None)
        .unwrap();
    let got = raw_records("rec-launch", b"nonutf8-launch");
    assert_eq!(got.len(), 1);
    assert_eq!(got[0][1..], [odd.as_os_str().as_bytes().to_vec()]);
}
