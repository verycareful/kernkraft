// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `hyprtoolkit.conf`, Hyprland's toolkit theme (parent spec section 6.5,
//! source 1). The file uses hyprlang syntax: `key = value` lines, `#` starts
//! a comment and `##` is a literal `#`; `$name = value` defines a variable
//! that later values may use as `$name`.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::color::Rgba;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HyprToolkit {
    pub background: Option<Rgba>,
    pub base: Option<Rgba>,
    pub alternate_base: Option<Rgba>,
    pub text: Option<Rgba>,
    pub link_text: Option<Rgba>,
    pub accent: Option<Rgba>,
    pub accent_secondary: Option<Rgba>,
    pub font_family: Option<String>,
    pub font_family_monospace: Option<String>,
    pub rounding_small: Option<u32>,
    pub rounding_large: Option<u32>,
    /// Each unknown key and each known key with an unusable value, as
    /// `key = value` text, in file order.
    pub ignored: Vec<String>,
}

/// Keys the file may carry that Kern has no token for. They are valid, so
/// they are not listed as ignored.
const UNMAPPED_KEYS: &[&str] = &["bright_text"];

/// Removes a hyprlang comment: everything from the first single `#`, with
/// `##` standing for a literal `#`.
fn strip_comment(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '#' {
            if chars.peek() == Some(&'#') {
                chars.next();
                out.push('#');
            } else {
                break;
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Longest value, after substitution, the parser accepts. A value is a
/// colour, a number or a font family: tens of bytes. 4 KiB is far above any
/// real one and keeps a chain of `$a = $b$b` lines from doubling without end.
const MAX_VALUE_LEN: usize = 4096;

/// Most bytes of variable definitions held at once. A real file defines a
/// handful of short variables, so it stays far below this. 1 MiB matches the
/// file size cap, yet a file under that cap can still reach it (a few hundred
/// lines, each defining a variable of [`MAX_VALUE_LEN`] bytes through
/// doubling); a definition that would pass it is ignored and listed. The cap
/// bounds memory for text handed straight to [`parse_hyprtoolkit`].
const MAX_VARS_TOTAL: usize = 1 << 20;

/// Replaces each `$name` in `value` with its definition. `None` when a
/// variable is not defined (hyprlang defines a variable before its use) or
/// the result would exceed [`MAX_VALUE_LEN`].
fn substitute(value: &str, vars: &HashMap<String, String>) -> Option<String> {
    let mut out = String::with_capacity(value.len());
    let mut rest = value;
    while let Some(i) = rest.find('$') {
        out.push_str(&rest[..i]);
        let after = &rest[i + 1..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        out.push_str(vars.get(&after[..end])?);
        if out.len() > MAX_VALUE_LEN {
            return None;
        }
        rest = &after[end..];
    }
    out.push_str(rest);
    (out.len() <= MAX_VALUE_LEN).then_some(out)
}

/// Parses the contents of a `hyprtoolkit.conf`. Unknown keys, lines without
/// `=` and bad values are skipped and recorded in [`HyprToolkit::ignored`].
pub fn parse_hyprtoolkit(contents: &str) -> HyprToolkit {
    let mut out = HyprToolkit::default();
    let mut vars: HashMap<String, String> = HashMap::new();
    let mut vars_total = 0usize;
    for raw in contents.lines() {
        let line = strip_comment(raw);
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            out.ignored.push(line.to_string());
            continue;
        };
        let (key, value) = (key.trim(), value.trim());
        let Some(substituted) = substitute(value, &vars) else {
            out.ignored.push(format!("{key} = {value}"));
            continue;
        };
        if let Some(name) = key.strip_prefix('$') {
            let old = vars.get(name).map_or(0, String::len);
            if vars_total - old + substituted.len() > MAX_VARS_TOTAL {
                out.ignored.push(format!("{key} = {value}"));
                continue;
            }
            vars_total = vars_total - old + substituted.len();
            vars.insert(name.to_string(), substituted);
            continue;
        }
        let value = substituted.as_str();
        let bad = |out: &mut HyprToolkit| out.ignored.push(format!("{key} = {value}"));
        let colour = |v: &str| Rgba::parse(v);
        match key {
            "background" => set(
                &mut out.background,
                colour(value),
                &mut out.ignored,
                key,
                value,
            ),
            "base" => set(&mut out.base, colour(value), &mut out.ignored, key, value),
            "alternate_base" => set(
                &mut out.alternate_base,
                colour(value),
                &mut out.ignored,
                key,
                value,
            ),
            "text" => set(&mut out.text, colour(value), &mut out.ignored, key, value),
            "link_text" => set(
                &mut out.link_text,
                colour(value),
                &mut out.ignored,
                key,
                value,
            ),
            "accent" => set(&mut out.accent, colour(value), &mut out.ignored, key, value),
            "accent_secondary" => set(
                &mut out.accent_secondary,
                colour(value),
                &mut out.ignored,
                key,
                value,
            ),
            "font_family" | "font_family_monospace" => {
                if value.is_empty() {
                    bad(&mut out);
                } else if key == "font_family" {
                    out.font_family = Some(value.to_string());
                } else {
                    out.font_family_monospace = Some(value.to_string());
                }
            }
            "rounding_small" | "rounding_large" => match value.parse::<u32>() {
                Ok(n) if key == "rounding_small" => out.rounding_small = Some(n),
                Ok(n) => out.rounding_large = Some(n),
                Err(_) => bad(&mut out),
            },
            k if UNMAPPED_KEYS.contains(&k) => {}
            _ => bad(&mut out),
        }
    }
    out
}

fn set(
    slot: &mut Option<Rgba>,
    parsed: Option<Rgba>,
    ignored: &mut Vec<String>,
    key: &str,
    value: &str,
) {
    match parsed {
        Some(c) => *slot = Some(c),
        None => ignored.push(format!("{key} = {value}")),
    }
}
