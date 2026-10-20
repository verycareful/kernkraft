// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A mock Tauri app with the plugin, a trash and a MIME database under one
//! temp directory, and every plugin event captured in order.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use kernkraft_fs::Trash;
use kernkraft_tauri::{
    Config, Kernkraft, KernkraftExt, OpDoneEvent, OpId, WirePath, EVENT_DIR_CHANGED,
    EVENT_DIR_GONE, EVENT_OP_CONFLICT, EVENT_OP_DONE, EVENT_OP_PROGRESS,
};
use serde_json::Value;
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime};
use tauri::{App, Listener};

/// Longest a test waits for an event before failing. Only a guard against
/// a hang: every wait ends on the event itself.
pub const EVENT_WAIT: Duration = Duration::from_secs(60);

pub struct Event {
    pub name: &'static str,
    pub payload: Value,
    pub at: Instant,
}

pub struct Env {
    pub base: PathBuf,
    pub trash_dir: PathBuf,
    pub app: App<MockRuntime>,
    pub kk: Kernkraft<MockRuntime>,
    pub events: Receiver<Event>,
    _root: tempfile::TempDir,
}

pub fn env() -> Env {
    let root = tempfile::tempdir().unwrap();
    let base = root.path().canonicalize().unwrap();
    let data = base.join("data");
    fs::create_dir_all(data.join("mime")).unwrap();
    fs::write(data.join("mime/globs2"), "50:text/plain:*.txt\n").unwrap();
    let trash_dir = base.join("trash");
    let config = Config {
        trash: Some(Trash::with_home_trash(trash_dir.clone())),
        mime_data_dirs: Some(vec![data]),
    };
    let app = mock_builder()
        .plugin(kernkraft_tauri::init_with(config))
        .build(mock_context(noop_assets()))
        .unwrap();
    let (tx, events) = mpsc::channel();
    for name in [
        EVENT_OP_PROGRESS,
        EVENT_OP_CONFLICT,
        EVENT_OP_DONE,
        EVENT_DIR_CHANGED,
        EVENT_DIR_GONE,
    ] {
        let tx = tx.clone();
        app.listen_any(name, move |e| {
            let payload = serde_json::from_str(e.payload()).unwrap();
            let _ = tx.send(Event {
                name,
                payload,
                at: Instant::now(),
            });
        });
    }
    let kk = app.handle().kernkraft();
    Env {
        base,
        trash_dir,
        app,
        kk,
        events,
        _root: root,
    }
}

impl Env {
    pub fn dir(&self, name: &str) -> PathBuf {
        let p = self.base.join(name);
        fs::create_dir_all(&p).unwrap();
        p
    }

    pub fn next_event(&self) -> Event {
        self.events
            .recv_timeout(EVENT_WAIT)
            .expect("no event arrived")
    }

    /// Events up to and including the `op-done` of `op`, and that done
    /// event's payload.
    pub fn wait_done(&self, op: OpId) -> (Vec<Event>, OpDoneEvent) {
        let mut seen = Vec::new();
        loop {
            let e = self.next_event();
            if e.name == EVENT_OP_DONE && e.payload["op"] == op {
                let done = serde_json::from_value(e.payload.clone()).unwrap();
                seen.push(e);
                return (seen, done);
            }
            seen.push(e);
        }
    }

    /// The next `op-conflict` event of `op`; other events are dropped.
    pub fn wait_conflict(&self, op: OpId) -> Value {
        loop {
            let e = self.next_event();
            if e.name == EVENT_OP_CONFLICT && e.payload["op"] == op {
                return e.payload;
            }
        }
    }

    pub fn trashed(&self) -> Vec<PathBuf> {
        let listing = Trash::with_home_trash(self.trash_dir.clone())
            .list()
            .unwrap();
        let mut v: Vec<PathBuf> = listing.items.into_iter().map(|i| i.original).collect();
        v.sort();
        v
    }
}

pub fn w(p: &Path) -> WirePath {
    WirePath::from(p)
}

pub fn path(w: &WirePath) -> PathBuf {
    PathBuf::try_from(w.clone()).unwrap()
}

pub fn names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}
