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
fn hyprlang_variables() {
    let h = parse_hyprtoolkit(
        "$main = rgb(010203)\n$font = Fira Sans\naccent = $main\ntext = $missing\nfont_family = $font $main\nbackground = $main$main\n",
    );
    assert_eq!(h.accent, c(1, 2, 3));
    assert_eq!(h.text, None);
    assert_eq!(h.font_family.as_deref(), Some("Fira Sans rgb(010203)"));
    assert_eq!(h.background, None);
    assert_eq!(
        h.ignored,
        vec!["text = $missing", "background = rgb(010203)rgb(010203)"]
    );
}

#[test]
fn variable_doubling_is_bounded() {
    let mut conf = String::from("$v0 = 0123456789abcdef\n");
    for i in 1..64 {
        conf.push_str(&format!("$v{i} = $v{}$v{}\n", i - 1, i - 1));
    }
    conf.push_str("accent = $v63\nfont_family = $v1\n");
    let start = std::time::Instant::now();
    let h = parse_hyprtoolkit(&conf);
    assert!(start.elapsed().as_secs() < 5);
    // 16 bytes doubles past the 4 KiB cap at $v9 (16 * 2^9 = 8192); the
    // lines from there on are ignored and their variables stay undefined.
    assert_eq!(
        h.font_family.as_deref(),
        Some("0123456789abcdef0123456789abcdef")
    );
    assert_eq!(h.accent, None);
    assert!(h.ignored.contains(&"$v9 = $v8$v8".to_string()));
    assert!(h.ignored.contains(&"$v63 = $v62$v62".to_string()));
    assert!(h.ignored.contains(&"accent = $v63".to_string()));
    assert!(!h.ignored.iter().any(|l| l.starts_with("$v8 ")));
}

#[test]
fn variable_total_cap_reported() {
    // Each definition holds 4000 bytes, under the 4 KiB value cap, so only
    // the 1 MiB total cap can refuse one: 1 MiB / 4000 is 262.1, so
    // $v0..$v261 fit and $v262 onwards do not.
    let value = "x".repeat(4000);
    let mut conf = String::new();
    for i in 0..300 {
        conf.push_str(&format!("$v{i} = {value}\n"));
    }
    conf.push_str("font_family = $v261\nfont_family_monospace = $v262\n");
    let h = parse_hyprtoolkit(&conf);
    assert_eq!(h.font_family.as_deref(), Some(value.as_str()));
    assert_eq!(h.font_family_monospace, None);
    for i in 262..300 {
        assert!(h.ignored.contains(&format!("$v{i} = {value}")), "v{i}");
    }
    assert!(!h.ignored.contains(&format!("$v261 = {value}")));
    assert!(h
        .ignored
        .contains(&"font_family_monospace = $v262".to_string()));
}

#[test]
fn oversize_theme_file_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("hyprtoolkit.conf");
    std::fs::write(&p, vec![b'#'; 1 << 20]).unwrap();
    assert!(read_hyprtoolkit(&p).unwrap().is_some());
    std::fs::write(&p, vec![b'#'; (1 << 20) + 1]).unwrap();
    assert!(matches!(
        read_hyprtoolkit(&p),
        Err(kernkraft_theme::ThemeError::TooLarge { .. })
    ));
    std::fs::write(dir.path().join("gtk.css"), vec![b' '; (1 << 20) + 1]).unwrap();
    assert!(read_gtk(dir.path()).is_err());
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

/// A `hyprtoolkit.conf` written with Hyprland's `0xAARRGGBB` notation, plus
/// the keys hyprtoolkit defines beyond the ones Kern maps.
const ARGB_HYPRTOOLKIT: &str = "\
# Colours in 0xAARRGGBB, the notation Hyprland's own files use.
background = 0xFF102030
base = 0xFF203040
alternate_base = 0xFF304050
text = 0xFFe0e0d0
bright_text = 0xFFffffff
accent = 0xFF8090f0
accent_secondary = 0xFFf07090

font_family = Example Sans
font_family_monospace = Example Mono
font_size = 11

icon_theme = example-icons
";

#[test]
fn parses_argb_hyprtoolkit() {
    let h = parse_hyprtoolkit(ARGB_HYPRTOOLKIT);
    assert_eq!(h.background, c(0x10, 0x20, 0x30));
    assert_eq!(h.base, c(0x20, 0x30, 0x40));
    assert_eq!(h.alternate_base, c(0x30, 0x40, 0x50));
    assert_eq!(h.text, c(0xe0, 0xe0, 0xd0));
    assert_eq!(h.accent, c(0x80, 0x90, 0xf0));
    assert_eq!(h.accent_secondary, c(0xf0, 0x70, 0x90));
    assert_eq!(h.font_family.as_deref(), Some("Example Sans"));
    assert_eq!(h.font_family_monospace.as_deref(), Some("Example Mono"));
}

/// Hyprland's legacy notation puts alpha first: `0xAARRGGBB`.
#[test]
fn argb_colour_has_alpha_first() {
    let want = Some(Rgba {
        r: 0xff,
        g: 0x80,
        b: 0x00,
        a: 0x40,
    });
    assert_eq!(Rgba::parse("0x40ff8000"), want);
    assert_eq!(Rgba::parse("0X40FF8000"), want);
    assert_eq!(Rgba::parse("0xff112233"), c(0x11, 0x22, 0x33));
}

/// Hyprland's decimal forms: channels 0 to 255, alpha a fraction 0.0 to 1.0.
#[test]
fn decimal_colour_forms() {
    assert_eq!(Rgba::parse("rgb(255,128,0)"), c(255, 128, 0));
    assert_eq!(Rgba::parse("rgb( 255 , 128 , 0 )"), c(255, 128, 0));
    assert_eq!(Rgba::parse("RGB(0,0,0)"), c(0, 0, 0));
    let with_alpha = |a: u8| {
        Some(Rgba {
            r: 179,
            g: 255,
            b: 26,
            a,
        })
    };
    assert_eq!(Rgba::parse("rgba(179,255,26,1)"), with_alpha(255));
    assert_eq!(Rgba::parse("rgba(179,255,26,1.0)"), with_alpha(255));
    assert_eq!(Rgba::parse("rgba(179,255,26,0)"), with_alpha(0));
    assert_eq!(Rgba::parse("rgba(179,255,26,0.5)"), with_alpha(128));
    assert_eq!(Rgba::parse("rgba(179,255,26,.5)"), with_alpha(128));
    assert_eq!(Rgba::parse("rgba(179,255,26,0.933)"), with_alpha(238));
    assert_eq!(Rgba::parse("rgba( 179, 255, 26, 0.5 )"), with_alpha(128));
}

#[test]
fn malformed_colours_are_refused() {
    for bad in [
        "0x",
        "0xff8000",
        "0x40ff80000",
        "0xgg000000",
        "0x-1000000",
        "40ff8000",
        "rgb(256,0,0)",
        "rgb(-1,0,0)",
        "rgb(1.5,0,0)",
        "rgb(1,2)",
        "rgb(1,2,3,4)",
        "rgb(1,2,,3)",
        "rgb(1,2,3,)",
        "rgba(1,2,3)",
        "rgba(1,2,3,4,5)",
        "rgba(1,2,3,1.5)",
        "rgba(1,2,3,-0.1)",
        "rgba(1,2,3,nan)",
        "rgba(1,2,3,inf)",
        "rgba(1,2,3,)",
        "rgba(256,2,3,1)",
    ] {
        assert_eq!(Rgba::parse(bad), None, "{bad:?}");
    }
}

#[test]
fn every_notation_in_one_file() {
    let h = parse_hyprtoolkit(
        "background = rgb(010203)\n\
         base = rgba(01020380)\n\
         alternate_base = 0x80010203\n\
         text = rgb(1,2,3)\n\
         accent = rgba(1,2,3,0.5) # trailing\n",
    );
    let half = |r, g, b| Some(Rgba { r, g, b, a: 0x80 });
    assert_eq!(h.background, c(1, 2, 3));
    assert_eq!(h.base, half(1, 2, 3));
    assert_eq!(h.alternate_base, half(1, 2, 3));
    assert_eq!(h.text, c(1, 2, 3));
    assert_eq!(h.accent, half(1, 2, 3));
    assert!(h.ignored.is_empty(), "{:?}", h.ignored);
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
    let g = parse_gtk(Some("[Settings]\ngtk-font-name=Times New Roman 12\n"), None);
    assert_eq!(g.font_name.as_deref(), Some("Times New Roman"));
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
