// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tauri 2 plugin that exposes kernkraft to a webview.
//!
//! The plugin is named `kernkraft`; its commands ([`COMMANDS`]) are invoked
//! from JavaScript as `plugin:kernkraft|<command>` and are granted by the
//! permission set `kernkraft:default`. Paths cross the IPC boundary as
//! [`WirePath`], which is lossless for names that are not UTF-8, and every
//! failure as [`WireError`], whose `kind` is the name of the error variant.
//!
//! File operations, searches and the Documents walk run in the background
//! and report through events (`kernkraft://op-progress`, `op-conflict`,
//! `op-done`, and `search-hit` for the walks; see the `ops` items); folder
//! watches report `kernkraft://dir-changed` and `dir-gone`, thumbnails
//! `kernkraft://thumb`, and theme changes `kernkraft://theme-changed`.
//! The same operations are methods of [`Kernkraft`], reached from Rust with
//! [`KernkraftExt::kernkraft`].
//!
//! This is an adapter crate: it is the only kernkraft crate that depends on
//! `tauri`. The core crates never do.

mod apps_cmds;
mod commands;
mod error;
mod fs_cmds;
mod ops;
mod search_cmds;
mod state;
mod text_cmds;
mod theme_cmds;
mod thumb_cmds;
mod trash_cmds;
mod volumes_cmds;
mod watch;
mod wire;

include!("command_list.rs");

pub use apps_cmds::{WireApp, WireAppDefault, WireAppList, WireMime};
pub use error::{PluginError, WireDetails, WireError, WireItemError};
pub use fs_cmds::{WireEntry, WireUndoOutcome, WireUndoState, WireUserDir, WireUserDirs};
pub use ops::{
    OpConflictEvent, OpDoneEvent, OpId, OpProgressEvent, WireDone, WireProperties, WireReport,
    EVENT_OP_CONFLICT, EVENT_OP_DONE, EVENT_OP_PROGRESS, PROGRESS_INTERVAL_MS,
};
pub use search_cmds::{SearchHitEvent, WireScanDone, EVENT_SEARCH_HIT, SEARCH_BATCH};
pub use state::{Config, Kernkraft, KernkraftExt};
pub use text_cmds::{WireReadOnly, WireStamp, WireText};
pub use theme_cmds::{WireTheme, EVENT_THEME_CHANGED};
pub use thumb_cmds::{ThumbEvent, EVENT_THUMB};
pub use trash_cmds::{WireTrashListing, WireTrashedItem};
pub use volumes_cmds::{WireMount, WireVolumes};
pub use watch::{DirEvent, DEBOUNCE_MS, EVENT_DIR_CHANGED, EVENT_DIR_GONE};
pub use wire::{WirePath, WirePathError};

use tauri::plugin::TauriPlugin;
use tauri::{Manager, RunEvent, Runtime, WindowEvent};

/// Name the plugin registers under, and the prefix of its permissions.
pub const PLUGIN_NAME: &str = "kernkraft";

/// The `kernkraft` plugin with the user's own trash and MIME database, to
/// be passed to `tauri::Builder::plugin`.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    init_with(Config::default())
}

/// The `kernkraft` plugin with the trash and MIME database `config` names.
pub fn init_with<R: Runtime>(config: Config) -> TauriPlugin<R> {
    tauri::plugin::Builder::new(PLUGIN_NAME)
        .invoke_handler(tauri::generate_handler![
            commands::list_dir,
            commands::create_dir,
            commands::create_file,
            commands::rename,
            commands::start_op,
            commands::answer_conflict,
            commands::cancel_op,
            commands::trash_list,
            commands::trash_restore,
            commands::trash_purge,
            commands::undo,
            commands::redo,
            commands::undo_state,
            commands::watch_dir,
            commands::unwatch_dir,
            commands::properties,
            commands::user_dirs,
            commands::text_open,
            commands::text_save,
            commands::text_check,
            commands::thumb_request,
            commands::thumb_cancel,
            commands::theme_get,
            commands::theme_set_prefs,
            commands::apps_for,
            commands::app_default,
            commands::all_apps,
            commands::launch_app,
            commands::open_default,
            commands::set_default_app,
            commands::open_terminal,
            commands::mime_of,
            commands::volumes_list,
            commands::volume_mount,
            commands::volume_unmount,
            commands::volume_eject,
            commands::search,
            commands::documents,
        ])
        .setup(move |app, _api| {
            app.manage(Kernkraft::new(app.clone(), config));
            Ok(())
        })
        .on_event(|app, event| {
            let Some(kk) = app.try_state::<Kernkraft<R>>() else {
                return;
            };
            match event {
                // A worker blocked on a conflict prompt would otherwise wait
                // for an answer no window can give. The exit then waits,
                // bounded, for the cancelled workers to remove their partial
                // output.
                RunEvent::Exit => {
                    kk.shutdown(None);
                    kk.wait_for_workers(ops::EXIT_WAIT);
                }
                RunEvent::WindowEvent {
                    label,
                    event: WindowEvent::Destroyed,
                    ..
                } => kk.shutdown(Some(label)),
                _ => {}
            }
        })
        .build()
}
