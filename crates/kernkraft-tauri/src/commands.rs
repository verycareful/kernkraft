// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The Tauri commands. Each is `async` and runs the [`Kernkraft`] method of
//! the same name on Tauri's blocking pool, so no file system call blocks
//! the IPC thread. Commands that start work in the background or watch a
//! folder pass the calling window's label, so closing the window cancels
//! that work.
//!
//! JavaScript passes arguments in camelCase (`newName`, `applyToAll`), as
//! Tauri maps them to these snake_case parameters.
//!
//! Every failure a command itself reports is a [`WireError`]. Some calls
//! are rejected by Tauri before any plugin code runs, and those reject
//! with a plain string instead: a missing argument key, an argument of the
//! wrong type other than a path (a path of any shape is accepted and, if
//! malformed, reported as a `BadPath` WireError), a call the ACL denies,
//! and an unknown command. Kern Desktop's `invoke` wrapper turns those
//! strings into `{ kind: "Ipc", message }`.

use kernkraft_theme::{Follow, UserChoice};
use kernkraft_volumes::{Device, DiskId};
use tauri::{AppHandle, Runtime, Window};

use crate::apps_cmds::{WireAppDefault, WireAppList, WireMime};
use crate::error::WireError;
use crate::fs_cmds::{WireEntry, WireUndoOutcome, WireUndoState, WireUserDirs};
use crate::ops::{OpId, WireReport};
use crate::state::{blocking, Kernkraft, KernkraftExt};
use crate::text_cmds::{WireStamp, WireText};
use crate::theme_cmds::WireTheme;
use crate::trash_cmds::{WireTrashListing, WireTrashedItem};
use crate::volumes_cmds::WireVolumes;
use crate::wire::WirePath;

fn origin<R: Runtime>(window: &Window<R>) -> Option<String> {
    Some(window.label().to_owned())
}

#[tauri::command]
pub(crate) async fn list_dir<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
    hidden: bool,
) -> Result<Vec<WireEntry>, WireError> {
    let kk: Kernkraft<R> = app.kernkraft();
    blocking(move || kk.list_dir(path, hidden)).await
}

#[tauri::command]
pub(crate) async fn create_dir<R: Runtime>(
    app: AppHandle<R>,
    parent: WirePath,
    name: String,
) -> Result<WirePath, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.create_dir(parent, name)).await
}

#[tauri::command]
pub(crate) async fn create_file<R: Runtime>(
    app: AppHandle<R>,
    parent: WirePath,
    name: String,
) -> Result<WirePath, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.create_file(parent, name)).await
}

#[tauri::command]
pub(crate) async fn rename<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
    new_name: String,
) -> Result<WirePath, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.rename(path, new_name)).await
}

#[tauri::command]
pub(crate) async fn start_op<R: Runtime>(
    window: Window<R>,
    kind: String,
    sources: Vec<WirePath>,
    dest: Option<WirePath>,
) -> Result<OpId, WireError> {
    let kk = window.kernkraft();
    let origin = origin(&window);
    blocking(move || kk.start_op(&kind, sources, dest, origin)).await
}

#[tauri::command]
pub(crate) async fn answer_conflict<R: Runtime>(
    app: AppHandle<R>,
    op: OpId,
    conflict: u64,
    choice: String,
    apply_to_all: bool,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.answer_conflict(op, conflict, &choice, apply_to_all)).await
}

#[tauri::command]
pub(crate) async fn cancel_op<R: Runtime>(app: AppHandle<R>, op: OpId) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.cancel_op(op)).await
}

#[tauri::command]
pub(crate) async fn trash_list<R: Runtime>(
    app: AppHandle<R>,
) -> Result<WireTrashListing, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.trash_list()).await
}

#[tauri::command]
pub(crate) async fn trash_restore<R: Runtime>(
    app: AppHandle<R>,
    items: Vec<WireTrashedItem>,
) -> Result<WireReport, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.trash_restore(items)).await
}

#[tauri::command]
pub(crate) async fn trash_purge<R: Runtime>(
    app: AppHandle<R>,
    items: Vec<WireTrashedItem>,
) -> Result<WireReport, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.trash_purge(items)).await
}

#[tauri::command]
pub(crate) async fn undo<R: Runtime>(app: AppHandle<R>) -> Result<WireUndoOutcome, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.undo()).await
}

#[tauri::command]
pub(crate) async fn redo<R: Runtime>(app: AppHandle<R>) -> Result<WireUndoOutcome, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.redo()).await
}

#[tauri::command]
pub(crate) async fn undo_state<R: Runtime>(app: AppHandle<R>) -> Result<WireUndoState, WireError> {
    let kk = app.kernkraft();
    blocking(move || Ok(kk.undo_state())).await
}

#[tauri::command]
pub(crate) async fn watch_dir<R: Runtime>(
    window: Window<R>,
    key: String,
    path: WirePath,
) -> Result<(), WireError> {
    let kk = window.kernkraft();
    let origin = origin(&window);
    blocking(move || kk.watch_dir(key, path, origin)).await
}

#[tauri::command]
pub(crate) async fn unwatch_dir<R: Runtime>(
    app: AppHandle<R>,
    key: String,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.unwatch_dir(&key)).await
}

#[tauri::command]
pub(crate) async fn properties<R: Runtime>(
    window: Window<R>,
    paths: Vec<WirePath>,
) -> Result<OpId, WireError> {
    let kk = window.kernkraft();
    let origin = origin(&window);
    blocking(move || kk.properties(paths, origin)).await
}

#[tauri::command]
pub(crate) async fn user_dirs<R: Runtime>(app: AppHandle<R>) -> Result<WireUserDirs, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.user_dirs()).await
}

#[tauri::command]
pub(crate) async fn text_open<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
) -> Result<WireText, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.text_open(path)).await
}

#[tauri::command]
pub(crate) async fn text_save<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
    text: String,
    ending: String,
    bom: bool,
    stamp: Option<WireStamp>,
) -> Result<WireStamp, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.text_save(path, text, ending, bom, stamp)).await
}

#[tauri::command]
pub(crate) async fn text_check<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
    stamp: WireStamp,
) -> Result<String, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.text_check(path, stamp)).await
}

#[tauri::command]
pub(crate) async fn thumb_request<R: Runtime>(
    window: Window<R>,
    key: String,
    paths: Vec<WirePath>,
    size: String,
) -> Result<(), WireError> {
    let kk = window.kernkraft();
    let origin = origin(&window);
    blocking(move || kk.thumb_request(key, paths, size, origin)).await
}

#[tauri::command]
pub(crate) async fn thumb_cancel<R: Runtime>(
    app: AppHandle<R>,
    key: String,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.thumb_cancel(key)).await
}

#[tauri::command]
pub(crate) async fn theme_get<R: Runtime>(
    app: AppHandle<R>,
    follow: Follow,
    user: UserChoice,
) -> Result<WireTheme, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.theme_get(follow, user)).await
}

#[tauri::command]
pub(crate) async fn theme_set_prefs<R: Runtime>(
    app: AppHandle<R>,
    follow: Follow,
    user: UserChoice,
) -> Result<WireTheme, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.theme_set_prefs(follow, user)).await
}

#[tauri::command]
pub(crate) async fn apps_for<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
) -> Result<WireAppList, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.apps_for(path)).await
}

#[tauri::command]
pub(crate) async fn app_default<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
) -> Result<WireAppDefault, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.app_default(path)).await
}

#[tauri::command]
pub(crate) async fn all_apps<R: Runtime>(app: AppHandle<R>) -> Result<WireAppList, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.all_apps()).await
}

#[tauri::command]
pub(crate) async fn launch_app<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    paths: Vec<WirePath>,
    terminal: Option<String>,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.launch_app(id, paths, terminal)).await
}

#[tauri::command]
pub(crate) async fn open_default<R: Runtime>(
    app: AppHandle<R>,
    paths: Vec<WirePath>,
    terminal: Option<String>,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.open_default(paths, terminal)).await
}

#[tauri::command]
pub(crate) async fn set_default_app<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    mime: String,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.set_default_app(id, mime)).await
}

#[tauri::command]
pub(crate) async fn open_terminal<R: Runtime>(
    app: AppHandle<R>,
    dir: WirePath,
    setting: Option<String>,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.open_terminal(dir, setting)).await
}

#[tauri::command]
pub(crate) async fn mime_of<R: Runtime>(
    app: AppHandle<R>,
    path: WirePath,
) -> Result<WireMime, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.mime_of(path)).await
}

#[tauri::command]
pub(crate) async fn volumes_list<R: Runtime>(app: AppHandle<R>) -> Result<WireVolumes, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.volumes_list()).await
}

#[tauri::command]
pub(crate) async fn volume_mount<R: Runtime>(
    app: AppHandle<R>,
    device: Device,
) -> Result<WirePath, WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.volume_mount(device)).await
}

#[tauri::command]
pub(crate) async fn volume_unmount<R: Runtime>(
    app: AppHandle<R>,
    mount_point: WirePath,
    disk: DiskId,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.volume_unmount(mount_point, disk)).await
}

#[tauri::command]
pub(crate) async fn volume_eject<R: Runtime>(
    app: AppHandle<R>,
    mount_point: WirePath,
    disk: DiskId,
) -> Result<(), WireError> {
    let kk = app.kernkraft();
    blocking(move || kk.volume_eject(mount_point, disk)).await
}

#[tauri::command]
pub(crate) async fn search<R: Runtime>(
    window: Window<R>,
    key: String,
    roots: Vec<WirePath>,
    query: String,
    hidden: bool,
) -> Result<OpId, WireError> {
    let kk = window.kernkraft();
    let origin = origin(&window);
    blocking(move || kk.search(key, roots, query, hidden, origin)).await
}

#[tauri::command]
pub(crate) async fn documents<R: Runtime>(
    window: Window<R>,
    key: String,
    roots: Vec<WirePath>,
) -> Result<OpId, WireError> {
    let kk = window.kernkraft();
    let origin = origin(&window);
    blocking(move || kk.documents(key, roots, origin)).await
}
