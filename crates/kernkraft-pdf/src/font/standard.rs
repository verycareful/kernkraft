// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The standard 14 fonts: ISO 32000-1:2008, 9.6.2.2.
//!
//! A font dictionary that names one of them needs no `Widths` entry: its
//! glyph widths are those of Adobe's metrics files. The widely used names
//! of the fonts that were drawn to the same metrics (Arial for Helvetica,
//! Times New Roman for Times, Courier New for Courier) have their widths
//! too, since a file that omits `Widths` for them means those metrics.

use super::data::standard14::{StandardMetrics, STANDARD_14};

/// The length of the prefix a subset font's name starts with: six
/// uppercase letters and a plus (9.6.4).
const SUBSET_PREFIX_LEN: usize = 7;

/// The name of a font without its subset prefix (`ABCDEF+Arial` is `Arial`).
pub(crate) fn strip_subset(name: &str) -> &str {
    let bytes = name.as_bytes();
    if bytes.len() > SUBSET_PREFIX_LEN
        && bytes[SUBSET_PREFIX_LEN - 1] == b'+'
        && bytes[..SUBSET_PREFIX_LEN - 1]
            .iter()
            .all(u8::is_ascii_uppercase)
    {
        &name[SUBSET_PREFIX_LEN..]
    } else {
        name
    }
}

#[derive(Clone, Copy)]
enum Family {
    Helvetica,
    Times,
    Courier,
}

/// The parts of a style suffix, which may be written in any order and with
/// the foundry's `PS` and `MT` marks.
const STYLE_WORDS: [&str; 6] = ["bold", "italic", "oblique", "roman", "ps", "mt"];
const SEPARATORS: [char; 2] = [',', '-'];

/// The families by the name a font file may start with, the longer of two
/// names with a common start first.
const FAMILIES: [(&str, Family); 6] = [
    ("TimesNewRoman", Family::Times),
    ("Times", Family::Times),
    ("CourierNew", Family::Courier),
    ("Courier", Family::Courier),
    ("Helvetica", Family::Helvetica),
    ("Arial", Family::Helvetica),
];

/// The standard font whose metrics `name` stands for, or `None`.
pub(crate) fn standard_metrics(name: &str) -> Option<&'static StandardMetrics> {
    let name: String = strip_subset(name).chars().filter(|c| *c != ' ').collect();
    let canonical = canonical(&name)?;
    STANDARD_14.iter().find(|font| font.name == canonical)
}

fn canonical(name: &str) -> Option<&'static str> {
    if name == "Symbol" || name == "SymbolMT" {
        return Some("Symbol");
    }
    if name == "ZapfDingbats" {
        return Some("ZapfDingbats");
    }
    let (rest, family) = FAMILIES
        .iter()
        .find_map(|(prefix, family)| name.strip_prefix(prefix).map(|rest| (rest, *family)))?;
    let mut rest = rest.to_ascii_lowercase();
    let (mut bold, mut italic) = (false, false);
    for word in STYLE_WORDS {
        if rest.contains(word) {
            bold |= word == "bold";
            italic |= word == "italic" || word == "oblique";
            rest = rest.replace(word, "");
        }
    }
    if !rest.chars().all(|c| SEPARATORS.contains(&c)) {
        return None;
    }
    Some(match (family, bold, italic) {
        (Family::Helvetica, false, false) => "Helvetica",
        (Family::Helvetica, true, false) => "Helvetica-Bold",
        (Family::Helvetica, false, true) => "Helvetica-Oblique",
        (Family::Helvetica, true, true) => "Helvetica-BoldOblique",
        (Family::Times, false, false) => "Times-Roman",
        (Family::Times, true, false) => "Times-Bold",
        (Family::Times, false, true) => "Times-Italic",
        (Family::Times, true, true) => "Times-BoldItalic",
        (Family::Courier, false, false) => "Courier",
        (Family::Courier, true, false) => "Courier-Bold",
        (Family::Courier, false, true) => "Courier-Oblique",
        (Family::Courier, true, true) => "Courier-BoldOblique",
    })
}

impl StandardMetrics {
    /// The width of glyph `name`, in thousandths of an em.
    pub(crate) fn width(&self, name: &str) -> Option<u16> {
        let index = self
            .widths
            .binary_search_by(|(glyph, _)| (*glyph).cmp(name))
            .ok()?;
        Some(self.widths[index].1)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn named(name: &str) -> Option<&'static str> {
        standard_metrics(name).map(|font| font.name)
    }

    #[test]
    fn the_14_names_are_themselves() {
        for font in &STANDARD_14 {
            assert_eq!(named(font.name), Some(font.name));
        }
    }

    #[test]
    fn a_subset_prefix_is_dropped() {
        assert_eq!(strip_subset("ABCDEF+Arial"), "Arial");
        assert_eq!(strip_subset("abcdef+Arial"), "abcdef+Arial");
        assert_eq!(strip_subset("ABCDE+Arial"), "ABCDE+Arial");
        assert_eq!(named("QWERTY+Helvetica-Bold"), Some("Helvetica-Bold"));
    }

    #[test]
    fn the_names_of_the_metric_compatible_families() {
        let cases = [
            ("Arial", "Helvetica"),
            ("ArialMT", "Helvetica"),
            ("Arial,Bold", "Helvetica-Bold"),
            ("Arial-BoldMT", "Helvetica-Bold"),
            ("Arial,Italic", "Helvetica-Oblique"),
            ("Arial-BoldItalicMT", "Helvetica-BoldOblique"),
            ("TimesNewRoman", "Times-Roman"),
            ("TimesNewRomanPSMT", "Times-Roman"),
            ("TimesNewRoman,BoldItalic", "Times-BoldItalic"),
            ("TimesNewRomanPS-BoldMT", "Times-Bold"),
            ("Times New Roman", "Times-Roman"),
            ("CourierNewPSMT", "Courier"),
            ("CourierNew,Bold", "Courier-Bold"),
            ("Courier-BoldOblique", "Courier-BoldOblique"),
            ("SymbolMT", "Symbol"),
        ];
        for (name, standard) in cases {
            assert_eq!(named(name), Some(standard), "{name}");
        }
    }

    #[test]
    fn other_families_and_styles_are_not_standard() {
        for name in [
            "ArialNarrow",
            "Arial-Black",
            "Arial-Light",
            "Verdana",
            "TimesTen",
            "",
        ] {
            assert_eq!(named(name), None, "{name}");
        }
    }

    #[test]
    fn widths_are_found_by_glyph_name() {
        // From the AFM files: Courier is 600 throughout; Helvetica's space
        // is 278 and its A 667.
        let courier = standard_metrics("Courier").expect("Courier");
        assert_eq!(courier.width("A"), Some(600));
        let helvetica = standard_metrics("Helvetica").expect("Helvetica");
        assert_eq!(helvetica.width("space"), Some(278));
        assert_eq!(helvetica.width("A"), Some(667));
        assert_eq!(helvetica.width("nonexistent"), None);
    }
}
