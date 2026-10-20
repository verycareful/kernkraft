// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tauri 2 plugin that exposes kernkraft to a webview.
//!
//! The plugin is named `kernkraft`; its commands are invoked from JavaScript
//! as `plugin:kernkraft|<command>` and are granted by the permission set
//! `kernkraft:default`. Paths cross the IPC boundary as [`WirePath`], which
//! is lossless for names that are not UTF-8, and every failure as
//! [`WireError`], whose `kind` is the name of the error variant.
//!
//! This is an adapter crate: it is the only kernkraft crate that depends on
//! `tauri`. The core crates never do.

mod error;
mod wire;

pub use error::WireError;
pub use wire::{WirePath, WirePathError};

use tauri::plugin::TauriPlugin;
use tauri::Runtime;

/// Name the plugin registers under, and the prefix of its permissions.
pub const PLUGIN_NAME: &str = "kernkraft";

/// The `kernkraft` plugin, to be passed to `tauri::Builder::plugin`.
pub fn init<R: Runtime>() -> TauriPlugin<R> {
    tauri::plugin::Builder::new(PLUGIN_NAME).build()
}
