// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The token resolver (parent spec section 6.5): theme sources in, one token
//! set out. It is pure: it reads no file and runs no process; the only
//! outside knowledge, which font families exist, arrives as a closure.
//!
//! Source priority, highest first: Hyprland, portal, GTK, fontconfig, Kern.
//! Each token records the [`Origin`] it came from in [`Tokens::origins`].

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::color::{contrast_ratio, Rgba, WCAG_AA_BODY};
use crate::kern_tokens::{
    self as kern, accent_soft_alpha, neutrals, Density, KernAccent, Scheme, BORDER_RATIO,
    TEXT_DIM_RATIO,
};
use crate::sources::Sources;

/// Which "follow system" groups are on. All are on by default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Follow {
    pub colours: bool,
    pub accent: bool,
    pub fonts: bool,
    pub radius: bool,
}

impl Default for Follow {
    fn default() -> Self {
        Follow {
            colours: true,
            accent: true,
            fonts: true,
            radius: true,
        }
    }
}

/// The user's theme mode setting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Follow the system: its palette and its light or dark preference.
    #[default]
    System,
    /// Pin the light scheme. Kern's own neutrals are used, unless colours are
    /// followed and Hyprland's palette is itself light; a dark Hyprland
    /// palette is not used (see [`Tokens::scheme_mismatch`]).
    Light,
    /// Pin the dark scheme, with the same rule mirrored.
    Dark,
}

/// What the user picked in Settings, used wherever a group is not followed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UserChoice {
    pub mode: Mode,
    pub accent: KernAccent,
    pub density: Density,
}

/// Where a resolved token came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Origin {
    Hyprland,
    Portal,
    Gtk,
    Fontconfig,
    Kern,
    User,
}

/// One resolved token set. Format identity hues are deliberately absent:
/// they identify files, not chrome, and stay Kern's.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Tokens {
    pub scheme: Scheme,
    pub bg: Rgba,
    pub surface: Rgba,
    pub sunken: Rgba,
    pub text: Rgba,
    pub text_dim: Rgba,
    pub border: Rgba,
    pub link: Rgba,
    pub accent: Rgba,
    pub accent_soft: Rgba,
    pub on_accent: Rgba,
    pub accent_secondary: Rgba,
    pub font_ui: String,
    pub font_mono: String,
    pub radius_small: u32,
    pub radius_large: u32,
    pub density: Density,
    /// Token name (the field name, for example `"text_dim"`) to its origin.
    /// Keys and the fallback names below are owned strings so the set
    /// deserialises. Every field except `density` has an entry.
    pub origins: BTreeMap<String, Origin>,
    /// Names of tokens that fell back to Kern's neutrals because the source
    /// palette failed WCAG AA for body text against bg, surface or sunken.
    /// The neutral set (bg, surface, sunken, text) reverts together; each of
    /// those that came from a source is named, in that order.
    pub wcag_fallbacks: Vec<String>,
    /// Colours are followed and a scheme is pinned, but Hyprland's palette is
    /// of the other scheme, so Kern's neutrals for the pinned scheme are used
    /// instead. Settings can say why the system palette is not applied.
    pub scheme_mismatch: bool,
}

/// Luminance at which white and black text contrast equally with a
/// background: `(1.05) / (L + 0.05) = (L + 0.05) / 0.05`, so
/// `L = sqrt(1.05 * 0.05) - 0.05`. Backgrounds below it read as dark.
fn is_dark_background(bg: Rgba) -> bool {
    bg.luminance() < (1.05f64 * 0.05).sqrt() - 0.05
}

fn pick_scheme(src: &Sources, user: &UserChoice, system_colours: bool) -> (Scheme, Origin) {
    match user.mode {
        Mode::Light => return (Scheme::Light, Origin::User),
        Mode::Dark => return (Scheme::Dark, Origin::User),
        Mode::System => {}
    }
    if system_colours {
        if let Some(bg) = src.hypr.as_ref().and_then(|h| h.background) {
            let s = if is_dark_background(bg) {
                Scheme::Dark
            } else {
                Scheme::Light
            };
            return (s, Origin::Hyprland);
        }
    }
    if let Some(s) = src.portal.as_ref().and_then(|p| p.color_scheme) {
        return (s, Origin::Portal);
    }
    if let Some(dark) = src.gtk.as_ref().and_then(|g| g.prefer_dark) {
        let s = if dark { Scheme::Dark } else { Scheme::Light };
        return (s, Origin::Gtk);
    }
    (Scheme::Light, Origin::Kern)
}

/// Resolves the sources into one token set.
///
/// The system palette (Hyprland's colours) applies when `follow.colours` is
/// on and the mode is [`Mode::System`]; a pinned Light or Dark mode uses
/// Kern's neutrals for that scheme. `fonts_ok` says whether fontconfig
/// resolves a family; a family it rejects falls to the next source's.
pub fn resolve(
    src: &Sources,
    follow: &Follow,
    user: &UserChoice,
    fonts_ok: &dyn Fn(&str) -> bool,
) -> Tokens {
    let hypr_scheme = src.hypr.as_ref().and_then(|h| h.background).map(|bg| {
        if is_dark_background(bg) {
            Scheme::Dark
        } else {
            Scheme::Light
        }
    });
    let pinned = match user.mode {
        Mode::System => None,
        Mode::Light => Some(Scheme::Light),
        Mode::Dark => Some(Scheme::Dark),
    };
    // A pinned scheme keeps Hyprland's palette only when the palette is of
    // that scheme; with no background there is nothing to compare, so the
    // palette is not used.
    let system_colours = follow.colours && (pinned.is_none() || pinned == hypr_scheme);
    let scheme_mismatch =
        follow.colours && pinned.is_some() && hypr_scheme.is_some() && pinned != hypr_scheme;
    let (scheme, scheme_origin) = pick_scheme(src, user, system_colours);
    let k = neutrals(scheme);
    let mut origins: BTreeMap<String, Origin> = BTreeMap::new();
    origins.insert("scheme".into(), scheme_origin);

    let hypr = src.hypr.as_ref();
    let hc = |f: fn(&crate::sources::HyprToolkit) -> Option<Rgba>| {
        if system_colours {
            hypr.and_then(f)
        } else {
            None
        }
    };
    let mut colour = |name: &str, from_hypr: Option<Rgba>, kern_value: Rgba| -> Rgba {
        match from_hypr {
            Some(c) => {
                origins.insert(name.into(), Origin::Hyprland);
                c
            }
            None => {
                origins.insert(name.into(), Origin::Kern);
                kern_value
            }
        }
    };
    let mut bg = colour("bg", hc(|h| h.background), k.bg);
    let mut surface = colour("surface", hc(|h| h.base), k.surface);
    let mut sunken = colour("sunken", hc(|h| h.alternate_base), k.sunken);
    let mut text = colour("text", hc(|h| h.text), k.text);

    // Body text must reach WCAG AA against every background it sits on. On a
    // failure the whole neutral set reverts together, so the palette stays
    // coherent, and every token that was taken from the source and is now
    // Kern's is named.
    let mut wcag_fallbacks = Vec::new();
    if [bg, surface, sunken]
        .iter()
        .any(|b| contrast_ratio(text, *b) < WCAG_AA_BODY)
    {
        for name in ["bg", "surface", "sunken", "text"] {
            if origins[name] != Origin::Kern {
                wcag_fallbacks.push(name.to_string());
                origins.insert(name.into(), Origin::Kern);
            }
        }
        (bg, surface, sunken, text) = (k.bg, k.surface, k.sunken, k.text);
    }

    let from_kern = origins["bg"] == Origin::Kern && origins["text"] == Origin::Kern;
    let (text_dim, border, derived_origin) = if from_kern {
        (k.text_dim, k.border, Origin::Kern)
    } else {
        (
            bg.mix(text, TEXT_DIM_RATIO),
            bg.mix(text, BORDER_RATIO),
            Origin::Hyprland,
        )
    };
    origins.insert("text_dim".into(), derived_origin);
    origins.insert("border".into(), derived_origin);

    // Accent: Hyprland, portal, GTK, then the user's pick.
    let (accent, accent_origin) = [
        (hypr.and_then(|h| h.accent), Origin::Hyprland),
        (src.portal.as_ref().and_then(|p| p.accent), Origin::Portal),
        (src.gtk.as_ref().and_then(|g| g.accent), Origin::Gtk),
    ]
    .into_iter()
    .filter(|_| follow.accent)
    .find_map(|(c, o)| c.map(|c| (c, o)))
    .unwrap_or((user.accent.rgba(), Origin::User));
    origins.insert("accent".into(), accent_origin);
    origins.insert("accent_soft".into(), accent_origin);
    origins.insert("on_accent".into(), accent_origin);
    let accent_soft = accent.with_alpha(accent_soft_alpha(scheme));
    let on_accent = if contrast_ratio(kern::ON_ACCENT_DARK, accent)
        > contrast_ratio(kern::ON_ACCENT_LIGHT, accent)
    {
        kern::ON_ACCENT_DARK
    } else {
        kern::ON_ACCENT_LIGHT
    };

    let secondary = if follow.accent {
        hypr.and_then(|h| h.accent_secondary)
    } else {
        None
    };
    let accent_secondary = match secondary {
        Some(c) => {
            origins.insert("accent_secondary".into(), Origin::Hyprland);
            c
        }
        None => {
            origins.insert("accent_secondary".into(), Origin::Kern);
            k.text_mid
        }
    };

    let link = match hc(|h| h.link_text) {
        Some(c) => {
            origins.insert("link".into(), Origin::Hyprland);
            c
        }
        None => {
            origins.insert("link".into(), accent_origin);
            accent
        }
    };

    // Fonts: the first candidate that fontconfig resolves.
    let (font_ui, ui_origin) = if follow.fonts {
        let cands = [
            (
                hypr.and_then(|h| h.font_family.as_deref()),
                Origin::Hyprland,
            ),
            (
                src.gtk.as_ref().and_then(|g| g.font_name.as_deref()),
                Origin::Gtk,
            ),
            (
                src.fontconfig.as_ref().and_then(|f| f.ui.as_deref()),
                Origin::Fontconfig,
            ),
        ];
        first_resolvable(&cands, fonts_ok)
    } else {
        None
    }
    .unwrap_or((kern::FONT_UI.to_string(), Origin::Kern));
    origins.insert("font_ui".into(), ui_origin);

    let (font_mono, mono_origin) = if follow.fonts {
        let cands = [
            (
                hypr.and_then(|h| h.font_family_monospace.as_deref()),
                Origin::Hyprland,
            ),
            (
                src.fontconfig.as_ref().and_then(|f| f.mono.as_deref()),
                Origin::Fontconfig,
            ),
        ];
        first_resolvable(&cands, fonts_ok)
    } else {
        None
    }
    .unwrap_or((kern::FONT_MONO.to_string(), Origin::Kern));
    origins.insert("font_mono".into(), mono_origin);

    let radius = |from_hypr: Option<u32>, kern_value: u32, name: &str, o: &mut BTreeMap<_, _>| {
        match from_hypr.filter(|_| follow.radius) {
            Some(r) => {
                o.insert(name.to_string(), Origin::Hyprland);
                r
            }
            None => {
                o.insert(name.to_string(), Origin::Kern);
                kern_value
            }
        }
    };
    let radius_small = radius(
        hypr.and_then(|h| h.rounding_small),
        kern::RADIUS_SMALL,
        "radius_small",
        &mut origins,
    );
    let radius_large = radius(
        hypr.and_then(|h| h.rounding_large),
        kern::RADIUS_LARGE,
        "radius_large",
        &mut origins,
    );

    Tokens {
        scheme,
        bg,
        surface,
        sunken,
        text,
        text_dim,
        border,
        link,
        accent,
        accent_soft,
        on_accent,
        accent_secondary,
        font_ui,
        font_mono,
        radius_small,
        radius_large,
        density: user.density,
        origins,
        wcag_fallbacks,
        scheme_mismatch,
    }
}

fn first_resolvable(
    cands: &[(Option<&str>, Origin)],
    fonts_ok: &dyn Fn(&str) -> bool,
) -> Option<(String, Origin)> {
    cands
        .iter()
        .filter_map(|(f, o)| f.map(|f| (f, *o)))
        .find(|(f, _)| fonts_ok(f))
        .map(|(f, o)| (f.to_string(), o))
}
