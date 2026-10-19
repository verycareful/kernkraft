// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use kernkraft_fs::{base_dirs_from, BaseDirs};
use kernkraft_theme::{gather, read_portal, watch_with, Gathered, Rgba, ThemeError, DEBOUNCE};

fn dirs(root: &Path) -> BaseDirs {
    let home = root.to_path_buf();
    let config = root.join("config");
    base_dirs_from(&|k| match k {
        "HOME" => Some(home.clone().into()),
        "XDG_CONFIG_HOME" => Some(config.clone().into()),
        _ => None,
    })
    .unwrap()
}

/// Writes `contents` to a temp name and renames it over `path`, as editors
/// and the dotfiles `theme` script replace a file.
fn replace(path: &Path, contents: &str) {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, contents).unwrap();
    std::fs::rename(&tmp, path).unwrap();
}

fn accent_conf(rgb: &str) -> String {
    format!("accent = rgb({rgb})\n")
}

fn collector() -> (Box<dyn Fn(Gathered) + Send>, mpsc::Receiver<Gathered>) {
    let (tx, rx) = mpsc::channel();
    (
        Box::new(move |g| {
            let _ = tx.send(g);
        }),
        rx,
    )
}

#[test]
fn watch_fires_on_replace() {
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    let hypr = base.config.join("hypr");
    std::fs::create_dir_all(&hypr).unwrap();
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("010203"));
    let (cb, rx) = collector();
    let _w = watch_with(&base, None, cb).unwrap();
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("0a0b0c"));
    let g = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert!(g.errors.is_empty(), "{:?}", g.errors);
    assert_eq!(
        g.sources.hypr.unwrap().accent,
        Some(Rgba::rgb(0x0a, 0x0b, 0x0c))
    );
}

#[test]
fn burst_coalesces() {
    // Three writes across two directories must land inside one debounce
    // window for the claim to apply. A loaded machine can stretch them past
    // it; that attempt proves nothing, so retry and skip if every attempt
    // was slow.
    for attempt in 0..5 {
        let root = tempfile::tempdir().unwrap();
        let base = dirs(root.path());
        let hypr = base.config.join("hypr");
        let gtk = base.config.join("gtk-4.0");
        std::fs::create_dir_all(&hypr).unwrap();
        std::fs::create_dir_all(&gtk).unwrap();
        let (cb, rx) = collector();
        let _w = watch_with(&base, None, cb).unwrap();
        let start = Instant::now();
        replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("010101"));
        std::fs::write(gtk.join("settings.ini"), "[Settings]\n").unwrap();
        replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("020202"));
        let span = start.elapsed();
        let first = rx.recv_timeout(Duration::from_secs(2)).unwrap();
        let extra = rx.recv_timeout(Duration::from_millis(600)).is_ok();
        if span > DEBOUNCE / 2 {
            eprintln!("attempt {attempt}: writes took {span:?}, retrying");
            continue;
        }
        assert!(!extra, "a second callback followed one burst");
        assert_eq!(
            first.sources.hypr.unwrap().accent,
            Some(Rgba::rgb(2, 2, 2)),
            "callback ran before the burst ended"
        );
        assert!(first.sources.gtk.is_some());
        return;
    }
    eprintln!("every attempt was too slow to land inside the debounce window: skipping");
}

#[test]
fn drop_stops_watching() {
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    let hypr = base.config.join("hypr");
    std::fs::create_dir_all(&hypr).unwrap();
    let (cb, rx) = collector();
    let w = watch_with(&base, None, cb).unwrap();
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("010101"));
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    drop(w);
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("020202"));
    // The callback and its sender were dropped with the worker thread, so
    // the channel reports disconnection, not a message, once drop returned.
    match rx.recv_timeout(Duration::from_millis(600)) {
        Err(mpsc::RecvTimeoutError::Disconnected) => {}
        other => panic!("expected disconnected, got {other:?}"),
    }
}

#[test]
fn directory_created_after_watch_starts() {
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    // Neither `config` nor `config/hypr` exists yet.
    let (cb, rx) = collector();
    let _w = watch_with(&base, None, cb).unwrap();
    let hypr = base.config.join("hypr");
    std::fs::create_dir_all(&hypr).unwrap();
    // The directory appearing is itself a change.
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    // The new directory is now watched directly.
    while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("112233"));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let g = rx.recv_timeout(left).expect("no callback after write");
        if g.sources
            .hypr
            .is_some_and(|h| h.accent == Some(Rgba::rgb(0x11, 0x22, 0x33)))
        {
            break;
        }
    }
}

#[test]
fn directory_removed_and_recreated() {
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    let hypr = base.config.join("hypr");
    std::fs::create_dir_all(&hypr).unwrap();
    let (cb, rx) = collector();
    let _w = watch_with(&base, None, cb).unwrap();
    std::fs::remove_dir_all(&hypr).unwrap();
    rx.recv_timeout(Duration::from_secs(2)).unwrap();
    while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
    std::fs::create_dir_all(&hypr).unwrap();
    while rx.recv_timeout(Duration::from_millis(300)).is_ok() {}
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("445566"));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let g = rx.recv_timeout(left).expect("no callback after recreate");
        if g.sources
            .hypr
            .is_some_and(|h| h.accent == Some(Rgba::rgb(0x44, 0x55, 0x66)))
        {
            break;
        }
    }
}

#[test]
fn gather_reports_unreadable_sources() {
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    // Nothing on disk: absent sources, no errors.
    let g = gather(&base, None);
    assert!(g.errors.is_empty(), "{:?}", g.errors);
    assert!(g.sources.hypr.is_none() && g.sources.gtk.is_none() && g.sources.portal.is_none());
    // An oversize file is an error, and does not hide the other source.
    let hypr = base.config.join("hypr");
    let gtk = base.config.join("gtk-4.0");
    std::fs::create_dir_all(&hypr).unwrap();
    std::fs::create_dir_all(&gtk).unwrap();
    std::fs::write(hypr.join("hyprtoolkit.conf"), vec![b'#'; (1 << 20) + 1]).unwrap();
    std::fs::write(
        gtk.join("settings.ini"),
        "[Settings]\ngtk-font-name=Sans 11\n",
    )
    .unwrap();
    let g = gather(&base, None);
    assert!(g.sources.hypr.is_none());
    assert_eq!(g.sources.gtk.unwrap().font_name.as_deref(), Some("Sans"));
    assert_eq!(g.errors.len(), 1);
    assert!(matches!(g.errors[0], ThemeError::TooLarge { .. }));
    // An unreadable path that is not a plain "missing" is an error too.
    std::fs::remove_file(hypr.join("hyprtoolkit.conf")).unwrap();
    std::fs::create_dir(hypr.join("hyprtoolkit.conf")).unwrap();
    let g = gather(&base, None);
    assert!(g.sources.hypr.is_none());
    assert!(matches!(g.errors[0], ThemeError::Io { .. }));
}

/// Needs a session bus; returns early without one. The portal itself is
/// optional: `Ok(None)` is a valid answer.
#[test]
fn live_bus_read_and_drop_ends_portal_thread() {
    let Ok(conn) = kernkraft_bus::session() else {
        eprintln!("no session bus: skipping");
        return;
    };
    let portal = read_portal(&conn);
    assert!(portal.is_ok(), "{portal:?}");
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    let (cb, _rx) = collector();
    let w = watch_with(&base, Some(conn), cb).unwrap();
    let start = Instant::now();
    drop(w);
    // Drop joins the portal thread, so returning at all proves it exited;
    // the bound catches a drop that waits on a timeout instead.
    assert!(
        start.elapsed() < Duration::from_secs(2),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn callback_panic_is_reported_and_watcher_survives() {
    let root = tempfile::tempdir().unwrap();
    let base = dirs(root.path());
    let hypr = base.config.join("hypr");
    std::fs::create_dir_all(&hypr).unwrap();
    let (tx, rx) = mpsc::channel();
    let calls = std::sync::atomic::AtomicUsize::new(0);
    let w = watch_with(
        &base,
        None,
        Box::new(move |g| {
            if calls.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                panic!("boom");
            }
            let _ = tx.send(g);
        }),
    )
    .unwrap();
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("010101"));
    // The first callback panics; wait for it to have run, then change again.
    std::thread::sleep(Duration::from_millis(600));
    replace(&hypr.join("hyprtoolkit.conf"), &accent_conf("020202"));
    let g = rx.recv_timeout(Duration::from_secs(2)).unwrap();
    assert_eq!(g.sources.hypr.unwrap().accent, Some(Rgba::rgb(2, 2, 2)));
    assert!(
        g.errors
            .iter()
            .any(|e| matches!(e, ThemeError::CallbackPanic(m) if m == "boom")),
        "{:?}",
        g.errors
    );
    drop(w);
}
