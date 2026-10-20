// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

// Included by build.rs, which generates one `allow-<name>` and one
// `deny-<name>` permission per entry, and by lib.rs, which registers the
// handlers. `permissions/default.toml` grants every one as
// `kernkraft:default`.

/// Every command the plugin exposes, as invoked from JavaScript
/// (`plugin:kernkraft|<name>`).
pub const COMMANDS: &[&str] = &[
    "list_dir",
    "create_dir",
    "create_file",
    "rename",
    "start_op",
    "answer_conflict",
    "cancel_op",
    "trash_list",
    "trash_restore",
    "trash_purge",
    "undo",
    "redo",
    "undo_state",
    "watch_dir",
    "unwatch_dir",
    "properties",
];
