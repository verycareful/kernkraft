// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tokens as CSS custom properties.

use std::fmt::Write;

use crate::kern_tokens::Scheme;
use crate::resolve::Tokens;

/// Quotes a family name as a CSS string: control characters are dropped and
/// `"`, `\` and `<` are escaped, so a hostile name cannot end the string.
fn quote(family: &str) -> String {
    let mut out = String::with_capacity(family.len() + 2);
    out.push('"');
    for c in family.chars().filter(|c| !c.is_control()) {
        match c {
            '"' | '\\' => {
                out.push('\\');
                out.push(c);
            }
            // `<` is escaped so the CSS stays inert if a consumer inlines it
            // into an HTML style element (`</style>`); the trailing space is
            // the escape's terminator.
            '<' => out.push_str("\\3c "),
            _ => out.push(c),
        }
    }
    out.push('"');
    out
}

/// One `:root{...}` block of `--kern-<name>` custom properties. Colours are
/// `#rrggbbaa`, fonts are quoted with a generic fallback, lengths are `px`.
pub fn to_css(t: &Tokens) -> String {
    let m = t.density.metrics();
    let mut s = String::from(":root{\n");
    let mut prop = |name: &str, value: String| {
        let _ = writeln!(s, "--kern-{name}:{value};");
    };
    prop(
        "scheme",
        match t.scheme {
            Scheme::Light => "light",
            Scheme::Dark => "dark",
        }
        .to_string(),
    );
    for (name, c) in [
        ("bg", t.bg),
        ("surface", t.surface),
        ("sunken", t.sunken),
        ("text", t.text),
        ("text-dim", t.text_dim),
        ("border", t.border),
        ("link", t.link),
        ("accent", t.accent),
        ("accent-soft", t.accent_soft),
        ("on-accent", t.on_accent),
        ("accent-secondary", t.accent_secondary),
    ] {
        prop(name, c.to_hex());
    }
    prop(
        "font-ui",
        format!("{},system-ui,sans-serif", quote(&t.font_ui)),
    );
    prop(
        "font-mono",
        format!("{},ui-monospace,monospace", quote(&t.font_mono)),
    );
    prop("radius-small", format!("{}px", t.radius_small));
    prop("radius-large", format!("{}px", t.radius_large));
    prop("density-row", format!("{}px", m.row));
    prop("density-pad", format!("{}px", m.pad));
    prop("density-gap", format!("{}px", m.gap));
    prop("density-tile", format!("{}px", m.tile));
    s.push_str("}\n");
    s
}
