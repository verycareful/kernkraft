// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A mock Tauri app with the plugin, a trash, a MIME database and XDG base
//! directories under one temp directory, and every plugin event captured in
//! order.

#![allow(clippy::unwrap_used, clippy::expect_used, dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use kernkraft_fs::{BaseDirs, Trash};
use kernkraft_tauri::{
    Config, Kernkraft, KernkraftExt, OpDoneEvent, OpId, WirePath, EVENT_DIR_CHANGED,
    EVENT_DIR_GONE, EVENT_OP_CONFLICT, EVENT_OP_DONE, EVENT_OP_PROGRESS, EVENT_SEARCH_HIT,
    EVENT_THEME_CHANGED, EVENT_THUMB,
};
use serde_json::Value;
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{
    get_ipc_response, mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY,
};
use tauri::utils::acl::ExecutionContext;
use tauri::webview::InvokeRequest;
use tauri::{App, Context, Listener, WebviewWindow, WebviewWindowBuilder};

/// Longest a test waits for an event before failing. Only a guard against
/// a hang: every wait ends on the event itself.
pub const EVENT_WAIT: Duration = Duration::from_secs(60);

pub struct Event {
    pub name: &'static str,
    pub payload: Value,
    pub at: Instant,
}

/// The MIME database every test app loads (Shared MIME-info Database 0.21,
/// `globs2` is `weight:type:glob`, `subclasses` is `type parent`).
pub const GLOBS2: &str = "50:text/plain:*.txt\n\
    50:text/markdown:*.md\n\
    50:application/x-shellscript:*.sh\n\
    50:image/svg+xml:*.svg\n\
    50:image/png:*.png\n\
    50:application/pdf:*.pdf\n";
pub const SUBCLASSES: &str = "application/x-shellscript text/plain\n";

pub struct Env {
    pub base: PathBuf,
    pub trash_dir: PathBuf,
    /// The XDG base directories the plugin was given; `home` is
    /// `base/home`, and every other directory is under it.
    pub xdg: BaseDirs,
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
    fs::write(data.join("mime/globs2"), GLOBS2).unwrap();
    fs::write(data.join("mime/subclasses"), SUBCLASSES).unwrap();
    let trash_dir = base.join("trash");
    let home = base.join("home");
    fs::create_dir_all(&home).unwrap();
    let xdg = BaseDirs {
        config: home.join(".config"),
        cache: home.join(".cache"),
        state: home.join(".local/state"),
        data: data.clone(),
        config_dirs: Vec::new(),
        data_dirs: Vec::new(),
        current_desktop: Vec::new(),
        home,
    };
    let config = Config {
        trash: Some(Trash::with_home_trash(trash_dir.clone())),
        mime_data_dirs: Some(vec![data]),
        base_dirs: Some(xdg.clone()),
    };
    let app = mock_builder()
        .plugin(kernkraft_tauri::init_with(config))
        .build(ipc_context())
        .unwrap();
    let (tx, events) = mpsc::channel();
    for name in [
        EVENT_OP_PROGRESS,
        EVENT_OP_CONFLICT,
        EVENT_OP_DONE,
        EVENT_DIR_CHANGED,
        EVENT_DIR_GONE,
        EVENT_THUMB,
        EVENT_THEME_CHANGED,
        EVENT_SEARCH_HIT,
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
        xdg,
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

    /// The next event named `name`; other events are dropped.
    pub fn wait_event(&self, name: &str) -> Value {
        loop {
            let e = self.next_event();
            if e.name == name {
                return e.payload;
            }
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

/// A mock context whose ACL lets the local origin call every kernkraft
/// command, so IPC requests reach the handlers.
///
/// `RuntimeAuthority::__allow_command` is public but doc-hidden Tauri API
/// with no stability promise; it is used here and nowhere else, so a Tauri
/// update that changes it breaks this one helper.
fn ipc_context() -> Context<MockRuntime> {
    let mut ctx = mock_context(noop_assets());
    for cmd in kernkraft_tauri::COMMANDS {
        ctx.runtime_authority_mut()
            .__allow_command(format!("plugin:kernkraft|{cmd}"), ExecutionContext::Local);
    }
    ctx
}

impl Env {
    /// A webview window to send IPC requests from.
    pub fn webview(&self, label: &str) -> WebviewWindow<MockRuntime> {
        WebviewWindowBuilder::new(&self.app, label, Default::default())
            .build()
            .unwrap()
    }
}

/// Invokes `plugin:kernkraft|<cmd>` with JSON `args` from `webview`, as the
/// frontend's `invoke` does, and returns the JSON reply or error.
pub fn invoke(
    webview: &WebviewWindow<MockRuntime>,
    cmd: &str,
    args: Value,
) -> Result<Value, Value> {
    let request = InvokeRequest {
        cmd: format!("plugin:kernkraft|{cmd}"),
        callback: CallbackFn(0),
        error: CallbackFn(1),
        url: "tauri://localhost".parse().unwrap(),
        body: InvokeBody::Json(args),
        headers: Default::default(),
        invoke_key: INVOKE_KEY.to_string(),
    };
    get_ipc_response(webview, request).map(|body| body.deserialize::<Value>().unwrap())
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
