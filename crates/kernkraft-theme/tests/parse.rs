// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use kernkraft_theme::{
    contrast_ratio, fc_default, fc_resolves, parse_gtk, parse_hyprtoolkit, read_gtk,
    read_hyprtoolkit, Rgba, WCAG_AA_BODY,
};

/// A real `hyprtoolkit.conf` as written on a Hyprland desktop.
const MAINTAINER_HYPRTOOLKIT: &str = "\
# hyprtoolkit: the toolkit hyprpolkitagent (the password prompt) and the other hypr* GUIs draw with.
# It ignores qt6ct and GTK, so it needs its own colours.
background = rgb(000000)
base = rgb(111111)
alternate_base = rgb(070707)
text = rgb(e4e8f0)
bright_text = rgb(e4e8f0)
link_text = rgb(6e8ff0)
accent = rgb(0660c6)
accent_secondary = rgb(a3abb5)

font_family = Oxanium
font_family_monospace = JetBrainsMono Nerd Font

rounding_large = 0
rounding_small = 0
";

fn c(r: u8, g: u8, b: u8) -> Option<Rgba> {
    Some(Rgba::rgb(r, g, b))
}

#[test]
fn parses_maintainers_hyprtoolkit() {
    let h = parse_hyprtoolkit(MAINTAINER_HYPRTOOLKIT);
    assert_eq!(h.background, c(0x00, 0x00, 0x00));
    assert_eq!(h.base, c(0x11, 0x11, 0x11));
    assert_eq!(h.alternate_base, c(0x07, 0x07, 0x07));
    assert_eq!(h.text, c(0xe4, 0xe8, 0xf0));
    assert_eq!(h.link_text, c(0x6e, 0x8f, 0xf0));
    assert_eq!(h.accent, c(0x06, 0x60, 0xc6));
    assert_eq!(h.accent_secondary, c(0xa3, 0xab, 0xb5));
    assert_eq!(h.font_family.as_deref(), Some("Oxanium"));
    assert_eq!(
        h.font_family_monospace.as_deref(),
        Some("JetBrainsMono Nerd Font")
    );
    assert_eq!(h.rounding_small, Some(0));
    assert_eq!(h.rounding_large, Some(0));
    assert!(h.ignored.is_empty(), "{:?}", h.ignored);
}

#[test]
fn bad_value_ignored_and_listed() {
    let h = parse_hyprtoolkit(
        "accent = banana\nunknown_key = 1\nrounding_small = -3\ntext = rgb(102030)\nnot a pair\n",
    );
    assert_eq!(h.accent, None);
    assert_eq!(h.rounding_small, None);
    assert_eq!(h.text, c(0x10, 0x20, 0x30));
    assert_eq!(
        h.ignored,
        vec![
            "accent = banana",
            "unknown_key = 1",
            "rounding_small = -3",
            "not a pair"
        ]
    );
}

#[test]
fn hyprlang_comments() {
    let h = parse_hyprtoolkit(
        "accent = rgb(010203) # trailing\n# text = rgb(ffffff)\nfont_family = A##B\n",
    );
    assert_eq!(h.accent, c(1, 2, 3));
    assert_eq!(h.text, None);
    assert_eq!(h.font_family.as_deref(), Some("A#B"));
    assert!(h.ignored.is_empty());
}

#[test]
fn colour_forms() {
    assert_eq!(Rgba::parse("rgb(ff8000)"), c(255, 128, 0));
    assert_eq!(
        Rgba::parse("rgba(ff800040)"),
        Some(Rgba {
            r: 255,
            g: 128,
            b: 0,
            a: 0x40
        })
    );
    assert_eq!(Rgba::parse("#FF8000"), c(255, 128, 0));
    assert_eq!(
        Rgba::parse("#ff800040"),
        Some(Rgba {
            r: 255,
            g: 128,
            b: 0,
            a: 0x40
        })
    );
    for bad in [
        "",
        "ff8000",
        "#ff80",
        "rgb(ff80)",
        "rgb(gg0000)",
        "rgba(ff8000)",
        "#é12345",
    ] {
        assert_eq!(Rgba::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn rgba_serialises_as_hex_string() {
    let v = Rgba {
        r: 1,
        g: 2,
        b: 0xab,
        a: 0xcd,
    };
    let json = serde_json::to_string(&v).unwrap();
    assert_eq!(json, "\"#0102abcd\"");
    assert_eq!(serde_json::from_str::<Rgba>(&json).unwrap(), v);
    assert!(serde_json::from_str::<Rgba>("\"nope\"").is_err());
}

#[test]
fn contrast_matches_wcag_extremes() {
    let black = Rgba::rgb(0, 0, 0);
    let white = Rgba::rgb(255, 255, 255);
    assert!((contrast_ratio(black, white) - 21.0).abs() < 1e-9);
    assert!((contrast_ratio(white, black) - 21.0).abs() < 1e-9);
    assert!((contrast_ratio(white, white) - 1.0).abs() < 1e-9);
    // #767676 on white is the well-known smallest grey that passes 4.5:1.
    let grey = Rgba::rgb(0x76, 0x76, 0x76);
    assert!(contrast_ratio(grey, white) >= WCAG_AA_BODY);
    assert!(contrast_ratio(Rgba::rgb(0x77, 0x77, 0x77), white) < WCAG_AA_BODY);
}

#[test]
fn gtk_font_name_strips_size() {
    let g = parse_gtk(Some("[Settings]\ngtk-font-name=Inter 11\n"), None);
    assert_eq!(g.font_name.as_deref(), Some("Inter"));
}

#[test]
fn gtk_font_name_strips_style_words() {
    let g = parse_gtk(Some("[Settings]\ngtk-font-name=Oxanium Medium 10\n"), None);
    assert_eq!(g.font_name.as_deref(), Some("Oxanium"));
    let g = parse_gtk(Some("[Settings]\ngtk-font-name=Noto Sans\n"), None);
    assert_eq!(g.font_name.as_deref(), Some("Noto Sans"));
    let g = parse_gtk(Some("[Settings]\ngtk-font-name=Bold 12\n"), None);
    assert_eq!(g.font_name.as_deref(), Some("Bold"));
}

#[test]
fn gtk_prefer_dark_and_sections() {
    let ini = "[Other]\ngtk-application-prefer-dark-theme=1\n[Settings]\ngtk-application-prefer-dark-theme=0\n";
    assert_eq!(parse_gtk(Some(ini), None).prefer_dark, Some(false));
    let ini = "[Settings]\ngtk-application-prefer-dark-theme=true\n";
    assert_eq!(parse_gtk(Some(ini), None).prefer_dark, Some(true));
    assert_eq!(parse_gtk(Some("[Settings]\n"), None).prefer_dark, None);
}

#[test]
fn gtk_css_accent() {
    let css = "/* @define-color accent_color #ff0000; */\n@define-color accent_bg_color #111111;\n@define-color accent_color     #0660c6;\n";
    assert_eq!(parse_gtk(None, Some(css)).accent, c(0x06, 0x60, 0xc6));
    assert_eq!(
        parse_gtk(None, Some("@define-color accent_color red;")).accent,
        None
    );
}

#[test]
fn read_sources_from_files() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(read_gtk(dir.path()).unwrap(), None);
    assert_eq!(
        read_hyprtoolkit(&dir.path().join("none.conf")).unwrap(),
        None
    );
    std::fs::write(
        dir.path().join("settings.ini"),
        "[Settings]\ngtk-font-name=Inter 11\n",
    )
    .unwrap();
    let g = read_gtk(dir.path()).unwrap().unwrap();
    assert_eq!(g.font_name.as_deref(), Some("Inter"));
    let p = dir.path().join("hyprtoolkit.conf");
    std::fs::write(&p, b"accent = rgb(010203)\nfont_family = Caf\xe9\n").unwrap();
    let h = read_hyprtoolkit(&p).unwrap().unwrap();
    assert_eq!(h.accent, c(1, 2, 3));
    assert_eq!(h.font_family.as_deref(), Some("Caf\u{fffd}"));
}

fn have_fc_match() -> bool {
    std::process::Command::new("fc-match")
        .arg("--version")
        .output()
        .is_ok()
}

#[test]
fn fc_match_answers() {
    if !have_fc_match() {
        assert!(!fc_resolves("Anything"));
        assert_eq!(fc_default("monospace"), None);
        return;
    }
    assert!(!fc_resolves("No Such Family Qzxv-9"));
    assert!(!fc_resolves(""));
    let mono = fc_default("monospace").expect("fontconfig names a monospace default");
    assert!(fc_resolves(&mono), "{mono}");
    assert!(fc_resolves(&mono.to_uppercase()), "case-insensitive");
}
