// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! GTK appearance (parent spec section 6.5, source 3): `gtk-font-name` and
//! `gtk-application-prefer-dark-theme` from `settings.ini`, and
//! `@define-color accent_color` from `gtk.css`.

use serde::{Deserialize, Serialize};

use crate::color::Rgba;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Gtk {
    /// Family only: no size, no style words.
    pub font_name: Option<String>,
    pub prefer_dark: Option<bool>,
    pub accent: Option<Rgba>,
}

/// Style words Pango accepts between the family and the size in a font
/// description (`pango_font_description_from_string`: weights, slants,
/// stretches). A trailing run of these is not part of the family.
const PANGO_STYLE_WORDS: &[&str] = &[
    "thin",
    "ultra-light",
    "ultralight",
    "extra-light",
    "extralight",
    "light",
    "semi-light",
    "semilight",
    "book",
    "regular",
    "medium",
    "semi-bold",
    "semibold",
    "bold",
    "ultra-bold",
    "ultrabold",
    "extra-bold",
    "extrabold",
    "heavy",
    "ultra-heavy",
    "italic",
    "oblique",
    "roman",
    "condensed",
    "extra-condensed",
    "ultra-condensed",
    "semi-condensed",
    "expanded",
    "extra-expanded",
    "ultra-expanded",
    "semi-expanded",
];

/// The family of a Pango font description such as `Inter 11` or
/// `Oxanium Medium 10`: the trailing size is removed, then any trailing
/// style words, always keeping the first word.
fn font_family(desc: &str) -> Option<String> {
    let mut words: Vec<&str> = desc.split_whitespace().collect();
    if words.last().is_some_and(|w| w.parse::<f64>().is_ok()) && words.len() > 1 {
        words.pop();
    }
    while words.len() > 1
        && words
            .last()
            .is_some_and(|w| PANGO_STYLE_WORDS.contains(&w.to_ascii_lowercase().as_str()))
    {
        words.pop();
    }
    let family = words.join(" ");
    (!family.is_empty()).then_some(family)
}

fn parse_bool(v: &str) -> Option<bool> {
    match v.trim().to_ascii_lowercase().as_str() {
        "1" | "true" => Some(true),
        "0" | "false" => Some(false),
        _ => None,
    }
}

fn strip_css_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        match rest[i + 2..].find("*/") {
            Some(j) => rest = &rest[i + 2 + j + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// Parses `settings.ini` and `gtk.css` contents; either may be absent.
/// Unusable values are left unset.
pub fn parse_gtk(settings_ini: Option<&str>, gtk_css: Option<&str>) -> Gtk {
    let mut out = Gtk::default();
    if let Some(ini) = settings_ini {
        let mut in_settings = false;
        for line in ini.lines() {
            let line = line.trim();
            if line.starts_with('#') || line.starts_with(';') || line.is_empty() {
                continue;
            }
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                in_settings = name == "Settings";
                continue;
            }
            if !in_settings {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                continue;
            };
            match key.trim() {
                "gtk-font-name" => out.font_name = font_family(value),
                "gtk-application-prefer-dark-theme" => out.prefer_dark = parse_bool(value),
                _ => {}
            }
        }
    }
    if let Some(css) = gtk_css {
        for stmt in strip_css_comments(css).split(';') {
            let Some(rest) = stmt.trim().strip_prefix("@define-color") else {
                continue;
            };
            let mut parts = rest.split_whitespace();
            if parts.next() == Some("accent_color") {
                if let Some(c) = parts.next().and_then(Rgba::parse) {
                    out.accent = Some(c);
                }
            }
        }
    }
    out
}
