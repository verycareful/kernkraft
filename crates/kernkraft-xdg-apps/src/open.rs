// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening files with their default applications.
//!
//! Specifications: MIME Applications Associations 1.0.1, section 4 (the
//! default application of a type, see [`AppRegistry::default_for`]) and
//! Desktop Entry Specification 1.5, "The Exec key" (one launch per
//! application can take all of its files when `Exec` has `%F` or `%U`, see
//! [`crate::expand_exec`]).
//!
//! Files are grouped by their default application, in the order the
//! applications first appear, and each group is started once. The group is
//! the unit of success: [`open_default`] reports one [`GroupOutcome`] per
//! application, so a caller can say which files were opened and which were
//! not, rather than that the call failed.

use std::path::PathBuf;

use crate::desktop_entry::DesktopEntry;
use crate::error::AppsError;
use crate::launch::launch_in_terminal;
use crate::mime::MimeDb;
use crate::mimeapps::AppRegistry;
use crate::terminal::TerminalPrefs;
use crate::Targets;

/// The files that one default application opens.
#[derive(Debug)]
pub struct DefaultGroup<'a> {
    pub app: &'a DesktopEntry,
    /// In the order they were given.
    pub paths: Vec<PathBuf>,
}

/// What starting one application for its files came to.
#[derive(Debug)]
pub struct GroupOutcome {
    /// The desktop file ID of the application.
    pub app: String,
    /// The files it was started for.
    pub paths: Vec<PathBuf>,
    /// `Ok` when it was started (its later exit status is not reported, see
    /// [`crate::launch`]); else why it was not.
    pub result: Result<(), AppsError>,
}

/// Groups `paths` by default application, using each file's type as
/// [`MimeDb::guess`] gives it. Every path is checked before anything is
/// returned: when one has no default application the result is
/// [`AppsError::NoDefaultApp`] naming the first such path and its type, so
/// the caller can offer a choice for it, and nothing has been started.
pub fn default_groups<'a>(
    registry: &'a AppRegistry,
    db: &MimeDb,
    paths: &[PathBuf],
) -> Result<Vec<DefaultGroup<'a>>, AppsError> {
    let mut groups: Vec<DefaultGroup<'a>> = Vec::new();
    for path in paths {
        let mime = db.guess(path);
        let Some(app) = registry.default_for(&mime, db) else {
            return Err(AppsError::NoDefaultApp {
                path: path.clone(),
                mime,
            });
        };
        match groups.iter_mut().find(|g| g.app.id == app.id) {
            Some(group) => group.paths.push(path.clone()),
            None => groups.push(DefaultGroup {
                app,
                paths: vec![path.clone()],
            }),
        }
    }
    Ok(groups)
}

/// Starts each group's application with the group's files, as
/// [`launch_in_terminal`] does with `terminal`. A group that fails to start does not stop the ones after it:
/// each has its outcome, in the order of `groups`.
pub fn launch_groups(
    groups: &[DefaultGroup<'_>],
    terminal: &TerminalPrefs<'_>,
) -> Vec<GroupOutcome> {
    groups
        .iter()
        .map(|g| GroupOutcome {
            app: g.app.id.clone(),
            paths: g.paths.clone(),
            result: launch_in_terminal(g.app, Targets::Files(&g.paths), None, terminal),
        })
        .collect()
}

/// Opens each of `paths` with its default application: [`default_groups`]
/// then [`launch_groups`]. `Err` only when some path has no default
/// application, in which case nothing was started; otherwise one outcome per
/// application, whether it started or not. A caller that must read a setting
/// only if some application needs a terminal (`DesktopEntry::terminal`)
/// calls the two steps itself.
pub fn open_default(
    registry: &AppRegistry,
    db: &MimeDb,
    paths: &[PathBuf],
    terminal: &TerminalPrefs<'_>,
) -> Result<Vec<GroupOutcome>, AppsError> {
    Ok(launch_groups(
        &default_groups(registry, db, paths)?,
        terminal,
    ))
}
