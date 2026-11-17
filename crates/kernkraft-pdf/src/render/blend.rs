// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Blend modes: ISO 32000-1:2008, 11.3.5 (Table 136, the separable and the
//! non-separable modes).

use tiny_skia::BlendMode;

use crate::syntax::Name;

/// The rasteriser's blend mode for a PDF one. `Compatible` is `Normal`
/// (11.3.5), and a name that is not in the table is `Normal` too, which is
/// what 11.6.3 says to do with a mode a reader does not know.
pub(super) fn blend_mode(name: &Name) -> BlendMode {
    match name.as_bytes() {
        b"Multiply" => BlendMode::Multiply,
        b"Screen" => BlendMode::Screen,
        b"Overlay" => BlendMode::Overlay,
        b"Darken" => BlendMode::Darken,
        b"Lighten" => BlendMode::Lighten,
        b"ColorDodge" => BlendMode::ColorDodge,
        b"ColorBurn" => BlendMode::ColorBurn,
        b"HardLight" => BlendMode::HardLight,
        b"SoftLight" => BlendMode::SoftLight,
        b"Difference" => BlendMode::Difference,
        b"Exclusion" => BlendMode::Exclusion,
        b"Hue" => BlendMode::Hue,
        b"Saturation" => BlendMode::Saturation,
        b"Color" => BlendMode::Color,
        b"Luminosity" => BlendMode::Luminosity,
        _ => BlendMode::SourceOver,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_mode_of_the_table_has_a_rasteriser_mode_and_unknown_names_are_normal() {
        let table: [(&[u8], BlendMode); 15] = [
            (b"Multiply", BlendMode::Multiply),
            (b"Screen", BlendMode::Screen),
            (b"Overlay", BlendMode::Overlay),
            (b"Darken", BlendMode::Darken),
            (b"Lighten", BlendMode::Lighten),
            (b"ColorDodge", BlendMode::ColorDodge),
            (b"ColorBurn", BlendMode::ColorBurn),
            (b"HardLight", BlendMode::HardLight),
            (b"SoftLight", BlendMode::SoftLight),
            (b"Difference", BlendMode::Difference),
            (b"Exclusion", BlendMode::Exclusion),
            (b"Hue", BlendMode::Hue),
            (b"Saturation", BlendMode::Saturation),
            (b"Color", BlendMode::Color),
            (b"Luminosity", BlendMode::Luminosity),
        ];
        for (name, mode) in table {
            assert_eq!(blend_mode(&Name::new(name.to_vec())), mode);
        }
        for normal in [&b"Normal"[..], b"Compatible", b"Unheard"] {
            assert_eq!(
                blend_mode(&Name::new(normal.to_vec())),
                BlendMode::SourceOver
            );
        }
    }
}
