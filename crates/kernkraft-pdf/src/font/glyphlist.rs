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
//!
//! The names of the TeX fonts that the list lacks, or gives only private use
//! characters for, are read from [`TEX_GLYPH_LIST`] first (the list's value
//! for any other name is the one used).

use super::data::glyphlist::GLYPH_LIST;
use super::data::texglyphs::TEX_GLYPH_LIST;

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
    if let Ok(index) = TEX_GLYPH_LIST.binary_search_by(|(name, _)| (*name).cmp(component)) {
        text.push_str(TEX_GLYPH_LIST[index].1);
    } else if let Ok(index) = GLYPH_LIST.binary_search_by(|(name, _)| (*name).cmp(component)) {
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
        assert!(TEX_GLYPH_LIST.windows(2).all(|pair| pair[0].0 < pair[1].0));
    }

    fn private_use(text: &str) -> bool {
        text.chars().all(|c| ('\u{E000}'..='\u{F8FF}').contains(&c))
    }

    /// The table of the TeX names holds only what the Adobe Glyph List lacks
    /// or gives private use characters for, no variation selector, and no
    /// name of a letter and a number.
    #[test]
    fn the_tex_names_add_to_the_list_and_never_replace_text() {
        for (name, text) in TEX_GLYPH_LIST {
            if let Ok(index) = GLYPH_LIST.binary_search_by(|(listed, _)| listed.cmp(&name)) {
                assert!(private_use(GLYPH_LIST[index].1), "{name}");
            }
            assert!(
                !text.chars().any(|c| ('\u{FE00}'..='\u{FE0F}').contains(&c)),
                "{name}"
            );
            assert!(!private_use(text), "{name}");
            let numbered = name.len() > 1
                && name.as_bytes()[0].is_ascii_alphabetic()
                && name[1..].bytes().all(|b| b.is_ascii_digit());
            assert!(!numbered, "{name}");
        }
    }

    /// Values of the `pdfx` package's glyphtounicode-cmr.tex: `mapsto` is
    /// U+21A6, `vextendsingle` U+20D3, `parenleftbig` is `0028 FE01` (the
    /// selector for the size is not text) and `bracketleftBigg` `005B FE04`.
    #[test]
    fn the_names_of_the_tex_fonts_have_their_text() {
        assert_eq!(text_for_name("mapsto").as_deref(), Some("\u{21A6}"));
        assert_eq!(text_for_name("vextendsingle").as_deref(), Some("\u{20D3}"));
        assert_eq!(text_for_name("parenleftbig").as_deref(), Some("("));
        assert_eq!(text_for_name("bracketleftBigg").as_deref(), Some("["));
    }

    /// The Adobe Glyph List gives `braceex` U+F8F4 and `arrowvertex` U+F8E6,
    /// in the private use area; the TeX file gives U+23AA and U+23D0.
    #[test]
    fn a_private_use_value_of_the_list_gives_way_to_the_tex_text() {
        assert_eq!(text_for_name("braceex").as_deref(), Some("\u{23AA}"));
        assert_eq!(text_for_name("arrowvertex").as_deref(), Some("\u{23D0}"));
    }

    /// A name the list has with text of its own is not overridden, and a
    /// numbered name stays unmapped (ZapfDingbats names `a1`).
    #[test]
    fn other_names_are_as_before() {
        assert_eq!(text_for_name("parenleft").as_deref(), Some("("));
        assert_eq!(text_for_name("a1"), None);
        assert_eq!(text_for_name("d47"), None);
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
