// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Colours and WCAG 2.1 contrast.
//!
//! WCAG 2.1, "Definition: relative luminance": each sRGB channel `c = v / 255`
//! is linearised (`c / 12.92` when `c <= 0.04045`, else
//! `((c + 0.055) / 1.055) ^ 2.4`). WCAG 2.1 prints the threshold as 0.03928;
//! 0.04045 is the value in the sRGB standard (IEC 61966-2-1). They give
//! identical results for every 8-bit channel, since no `v / 255` lies between
//! them (10/255 is 0.0392, 11/255 is 0.0431). Then
//! `L = 0.2126 R + 0.7152 G + 0.0722 B`.
//! WCAG 2.1, "Definition: contrast ratio": `(L1 + 0.05) / (L2 + 0.05)` with
//! `L1` the lighter. Alpha is ignored by both.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// WCAG 2.1 SC 1.4.3 (Contrast, Minimum): body text needs at least 4.5:1.
pub const WCAG_AA_BODY: f64 = 4.5;

/// An sRGB colour with alpha.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Rgba {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba {
    pub const fn rgb(r: u8, g: u8, b: u8) -> Rgba {
        Rgba { r, g, b, a: 255 }
    }

    /// Parses the colour notations Hyprland accepts, plus `#` hex:
    ///
    /// - `rgb(rrggbb)` and `rgba(rrggbbaa)`: hex, alpha last;
    /// - `rgb(r,g,b)` and `rgba(r,g,b,a)`: decimal channels 0 to 255 and an
    ///   alpha fraction 0.0 to 1.0, spaces around the numbers allowed;
    /// - `0xAARRGGBB`: hex, alpha FIRST, all eight digits;
    /// - `#rrggbb` and `#rrggbbaa`: hex, alpha last.
    ///
    /// Anything else, including an out-of-range number, is `None`.
    pub fn parse(s: &str) -> Option<Rgba> {
        let s = s.trim();
        if let Some(h) = s.strip_prefix('#') {
            return parse_hex(h, h.len() == 8);
        }
        if let Some(h) = strip_prefix_ignore_case(s, "0x") {
            return parse_argb(h);
        }
        let (inner, with_alpha) = match strip_call(s, "rgba") {
            Some(inner) => (inner, true),
            None => (strip_call(s, "rgb")?, false),
        };
        if inner.contains(',') {
            parse_decimal(inner, with_alpha)
        } else {
            parse_hex(inner, with_alpha)
        }
    }

    /// `#rrggbbaa`, lower case.
    pub fn to_hex(self) -> String {
        format!("#{:02x}{:02x}{:02x}{:02x}", self.r, self.g, self.b, self.a)
    }

    /// The colour with its alpha replaced by `alpha` (0.0 to 1.0, rounded).
    pub fn with_alpha(self, alpha: f64) -> Rgba {
        Rgba {
            a: (alpha.clamp(0.0, 1.0) * 255.0).round() as u8,
            ..self
        }
    }

    /// WCAG 2.1 relative luminance.
    pub fn luminance(self) -> f64 {
        let lin = |v: u8| {
            let c = f64::from(v) / 255.0;
            if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * lin(self.r) + 0.7152 * lin(self.g) + 0.0722 * lin(self.b)
    }

    /// Per-channel linear interpolation in sRGB from `self` towards `other`
    /// by `t` (0.0 is `self`, 1.0 is `other`). The result is opaque.
    pub fn mix(self, other: Rgba, t: f64) -> Rgba {
        let ch = |a: u8, b: u8| {
            let (a, b) = (f64::from(a), f64::from(b));
            (a + (b - a) * t).round().clamp(0.0, 255.0) as u8
        };
        Rgba::rgb(
            ch(self.r, other.r),
            ch(self.g, other.g),
            ch(self.b, other.b),
        )
    }
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    let head = s.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &s[prefix.len()..])
}

fn strip_call<'a>(s: &'a str, name: &str) -> Option<&'a str> {
    strip_prefix_ignore_case(s, name)?
        .trim_start()
        .strip_prefix('(')?
        .strip_suffix(')')
}

/// Six hex digits, or eight when `with_alpha`, in red, green, blue, alpha
/// order.
fn parse_hex(digits: &str, with_alpha: bool) -> Option<Rgba> {
    let digits = digits.trim();
    let want = if with_alpha { 8 } else { 6 };
    if digits.len() != want || !digits.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&digits[i * 2..i * 2 + 2], 16).ok();
    Some(Rgba {
        r: byte(0)?,
        g: byte(1)?,
        b: byte(2)?,
        a: if with_alpha { byte(3)? } else { 255 },
    })
}

/// Exactly eight hex digits, alpha first (`AARRGGBB`).
fn parse_argb(digits: &str) -> Option<Rgba> {
    let rgba = parse_hex(digits, true)?;
    // `parse_hex` read the digits as RRGGBBAA, so each field is one place
    // early: the first byte it called red is the alpha.
    Some(Rgba {
        a: rgba.r,
        r: rgba.g,
        g: rgba.b,
        b: rgba.a,
    })
}

/// `r,g,b` (three whole numbers 0 to 255) or, when `with_alpha`, `r,g,b,a`
/// with `a` a fraction 0.0 to 1.0.
fn parse_decimal(inner: &str, with_alpha: bool) -> Option<Rgba> {
    let parts: Vec<&str> = inner.split(',').map(str::trim).collect();
    if parts.len() != if with_alpha { 4 } else { 3 } {
        return None;
    }
    let channel = |p: &str| {
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        p.parse::<u8>().ok()
    };
    let a = if with_alpha {
        let p = parts[3];
        // Digits and a point only: `f64::from_str` would also take `nan`,
        // `inf`, a sign and an exponent, none of which is a fraction.
        if p.is_empty() || !p.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
            return None;
        }
        let fraction = p.parse::<f64>().ok().filter(|f| (0.0..=1.0).contains(f))?;
        (fraction * 255.0).round() as u8
    } else {
        255
    };
    Some(Rgba {
        r: channel(parts[0])?,
        g: channel(parts[1])?,
        b: channel(parts[2])?,
        a,
    })
}

/// WCAG 2.1 contrast ratio between two colours, 1.0 to 21.0, in either order.
pub fn contrast_ratio(a: Rgba, b: Rgba) -> f64 {
    let (la, lb) = (a.luminance(), b.luminance());
    let (hi, lo) = if la >= lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

impl Serialize for Rgba {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for Rgba {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Rgba, D::Error> {
        let s = String::deserialize(d)?;
        Rgba::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("bad colour {s:?}")))
    }
}
