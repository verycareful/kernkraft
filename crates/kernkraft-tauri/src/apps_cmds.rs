// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Applications: which ones open a file, the defaults, launching, the
//! terminal, and a file's type.
//!
//! Specifications: Desktop Entry Specification 1.5 (entries, `Exec`) and
//! MIME Applications Associations 1.0.1 (`mimeapps.list`), through
//! `kernkraft-xdg-apps`.
//!
//! The application registry is read again on every call: applications are
//! installed and defaults changed while the app runs (by
//! [`Kernkraft::set_default_app`] among others), and a cached registry
//! would offer what is no longer there. A desktop entry or `mimeapps.list`
//! that cannot be used does not hide the others; it is left out and its
//! error returned in `problems`, so the UI can say why an application is
//! missing.
//!
//! A file's type for choosing an application is looked up per file, by name
//! and then, when no name pattern matches, by content (`MimeDb::guess`);
//! these commands act on files one at a time, unlike listings, which type by
//! name only.

use std::path::PathBuf;

use kernkraft_xdg_apps::{
    launch_in_terminal, open_terminal, set_default, AppRegistry, DesktopEntry, MimeDb,
};
use serde::{Deserialize, Serialize};
use tauri::Runtime;

use crate::error::{PluginError, WireError};
use crate::ops::check_dest;
use crate::state::Kernkraft;
use crate::wire::WirePath;

/// An installed application. `id` is its desktop file ID, what
/// [`Kernkraft::launch_app`] and [`Kernkraft::set_default_app`] take;
/// `icon` is the `Icon` key as written (an icon name or an absolute path).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireApp {
    pub id: String,
    pub name: String,
    pub icon: Option<String>,
    /// `Terminal=true`: it runs inside a terminal.
    pub terminal: bool,
    /// `NoDisplay=true`: it should not appear in menus of all applications.
    pub no_display: bool,
    pub mime_types: Vec<String>,
}

impl From<&DesktopEntry> for WireApp {
    fn from(e: &DesktopEntry) -> WireApp {
        WireApp {
            id: e.id.clone(),
            name: e.name.clone(),
            icon: e.icon.clone(),
            terminal: e.terminal,
            no_display: e.no_display,
            mime_types: e.mime_types.clone(),
        }
    }
}

/// Applications, and the entries and association files that could not be
/// read while looking.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireAppList {
    pub apps: Vec<WireApp>,
    pub problems: Vec<WireError>,
}

/// The default application, if one is set, and the problems as in
/// [`WireAppList`]: a broken `mimeapps.list` can be why there is none.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireAppDefault {
    pub app: Option<WireApp>,
    pub problems: Vec<WireError>,
}

/// A file's type, and whether the editor can open it as text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireMime {
    pub mime: String,
    pub is_text: bool,
}

fn problems(registry: &AppRegistry) -> Vec<WireError> {
    registry.skipped().iter().map(WireError::from).collect()
}

fn decode(paths: Vec<WirePath>) -> Result<Vec<PathBuf>, WireError> {
    paths
        .into_iter()
        .map(|p| PathBuf::try_from(p).map_err(WireError::from))
        .collect()
}

impl<R: Runtime> Kernkraft<R> {
    fn registry(&self) -> Result<AppRegistry, WireError> {
        Ok(AppRegistry::load(self.base()?)?)
    }

    /// Applications that open `path`, best first (see
    /// `AppRegistry::apps_for`): `NoDisplay` and hidden entries are left out.
    pub fn apps_for(&self, path: WirePath) -> Result<WireAppList, WireError> {
        let path = PathBuf::try_from(path)?;
        let db = self.mime()?;
        let registry = self.registry()?;
        let mime = db.guess(&path);
        Ok(WireAppList {
            apps: registry
                .apps_for(&mime, db)
                .into_iter()
                .map(WireApp::from)
                .collect(),
            problems: problems(&registry),
        })
    }

    /// The default application for `path`'s type, if `mimeapps.list` names
    /// an installed one for it or a parent type.
    pub fn app_default(&self, path: WirePath) -> Result<WireAppDefault, WireError> {
        let path = PathBuf::try_from(path)?;
        let db = self.mime()?;
        let registry = self.registry()?;
        let mime = db.guess(&path);
        Ok(WireAppDefault {
            app: registry.default_for(&mime, db).map(WireApp::from),
            problems: problems(&registry),
        })
    }

    /// Every installed application, by ID, `NoDisplay` ones included and
    /// marked.
    pub fn all_apps(&self) -> Result<WireAppList, WireError> {
        let registry = self.registry()?;
        Ok(WireAppList {
            apps: registry.all().into_iter().map(WireApp::from).collect(),
            problems: problems(&registry),
        })
    }

    /// Starts application `id` with `paths`, detached (see
    /// `kernkraft_xdg_apps::launch_in_terminal`). `terminal` is the user's
    /// Terminal setting: for a `Terminal=true` application it is the last
    /// choice of terminal, after `xdg-terminal-exec` and `$TERMINAL`.
    /// `UnknownApp` when no installed application has that ID.
    pub fn launch_app(
        &self,
        id: String,
        paths: Vec<WirePath>,
        terminal: Option<String>,
    ) -> Result<(), WireError> {
        let paths = decode(paths)?;
        let registry = self.registry()?;
        let entry = registry
            .get(&id)
            .ok_or_else(|| PluginError::UnknownApp(id.clone()))?;
        Ok(launch_in_terminal(
            entry,
            &paths,
            None,
            terminal.as_deref(),
        )?)
    }

    /// Opens each of `paths` with the default application for its type,
    /// one launch per application with all of its paths, in the order the
    /// applications first appear. Every path is checked first: if any has
    /// no default application, nothing is opened and the error is
    /// `NoDefaultApp` naming the first such path, so the UI can offer
    /// "Open with" for it. A launch that fails stops the ones after it;
    /// the ones before have started. `terminal` is the user's Terminal
    /// setting, as for [`Kernkraft::launch_app`].
    pub fn open_default(
        &self,
        paths: Vec<WirePath>,
        terminal: Option<String>,
    ) -> Result<(), WireError> {
        let paths = decode(paths)?;
        let db = self.mime()?;
        let registry = self.registry()?;
        let mut groups: Vec<(&DesktopEntry, Vec<PathBuf>)> = Vec::new();
        for path in paths {
            let mime = db.guess(&path);
            let Some(app) = registry.default_for(&mime, db) else {
                return Err(WireError::from(PluginError::NoDefaultApp { path, mime }));
            };
            match groups.iter_mut().find(|(a, _)| a.id == app.id) {
                Some((_, group)) => group.push(path),
                None => groups.push((app, vec![path])),
            }
        }
        for (app, group) in groups {
            launch_in_terminal(app, &group, None, terminal.as_deref())?;
        }
        Ok(())
    }

    /// Makes application `id` the default for `mime`, through
    /// `xdg-mime default`. `UnknownApp` when no installed application has
    /// that ID; nothing is run then.
    pub fn set_default_app(&self, id: String, mime: String) -> Result<(), WireError> {
        let registry = self.registry()?;
        if registry.get(&id).is_none() {
            return Err(WireError::from(PluginError::UnknownApp(id)));
        }
        Ok(set_default(&id, &mime)?)
    }

    /// Opens a terminal in folder `dir`; `setting` is the user's terminal
    /// preference, the last choice (see `kernkraft_xdg_apps::terminal_command`).
    pub fn open_terminal(&self, dir: WirePath, setting: Option<String>) -> Result<(), WireError> {
        let dir = PathBuf::try_from(dir)?;
        check_dest(&dir)?;
        Ok(open_terminal(&dir, setting.as_deref())?)
    }

    /// The type of `path`, looking at its content when its name says
    /// nothing, and whether it is text.
    pub fn mime_of(&self, path: WirePath) -> Result<WireMime, WireError> {
        let path = PathBuf::try_from(path)?;
        let db: &MimeDb = self.mime()?;
        let mime = db.guess(&path);
        Ok(WireMime {
            is_text: db.is_text(&mime),
            mime,
        })
    }
}
