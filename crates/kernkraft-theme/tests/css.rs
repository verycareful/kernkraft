// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use kernkraft_theme::{resolve, to_css, Follow, HyprToolkit, Sources, UserChoice};

fn tokens(font: &str) -> kernkraft_theme::Tokens {
    let src = Sources {
        hypr: Some(HyprToolkit {
            font_family: Some(font.into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    resolve(&src, &Follow::default(), &UserChoice::default(), &|_| true)
}

#[test]
fn css_is_one_root_block() {
    let css = to_css(&tokens("Oxanium"));
    assert!(css.starts_with(":root{\n"));
    assert!(css.ends_with("}\n"));
    assert_eq!(css.matches(':').count() - css.matches("--kern-").count(), 1);
    assert_eq!(css.matches('{').count(), 1);
    assert_eq!(css.matches('}').count(), 1);
    for line in css.lines().skip(1).take_while(|l| *l != "}") {
        assert!(line.starts_with("--kern-") && line.ends_with(';'), "{line}");
    }
}

#[test]
fn css_values() {
    let css = to_css(&tokens("Oxanium"));
    assert!(css.contains("--kern-accent:#3f5b8bff;"));
    assert!(css.contains("--kern-bg:#ffffffff;"));
    assert!(css.contains("--kern-font-ui:\"Oxanium\",system-ui,sans-serif;"));
    assert!(css.contains("--kern-font-mono:\"IBM Plex Mono\",ui-monospace,monospace;"));
    assert!(css.contains("--kern-radius-small:6px;"));
    assert!(css.contains("--kern-density-row:70px;"));
    assert!(css.contains("--kern-scheme:light;"));
}

#[test]
fn no_format_hues_in_css() {
    let css = to_css(&tokens("Oxanium"));
    assert!(!css.contains("--kern-format-"));
    for hue in kernkraft_theme::kern_tokens::FORMAT_HUES {
        assert!(!css.contains(&hue.light.to_hex()[..7]), "{}", hue.label);
        assert!(!css.contains(&hue.dark.to_hex()[..7]), "{}", hue.label);
    }
}

#[test]
fn hostile_font_name_cannot_leave_the_string() {
    let css = to_css(&tokens("A\"; } body { x: y\\\n"));
    assert!(
        css.contains("--kern-font-ui:\"A\\\"; } body { x: y\\\\\",system-ui"),
        "{css}"
    );
}
