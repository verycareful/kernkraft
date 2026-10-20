// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Live theme updates use the preferences the app last gave.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::fs;

use common::env;
use kernkraft_tauri::EVENT_THEME_CHANGED;
use kernkraft_theme::{Follow, KernAccent, Mode, UserChoice};

#[test]
fn watcher_events_use_the_prefs_of_theme_get() {
    let e = env();
    let hypr = e.xdg.config.join("hypr");
    fs::create_dir_all(&hypr).unwrap();
    let dark = UserChoice {
        mode: Mode::Dark,
        accent: KernAccent::Teal,
        ..UserChoice::default()
    };
    let first = e.kk.theme_get(Follow::default(), dark).unwrap();
    assert_eq!(first.tokens.scheme, kernkraft_theme::Scheme::Dark);

    // A light Hyprland palette appears. Under the default prefs (system
    // mode) it would resolve light; the pinned dark mode given to
    // theme_get holds.
    fs::write(hypr.join("hyprtoolkit.conf"), "background = rgb(ffffff)\n").unwrap();
    let ev = e.wait_event(EVENT_THEME_CHANGED);
    assert_eq!(ev["tokens"]["scheme"], "dark");
}
