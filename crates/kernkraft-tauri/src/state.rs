// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The plugin's state: the XDG base directories, the MIME database, the
//! trash, the undo journal, the running operations, the folder watches, the
//! thumbnail queue, the theme and the volume watcher, shared by every command.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use kernkraft_fs::{base_dirs, BaseDirs, Journal, Trash};
use kernkraft_xdg_apps::MimeDb;
use tauri::{AppHandle, Manager, Runtime};

use crate::error::{PluginError, WireError};
use crate::ops::Registry;
use crate::theme_cmds::ThemeState;
use crate::thumb_cmds::Thumbs;
use crate::volumes_cmds::VolumesState;
use crate::watch::DirWatch;

/// Where the plugin finds the trash, the MIME database and the XDG
/// directories. `None` takes the user's own: the base directories from the
/// environment (XDG Base Directory Specification 0.8), the home trash at
/// `$XDG_DATA_HOME/Trash` with the per-mount trashes, and the MIME database
/// of `$XDG_DATA_HOME` then each of `$XDG_DATA_DIRS`.
#[derive(Clone, Debug, Default)]
pub struct Config {
    pub trash: Option<Trash>,
    /// Data directories whose `mime/` subdirectory is read, most important
    /// first.
    pub mime_data_dirs: Option<Vec<PathBuf>>,
    /// The base directories for everything else: applications and
    /// `mimeapps.list` (data and config), the thumbnail cache, the user
    /// directories, and the theme sources. Also the default for `trash` and
    /// `mime_data_dirs` when those are `None`.
    pub base_dirs: Option<BaseDirs>,
}

/// The plugin's state, shared by every command. Cloning it is cheap; every
/// clone is the same state.
///
/// Each method is the command of the same name, run synchronously on the
/// calling thread. The commands run them on Tauri's blocking pool; a Rust
/// caller (or a test) may call them directly.
pub struct Kernkraft<R: Runtime>(pub(crate) Arc<Inner<R>>);

impl<R: Runtime> Clone for Kernkraft<R> {
    fn clone(&self) -> Self {
        Kernkraft(self.0.clone())
    }
}

pub(crate) struct Inner<R: Runtime> {
    pub app: AppHandle<R>,
    /// `None` when `HOME` is unset, empty or relative; every command that
    /// needs a base directory then fails with `NoHome`.
    pub base: Option<BaseDirs>,
    /// Loaded once at start. A database that fails to load is reported by
    /// every listing, never replaced by a guess.
    pub mime: Result<MimeDb, WireError>,
    pub trash: Result<Trash, WireError>,
    pub journal: Mutex<Journal>,
    pub registry: Mutex<Registry>,
    /// Notified each time a background worker finishes.
    pub idle: Condvar,
    pub watches: Mutex<HashMap<String, DirWatch>>,
    pub thumbs: Thumbs,
    pub theme: Mutex<ThemeState>,
    pub volumes: Mutex<VolumesState>,
}

impl<R: Runtime> Kernkraft<R> {
    pub(crate) fn new(app: AppHandle<R>, config: Config) -> Kernkraft<R> {
        let base = config.base_dirs.or_else(base_dirs);
        let trash = match (config.trash, &base) {
            (Some(trash), _) => Ok(trash),
            (None, Some(base)) => Ok(Trash::new(base)),
            (None, None) => Err(WireError::from(PluginError::NoHome)),
        };
        let mime_dirs = match (config.mime_data_dirs, &base) {
            (Some(dirs), _) => Ok(dirs),
            (None, Some(base)) => Ok(std::iter::once(base.data.clone())
                .chain(base.data_dirs.iter().cloned())
                .collect::<Vec<_>>()),
            (None, None) => Err(WireError::from(PluginError::NoHome)),
        };
        let mime = mime_dirs.and_then(|dirs| MimeDb::load(&dirs).map_err(WireError::from));
        Kernkraft(Arc::new(Inner {
            app,
            base,
            mime,
            trash,
            journal: Mutex::new(Journal::new()),
            registry: Mutex::new(Registry::default()),
            idle: Condvar::new(),
            watches: Mutex::new(HashMap::new()),
            thumbs: Thumbs::default(),
            theme: Mutex::new(ThemeState::default()),
            volumes: Mutex::new(VolumesState::default()),
        }))
    }

    pub(crate) fn trash(&self) -> Result<&Trash, WireError> {
        self.0.trash.as_ref().map_err(Clone::clone)
    }

    pub(crate) fn mime(&self) -> Result<&MimeDb, WireError> {
        self.0.mime.as_ref().map_err(Clone::clone)
    }

    pub(crate) fn base(&self) -> Result<&BaseDirs, WireError> {
        self.0
            .base
            .as_ref()
            .ok_or_else(|| WireError::from(PluginError::NoHome))
    }

    /// Cancels the operations, drops the folder watches and the queued
    /// thumbnails started from window `label`, or all of them for `None`.
    /// A worker blocked on a conflict prompt is released. For `None` (the
    /// app exiting) the thumbnail workers, the theme watcher and the volume watcher stop too.
    pub(crate) fn shutdown(&self, label: Option<&str>) {
        self.cancel_from(label);
        self.thumbs_shutdown(label);
        if label.is_none() {
            self.theme_shutdown();
            self.volumes_shutdown();
        }
        let dropped: Vec<DirWatch> = {
            let mut watches = lock(&self.0.watches);
            let keys: Vec<String> = watches
                .iter()
                .filter(|(_, w)| label.is_none() || w.origin.as_deref() == label)
                .map(|(k, _)| k.clone())
                .collect();
            keys.iter().filter_map(|k| watches.remove(k)).collect()
        };
        // Dropped outside the lock: each drop waits for its thread.
        drop(dropped);
    }
}

/// Access to the plugin's state from an app, app handle, window or webview.
pub trait KernkraftExt<R: Runtime> {
    /// The plugin's state. Panics if the plugin is not registered on the
    /// app, as Tauri's `state()` does for any unmanaged type.
    fn kernkraft(&self) -> Kernkraft<R>;
}

impl<R: Runtime, M: Manager<R>> KernkraftExt<R> for M {
    fn kernkraft(&self) -> Kernkraft<R> {
        self.state::<Kernkraft<R>>().inner().clone()
    }
}

/// Locks `m`. A poisoned lock (a thread panicked while holding it) is taken
/// over as it is: every guarded structure here is checked against the disk
/// before it is acted on, so a half-updated one cannot cause a wrong action.
pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `f` on Tauri's blocking pool, for an `async` command.
pub(crate) async fn blocking<T, F>(f: F) -> Result<T, WireError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, WireError> + Send + 'static,
{
    tauri::async_runtime::spawn_blocking(f)
        .await
        .map_err(|e| WireError::from(PluginError::Internal(e.to_string())))?
}
