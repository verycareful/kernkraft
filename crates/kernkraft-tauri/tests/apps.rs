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
    "rec-browser",
    "rec-shell",
    "rec-open",
    "rec-pics",
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
    let outcomes = e.kk.open_default(vec![w(&a), w(&b)], None).unwrap();
    assert_eq!(outcomes.len(), 1);
    assert_eq!(outcomes[0].app, "viewer.desktop");
    assert_eq!(outcomes[0].paths, [w(&a), w(&b)]);
    assert_eq!(outcomes[0].error, None);
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

// One application failing to start is its own outcome, with its own files;
// the application before it and the one after it are started and say so.
#[test]
fn open_default_reports_each_application_and_a_failure_stops_no_other() {
    stubs();
    let e = env();
    viewer(&e);
    desktop(
        &e,
        "broken.desktop",
        "[Desktop Entry]\nType=Application\nName=Broken\nExec=/definitely/not/a/program %F\nMimeType=application/pdf;\n",
    );
    desktop(
        &e,
        "pics.desktop",
        "[Desktop Entry]\nType=Application\nName=Pics\nExec=rec-pics %F\nMimeType=image/png;\n",
    );
    mimeapps(
        &e,
        "[Default Applications]\ntext/plain=viewer.desktop\napplication/pdf=broken.desktop\nimage/png=pics.desktop\n",
    );
    let dir = e.dir("each");
    let (a, doc, pic) = (
        file(&dir, "a.txt", b"a"),
        file(&dir, "doc.pdf", b"d"),
        file(&dir, "p.png", b"p"),
    );

    let outcomes =
        e.kk.open_default(vec![w(&a), w(&doc), w(&pic)], None)
            .unwrap();

    let apps: Vec<&str> = outcomes.iter().map(|o| o.app.as_str()).collect();
    assert_eq!(apps, ["viewer.desktop", "broken.desktop", "pics.desktop"]);
    assert_eq!(outcomes[0].error, None);
    let failed = outcomes[1].error.as_ref().unwrap();
    assert_eq!(failed.kind, "Launch");
    assert_eq!(outcomes[1].paths, [w(&doc)]);
    assert_eq!(outcomes[2].error, None);
    assert_eq!(outcomes[2].paths, [w(&pic)]);
    // The two that were started got exactly their own files.
    assert_eq!(
        records("rec-open", "each", 1)[0][1..],
        [a.to_string_lossy().into_owned()]
    );
    assert_eq!(
        records("rec-pics", "each", 1)[0][1..],
        [pic.to_string_lossy().into_owned()]
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
    // The default first, then the desktop entries naming the type, by ID.
    assert_eq!(ids, ["viewer.desktop", "notes.desktop"]);
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

// NoDisplay hides an application from the "Open with" list, but never the
// current default: here `custom.desktop` is NoDisplay and is the default.
#[test]
fn apps_for_omits_no_display_except_the_default() {
    stubs();
    let e = env();
    viewer(&e);
    desktop(
        &e,
        "custom.desktop",
        "[Desktop Entry]\nType=Application\nName=Custom\nExec=rec-open %f\nNoDisplay=true\nMimeType=text/plain;\n",
    );
    let doc = file(&e.dir("nd"), "a.txt", b"x");
    let ids = |e: &Env| -> Vec<String> {
        e.kk.apps_for(w(&doc))
            .unwrap()
            .apps
            .into_iter()
            .map(|a| a.id)
            .collect()
    };
    // viewer is the default; custom is NoDisplay and not the default.
    assert_eq!(ids(&e), ["viewer.desktop"]);
    mimeapps(&e, "[Default Applications]\ntext/plain=custom.desktop\n");
    assert_eq!(ids(&e), ["custom.desktop", "viewer.desktop"]);
    let default = e.kk.app_default(w(&doc)).unwrap().app.unwrap();
    assert!(default.no_display);
    assert_eq!(default.id, "custom.desktop");
}

// The default is always in the "Open with" list, first, even when it is named
// under [Default Applications] without registering the type: here
// `other.desktop` lists only image/png and is the default for text/plain.
#[test]
fn apps_for_shows_a_default_that_is_not_an_association_first() {
    stubs();
    let e = env();
    viewer(&e);
    desktop(
        &e,
        "other.desktop",
        "[Desktop Entry]\nType=Application\nName=Other\nExec=rec-open %f\nMimeType=image/png;\n",
    );
    mimeapps(&e, "[Default Applications]\ntext/plain=other.desktop\n");
    let doc = file(&e.dir("na"), "a.txt", b"x");
    let ids: Vec<String> =
        e.kk.apps_for(w(&doc))
            .unwrap()
            .apps
            .into_iter()
            .map(|a| a.id)
            .collect();
    assert_eq!(ids, ["other.desktop", "viewer.desktop"]);
    assert_eq!(
        e.kk.app_default(w(&doc)).unwrap().app.unwrap().id,
        "other.desktop"
    );
}

// A default that is an association too is first as well, whatever the order
// the associations come in: `a-first.desktop` sorts before `viewer.desktop`,
// which is the default.
#[test]
fn apps_for_puts_the_default_first_among_the_associations() {
    stubs();
    let e = env();
    viewer(&e);
    desktop(
        &e,
        "a-first.desktop",
        "[Desktop Entry]\nType=Application\nName=First\nExec=rec-open %f\nMimeType=text/plain;\n",
    );
    let doc = file(&e.dir("df"), "a.txt", b"x");
    let ids: Vec<String> =
        e.kk.apps_for(w(&doc))
            .unwrap()
            .apps
            .into_iter()
            .map(|a| a.id)
            .collect();
    assert_eq!(ids, ["viewer.desktop", "a-first.desktop"]);
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

// ---- open_url ----

fn browser(e: &Env, exec: &str) {
    desktop(
        e,
        "browser.desktop",
        &format!("[Desktop Entry]\nType=Application\nName=Browser\nExec={exec}\n"),
    );
    mimeapps(
        e,
        "[Default Applications]\nx-scheme-handler/https=browser.desktop\n",
    );
}

#[test]
fn open_url_starts_the_default_for_https_with_the_url_as_it_is() {
    stubs();
    let e = env();
    browser(&e, "rec-browser --new-tab %u");
    let url = "https://github.com/verycareful/kern?from=open-url-test#readme";
    e.kk.open_url(url.to_owned(), None).unwrap();
    let got = records("rec-browser", "open-url-test", 1);
    assert_eq!(got.len(), 1);
    assert_eq!(got[0][1..], ["--new-tab", url]);
}

#[test]
fn open_url_refuses_anything_but_an_https_url_and_starts_nothing() {
    stubs();
    let e = env();
    // Every launch of this handler carries the marker, whatever its URL.
    browser(&e, "rec-browser open-url-refused %u");
    for bad in [
        "http://refused-scheme.example/",
        "file:///etc/passwd",
        "javascript:alert(1)",
        "HTTPS://refused-case.example/",
        "https://",
        "https:///refused-no-host",
        "https://refused space.example/",
        "https://refused-quote.example/\"x",
        "https://refused-newline.example/\n",
        "https://refused-non-ascii.example/\u{e4}",
        "https://refused-backslash.example\\x",
        "https://@/refused-userinfo-only",
        "https://:443/refused-port-only",
        "https://[/refused-open-bracket",
        "https://[]/refused-empty-literal",
        "",
    ] {
        let err = e.kk.open_url(bad.to_owned(), None).unwrap_err();
        assert_eq!(err.kind, "InvalidArgument", "{bad:?}");
    }
    assert!(no_records("rec-browser", "open-url-refused"));
}

#[test]
fn open_url_takes_a_host_after_userinfo_before_a_port_and_in_brackets() {
    stubs();
    let e = env();
    browser(&e, "rec-browser %u");
    let urls = [
        "https://user@example.org:8443/open-url-host-test",
        "https://[2001:db8::1]/open-url-host-test",
    ];
    for url in urls {
        e.kk.open_url(url.to_owned(), None).unwrap();
    }
    let got = records("rec-browser", "open-url-host-test", 2);
    let mut want: Vec<Vec<String>> = urls.iter().map(|u| vec![u.to_string()]).collect();
    want.sort();
    let mut seen: Vec<Vec<String>> = got.iter().map(|r| r[1..].to_vec()).collect();
    seen.sort();
    assert_eq!(seen, want);
}

#[test]
fn open_url_refuses_a_default_that_puts_the_url_in_a_shell_command_line() {
    stubs();
    let e = env();
    // The URL holds shell syntax RFC 3986 allows; inside a quoted argument
    // a shell would run it, so the entry is refused and nothing starts.
    browser(&e, "rec-shell -c \"rec-browser %u\"");
    let err =
        e.kk.open_url(
            "https://example.org/$(open-url-shell-test)".to_owned(),
            None,
        )
        .unwrap_err();
    assert_eq!(err.kind, "InvalidExec");
    assert!(no_records("rec-shell", "open-url-shell-test"));
    assert!(no_records("rec-browser", "open-url-shell-test"));
}

#[test]
fn open_url_without_a_default_for_https_says_so() {
    stubs();
    let e = env();
    mimeapps(&e, "[Default Applications]\n");
    let err =
        e.kk.open_url("https://example.org/no-handler".to_owned(), None)
            .unwrap_err();
    assert_eq!(err.kind, "NoUrlHandler");
}

#[test]
fn open_url_refuses_a_default_that_takes_only_files() {
    stubs();
    let e = env();
    browser(&e, "rec-browser %F");
    let err =
        e.kk.open_url("https://example.org/files-only-test".to_owned(), None)
            .unwrap_err();
    assert_eq!(err.kind, "NoUrlArgument");
    assert!(no_records("rec-browser", "files-only-test"));
}
