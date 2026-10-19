// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Theme sources (parent spec section 6.5): Hyprland's `hyprtoolkit.conf`,
//! the xdg-desktop-portal appearance settings, GTK's `settings.ini` and
//! `gtk.css`, and fontconfig defaults.

mod fontconfig;
mod gtk;
mod hyprtoolkit;

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::color::Rgba;
use crate::error::ThemeError;
use crate::kern_tokens::Scheme;

pub use fontconfig::{fc_default, fc_resolves, Fontconfig};
pub use gtk::{parse_gtk, Gtk};
pub use hyprtoolkit::{parse_hyprtoolkit, HyprToolkit};

/// What `org.freedesktop.appearance` reports. The reader that fills it is
/// separate from the resolver; an unset field means the portal said nothing.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Portal {
    /// `color-scheme`: 1 is dark, 2 is light, 0 (no preference) is `None`.
    pub color_scheme: Option<Scheme>,
    /// `accent-color`.
    pub accent: Option<Rgba>,
    /// `contrast` reports more contrast.
    pub high_contrast: bool,
}

/// Every source the resolver reads. A `None` source is absent, not failed.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Sources {
    pub hypr: Option<HyprToolkit>,
    pub portal: Option<Portal>,
    pub gtk: Option<Gtk>,
    pub fontconfig: Option<Fontconfig>,
}

fn read_optional(path: &Path) -> Result<Option<String>, ThemeError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(String::from_utf8_lossy(&bytes).into_owned())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ThemeError::Io {
            path: path.to_path_buf(),
            source,
        }),
    }
}

/// Reads and parses `hyprtoolkit.conf`. A missing file is `Ok(None)`.
pub fn read_hyprtoolkit(path: &Path) -> Result<Option<HyprToolkit>, ThemeError> {
    Ok(read_optional(path)?.map(|s| parse_hyprtoolkit(&s)))
}

/// Reads and parses a GTK config directory's `settings.ini` and `gtk.css`
/// (for example `~/.config/gtk-4.0`). `None` when neither file exists.
pub fn read_gtk(dir: &Path) -> Result<Option<Gtk>, ThemeError> {
    let ini = read_optional(&dir.join("settings.ini"))?;
    let css = read_optional(&dir.join("gtk.css"))?;
    if ini.is_none() && css.is_none() {
        return Ok(None);
    }
    Ok(Some(parse_gtk(ini.as_deref(), css.as_deref())))
}
