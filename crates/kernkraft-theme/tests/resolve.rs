// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

use kernkraft_theme::kern_tokens::{
    self as kern, Density, KernAccent, FORMAT_HUES, ON_ACCENT_DARK, ON_ACCENT_LIGHT,
};
use kernkraft_theme::{
    contrast_ratio, resolve, Follow, Fontconfig, Gtk, HyprToolkit, Mode, Origin, Portal, Rgba,
    Scheme, Sources, Tokens, UserChoice, WCAG_AA_BODY,
};

fn all_fonts(_: &str) -> bool {
    true
}

fn rgb(v: u32) -> Rgba {
    Rgba::rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn run(src: &Sources) -> Tokens {
    resolve(src, &Follow::default(), &UserChoice::default(), &all_fonts)
}

#[test]
fn hyprland_beats_gtk_accent() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            accent: Some(rgb(0x0660c6)),
            ..Default::default()
        }),
        portal: Some(Portal {
            accent: Some(rgb(0x00ff00)),
            ..Default::default()
        }),
        gtk: Some(Gtk {
            accent: Some(rgb(0xff0000)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let t = run(&src);
    assert_eq!(t.accent, rgb(0x0660c6));
    assert_eq!(t.origins["accent"], Origin::Hyprland);

    let no_hypr = Sources {
        hypr: None,
        ..src.clone()
    };
    let t = run(&no_hypr);
    assert_eq!(
        (t.accent, t.origins["accent"]),
        (rgb(0x00ff00), Origin::Portal)
    );

    let only_gtk = Sources {
        portal: None,
        ..no_hypr
    };
    let t = run(&only_gtk);
    assert_eq!(
        (t.accent, t.origins["accent"]),
        (rgb(0xff0000), Origin::Gtk)
    );
}

#[test]
fn follow_accent_off_uses_user_pick() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            accent: Some(rgb(0x0660c6)),
            accent_secondary: Some(rgb(0xa3abb5)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let follow = Follow {
        accent: false,
        ..Follow::default()
    };
    let user = UserChoice {
        accent: KernAccent::Teal,
        ..UserChoice::default()
    };
    let t = resolve(&src, &follow, &user, &all_fonts);
    assert_eq!(t.accent, KernAccent::Teal.rgba());
    assert_eq!(t.origins["accent"], Origin::User);
    assert_eq!(t.origins["accent_secondary"], Origin::Kern);
}

#[test]
fn defaults_are_kern_slate() {
    let t = run(&Sources::default());
    assert_eq!(t.accent, rgb(0x3F5B8B));
    assert_eq!(t.scheme, Scheme::Light);
    assert_eq!((t.bg, t.text), (kern::LIGHT.bg, kern::LIGHT.text));
    assert_eq!(
        (t.text_dim, t.border),
        (kern::LIGHT.text_dim, kern::LIGHT.border)
    );
    assert_eq!((t.radius_small, t.radius_large), (6, 12));
    assert_eq!(
        (t.font_ui.as_str(), t.font_mono.as_str()),
        ("Outfit", "IBM Plex Mono")
    );
    assert_eq!(t.density, Density::Cozy);
    assert!(t.wcag_fallbacks.is_empty());
    assert!(t
        .origins
        .values()
        .all(|o| *o == Origin::Kern || *o == Origin::User));
}

#[test]
fn pinned_mode_uses_kern_neutrals() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            background: Some(rgb(0x000000)),
            text: Some(rgb(0xe4e8f0)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let user = UserChoice {
        mode: Mode::Light,
        ..UserChoice::default()
    };
    let t = resolve(&src, &Follow::default(), &user, &all_fonts);
    assert_eq!((t.scheme, t.bg), (Scheme::Light, kern::LIGHT.bg));
    assert_eq!(t.origins["scheme"], Origin::User);
    let user = UserChoice {
        mode: Mode::Dark,
        ..UserChoice::default()
    };
    let t = resolve(&Sources::default(), &Follow::default(), &user, &all_fonts);
    assert_eq!(
        (t.scheme, t.bg, t.surface),
        (Scheme::Dark, kern::DARK.bg, kern::DARK.surface)
    );
}

#[test]
fn scheme_follows_hyprland_then_portal_then_gtk() {
    let hypr_dark = HyprToolkit {
        background: Some(rgb(0x070707)),
        ..Default::default()
    };
    let hypr_light = HyprToolkit {
        background: Some(rgb(0xf0f0f0)),
        ..Default::default()
    };
    let portal_dark = Portal {
        color_scheme: Some(Scheme::Dark),
        ..Default::default()
    };
    let gtk_dark = Gtk {
        prefer_dark: Some(true),
        ..Default::default()
    };

    let t = run(&Sources {
        hypr: Some(hypr_dark),
        portal: Some(Portal {
            color_scheme: Some(Scheme::Light),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert_eq!(
        (t.scheme, t.origins["scheme"]),
        (Scheme::Dark, Origin::Hyprland)
    );
    let t = run(&Sources {
        hypr: Some(hypr_light),
        portal: Some(portal_dark.clone()),
        ..Default::default()
    });
    assert_eq!(t.scheme, Scheme::Light);
    let t = run(&Sources {
        portal: Some(portal_dark),
        gtk: Some(Gtk {
            prefer_dark: Some(false),
            ..Default::default()
        }),
        ..Default::default()
    });
    assert_eq!(
        (t.scheme, t.origins["scheme"]),
        (Scheme::Dark, Origin::Portal)
    );
    let t = run(&Sources {
        gtk: Some(gtk_dark),
        ..Default::default()
    });
    assert_eq!((t.scheme, t.origins["scheme"]), (Scheme::Dark, Origin::Gtk));
}

#[test]
fn hyprland_palette_maps_onto_tokens() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            background: Some(rgb(0x000000)),
            base: Some(rgb(0x111111)),
            alternate_base: Some(rgb(0x070707)),
            text: Some(rgb(0xe4e8f0)),
            link_text: Some(rgb(0x6e8ff0)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let t = run(&src);
    assert_eq!(t.scheme, Scheme::Dark);
    assert_eq!(
        (t.bg, t.surface, t.sunken, t.text, t.link),
        (
            rgb(0),
            rgb(0x111111),
            rgb(0x070707),
            rgb(0xe4e8f0),
            rgb(0x6e8ff0)
        )
    );
    // Dim text and border sit between bg and text at Kern's ratios.
    assert_eq!(t.text_dim, t.bg.mix(t.text, kern::TEXT_DIM_RATIO));
    assert_eq!(t.border, t.bg.mix(t.text, kern::BORDER_RATIO));
    assert!(contrast_ratio(t.text_dim, t.bg) < contrast_ratio(t.text, t.bg));
    assert!(contrast_ratio(t.border, t.bg) < contrast_ratio(t.text_dim, t.bg));
    assert_eq!(t.origins["text_dim"], Origin::Hyprland);
}

#[test]
fn kern_ratios_reproduce_the_handoff() {
    // One mean ratio per token fits six hand-picked channel values, so the
    // interpolation lands near the handoff, not on it (the handoff tints its
    // neutrals blue, an interpolation towards near-black text does not).
    for k in [&kern::LIGHT, &kern::DARK] {
        let dim = k.bg.mix(k.text, kern::TEXT_DIM_RATIO);
        let border = k.bg.mix(k.text, kern::BORDER_RATIO);
        for (got, want) in [(dim, k.text_dim), (border, k.border)] {
            for (g, w) in [(got.r, want.r), (got.g, want.g), (got.b, want.b)] {
                assert!(g.abs_diff(w) <= 13, "{got:?} vs {want:?}");
            }
        }
    }
}

#[test]
fn low_contrast_falls_back() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            background: Some(rgb(0x777777)),
            base: Some(rgb(0x808080)),
            text: Some(rgb(0x888888)),
            ..Default::default()
        }),
        ..Default::default()
    };
    let t = run(&src);
    let k = kern::neutrals(t.scheme);
    assert_eq!((t.bg, t.text), (k.bg, k.text));
    assert_eq!(t.surface, k.surface);
    assert_eq!(t.wcag_fallbacks, vec!["bg", "text"]);
    assert_eq!(t.origins["bg"], Origin::Kern);
    assert!(contrast_ratio(t.text, t.bg) >= WCAG_AA_BODY);
}

#[test]
fn passing_contrast_is_kept() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            background: Some(rgb(0x000000)),
            text: Some(rgb(0xe4e8f0)),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(run(&src).wcag_fallbacks.is_empty());
}

#[test]
fn on_accent_max_contrast() {
    for hex in [
        0x000000, 0xffffff, 0x0660c6, 0xf2c14e, 0x3F5B8B, 0xC9881F, 0x777777,
    ] {
        let src = Sources {
            hypr: Some(HyprToolkit {
                accent: Some(rgb(hex)),
                ..Default::default()
            }),
            ..Default::default()
        };
        let t = run(&src);
        let best =
            contrast_ratio(ON_ACCENT_LIGHT, t.accent).max(contrast_ratio(ON_ACCENT_DARK, t.accent));
        assert!(
            (contrast_ratio(t.on_accent, t.accent) - best).abs() < 1e-12,
            "{hex:06x}"
        );
    }
    let on = |hex| {
        run(&Sources {
            hypr: Some(HyprToolkit {
                accent: Some(rgb(hex)),
                ..Default::default()
            }),
            ..Default::default()
        })
        .on_accent
    };
    assert_eq!(on(0x000000), ON_ACCENT_LIGHT);
    assert_eq!(on(0xffffff), ON_ACCENT_DARK);
}

#[test]
fn accent_soft_alpha_per_scheme() {
    let accent = Some(rgb(0x0660c6));
    let hypr = |bg| HyprToolkit {
        background: Some(rgb(bg)),
        accent,
        ..Default::default()
    };
    let dark = run(&Sources {
        hypr: Some(hypr(0x000000)),
        ..Default::default()
    });
    assert_eq!(
        (dark.accent_soft.a, dark.accent_soft.r),
        ((0.20f64 * 255.0).round() as u8, 0x06)
    );
    let light = run(&Sources {
        hypr: Some(hypr(0xffffff)),
        ..Default::default()
    });
    assert_eq!(light.accent_soft.a, (0.12f64 * 255.0).round() as u8);
}

fn fonts_src() -> Sources {
    Sources {
        hypr: Some(HyprToolkit {
            font_family: Some("Oxanium".into()),
            font_family_monospace: Some("JetBrainsMono Nerd Font".into()),
            ..Default::default()
        }),
        gtk: Some(Gtk {
            font_name: Some("Inter".into()),
            ..Default::default()
        }),
        fontconfig: Some(Fontconfig {
            ui: Some("DejaVu Sans".into()),
            mono: Some("DejaVu Sans Mono".into()),
        }),
        ..Default::default()
    }
}

#[test]
fn unresolvable_font_falls_down() {
    let src = fonts_src();
    let f = Follow::default();
    let u = UserChoice::default();
    let t = resolve(&src, &f, &u, &|n| n != "Oxanium");
    assert_eq!(
        (t.font_ui.as_str(), t.origins["font_ui"]),
        ("Inter", Origin::Gtk)
    );
    let t = resolve(&src, &f, &u, &|n| n != "Oxanium" && n != "Inter");
    assert_eq!(
        (t.font_ui.as_str(), t.origins["font_ui"]),
        ("DejaVu Sans", Origin::Fontconfig)
    );
    let t = resolve(&src, &f, &u, &|_| false);
    assert_eq!(
        (t.font_ui.as_str(), t.origins["font_ui"]),
        ("Outfit", Origin::Kern)
    );
    assert_eq!(t.font_mono, "IBM Plex Mono");
    let t = resolve(&src, &f, &u, &|n| n != "JetBrainsMono Nerd Font");
    assert_eq!(t.font_mono, "DejaVu Sans Mono");
    let t = resolve(&src, &f, &u, &all_fonts);
    assert_eq!(
        (t.font_ui.as_str(), t.origins["font_ui"]),
        ("Oxanium", Origin::Hyprland)
    );
}

#[test]
fn follow_fonts_off_uses_kern() {
    let f = Follow {
        fonts: false,
        ..Follow::default()
    };
    let t = resolve(&fonts_src(), &f, &UserChoice::default(), &all_fonts);
    assert_eq!(
        (t.font_ui.as_str(), t.font_mono.as_str()),
        ("Outfit", "IBM Plex Mono")
    );
}

#[test]
fn radius_zero_kept() {
    let src = Sources {
        hypr: Some(HyprToolkit {
            rounding_small: Some(0),
            rounding_large: Some(0),
            ..Default::default()
        }),
        ..Default::default()
    };
    let t = run(&src);
    assert_eq!((t.radius_small, t.radius_large), (0, 0));
    assert_eq!(t.origins["radius_small"], Origin::Hyprland);
    let f = Follow {
        radius: false,
        ..Follow::default()
    };
    let t = resolve(&src, &f, &UserChoice::default(), &all_fonts);
    assert_eq!((t.radius_small, t.radius_large), (6, 12));
}

#[test]
fn format_hues_not_in_tokens() {
    // Tokens has no format fields: this destructuring names every field and
    // fails to compile if one is added without a decision.
    let Tokens {
        scheme: _,
        bg: _,
        surface: _,
        sunken: _,
        text: _,
        text_dim: _,
        border: _,
        link: _,
        accent: _,
        accent_soft: _,
        on_accent: _,
        accent_secondary: _,
        font_ui: _,
        font_mono: _,
        radius_small: _,
        radius_large: _,
        density: _,
        origins: _,
        wcag_fallbacks: _,
    } = run(&Sources::default());
    assert_eq!(FORMAT_HUES.len(), 6);
}

#[test]
fn tokens_round_trip_through_json() {
    let t = run(&fonts_src());
    let json = serde_json::to_string(&t).unwrap();
    assert_eq!(serde_json::from_str::<Tokens>(&json).unwrap(), t);
}

#[test]
fn kern_palette_matches_handoff() {
    assert_eq!(KernAccent::ALL.len(), 12);
    assert_eq!(KernAccent::default(), KernAccent::Slate);
    assert_eq!(KernAccent::Slate.rgba(), rgb(0x3F5B8B));
    assert_eq!(KernAccent::Graphite.rgba(), rgb(0x52525B));
    assert_eq!(Density::Compact.metrics().row, 60);
    assert_eq!(Density::Cozy.metrics().tile, 44);
}
