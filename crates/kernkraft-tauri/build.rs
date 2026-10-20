// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

/// Every command the plugin exposes. Tauri generates one `allow-<name>` and
/// one `deny-<name>` permission for each; `permissions/default.toml` grants
/// them all as `kernkraft:default`.
const COMMANDS: &[&str] = &[];

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
