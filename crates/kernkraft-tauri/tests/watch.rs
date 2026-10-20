// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Folder watching: `dir-changed` once per burst, `dir-gone` when the
//! folder itself goes away.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

use common::{env, w};
use kernkraft_tauri::{DEBOUNCE_MS, EVENT_DIR_CHANGED, EVENT_DIR_GONE};

#[test]
fn watch_dir_changed_debounced() {
    let e = env();
    let dir = e.dir("watched");
    e.kk.watch_dir("tab-1".into(), w(&dir), None).unwrap();

    // A burst: many changes in quick succession.
    for i in 0..50 {
        fs::write(dir.join(format!("f{i}")), "x").unwrap();
    }
    let last_change = Instant::now();

    let event = e.next_event();
    assert_eq!(event.name, EVENT_DIR_CHANGED);
    assert_eq!(event.payload, serde_json::json!({"key": "tab-1"}));
    // The event waits for the burst to go quiet for the debounce time.
    assert!(event.at.duration_since(last_change) >= Duration::from_millis(DEBOUNCE_MS));
    // One event for the whole burst: nothing follows it once the folder is
    // quiet for several debounce periods.
    assert_eq!(
        e.events
            .recv_timeout(Duration::from_millis(DEBOUNCE_MS * 4))
            .err(),
        Some(RecvTimeoutError::Timeout)
    );

    e.kk.unwatch_dir("tab-1").unwrap();
    fs::write(dir.join("after"), "x").unwrap();
    assert_eq!(
        e.events
            .recv_timeout(Duration::from_millis(DEBOUNCE_MS * 4))
            .err(),
        Some(RecvTimeoutError::Timeout)
    );
    assert_eq!(e.kk.unwatch_dir("tab-1").unwrap_err().kind, "UnknownWatch");
}

#[test]
fn watch_dir_gone_on_rmdir() {
    let e = env();
    let dir = e.dir("watched");
    e.kk.watch_dir("tab-1".into(), w(&dir), None).unwrap();
    fs::remove_dir(&dir).unwrap();

    let event = loop {
        let ev = e.next_event();
        if ev.name == EVENT_DIR_GONE {
            break ev;
        }
    };
    assert_eq!(event.payload, serde_json::json!({"key": "tab-1"}));
    // The watch stays registered until the UI drops it.
    e.kk.unwatch_dir("tab-1").unwrap();
}

#[test]
fn watch_dir_gone_on_move_away() {
    let e = env();
    let dir = e.dir("watched");
    e.kk.watch_dir("k".into(), w(&dir), None).unwrap();
    fs::rename(&dir, e.base.join("elsewhere")).unwrap();
    let event = loop {
        let ev = e.next_event();
        if ev.name == EVENT_DIR_GONE {
            break ev;
        }
    };
    assert_eq!(event.payload["key"], "k");
}

#[test]
fn watch_dir_refuses_a_file_and_a_missing_path() {
    let e = env();
    let file = e.base.join("file");
    fs::write(&file, "").unwrap();
    assert_eq!(
        e.kk.watch_dir("k".into(), w(&file), None).unwrap_err().kind,
        "Watch"
    );
    assert_eq!(
        e.kk.watch_dir("k".into(), w(&e.base.join("missing")), None)
            .unwrap_err()
            .kind,
        "NotFound"
    );
}

#[test]
fn rewatching_a_key_replaces_the_watch() {
    let e = env();
    let first = e.dir("first");
    let second = e.dir("second");
    e.kk.watch_dir("k".into(), w(&first), None).unwrap();
    e.kk.watch_dir("k".into(), w(&second), None).unwrap();
    fs::write(first.join("ignored"), "").unwrap();
    fs::write(second.join("seen"), "").unwrap();
    let event = e.next_event();
    assert_eq!(event.name, EVENT_DIR_CHANGED);
    assert_eq!(
        e.events
            .recv_timeout(Duration::from_millis(DEBOUNCE_MS * 4))
            .err(),
        Some(RecvTimeoutError::Timeout)
    );
}

#[test]
fn renaming_a_parent_sends_no_event() {
    // The watch follows the folder itself; a rename above it changes only
    // the path, which the view finds out from its next listing.
    let e = env();
    let child = e.dir("parent/child");
    e.kk.watch_dir("k".into(), w(&child), None).unwrap();
    fs::rename(e.base.join("parent"), e.base.join("renamed")).unwrap();
    assert_eq!(
        e.events
            .recv_timeout(Duration::from_millis(DEBOUNCE_MS * 4))
            .err(),
        Some(RecvTimeoutError::Timeout)
    );
    assert_eq!(
        e.kk.list_dir(w(&child), false).unwrap_err().kind,
        "NotFound"
    );
}
