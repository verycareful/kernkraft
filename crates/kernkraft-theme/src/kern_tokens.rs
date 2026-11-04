// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Kern's own design tokens, the last source in the resolver and the value a
//! switched-off "follow system" group falls back to.
//!
//! Ported value for value from Kern's design handoff, the files
//! `kern_kt/.internal/design/design_handoff_kern_ui/kern-theme.jsx`
//! (`ACCENTS`, `FORMATS`, `buildTheme`, `FONT_STACKS`, `MONO`, `LOGO_FONT`)
//! and `README.md` in the same folder (fonts, radii, alpha values).
//!
//! Values the handoff does not state, and where each number comes from:
//!
//! - `link`: the handoff uses the accent for links ("Accent drives
//!   interactive chrome"), so Kern's link is the accent.
//! - `accent_secondary`: the handoff's `textMid` (secondary text), per scheme.
//! - `on_accent`: the handoff's `accentOn` is `#FFFFFF` in both schemes. The
//!   dark candidate is the handoff's light-scheme text `#17181C`, so the
//!   resolver can pick whichever contrasts more with a system accent.
//! - `radius_large`: the handoff's `rounded` step, 12 px (`buildTheme`
//!   radii: sharp 0, soft 6, rounded 12, round 20).
//! - [`TEXT_DIM_RATIO`] and [`BORDER_RATIO`]: see their docs.

use serde::{Deserialize, Serialize};

use crate::color::Rgba;

/// Light or dark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Light,
    Dark,
}

/// Kern's neutrals for one scheme (`buildTheme`).
#[derive(Debug, Clone, Copy)]
pub struct Neutrals {
    pub bg: Rgba,
    pub surface: Rgba,
    pub sunken: Rgba,
    pub text: Rgba,
    /// The handoff's `textMid`.
    pub text_mid: Rgba,
    pub text_dim: Rgba,
    pub border: Rgba,
}

const fn hex(v: u32) -> Rgba {
    Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const LIGHT: Neutrals = Neutrals {
    bg: hex(0xFFFFFF),
    surface: hex(0xFFFFFF),
    sunken: hex(0xF1F2F4),
    text: hex(0x17181C),
    text_mid: hex(0x5F636B),
    text_dim: hex(0x9CA0A8),
    border: hex(0xE3E5E9),
};

pub const DARK: Neutrals = Neutrals {
    bg: hex(0x000000),
    surface: hex(0x0E0E0F),
    sunken: hex(0x161617),
    text: hex(0xEDEDF0),
    text_mid: hex(0xA1A1A8),
    text_dim: hex(0x67676E),
    border: hex(0x2A2A2D),
};

pub const fn neutrals(scheme: Scheme) -> &'static Neutrals {
    match scheme {
        Scheme::Light => &LIGHT,
        Scheme::Dark => &DARK,
    }
}

/// How far `text_dim` sits from bg towards text. The handoff's dim text is
/// hand-picked; this is the mean over its six scheme and channel pairs of
/// `(bg - dim) / (bg - text)`: light 0.427, 0.411, 0.383, dark 0.435, 0.435,
/// 0.458, mean 0.425, rounded to 0.42. A test checks the value reproduces the
/// handoff's dim text within a few levels.
pub const TEXT_DIM_RATIO: f64 = 0.42;

/// How far `border` sits from bg towards text, derived as
/// [`TEXT_DIM_RATIO`] is from the handoff's `border`: light 0.121, 0.113,
/// 0.097, dark 0.177, 0.177, 0.188, mean 0.146, rounded to 0.15.
pub const BORDER_RATIO: f64 = 0.15;

/// `accentSoft` alpha, dark scheme (`buildTheme`: `hexA(accentBase, 0.20)`).
pub const ACCENT_SOFT_ALPHA_DARK: f64 = 0.20;
/// `accentSoft` alpha, light scheme (`buildTheme`: `hexA(accentBase, 0.12)`).
pub const ACCENT_SOFT_ALPHA_LIGHT: f64 = 0.12;

pub const fn accent_soft_alpha(scheme: Scheme) -> f64 {
    match scheme {
        Scheme::Light => ACCENT_SOFT_ALPHA_LIGHT,
        Scheme::Dark => ACCENT_SOFT_ALPHA_DARK,
    }
}

/// The handoff's `accentOn`, `#FFFFFF`: the light text-on-accent candidate.
pub const ON_ACCENT_LIGHT: Rgba = hex(0xFFFFFF);
/// The handoff's light-scheme text colour, the dark text-on-accent candidate.
pub const ON_ACCENT_DARK: Rgba = hex(0x17181C);

/// Kern's 12 accents (`ACCENTS`), in the handoff's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum KernAccent {
    /// Kern's seed brand and the default.
    #[default]
    Slate,
    Indigo,
    Violet,
    Plum,
    Ocean,
    Teal,
    Forest,
    Amber,
    Copper,
    Crimson,
    Rose,
    Graphite,
}

impl KernAccent {
    pub const ALL: [KernAccent; 12] = [
        KernAccent::Slate,
        KernAccent::Indigo,
        KernAccent::Violet,
        KernAccent::Plum,
        KernAccent::Ocean,
        KernAccent::Teal,
        KernAccent::Forest,
        KernAccent::Amber,
        KernAccent::Copper,
        KernAccent::Crimson,
        KernAccent::Rose,
        KernAccent::Graphite,
    ];

    pub const fn rgba(self) -> Rgba {
        hex(match self {
            KernAccent::Slate => 0x3F5B8B,
            KernAccent::Indigo => 0x4F46E5,
            KernAccent::Violet => 0x7C6CF4,
            KernAccent::Plum => 0x9B4DCA,
            KernAccent::Ocean => 0x0E7490,
            KernAccent::Teal => 0x1E8C84,
            KernAccent::Forest => 0x2F7A52,
            KernAccent::Amber => 0xC9881F,
            KernAccent::Copper => 0xB5651D,
            KernAccent::Crimson => 0xBE3455,
            KernAccent::Rose => 0xC24A6B,
            KernAccent::Graphite => 0x52525B,
        })
    }
}

/// Row density (`buildTheme`: `dens`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Density {
    Compact,
    #[default]
    Cozy,
}

/// Pixel metrics of a density.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DensityMetrics {
    pub row: u32,
    pub pad: u32,
    pub gap: u32,
    pub tile: u32,
}

impl Density {
    pub const fn metrics(self) -> DensityMetrics {
        match self {
            Density::Compact => DensityMetrics {
                row: 60,
                pad: 14,
                gap: 8,
                tile: 38,
            },
            Density::Cozy => DensityMetrics {
                row: 70,
                pad: 18,
                gap: 12,
                tile: 44,
            },
        }
    }
}

/// Corner radius, "Soft" (`buildTheme`: soft 6).
pub const RADIUS_SMALL: u32 = 6;
/// The handoff's `rounded` step (12), used for large corners.
pub const RADIUS_LARGE: u32 = 12;

/// Kern's UI face (README, Typography).
pub const FONT_UI: &str = "Outfit";
/// Wordmark only (README: Quicksand 700, the "kern" wordmark).
pub const FONT_WORDMARK: &str = "Quicksand";
/// Monospace face (`MONO`).
pub const FONT_MONO: &str = "IBM Plex Mono";
/// EPUB reading body only (README).
pub const FONT_READING: &str = "Sora";

/// A file format's identity hue (`FORMATS`). These identify files, not
/// chrome, so they are never part of the resolved tokens or the CSS.
#[derive(Debug, Clone, Copy)]
pub struct FormatHue {
    pub label: &'static str,
    pub light: Rgba,
    pub dark: Rgba,
}

pub const FORMAT_HUES: [FormatHue; 6] = [
    FormatHue {
        label: "PDF",
        light: hex(0xC8453B),
        dark: hex(0xF08177),
    },
    FormatHue {
        label: "Word",
        light: hex(0x2E68C4),
        dark: hex(0x7FA8F0),
    },
    FormatHue {
        label: "PowerPoint",
        light: hex(0xD06A2C),
        dark: hex(0xF0A06A),
    },
    FormatHue {
        label: "Excel",
        light: hex(0x1F8454),
        dark: hex(0x5FC893),
    },
    FormatHue {
        label: "CSV",
        light: hex(0x0E8E9A),
        dark: hex(0x54C9D4),
    },
    FormatHue {
        label: "EPUB",
        light: hex(0x8657C6),
        dark: hex(0xBB97EC),
    },
];
