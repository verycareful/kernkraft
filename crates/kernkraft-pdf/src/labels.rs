// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page labels: ISO 32000-1:2008, 12.4.2 (Table 159), read from the number
//! tree of the catalog's `PageLabels` (7.7.2, 7.9.7).
//!
//! Each key of the tree is the index of the first page of a labelling
//! range; its value, a page label dictionary, gives the range's prefix,
//! numbering style and first number. A label is kept as those parts
//! ([`PageLabel`]) and written out by its `Display`, so a range whose
//! first number is very large costs nothing until it is shown.
//!
//! The tree shall have a range starting at page index 0. Pages before the
//! first range of a tree that has none there have no label, and
//! [`DiagnosticKind::PageLabelsStartLate`] says so. A page label dictionary
//! with an entry of the wrong type, a style Table 159 does not define, or
//! a first number below 1 is [`PdfError::InvalidEntry`].

use std::fmt;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{decode_text_string, Document, NumberTree, Resolved};
use crate::syntax::{Object, ObjectId, Value};
use crate::PdfError;

const PAGE_LABELS_KEY: &[u8] = b"PageLabels";
const STYLE_KEY: &[u8] = b"S";
const PREFIX_KEY: &[u8] = b"P";
const START_KEY: &[u8] = b"St";
/// Table 159, `St`: the first number of a range when none is given.
const DEFAULT_START: u64 = 1;
/// The letters of the alphabetic styles.
const LETTERS: u64 = 26;
/// The roman numerals of each decimal digit, units first: Table 159
/// names the style and the numerals are those of common use. A thousand
/// is one `M` each, however many.
const ROMAN: [[&str; 10]; 3] = [
    ["", "I", "II", "III", "IV", "V", "VI", "VII", "VIII", "IX"],
    ["", "X", "XX", "XXX", "XL", "L", "LX", "LXX", "LXXX", "XC"],
    ["", "C", "CC", "CCC", "CD", "D", "DC", "DCC", "DCCC", "CM"],
];
const ROMAN_THOUSAND: &str = "M";
const DECIMAL_BASE: u64 = 10;
const THOUSAND: u64 = 1000;

/// The numbering style of a labelling range (Table 159, `S`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LabelStyle {
    /// `D`: 1, 2, 3.
    Decimal,
    /// `R`: I, II, III.
    UpperRoman,
    /// `r`: i, ii, iii.
    LowerRoman,
    /// `A`: A to Z, then AA to ZZ, then AAA, one letter repeated.
    UpperLetters,
    /// `a`: as `A`, in lower case.
    LowerLetters,
}

impl LabelStyle {
    fn from_name(name: &[u8]) -> Option<Self> {
        Some(match name {
            b"D" => LabelStyle::Decimal,
            b"R" => LabelStyle::UpperRoman,
            b"r" => LabelStyle::LowerRoman,
            b"A" => LabelStyle::UpperLetters,
            b"a" => LabelStyle::LowerLetters,
            _ => return None,
        })
    }
}

/// The label of one page: its range's prefix, then its number in the
/// range's style. A range with no style labels each page with the prefix
/// alone, which may be empty (Table 159).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageLabel {
    pub prefix: String,
    pub style: Option<LabelStyle>,
    /// The page's number in its range: the range's first number plus the
    /// page's place in it. At least 1.
    pub number: u64,
}

impl fmt::Display for PageLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.prefix)?;
        let Some(style) = self.style else {
            return Ok(());
        };
        match style {
            LabelStyle::Decimal => write!(f, "{}", self.number),
            LabelStyle::UpperRoman => roman(f, self.number, false),
            LabelStyle::LowerRoman => roman(f, self.number, true),
            LabelStyle::UpperLetters => letters(f, self.number, b'A'),
            LabelStyle::LowerLetters => letters(f, self.number, b'a'),
        }
    }
}

fn roman(f: &mut fmt::Formatter<'_>, number: u64, lower: bool) -> fmt::Result {
    let mut write = |text: &str| {
        if lower {
            f.write_str(&text.to_ascii_lowercase())
        } else {
            f.write_str(text)
        }
    };
    for _ in 0..number / THOUSAND {
        write(ROMAN_THOUSAND)?;
    }
    let mut rest = number % THOUSAND;
    let mut digits = [0usize; 3];
    for digit in &mut digits {
        // A remainder of division by ten is below ten.
        *digit = (rest % DECIMAL_BASE) as usize;
        rest /= DECIMAL_BASE;
    }
    for (place, digit) in digits.iter().enumerate().rev() {
        write(ROMAN[place][*digit])?;
    }
    Ok(())
}

/// Number `n` (at least 1) as letters: the `(n - 1) % 26`th letter,
/// written `(n - 1) / 26 + 1` times.
fn letters(f: &mut fmt::Formatter<'_>, number: u64, first: u8) -> fmt::Result {
    let index = number.saturating_sub(1);
    // A remainder of division by 26 fits a byte, and `first` plus it is a
    // letter.
    let letter = char::from(first + (index % LETTERS) as u8);
    for _ in 0..=index / LETTERS {
        fmt::Write::write_char(f, letter)?;
    }
    Ok(())
}

/// A labelling range: its first page index and its label dictionary.
struct Range {
    first: u64,
    prefix: String,
    style: Option<LabelStyle>,
    start: u64,
}

impl Document {
    /// The label of every page, in page order, or `None` when the
    /// document defines no page labels. A page that no labelling range
    /// covers has no label.
    pub fn page_labels(&self) -> Result<Option<Vec<Option<PageLabel>>>, PdfError> {
        let catalog = self.catalog()?;
        let Some(root) = catalog
            .dictionary()
            .and_then(|catalog| catalog.get(PAGE_LABELS_KEY))
        else {
            return Ok(None);
        };
        if matches!(self.resolve(root)?, Resolved::Null) {
            return Ok(None);
        }
        let mut ranges = Vec::new();
        for (key, value) in NumberTree::new(self, root.clone()).entries()? {
            let first = u64::try_from(key).map_err(|_| PdfError::InvalidTree {
                object: reference(root),
            })?;
            ranges.push(self.label_range(first, &value)?);
        }
        ranges.sort_by_key(|range| range.first);
        let count = u64::from(self.page_count()?);
        if ranges.first().is_some_and(|range| range.first > 0) && count > 0 {
            let mut diagnostics = Diagnostics::new();
            diagnostics.set_object(Some(catalog.id));
            diagnostics.push(DiagnosticKind::PageLabelsStartLate, 0);
            self.record_diagnostics(diagnostics);
        }
        let mut labels = Vec::new();
        let mut next = ranges.iter().peekable();
        let mut current: Option<&Range> = None;
        for index in 0..count {
            while let Some(range) = next.next_if(|range| range.first <= index) {
                current = Some(range);
            }
            let label = match current {
                None => None,
                Some(range) => Some(PageLabel {
                    prefix: range.prefix.clone(),
                    style: range.style,
                    number: range.start.checked_add(index - range.first).ok_or(
                        PdfError::InvalidEntry {
                            object: None,
                            key: "St",
                        },
                    )?,
                }),
            };
            labels.push(label);
        }
        Ok(Some(labels))
    }

    /// The range starting at page index `first`, from its label
    /// dictionary `value`.
    fn label_range(&self, first: u64, value: &Object) -> Result<Range, PdfError> {
        let resolved = self.resolve(value)?;
        let object = resolved.id();
        let invalid = |key| PdfError::InvalidEntry { object, key };
        let dictionary = resolved.dictionary().ok_or(invalid("PageLabels"))?;
        let style = match self.get(dictionary, STYLE_KEY)?.value() {
            Value::Null => None,
            Value::Name(name) => Some(LabelStyle::from_name(name.as_bytes()).ok_or(invalid("S"))?),
            _ => return Err(invalid("S")),
        };
        let prefix = match self.get(dictionary, PREFIX_KEY)?.value() {
            Value::Null => String::new(),
            Value::String(prefix) => decode_text_string(prefix.as_bytes()).text,
            _ => return Err(invalid("P")),
        };
        let start = match self.get(dictionary, START_KEY)?.value() {
            Value::Null => DEFAULT_START,
            Value::Integer(start) if *start >= 1 => start.unsigned_abs(),
            _ => return Err(invalid("St")),
        };
        Ok(Range {
            first,
            prefix,
            style,
            start,
        })
    }
}

fn reference(object: &Object) -> Option<ObjectId> {
    match object.value() {
        Value::Reference(id) => Some(*id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shown(style: LabelStyle, number: u64) -> String {
        PageLabel {
            prefix: String::new(),
            style: Some(style),
            number,
        }
        .to_string()
    }

    /// Table 159's letters: A to Z, then AA to ZZ, then AAA.
    #[test]
    fn letters_repeat_one_letter() {
        assert_eq!(shown(LabelStyle::UpperLetters, 1), "A");
        assert_eq!(shown(LabelStyle::UpperLetters, 26), "Z");
        assert_eq!(shown(LabelStyle::UpperLetters, 27), "AA");
        assert_eq!(shown(LabelStyle::UpperLetters, 28), "BB");
        assert_eq!(shown(LabelStyle::LowerLetters, 52), "zz");
        assert_eq!(shown(LabelStyle::LowerLetters, 53), "aaa");
    }

    #[test]
    fn roman_numerals() {
        let cases = [
            (1, "I"),
            (4, "IV"),
            (9, "IX"),
            (14, "XIV"),
            (40, "XL"),
            (90, "XC"),
            (400, "CD"),
            (1994, "MCMXCIV"),
            (3999, "MMMCMXCIX"),
            (4000, "MMMM"),
        ];
        for (number, expected) in cases {
            assert_eq!(shown(LabelStyle::UpperRoman, number), expected);
            assert_eq!(
                shown(LabelStyle::LowerRoman, number),
                expected.to_ascii_lowercase()
            );
        }
    }

    #[test]
    fn a_prefix_alone_and_a_prefixed_number() {
        let contents = PageLabel {
            prefix: "Contents".to_string(),
            style: None,
            number: 3,
        };
        assert_eq!(contents.to_string(), "Contents");
        let appendix = PageLabel {
            prefix: "A-".to_string(),
            style: Some(LabelStyle::Decimal),
            number: 8,
        };
        assert_eq!(appendix.to_string(), "A-8");
    }
}
