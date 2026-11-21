// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Variable fonts: the axes a font has and the instance a caller picks.
//!
//! Formats: OpenType 1.9 (`fvar`, `avar`, `gvar`, `CFF2`, `name` IDs 16 and
//! 25), the OpenType registered axes (`wght`, `wdth`, `ital`, `slnt`, `opsz`)
//! and Adobe Technical Note 5902, Generating PostScript Names for Fonts Using
//! OpenType Font Variations (section 2 and its appendix).
//!
//! An instance is one value for some of a font's axes ([`Variations`]); an
//! axis not named stays at its default. [`Program::axes`] lists what a font
//! offers, and [`Program::with_variations`] checks a choice against it.
//!
//! [`Program::axes`]: crate::Program::axes
//! [`Program::with_variations`]: crate::Program::with_variations

use crate::error::FontError;

/// The four-byte tag of an axis, such as `*b"wght"`.
pub type AxisTag = [u8; 4];

/// The OpenType registered weight axis (user-space values like `usWeightClass`).
pub const AXIS_WEIGHT: AxisTag = *b"wght";
/// The registered italic axis: 0 is Roman, 1 is italic, and the registry
/// defines nothing between them.
pub const AXIS_ITALIC: AxisTag = *b"ital";
/// The registered slant axis, in degrees counter-clockwise from the vertical.
pub const AXIS_SLANT: AxisTag = *b"slnt";

/// One axis of a variable font, in user-space values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Axis {
    pub tag: AxisTag,
    pub min: f32,
    pub default: f32,
    pub max: f32,
}

/// An instance of a variable font: a value for each axis the caller names.
/// Axes left out keep their default.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Variations {
    settings: Vec<(AxisTag, f32)>,
}

impl Variations {
    /// No axis named: the font's default instance.
    pub fn new() -> Variations {
        Variations::default()
    }

    /// Adds `value` for the axis `tag`. A tag given twice is an error when
    /// the instance is applied ([`FontError::DuplicateAxis`]).
    pub fn with(mut self, tag: AxisTag, value: f32) -> Variations {
        self.settings.push((tag, value));
        self
    }

    /// Whether no axis is named.
    pub fn is_empty(&self) -> bool {
        self.settings.is_empty()
    }

    /// The axes named, in the order they were added.
    pub fn settings(&self) -> &[(AxisTag, f32)] {
        &self.settings
    }

    /// The value given for `tag`.
    pub fn get(&self, tag: AxisTag) -> Option<f32> {
        self.settings
            .iter()
            .find(|(t, _)| *t == tag)
            .map(|(_, v)| *v)
    }

    /// Checks the instance against `axes`: every tag is an axis of the font,
    /// appears once, and its value is within the axis's range.
    ///
    /// [`FontError::NoVariationAxes`] when the font has no axes and the
    /// instance names one; [`FontError::UnknownAxis`],
    /// [`FontError::DuplicateAxis`] and [`FontError::AxisOutOfRange`]
    /// otherwise. A value that is not a number is out of range.
    pub fn check(&self, axes: &[Axis]) -> Result<(), FontError> {
        if self.settings.is_empty() {
            return Ok(());
        }
        if axes.is_empty() {
            return Err(FontError::NoVariationAxes);
        }
        for (i, &(tag, value)) in self.settings.iter().enumerate() {
            let axis = axes
                .iter()
                .find(|a| a.tag == tag)
                .ok_or(FontError::UnknownAxis { tag })?;
            if self.settings[..i].iter().any(|(t, _)| *t == tag) {
                return Err(FontError::DuplicateAxis { tag });
            }
            if !(axis.min <= value && value <= axis.max) {
                return Err(FontError::AxisOutOfRange { tag });
            }
        }
        Ok(())
    }
}

/// Adobe TN 5902, section 2: a generated PostScript name longer than this is
/// replaced by the last-resort form.
const MAX_NAME_LENGTH: usize = 127;
/// Adobe TN 5902, appendix: axis values are 16.16 fixed numbers.
const FIXED_ONE: f64 = 65536.0;
/// Decimals the appendix prints before it looks for the first digit at which
/// the two ends of the 16.16 rounding interval differ.
const INTERVAL_DECIMALS: usize = 8;
/// Hex digits of the identifier in a last-resort name (the note's example
/// shows seven).
const IDENTIFIER_DIGITS: usize = 7;
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// `s` with everything but ASCII letters and digits removed (TN 5902,
/// section 2).
pub(crate) fn ascii_alphanumeric(s: &str) -> String {
    s.chars().filter(char::is_ascii_alphanumeric).collect()
}

/// The decimal form of an axis value that TN 5902 (section 2, step 2 and the
/// appendix) asks for: the fewest decimals that read back as the same 16.16
/// number, no leading or trailing zeros past the one of a fraction, no `+`,
/// and `0` for zero.
pub(crate) fn user_decimal(value: f32) -> String {
    let fixed = (f64::from(value) * FIXED_ONE).round();
    let v = fixed / FIXED_ONE;
    if fixed == 0.0 {
        return "0".to_string();
    }
    let eps = 0.5 / FIXED_ONE;
    let (lo, hi) = (v - eps, v + eps);
    // An interval that holds a whole number is that number.
    if lo.trunc() != hi.trunc() {
        return format!("{}", v.round() as i64);
    }
    let lo_text = format!("{lo:.INTERVAL_DECIMALS$}");
    let hi_text = format!("{hi:.INTERVAL_DECIMALS$}");
    let differs = lo_text
        .bytes()
        .zip(hi_text.bytes())
        .position(|(a, b)| a != b)
        .unwrap_or(lo_text.len() - 1);
    let period = lo_text.find('.').unwrap_or(0);
    let decimals = differs.saturating_sub(period).max(1);
    let text = format!("{v:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_string()
    } else {
        text
    }
}

/// The name of a named instance: the prefix, `-`, and the instance's
/// subfamily name reduced to letters and digits (TN 5902, section 2).
pub(crate) fn named_instance_name(prefix: &str, subfamily: &str) -> String {
    limited(
        prefix,
        format!("{prefix}-{}", ascii_alphanumeric(subfamily)),
    )
}

/// The name of an arbitrary instance: the prefix and, for each axis given, `_`,
/// its value and its tag without trailing spaces (TN 5902, section 2).
pub(crate) fn arbitrary_instance_name(prefix: &str, axes: &[(AxisTag, f32)]) -> String {
    let mut name = prefix.to_string();
    for (tag, value) in axes {
        name.push('_');
        name.push_str(&user_decimal(*value));
        name.push_str(String::from_utf8_lossy(tag).trim_end_matches(' '));
    }
    limited(prefix, name)
}

/// `name`, or when it is longer than the note allows the last-resort form:
/// the prefix, `-`, a stable identifier of the full name, and `...`.
fn limited(prefix: &str, name: String) -> String {
    if name.len() <= MAX_NAME_LENGTH {
        return name;
    }
    let hash = name.bytes().fold(FNV_OFFSET, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(FNV_PRIME)
    });
    let digits: String = format!("{hash:016X}")
        .chars()
        .take(IDENTIFIER_DIGITS)
        .collect();
    format!("{prefix}-{digits}...")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axes() -> Vec<Axis> {
        vec![
            Axis {
                tag: AXIS_WEIGHT,
                min: 100.0,
                default: 400.0,
                max: 900.0,
            },
            Axis {
                tag: *b"wdth",
                min: 75.0,
                default: 100.0,
                max: 125.0,
            },
        ]
    }

    #[test]
    fn an_empty_instance_is_valid_for_any_font() {
        assert_eq!(Variations::new().check(&axes()), Ok(()));
        assert_eq!(Variations::new().check(&[]), Ok(()));
    }

    #[test]
    fn values_at_the_ends_of_the_range_are_valid() {
        let v = Variations::new()
            .with(AXIS_WEIGHT, 100.0)
            .with(*b"wdth", 125.0);
        assert_eq!(v.check(&axes()), Ok(()));
    }

    #[test]
    fn a_font_without_axes_refuses_an_axis() {
        let v = Variations::new().with(AXIS_WEIGHT, 700.0);
        assert_eq!(v.check(&[]), Err(FontError::NoVariationAxes));
    }

    #[test]
    fn an_axis_the_font_lacks_is_unknown() {
        let v = Variations::new().with(*b"opsz", 12.0);
        assert_eq!(
            v.check(&axes()),
            Err(FontError::UnknownAxis { tag: *b"opsz" })
        );
    }

    #[test]
    fn a_repeated_axis_is_refused() {
        let v = Variations::new()
            .with(AXIS_WEIGHT, 400.0)
            .with(AXIS_WEIGHT, 700.0);
        assert_eq!(
            v.check(&axes()),
            Err(FontError::DuplicateAxis { tag: AXIS_WEIGHT })
        );
    }

    #[test]
    fn a_value_outside_the_range_or_not_a_number_is_refused() {
        for value in [99.9, 900.1, f32::NAN, f32::INFINITY] {
            let v = Variations::new().with(AXIS_WEIGHT, value);
            assert_eq!(
                v.check(&axes()),
                Err(FontError::AxisOutOfRange { tag: AXIS_WEIGHT }),
                "{value}"
            );
        }
    }

    #[test]
    fn axis_values_print_with_the_fewest_decimals_that_read_back() {
        for (value, text) in [
            (0.0, "0"),
            (-0.0, "0"),
            (900.0, "900"),
            (5.5, "5.5"),
            (-2.9, "-2.9"),
            (-1.4, "-1.4"),
            (0.5, "0.5"),
            (12.25, "12.25"),
            (100.0, "100"),
            (-12.0, "-12"),
        ] {
            assert_eq!(user_decimal(value), text, "{value}");
        }
    }

    #[test]
    fn names_follow_the_examples_of_the_note() {
        assert_eq!(
            arbitrary_instance_name("AndreVar", &[(*b"wght", 900.0), (*b"wdth", 5.5)]),
            "AndreVar_900wght_5.5wdth"
        );
        assert_eq!(
            arbitrary_instance_name("AndreVar", &[(*b"wght", -2.9), (*b"wdth", -1.4)]),
            "AndreVar_-2.9wght_-1.4wdth"
        );
        // A tag shorter than four letters is padded with spaces in the font.
        assert_eq!(arbitrary_instance_name("F", &[(*b"ab  ", 1.0)]), "F_1ab");
        assert_eq!(named_instance_name("AndreVar", "Black"), "AndreVar-Black");
        assert_eq!(
            named_instance_name("AndreVar", "Extra-Bold"),
            "AndreVar-ExtraBold"
        );
    }

    #[test]
    fn a_name_past_127_characters_is_replaced_by_the_last_resort_form() {
        let axes: Vec<(AxisTag, f32)> = (0..20u8)
            .map(|i| {
                (
                    [b'a', b'x', b'0' + i / 10, b'0' + i % 10],
                    100.0 + f32::from(i),
                )
            })
            .collect();
        let long = arbitrary_instance_name("AndreVar", &axes);
        assert!(long.starts_with("AndreVar-"), "{long}");
        assert!(long.ends_with("..."));
        assert_eq!(long.len(), "AndreVar-".len() + IDENTIFIER_DIGITS + 3);
        assert!(long["AndreVar-".len()..long.len() - 3]
            .chars()
            .all(|c| c.is_ascii_hexdigit()));
        // The same instance gives the same name, another a different one.
        assert_eq!(long, arbitrary_instance_name("AndreVar", &axes));
        let mut other = axes.clone();
        other[0].1 += 1.0;
        assert_ne!(long, arbitrary_instance_name("AndreVar", &other));
    }
}
