// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Commands through Tauri's access control, with the permission manifest the
//! plugin build files under the package name and a capability naming
//! `<package>:default`, as an application's does. A plugin whose runtime
//! name differs from its package name is refused here with "not allowed.
//! Plugin not found".

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use common::{env, env_with_permissions, invoke};
use serde_json::json;

#[test]
fn runtime_name_is_the_package_name() {
    assert_eq!(kernkraft_tauri::PLUGIN_NAME, env!("CARGO_PKG_NAME"));
}

#[test]
fn default_permission_allows_commands() {
    let e = env();
    let v = e.webview("main");
    invoke(&v, "user_dirs", json!({})).unwrap();
    let theme = json!({
        "follow": {"colours": true, "accent": true, "fonts": true, "radius": true},
        "user": {"mode": "dark", "accent": "teal", "density": "compact"}
    });
    invoke(&v, "theme_get", theme).unwrap();
    let dir = e.dir("d");
    let same = json!({"sources": [common::w(&dir)], "dest": common::w(&dir)});
    let answer = invoke(&v, "same_filesystem", same).unwrap();
    assert_eq!(answer, json!([{"state": "checked", "same": true}]));
}

#[test]
fn same_filesystem_is_granted_by_its_own_permission() {
    let e = env_with_permissions(&["kernkraft-tauri:allow-same-filesystem"]);
    let v = e.webview("main");
    let dir = e.dir("d");
    let same = json!({"sources": [common::w(&dir)], "dest": common::w(&dir)});
    invoke(&v, "same_filesystem", same).unwrap();
    let refused = invoke(&v, "user_dirs", json!({})).unwrap_err();
    assert!(
        refused.as_str().unwrap().contains("not allowed"),
        "{refused}"
    );
}

#[test]
fn a_command_outside_the_granted_permissions_is_refused() {
    let e = env_with_permissions(&["kernkraft-tauri:allow-user-dirs"]);
    let v = e.webview("main");
    invoke(&v, "user_dirs", json!({})).unwrap();
    let refused = invoke(&v, "theme_get", json!({})).unwrap_err();
    assert!(
        refused.as_str().unwrap().contains("not allowed"),
        "{refused}"
    );
}
