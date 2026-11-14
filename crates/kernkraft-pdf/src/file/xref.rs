// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Cross-reference sections: ISO 32000-1:2008, 7.5.4 (cross-reference
//! table), 7.5.5 (file trailer), 7.5.8.2 and 7.5.8.3 (cross-reference
//! stream dictionary and data, Tables 17 and 18).

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::error::Limit;
use crate::syntax::{Dictionary, Parser, Span, Token, TokenKind, Value};
use crate::PdfError;

/// The bytes of one cross-reference table entry (7.5.4).
const ENTRY_BYTES: u64 = 20;
/// The digits of an entry's first field: a byte offset, or the number of
/// the next free object.
const OFFSET_DIGITS: u64 = 10;
/// The digits of an entry's generation number.
const GENERATION_DIGITS: u64 = 5;
/// The fields of a cross-reference stream entry that Table 18 defines.
const DEFINED_FIELDS: usize = 3;
/// The bytes of the widest field read: a field is read into a `u64`.
const MAX_FIELD_BYTES: usize = size_of::<u64>();

const TYPE_KEY: &[u8] = b"Type";
const XREF_TYPE: &[u8] = b"XRef";
const SIZE_KEY: &[u8] = b"Size";
const WIDTHS_KEY: &[u8] = b"W";
const INDEX_KEY: &[u8] = b"Index";

/// What the cross-reference says of one object number (7.5.4, Table 18).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum XrefEntry {
    /// The object is free. `next` is the number of the next free object
    /// and `generation` the generation to use when the number is used
    /// again.
    Free { next: u32, generation: u16 },
    /// The object starts at `offset` in the file.
    InUse { offset: u64, generation: u16 },
    /// The object is the one at `index` in object stream `stream`. Its
    /// generation, and the stream's, is 0.
    Compressed { stream: u32, index: u32 },
    /// An entry of a type the format does not define. The object is the
    /// null object (7.5.8.3).
    Null,
}

/// Entries for a run of consecutive object numbers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Subsection {
    pub first: u32,
    pub entries: Vec<XrefEntry>,
}

impl Subsection {
    /// One past the last object number, which may be one past `u32::MAX`.
    fn end(&self) -> u64 {
        // A Vec holds at most isize::MAX elements, which fits a u64.
        u64::from(self.first) + self.entries.len() as u64
    }
}

/// The entries of one cross-reference table or stream: subsections that
/// share no object number, ordered by object number.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct Entries {
    subsections: Vec<Subsection>,
}

impl Entries {
    /// Orders the subsections. Two that hold the same object number are
    /// [`PdfError::BadXref`] at `offset` (7.5.4: a number has an entry in
    /// at most one subsection of a section).
    pub fn new(mut subsections: Vec<Subsection>, offset: u64) -> Result<Self, PdfError> {
        subsections.retain(|subsection| !subsection.entries.is_empty());
        subsections.sort_by_key(|subsection| subsection.first);
        let overlap = subsections
            .windows(2)
            .any(|pair| matches!(pair, [a, b] if a.end() > u64::from(b.first)));
        if overlap {
            return Err(PdfError::BadXref { offset });
        }
        Ok(Self { subsections })
    }

    pub fn get(&self, number: u32) -> Option<XrefEntry> {
        let after = self
            .subsections
            .partition_point(|subsection| subsection.first <= number);
        let subsection = self.subsections.get(after.checked_sub(1)?)?;
        let index = usize::try_from(number - subsection.first).ok()?;
        subsection.entries.get(index).copied()
    }

    /// One past the highest object number that has an entry; 0 when
    /// there is none.
    pub fn end(&self) -> u64 {
        self.subsections.last().map_or(0, Subsection::end)
    }

    /// Every entry with its object number, in order of object number.
    pub fn iter(&self) -> impl Iterator<Item = (u32, XrefEntry)> + '_ {
        self.subsections.iter().flat_map(|subsection| {
            (subsection.first..)
                .zip(&subsection.entries)
                .map(|(number, entry)| (number, *entry))
        })
    }
}

/// How one cross-reference section is written in the file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionForm {
    /// A cross-reference table and a trailer (7.5.4, 7.5.5).
    Table,
    /// A cross-reference stream (7.5.8).
    Stream,
    /// Not in the file: built by reading the file through, because its
    /// cross-reference could not be read.
    Rebuilt,
}

/// One cross-reference section and its trailer: the original one of a file
/// or the one an incremental update added (7.5.6).
#[derive(Debug, Clone, PartialEq)]
pub struct Section {
    pub(crate) offset: u64,
    pub(crate) form: SectionForm,
    pub(crate) trailer: Dictionary,
    pub(crate) size: u64,
    pub(crate) entries: Entries,
    /// The cross-reference stream a table's trailer names with `XRefStm`
    /// (7.5.8.4): its offset and its entries.
    pub(crate) hybrid: Option<(u64, Entries)>,
}

impl Section {
    /// Where the section starts: the offset of `xref`, or of the
    /// cross-reference stream's object. 0 for a rebuilt section.
    pub fn offset(&self) -> u64 {
        self.offset
    }

    pub fn form(&self) -> SectionForm {
        self.form
    }

    /// The trailer dictionary; for a cross-reference stream, the stream's
    /// dictionary, which holds the trailer's entries (7.5.8.1). A rebuilt
    /// section has `Size` and whichever of `Root`, `Info`, `ID` and
    /// `Encrypt` the file's own trailers gave.
    pub fn trailer(&self) -> &Dictionary {
        &self.trailer
    }

    /// The trailer's `Size`: by Table 15, one more than the highest object
    /// number the file defines, as this section saw it. Files are written
    /// with a `Size` below that, so an entry for a higher number is kept
    /// and reported
    /// ([`EntryPastSize`](crate::DiagnosticKind::EntryPastSize)).
    pub fn size(&self) -> u64 {
        self.size
    }

    /// The offset of the cross-reference stream that the trailer of a
    /// hybrid-reference file names with `XRefStm` (7.5.8.4).
    pub fn hybrid_offset(&self) -> Option<u64> {
        self.hybrid.as_ref().map(|(offset, _)| *offset)
    }

    /// What this section alone says of object `number`: its own entry,
    /// or failing that the entry of its `XRefStm` stream (7.5.8.4).
    pub fn entry(&self, number: u32) -> Option<XrefEntry> {
        self.entries.get(number).or_else(|| {
            self.hybrid
                .as_ref()
                .and_then(|(_, entries)| entries.get(number))
        })
    }

    /// The section's own entries with their object numbers, in order of
    /// object number, then those of its `XRefStm` stream.
    pub fn entries(&self) -> impl Iterator<Item = (u32, XrefEntry)> + '_ {
        self.entries
            .iter()
            .chain(self.hybrid.iter().flat_map(|(_, entries)| entries.iter()))
    }
}

/// What the sections from the first of `sections` back say of object
/// `number`: the entry of the first section that has one (7.5.6).
pub(crate) fn lookup(sections: &[Section], number: u32) -> Option<XrefEntry> {
    sections.iter().find_map(|section| section.entry(number))
}

/// Counts the cross-reference entries read against the caller's limit.
pub(crate) struct Budget {
    used: usize,
    max: usize,
}

impl Budget {
    pub fn new(max: usize) -> Self {
        Self { used: 0, max }
    }

    /// Counts `count` more entries, read from the section at `offset`.
    pub fn take(&mut self, count: usize, offset: u64) -> Result<(), PdfError> {
        match self.used.checked_add(count) {
            Some(used) if used <= self.max => {
                self.used = used;
                Ok(())
            }
            _ => Err(PdfError::LimitExceeded {
                offset,
                // A usize is at most 64 bits wide on every supported target.
                limit: Limit::XrefEntries {
                    max: self.max as u64,
                },
            }),
        }
    }
}

/// The `Size` of a trailer or cross-reference stream dictionary (Tables 15
/// and 17): a direct integer that is not negative.
pub(crate) fn size(dictionary: &Dictionary, offset: u64) -> Result<u64, PdfError> {
    unsigned(dictionary, SIZE_KEY)
        .flatten()
        .ok_or(PdfError::BadXref { offset })
}

/// The value of `key` when it is an integer that is not negative:
/// `Some(None)` when the dictionary has no such key or holds null for it,
/// `None` when it holds anything else.
pub(crate) fn unsigned(dictionary: &Dictionary, key: &[u8]) -> Option<Option<u64>> {
    match dictionary.get(key).map(|object| object.value()) {
        None | Some(Value::Null) => Some(None),
        Some(Value::Integer(value)) => u64::try_from(*value).ok().map(Some),
        Some(_) => None,
    }
}

/// Reads a cross-reference table (7.5.4) and the trailer dictionary after
/// it (7.5.5). `parser` stands straight after the `xref` keyword, which is
/// at `offset`.
pub(crate) fn read_table(
    parser: &mut Parser<'_>,
    offset: u64,
    budget: &mut Budget,
    diagnostics: &mut Diagnostics,
) -> Result<(Entries, Dictionary), PdfError> {
    let bad = |at: u64| PdfError::BadXref { offset: at };
    // The lexer judges every number as a PDF integer; the fields of a
    // table are not objects, so what it says of them is not kept.
    let mut unkept = Diagnostics::new();
    let mut subsections = Vec::new();
    let mut layout_reported = false;
    loop {
        let token = parser.lexer().next_token(&mut unkept)?;
        let first = match token.kind {
            TokenKind::Keyword(ref keyword) if keyword == b"trailer" => break,
            TokenKind::Integer(first) => first,
            _ => return Err(bad(token.span.start)),
        };
        let count_token = parser.lexer().next_token(&mut unkept)?;
        let TokenKind::Integer(count) = count_token.kind else {
            return Err(bad(count_token.span.start));
        };
        let first = u32::try_from(first).map_err(|_| bad(token.span.start))?;
        let count = u64::try_from(count).map_err(|_| bad(count_token.span.start))?;
        // The last object number of the subsection is one an object can
        // carry.
        if u64::from(first) + count > u64::from(u32::MAX) + 1 {
            return Err(bad(token.span.start));
        }
        let mut entries = Vec::new();
        let mut expected_start = None;
        for _ in 0..count {
            let (entry, span, laid_out) = read_entry(parser, &mut unkept)?;
            budget.take(1, offset)?;
            entries.push(entry);
            let in_place = expected_start.is_none_or(|expected| expected == span.start);
            if !(laid_out && in_place) && !layout_reported {
                diagnostics.push(DiagnosticKind::XrefEntryLayout, span.start);
                layout_reported = true;
            }
            expected_start = Some(span.start + ENTRY_BYTES);
        }
        subsections.push(Subsection { first, entries });
    }
    let trailer = parser.parse_object(diagnostics)?;
    let start = trailer.span().map_or(offset, |span| span.start);
    match trailer.into_value() {
        Value::Dictionary(dictionary) => Ok((Entries::new(subsections, offset)?, dictionary)),
        _ => Err(bad(start)),
    }
}

/// One table entry: its value, the span from its first digit to its
/// keyword, and whether those bytes are laid out as 7.5.4 says.
fn read_entry(
    parser: &mut Parser<'_>,
    unkept: &mut Diagnostics,
) -> Result<(XrefEntry, Span, bool), PdfError> {
    let bad = |token: &Token| PdfError::BadXref {
        offset: token.span.start,
    };
    let field = parser.lexer().next_token(unkept)?;
    let generation = parser.lexer().next_token(unkept)?;
    let keyword = parser.lexer().next_token(unkept)?;
    let TokenKind::Integer(first) = field.kind else {
        return Err(bad(&field));
    };
    let first = u64::try_from(first).map_err(|_| bad(&field))?;
    let generation_value = match generation.kind {
        TokenKind::Integer(value) => u16::try_from(value).map_err(|_| bad(&generation))?,
        _ => return Err(bad(&generation)),
    };
    let entry = match keyword.kind {
        TokenKind::Keyword(ref word) if word == b"n" => XrefEntry::InUse {
            offset: first,
            generation: generation_value,
        },
        TokenKind::Keyword(ref word) if word == b"f" => XrefEntry::Free {
            next: u32::try_from(first).map_err(|_| bad(&field))?,
            generation: generation_value,
        },
        _ => return Err(bad(&keyword)),
    };
    let start = field.span.start;
    let generation_start = start + OFFSET_DIGITS + 1;
    let keyword_start = generation_start + GENERATION_DIGITS + 1;
    let laid_out = field.span.end == start + OFFSET_DIGITS
        && generation.span.start == generation_start
        && generation.span.end == generation_start + GENERATION_DIGITS
        && keyword.span.start == keyword_start;
    let span = Span {
        start,
        end: keyword.span.end,
    };
    Ok((entry, span, laid_out))
}

/// How the data of a cross-reference stream is laid out, read from its
/// dictionary (Table 17).
#[derive(Debug)]
pub(crate) struct StreamLayout {
    /// The bytes of each of the three fields of Table 18.
    widths: [usize; DEFINED_FIELDS],
    /// The bytes of one entry: the sum of every element of `W`.
    entry_bytes: usize,
    /// The first object number and the entry count of each subsection.
    ranges: Vec<(u32, usize)>,
    /// The entries of all subsections together.
    pub entry_count: usize,
    /// The bytes the decoded data holds: `entry_count` entries.
    pub data_bytes: usize,
}

impl StreamLayout {
    /// Reads `Type`, `Size`, `W` and `Index`. `offset` is where the
    /// stream's object starts, for the error.
    pub fn read(dictionary: &Dictionary, offset: u64) -> Result<Self, PdfError> {
        let bad = || PdfError::BadXref { offset };
        match dictionary.get(TYPE_KEY).map(|object| object.value()) {
            Some(Value::Name(name)) if name.as_bytes() == XREF_TYPE => {}
            _ => return Err(bad()),
        }
        let size = size(dictionary, offset)?;

        let Some(Value::Array(items)) = dictionary.get(WIDTHS_KEY).map(|object| object.value())
        else {
            return Err(bad());
        };
        let all_widths = items
            .iter()
            .map(|item| match item.value() {
                Value::Integer(width) => usize::try_from(*width).ok(),
                _ => None,
            })
            .collect::<Option<Vec<usize>>>()
            .ok_or_else(bad)?;
        let widths: [usize; DEFINED_FIELDS] = all_widths
            .get(..DEFINED_FIELDS)
            .and_then(|defined| defined.try_into().ok())
            .ok_or_else(bad)?;
        if widths.iter().any(|&width| width > MAX_FIELD_BYTES) {
            return Err(bad());
        }
        let entry_bytes = all_widths
            .iter()
            .try_fold(0usize, |sum, &width| sum.checked_add(width))
            .filter(|&sum| sum > 0)
            .ok_or_else(bad)?;

        let pairs: Vec<u64> = match dictionary.get(INDEX_KEY).map(|object| object.value()) {
            // Table 17: the default is one subsection of every object.
            None | Some(Value::Null) => vec![0, size],
            Some(Value::Array(items)) if items.len() % 2 == 0 => items
                .iter()
                .map(|item| match item.value() {
                    Value::Integer(value) => u64::try_from(*value).ok(),
                    _ => None,
                })
                .collect::<Option<_>>()
                .ok_or_else(bad)?,
            Some(_) => return Err(bad()),
        };
        let mut ranges = Vec::new();
        let mut entry_count = 0usize;
        for &[first, count] in pairs.as_chunks::<2>().0 {
            // The last object number of the subsection is one an object
            // can carry.
            if first + count.min(u64::from(u32::MAX) + 1) > u64::from(u32::MAX) + 1 {
                return Err(bad());
            }
            let first = u32::try_from(first).map_err(|_| bad())?;
            let count = usize::try_from(count).map_err(|_| bad())?;
            entry_count = entry_count.checked_add(count).ok_or_else(bad)?;
            ranges.push((first, count));
        }
        let data_bytes = entry_count.checked_mul(entry_bytes).ok_or_else(bad)?;
        Ok(Self {
            widths,
            entry_bytes,
            ranges,
            entry_count,
            data_bytes,
        })
    }

    /// The entries that `data`, the decoded stream data, holds (7.5.8.3).
    /// `data` is exactly `data_bytes` long. `offset` is where the
    /// stream's object starts, for the error.
    pub fn entries(&self, data: &[u8], offset: u64) -> Result<Entries, PdfError> {
        let bad = || PdfError::BadXref { offset };
        if data.len() != self.data_bytes {
            return Err(bad());
        }
        let mut rows = data.chunks_exact(self.entry_bytes);
        let mut subsections = Vec::new();
        for &(first, count) in &self.ranges {
            let entries = rows
                .by_ref()
                .take(count)
                .map(|row| self.entry(row))
                .collect::<Option<Vec<XrefEntry>>>()
                .ok_or_else(bad)?;
            if entries.len() != count {
                return Err(bad());
            }
            subsections.push(Subsection { first, entries });
        }
        Entries::new(subsections, offset)
    }

    /// One entry of Table 18 from its bytes. `None` when a field holds a
    /// value its meaning has no room for.
    fn entry(&self, row: &[u8]) -> Option<XrefEntry> {
        let [type_width, second_width, third_width] = self.widths;
        let (type_field, rest) = row.split_at_checked(type_width)?;
        let (second, rest) = rest.split_at_checked(second_width)?;
        let third = rest.get(..third_width)?;
        // Table 17: an absent type field means type 1. An absent second
        // or third field is read as 0, which is the default Table 18
        // gives the one field that has a default.
        let entry_type = if type_width == 0 {
            1
        } else {
            big_endian(type_field)
        };
        let second = big_endian(second);
        let third = big_endian(third);
        Some(match entry_type {
            0 => XrefEntry::Free {
                next: u32::try_from(second).ok()?,
                generation: u16::try_from(third).ok()?,
            },
            1 => XrefEntry::InUse {
                offset: second,
                generation: u16::try_from(third).ok()?,
            },
            2 => XrefEntry::Compressed {
                stream: u32::try_from(second).ok()?,
                index: u32::try_from(third).ok()?,
            },
            _ => XrefEntry::Null,
        })
    }
}

/// 7.5.8.3: a field is stored with its high-order byte first. `bytes` is
/// at most [`MAX_FIELD_BYTES`] long.
fn big_endian(bytes: &[u8]) -> u64 {
    bytes
        .iter()
        .fold(0, |value, &byte| value << u8::BITS | u64::from(byte))
}
