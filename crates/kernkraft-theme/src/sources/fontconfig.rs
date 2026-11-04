// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! fontconfig defaults (parent spec section 6.5, source 4), read by running
//! `fc-match`. A missing `fc-match`, a failed run or empty output never
//! panics: [`fc_resolves`] answers `false` and [`fc_default`] answers `None`.

use std::process::Command;

use serde::{Deserialize, Serialize};

/// The families fontconfig picks for the generic names, used for any family
/// the higher sources left unset.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fontconfig {
    /// Default for `system-ui`.
    pub ui: Option<String>,
    /// Default for `monospace`.
    pub mono: Option<String>,
}

impl Fontconfig {
    /// Asks fontconfig for both defaults.
    pub fn detect() -> Fontconfig {
        Fontconfig {
            ui: fc_default("system-ui"),
            mono: fc_default("monospace"),
        }
    }
}

/// Runs `fc-match --format=%{family} <pattern>` and returns its stdout.
fn fc_match(pattern: &str) -> Option<String> {
    if pattern.is_empty() || pattern.contains('\0') {
        return None;
    }
    let out = Command::new("fc-match")
        .arg("--format=%{family}")
        .arg("--")
        .arg(pattern)
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Escapes the characters fontconfig's pattern syntax treats specially
/// (`\`, `-`, `:`, `,`), so a family name is matched as a family name.
fn escape_pattern(family: &str) -> String {
    let mut out = String::with_capacity(family.len());
    for c in family.chars() {
        if matches!(c, '\\' | '-' | ':' | ',') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// Whether fontconfig resolves `family` to itself. `fc-match` always returns
/// some font, so a family counts as resolved only when the family it names
/// (a comma-separated list of its localised names) contains the request,
/// compared case-insensitively. `false` when `fc-match` is unavailable.
pub fn fc_resolves(family: &str) -> bool {
    let family = family.trim();
    let Some(answer) = fc_match(&escape_pattern(family)) else {
        return false;
    };
    answer
        .split(',')
        .any(|name| name.trim().eq_ignore_ascii_case(family))
}

/// The first family fontconfig names for a generic such as `system-ui` or
/// `monospace`. `None` when `fc-match` is unavailable or names nothing.
pub fn fc_default(generic: &str) -> Option<String> {
    let answer = fc_match(&escape_pattern(generic.trim()))?;
    let first = answer.split(',').next()?.trim();
    (!first.is_empty()).then(|| first.to_string())
}
