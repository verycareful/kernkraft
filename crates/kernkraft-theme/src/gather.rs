// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Collects every theme source (parent spec section 6.5) in one call.

use std::path::{Path, PathBuf};

use kernkraft_fs::BaseDirs;
use zbus::blocking::Connection;

use crate::error::ThemeError;
use crate::portal::read_portal;
use crate::sources::{read_gtk, read_hyprtoolkit, Fontconfig, Sources};

/// `hyprtoolkit.conf` under `$XDG_CONFIG_HOME`.
pub(crate) const HYPR_DIR: &str = "hypr";
pub(crate) const HYPR_FILE: &str = "hyprtoolkit.conf";
/// GTK 4 config directory under `$XDG_CONFIG_HOME`, holding `settings.ini`
/// and `gtk.css`.
pub(crate) const GTK_DIR: &str = "gtk-4.0";

/// What [`gather`] found: the sources, and every source that could not be
/// read.
///
/// A source that is absent (no file, no portal) is `None` in
/// [`sources`](Gathered::sources) and is not an error. A source that exists
/// but could not be read (permission denied, a file over the size limit, a
/// portal error) is also `None` there, and its reason is in
/// [`errors`](Gathered::errors), so nothing fails silently and one broken
/// source does not hide the others.
#[derive(Debug, Default)]
pub struct Gathered {
    pub sources: Sources,
    pub errors: Vec<ThemeError>,
}

fn collect<T>(r: Result<Option<T>, ThemeError>, errors: &mut Vec<ThemeError>) -> Option<T> {
    match r {
        Ok(v) => v,
        Err(e) => {
            errors.push(e);
            None
        }
    }
}

/// Reads `hypr/hyprtoolkit.conf`, `gtk-4.0/settings.ini` and
/// `gtk-4.0/gtk.css` under `base.config`, asks the portal when `conn` is
/// given, and asks fontconfig for its defaults.
pub fn gather(base: &BaseDirs, conn: Option<&Connection>) -> Gathered {
    gather_from(&base.config, conn)
}

pub(crate) fn gather_from(config: &Path, conn: Option<&Connection>) -> Gathered {
    let mut errors = Vec::new();
    let hypr: PathBuf = config.join(HYPR_DIR).join(HYPR_FILE);
    let sources = Sources {
        hypr: collect(read_hyprtoolkit(&hypr), &mut errors),
        portal: conn.and_then(|c| collect(read_portal(c), &mut errors)),
        gtk: collect(read_gtk(&config.join(GTK_DIR)), &mut errors),
        fontconfig: Some(Fontconfig::detect()),
    };
    Gathered { sources, errors }
}
