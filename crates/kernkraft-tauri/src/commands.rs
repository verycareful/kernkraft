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

use tauri::{AppHandle, Runtime, Window};

use crate::error::WireError;
use crate::fs_cmds::{WireEntry, WireUndoOutcome, WireUndoState};
use crate::ops::{OpId, WireReport};
use crate::state::{blocking, Kernkraft, KernkraftExt};
use crate::trash_cmds::{WireTrashListing, WireTrashedItem};
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
