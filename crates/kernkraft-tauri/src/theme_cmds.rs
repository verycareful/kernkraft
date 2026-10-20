// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The system theme (parent spec section 6.5, through `kernkraft-theme`):
//! the resolved tokens as CSS custom properties, and live updates.
//!
//! The sources are gathered and the watcher started on the first
//! [`Kernkraft::theme_get`] or [`Kernkraft::theme_set_prefs`], not at plugin
//! setup: until the app asks for its theme nobody listens for changes, and
//! gathering runs `fc-match` and opens a session bus connection, which an
//! app that never themes (or a test of file operations) should not pay for.
//! After that, every change the watcher sees (Hyprland's `hyprtoolkit.conf`,
//! GTK's `settings.ini` and `gtk.css`, the portal's appearance settings) is
//! resolved with the preferences last given to `theme_set_prefs` and sent as
//! [`EVENT_THEME_CHANGED`].
//!
//! The portal is reached over the session bus's Unix socket only. A bus
//! that is absent leaves the portal out without a word. A bus the
//! environment names but that cannot be reached also leaves it out (the
//! theme follows the files alone), and that error is reported once, in the
//! `problems` of the next payload. Sources that exist but cannot be read are
//! in `problems` of every payload resolved from them.

use std::sync::Arc;

use kernkraft_theme::{
    fc_resolves, gather, resolve, to_css, watch_with, Follow, Gathered, Sources, ThemeError,
    Tokens, UserChoice, Watcher,
};
use serde::{Deserialize, Serialize};
use tauri::{Emitter, Runtime};

use crate::error::WireError;
use crate::state::{lock, Inner, Kernkraft};

pub const EVENT_THEME_CHANGED: &str = "kernkraft://theme-changed";

/// The resolved theme. `css` is the token set as CSS custom properties for
/// `:root`; `fallbacks` names the tokens that fell back to Kern's neutrals
/// because the system palette failed WCAG 2.1 AA (`Tokens::wcag_fallbacks`);
/// `problems` lists the theme sources that could not be read, and, once,
/// a session bus or watcher failure.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WireTheme {
    pub css: String,
    pub tokens: Tokens,
    pub fallbacks: Vec<String>,
    pub problems: Vec<WireError>,
}

#[derive(Default)]
pub(crate) struct ThemeState {
    started: bool,
    follow: Follow,
    user: UserChoice,
    sources: Sources,
    /// What the last gather could not read.
    errors: Vec<WireError>,
    /// Bus and watcher failures, each reported in one payload only.
    once: Vec<WireError>,
    watcher: Option<Watcher>,
}

impl ThemeState {
    fn payload(&mut self, follow: &Follow, user: &UserChoice) -> WireTheme {
        let tokens = resolve(&self.sources, follow, user, &fc_resolves);
        let mut problems = std::mem::take(&mut self.once);
        problems.extend(self.errors.iter().cloned());
        WireTheme {
            css: to_css(&tokens),
            fallbacks: tokens.wcag_fallbacks.clone(),
            tokens,
            problems,
        }
    }

    fn take(&mut self, g: Gathered) {
        self.sources = g.sources;
        self.errors = g.errors.into_iter().map(WireError::from).collect();
    }
}

impl<R: Runtime> Kernkraft<R> {
    /// The theme resolved with `follow` and `user`, from the sources as
    /// last read. It does not change the preferences live updates use; see
    /// [`Kernkraft::theme_set_prefs`].
    pub fn theme_get(&self, follow: Follow, user: UserChoice) -> Result<WireTheme, WireError> {
        let mut theme = lock(&self.0.theme);
        self.theme_start(&mut theme)?;
        Ok(theme.payload(&follow, &user))
    }

    /// Makes `follow` and `user` the preferences every later
    /// [`EVENT_THEME_CHANGED`] is resolved with, and sends one at once with
    /// them. Returns the same payload.
    pub fn theme_set_prefs(
        &self,
        follow: Follow,
        user: UserChoice,
    ) -> Result<WireTheme, WireError> {
        let mut theme = lock(&self.0.theme);
        self.theme_start(&mut theme)?;
        theme.follow = follow;
        theme.user = user;
        let payload = theme.payload(&follow, &user);
        // Sent under the lock, so a change the watcher reports meanwhile
        // goes out after this one, with the new preferences.
        let _ = self.0.app.emit(EVENT_THEME_CHANGED, payload.clone());
        Ok(payload)
    }

    /// Gathers the sources and starts the watcher, once.
    fn theme_start(&self, theme: &mut ThemeState) -> Result<(), WireError> {
        if theme.started {
            return Ok(());
        }
        let base = self.base()?.clone();
        theme.started = true;
        let conn = match kernkraft_bus::session() {
            Ok(conn) => Some(conn),
            Err(e) if e.is_absent() => None,
            Err(e) => {
                theme.once.push(WireError::from(ThemeError::Portal(format!(
                    "session bus, so the portal's appearance settings are not followed: {e}"
                ))));
                None
            }
        };
        theme.take(gather(&base, conn.as_ref()));
        let weak = Arc::downgrade(&self.0);
        let on_change = Box::new(move |g: Gathered| {
            if let Some(inner) = weak.upgrade() {
                theme_changed(&inner, g);
            }
        });
        match watch_with(&base, conn, on_change) {
            Ok(w) => theme.watcher = Some(w),
            Err(e) => theme.once.push(WireError::from(e)),
        }
        Ok(())
    }

    /// Stops the watcher. Dropped outside the lock: the drop waits for the
    /// watcher's worker, which may be waiting on the lock to report.
    pub(crate) fn theme_shutdown(&self) {
        let watcher = lock(&self.0.theme).watcher.take();
        drop(watcher);
    }
}

fn theme_changed<R: Runtime>(inner: &Arc<Inner<R>>, g: Gathered) {
    let mut theme = lock(&inner.theme);
    theme.take(g);
    let (follow, user) = (theme.follow, theme.user);
    let payload = theme.payload(&follow, &user);
    let _ = inner.app.emit(EVENT_THEME_CHANGED, payload);
}
