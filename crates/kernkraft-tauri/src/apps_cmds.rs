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
    launch_in_terminal, open_default, open_terminal, set_default, AppRegistry, DesktopEntry,
    GroupOutcome, MimeDb, Targets, TerminalPrefs,
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

/// What starting one default application came to: `app` is its desktop file
/// ID, `paths` the files it was started for, and `error` is why it was not
/// started, `None` when it was.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct WireOpened {
    pub app: String,
    pub paths: Vec<WirePath>,
    pub error: Option<WireError>,
}

impl From<GroupOutcome> for WireOpened {
    fn from(o: GroupOutcome) -> WireOpened {
        WireOpened {
            app: o.app,
            paths: o.paths.iter().map(WirePath::from).collect(),
            error: o.result.err().map(WireError::from),
        }
    }
}

/// The one scheme [`Kernkraft::open_url`] hands on: the links Kern shows (its
/// source, its sibling apps) are HTTPS pages.
const URL_SCHEME: &str = "https";

/// Whether `url` is `https://` with a non-empty host, every character in
/// RFC 3986's unreserved and reserved sets or a `%` of a percent-encoding
/// (section 2). Spaces, controls, quotes and anything outside ASCII are not.
/// The host is the authority (section 3.2) after any userinfo and before any
/// port, an IP literal being what its brackets hold (section 3.2.2).
fn is_https_url(url: &str) -> bool {
    const RESERVED_AND_UNRESERVED: &str = "-._~:/?#[]@!$&'()*+,;=%";
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    let host_port = authority.rsplit('@').next().unwrap_or_default();
    let host = match host_port.strip_prefix('[') {
        Some(literal) => literal.split_once(']').map_or("", |(h, _)| h),
        None => host_port.split(':').next().unwrap_or_default(),
    };
    !host.is_empty()
        && url
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || RESERVED_AND_UNRESERVED.contains(c))
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

    /// The terminal choice for a launch: the user's `setting` and this
    /// plugin's XDG directories, where the terminal lists are read from.
    fn terminal_prefs<'a>(
        &'a self,
        setting: Option<&'a str>,
    ) -> Result<TerminalPrefs<'a>, WireError> {
        Ok(TerminalPrefs {
            setting,
            base: Some(self.base()?),
        })
    }

    /// Applications that open `path`: the default first, then the others as
    /// `AppRegistry::apps_for` orders them. `NoDisplay` entries are left out
    /// of the list, except the current default. The default is always in it:
    /// one that `mimeapps.list` names without the application registering
    /// the type is not an association, and is added all the same.
    pub fn apps_for(&self, path: WirePath) -> Result<WireAppList, WireError> {
        let path = PathBuf::try_from(path)?;
        let db = self.mime()?;
        let registry = self.registry()?;
        let mime = db.guess(&path);
        let default = registry.default_for(&mime, db);
        let default_id = default.map(|e| e.id.as_str());
        let mut apps: Vec<WireApp> = registry
            .apps_for(&mime, db)
            .into_iter()
            .filter(|e| !e.no_display || Some(e.id.as_str()) == default_id)
            .map(WireApp::from)
            .collect();
        if let Some(default) = default {
            apps.retain(|a| a.id != default.id);
            apps.insert(0, WireApp::from(default));
        }
        Ok(WireAppList {
            apps,
            problems: problems(&registry),
        })
    }

    /// The default application for `path`'s type (see
    /// `AppRegistry::default_for`): the one `mimeapps.list` names, else the
    /// most-preferred association, which is the head of `apps_for`.
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
    /// Terminal setting: for a `Terminal=true` application it is the first
    /// choice of terminal, ahead of the terminal lists and `$TERMINAL`.
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
        let prefs = self.terminal_prefs(terminal.as_deref())?;
        Ok(launch_in_terminal(
            entry,
            Targets::Files(&paths),
            None,
            &prefs,
        )?)
    }

    /// Opens each of `paths` with the default application for its type,
    /// one launch per application with all of its paths, in the order the
    /// applications first appear (`kernkraft_xdg_apps::open_default`). Every
    /// path is checked first: if any has no default application, nothing is
    /// opened and the error is `NoDefaultApp` naming the first such path, so
    /// the UI can offer "Open with" for it. Otherwise the answer holds one
    /// [`WireOpened`] per application, started or not: an application that
    /// fails to start is reported with its paths and does not stop the
    /// others. `terminal` is the user's Terminal setting, as for
    /// [`Kernkraft::launch_app`].
    pub fn open_default(
        &self,
        paths: Vec<WirePath>,
        terminal: Option<String>,
    ) -> Result<Vec<WireOpened>, WireError> {
        let paths = decode(paths)?;
        let db = self.mime()?;
        let registry = self.registry()?;
        let prefs = self.terminal_prefs(terminal.as_deref())?;
        let outcomes = open_default(&registry, db, &paths, &prefs)?;
        Ok(outcomes.into_iter().map(WireOpened::from).collect())
    }

    /// Opens `url` with the default application for its scheme, launched
    /// with the URL. A scheme's default is set like a MIME type's, as
    /// `x-scheme-handler/<scheme>` (MIME Applications Associations 1.0.1),
    /// and the application takes the URL through `%u` or `%U` (Desktop Entry
    /// Specification 1.5, "The Exec key"); one that takes only files is
    /// refused with `NoUrlArgument`, since Kern downloads nothing. Kern
    /// connects to nothing: the application started does.
    ///
    /// Only an `https` URL with a host, made of RFC 3986 characters (section
    /// 2), is taken; anything else is `InvalidArgument` and nothing is
    /// started. `NoUrlHandler` when no default application is set for the
    /// scheme. `terminal` is the user's Terminal setting, as for
    /// [`Kernkraft::launch_app`].
    pub fn open_url(&self, url: String, terminal: Option<String>) -> Result<(), WireError> {
        if !is_https_url(&url) {
            return Err(WireError::from(PluginError::InvalidArgument {
                what: "https URL",
                value: url,
            }));
        }
        let db = self.mime()?;
        let registry = self.registry()?;
        let handler = format!("x-scheme-handler/{URL_SCHEME}");
        let Some(app) = registry.default_for(&handler, db) else {
            return Err(WireError::from(PluginError::NoUrlHandler {
                scheme: URL_SCHEME.to_owned(),
            }));
        };
        let prefs = self.terminal_prefs(terminal.as_deref())?;
        Ok(launch_in_terminal(app, Targets::Url(&url), None, &prefs)?)
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
    /// preference, the first choice (see `kernkraft_xdg_apps::terminal_command`).
    pub fn open_terminal(&self, dir: WirePath, setting: Option<String>) -> Result<(), WireError> {
        let dir = PathBuf::try_from(dir)?;
        check_dest(&dir)?;
        let prefs = self.terminal_prefs(setting.as_deref())?;
        Ok(open_terminal(&prefs, &dir)?)
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
