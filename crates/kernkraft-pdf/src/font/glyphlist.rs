// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The text a glyph name stands for: the Adobe Glyph List Specification
//! (github.com/adobe-type-tools/agl-specification), which ISO 32000-1:2008,
//! 9.10.2 names for this.
//!
//! A name is cut at its first period, split into components at each
//! underscore, and each component read as a name of the list, as `uni`
//! followed by groups of four uppercase hexadecimal digits, or as `u`
//! followed by four to six. A component that is none of these stands for
//! nothing; a name none of whose components stand for anything has no text.

use super::data::glyphlist::GLYPH_LIST;

/// The code units one `uni` group stands for.
const UNI_GROUP_DIGITS: usize = 4;
/// `u` is followed by four to six digits.
const U_MIN_DIGITS: usize = 4;
const U_MAX_DIGITS: usize = 6;
const SURROGATES: std::ops::Range<u32> = 0xD800..0xE000;

/// The text of glyph `name`, or `None` when the name stands for none.
pub(crate) fn text_for_name(name: &str) -> Option<String> {
    let base = name.split('.').next().unwrap_or_default();
    let mut text = String::new();
    for component in base.split('_') {
        push_component(component, &mut text);
    }
    (!text.is_empty()).then_some(text)
}

fn push_component(component: &str, text: &mut String) {
    if let Ok(index) = GLYPH_LIST.binary_search_by(|(name, _)| (*name).cmp(component)) {
        text.push_str(GLYPH_LIST[index].1);
    } else if let Some(digits) = component.strip_prefix("uni") {
        push_uni(digits, text);
    } else if let Some(digits) = component.strip_prefix('u') {
        push_u(digits, text);
    }
}

fn is_upper_hex(digits: &str) -> bool {
    digits
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(&byte))
}

/// `uniXXXX` and `uniXXXXYYYY...`: every group a scalar value outside the
/// surrogates. A name any group of which is not one stands for nothing.
fn push_uni(digits: &str, text: &mut String) {
    if digits.is_empty() || !digits.len().is_multiple_of(UNI_GROUP_DIGITS) || !is_upper_hex(digits)
    {
        return;
    }
    let mut chars = Vec::new();
    for group in digits.as_bytes().chunks(UNI_GROUP_DIGITS) {
        let value = std::str::from_utf8(group)
            .ok()
            .and_then(|group| u32::from_str_radix(group, 16).ok())
            .filter(|value| !SURROGATES.contains(value))
            .and_then(char::from_u32);
        match value {
            Some(c) => chars.push(c),
            None => return,
        }
    }
    text.extend(chars);
}

/// `uXXXX` to `uXXXXXX`: one scalar value.
fn push_u(digits: &str, text: &mut String) {
    if !(U_MIN_DIGITS..=U_MAX_DIGITS).contains(&digits.len()) || !is_upper_hex(digits) {
        return;
    }
    if let Some(c) = u32::from_str_radix(digits, 16)
        .ok()
        .filter(|value| !SURROGATES.contains(value))
        .and_then(char::from_u32)
    {
        text.push(c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_list_is_sorted_for_binary_search() {
        assert!(GLYPH_LIST.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    /// The examples of the Adobe Glyph List Specification, section 2.
    #[test]
    fn the_specification_examples() {
        assert_eq!(text_for_name("Lcommaaccent").as_deref(), Some("\u{013B}"));
        assert_eq!(
            text_for_name("uni20AC0308").as_deref(),
            Some("\u{20AC}\u{0308}")
        );
        assert_eq!(text_for_name("u1040C").as_deref(), Some("\u{1040C}"));
        assert_eq!(text_for_name("uniD801DC0C"), None);
        assert_eq!(text_for_name("uni20ac"), None);
        assert_eq!(
            text_for_name("Lcommaaccent_uni20AC0308_u1040C.alternate").as_deref(),
            Some("\u{013B}\u{20AC}\u{0308}\u{1040C}")
        );
        assert_eq!(text_for_name("uni013B").as_deref(), Some("\u{013B}"));
        assert_eq!(text_for_name("foo"), None);
        assert_eq!(text_for_name(".notdef"), None);
        assert_eq!(text_for_name("a_b_c").as_deref(), Some("abc"));
    }

    #[test]
    fn a_name_with_a_suffix_reads_as_its_base() {
        assert_eq!(text_for_name("A.swash").as_deref(), Some("A"));
        assert_eq!(text_for_name("one.oldstyle").as_deref(), Some("1"));
    }

    #[test]
    fn the_list_has_multi_character_entries() {
        assert_eq!(
            text_for_name("dalethatafpatah").map(|t| t.chars().count()),
            Some(2)
        );
    }
}
