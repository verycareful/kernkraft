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
//! resolved with the current preferences and sent as
//! [`EVENT_THEME_CHANGED`].
//!
//! The current preferences are the `follow` and `user` of the last
//! `theme_get` or `theme_set_prefs`, so what the app last asked for and
//! what the events resolve with never disagree. At startup the app calls
//! one of them with its saved settings; until then events resolve with the
//! defaults (follow everything, system mode).
//!
//! The portal is reached over the session bus's Unix socket only. A bus
//! that is absent leaves the portal out without a word. A bus the
//! environment names but that cannot be reached also leaves it out (the
//! theme follows the files alone), and so does a bus that is reached but
//! refuses the portal subscription (the match rule or `AddMatch`); either
//! error is reported once, in the `problems` of the next payload. Sources
//! that exist but cannot be read are in `problems` of every payload
//! resolved from them.

use std::sync::Arc;

use kernkraft_fs::BaseDirs;
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
    /// last read. `follow` and `user` become the current preferences, which
    /// later [`EVENT_THEME_CHANGED`] events are resolved with; no event is
    /// sent for this call.
    pub fn theme_get(&self, follow: Follow, user: UserChoice) -> Result<WireTheme, WireError> {
        let mut theme = lock(&self.0.theme);
        self.theme_start(&mut theme)?;
        theme.follow = follow;
        theme.user = user;
        Ok(theme.payload(&follow, &user))
    }

    /// Makes `follow` and `user` the current preferences, as
    /// [`Kernkraft::theme_get`] does, and sends an [`EVENT_THEME_CHANGED`]
    /// at once with them, so every window restyles. Returns the same
    /// payload.
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
        let on_change = || -> OnChange {
            let weak = weak.clone();
            Box::new(move |g: Gathered| {
                if let Some(inner) = weak.upgrade() {
                    theme_changed(&inner, g);
                }
            })
        };
        let (watcher, problems) = start_watcher(&base, conn, &on_change, &watch_with);
        theme.watcher = watcher;
        theme.once.extend(problems);
        Ok(())
    }

    /// Stops the watcher. Dropped outside the lock: the drop waits for the
    /// watcher's worker, which may be waiting on the lock to report.
    pub(crate) fn theme_shutdown(&self) {
        let watcher = lock(&self.0.theme).watcher.take();
        drop(watcher);
    }
}

type OnChange = Box<dyn Fn(Gathered) + Send>;

/// What starts a watcher: `kernkraft_theme::watch_with`, or a stand-in.
type WatchFn<'a, C> = dyn Fn(&BaseDirs, Option<C>, OnChange) -> Result<Watcher, ThemeError> + 'a;

/// Starts the watcher through `watch` (`kernkraft_theme::watch_with`, or a
/// stand-in in tests), with the portal when `conn` is given. A portal
/// failure (`ThemeError::Portal`: the match rule cannot be built, or the
/// bus refuses `AddMatch`) starts it again without the portal, so the files
/// are still followed; the portal error is returned to be reported. Any
/// other failure (inotify) leaves no watcher, and is returned. `on_change`
/// makes a fresh callback for each attempt.
fn start_watcher<C>(
    base: &BaseDirs,
    conn: Option<C>,
    on_change: &dyn Fn() -> OnChange,
    watch: &WatchFn<'_, C>,
) -> (Option<Watcher>, Vec<WireError>) {
    let with_portal = conn.is_some();
    match watch(base, conn, on_change()) {
        Ok(w) => (Some(w), Vec::new()),
        Err(e @ ThemeError::Portal(_)) if with_portal => {
            let mut problems = vec![WireError::from(e)];
            match watch(base, None, on_change()) {
                Ok(w) => (Some(w), problems),
                Err(e) => {
                    problems.push(WireError::from(e));
                    (None, problems)
                }
            }
        }
        Err(e) => (None, vec![WireError::from(e)]),
    }
}

fn theme_changed<R: Runtime>(inner: &Arc<Inner<R>>, g: Gathered) {
    let mut theme = lock(&inner.theme);
    theme.take(g);
    let (follow, user) = (theme.follow, theme.user);
    let payload = theme.payload(&follow, &user);
    let _ = inner.app.emit(EVENT_THEME_CHANGED, payload);
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use std::cell::Cell;
    use std::sync::mpsc;
    use std::time::Duration;

    use kernkraft_fs::BaseDirs;
    use kernkraft_theme::{watch_with, ThemeError};

    use super::{start_watcher, OnChange};

    fn base(root: &std::path::Path) -> BaseDirs {
        BaseDirs {
            home: root.to_path_buf(),
            config: root.join("config"),
            cache: root.join("cache"),
            state: root.join("state"),
            data: root.join("data"),
            config_dirs: Vec::new(),
            data_dirs: Vec::new(),
            current_desktop: Vec::new(),
        }
    }

    /// A bus that is reached but refuses the portal subscription: the
    /// watcher starts again without the portal, follows the files, and the
    /// refusal is reported.
    #[test]
    fn portal_refusal_falls_back_to_files() {
        let root = tempfile::tempdir().unwrap();
        let base = base(root.path());
        std::fs::create_dir_all(base.config.join("hypr")).unwrap();
        let (tx, rx) = mpsc::channel();
        let on_change = || -> OnChange {
            let tx = tx.clone();
            Box::new(move |_| {
                let _ = tx.send(());
            })
        };
        let attempts = Cell::new(Vec::new());
        // `()` stands for a connection; a portal subscription on it fails
        // as zbus reports a refused AddMatch.
        let watch = |b: &BaseDirs, conn: Option<()>, cb: OnChange| {
            let mut a = attempts.take();
            a.push(conn.is_some());
            attempts.set(a);
            match conn {
                Some(()) => Err(ThemeError::Portal(
                    "subscribe: org.freedesktop.DBus.Error.AccessDenied".to_owned(),
                )),
                None => watch_with(b, None, cb),
            }
        };
        let (watcher, problems) = start_watcher(&base, Some(()), &on_change, &watch);
        assert_eq!(attempts.take(), [true, false]);
        assert!(watcher.is_some());
        assert_eq!(problems.len(), 1);
        assert_eq!(problems[0].kind, "Portal");

        // The file-only watcher is live.
        std::fs::write(
            base.config.join("hypr/hyprtoolkit.conf"),
            "background = rgba(101010ff)\n",
        )
        .unwrap();
        rx.recv_timeout(Duration::from_secs(30))
            .expect("the file-only watcher reports the change");
        drop(watcher);
    }

    /// Without a connection, a failure is not retried.
    #[test]
    fn failure_without_portal_is_reported_once() {
        let root = tempfile::tempdir().unwrap();
        let base = base(root.path());
        let on_change = || -> OnChange { Box::new(|_| {}) };
        let calls = Cell::new(0);
        let watch = |_: &BaseDirs, _: Option<()>, _: OnChange| {
            calls.set(calls.get() + 1);
            Err(ThemeError::Portal("x".to_owned()))
        };
        let (watcher, problems) = start_watcher(&base, None, &on_change, &watch);
        assert!(watcher.is_none());
        assert_eq!(calls.get(), 1);
        assert_eq!(problems.len(), 1);
    }
}
