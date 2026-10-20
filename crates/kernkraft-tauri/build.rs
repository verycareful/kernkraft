// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

include!("src/command_list.rs");

fn main() {
    tauri_plugin::Builder::new(COMMANDS).build();
}
