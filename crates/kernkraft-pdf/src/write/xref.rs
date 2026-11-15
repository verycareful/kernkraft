// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The cross-reference section of a written file: a table and trailer
//! (7.5.4, 7.5.5), or a cross-reference stream (7.5.8), and the end of
//! the file (7.5.5: `startxref`, the offset, `%%EOF`).
//!
//! Every object number below `Size` has an entry. The numbers no object
//! uses are free, linked into the list 7.5.4 describes: entry 0 names the
//! first free number, each free entry the next, the last names 0. A free
//! entry's generation is one more than that of the object last there.

use std::io::Write as _;

use super::objects::{integer, name, Writer};
use crate::doc::Document;
use crate::file::XrefEntry;
use crate::filter::flate_encode;
use crate::syntax::{write_object, Dictionary, Name, Object, Value};
use crate::PdfError;

/// The generation of the head of the free list, object 0 (7.5.4).
pub(super) const FREE_HEAD_GENERATION: u16 = u16::MAX;
/// Table 18: the type field of each kind of entry.
pub(super) const TYPE_FREE: u8 = 0;
pub(super) const TYPE_IN_USE: u8 = 1;
pub(super) const TYPE_COMPRESSED: u8 = 2;
/// The width of the type field.
pub(super) const TYPE_WIDTH: usize = 1;

/// The entry of every object number from 0 to the highest in `entries`.
fn all_entries(document: &Document, mut entries: Vec<(u32, XrefEntry)>) -> Vec<XrefEntry> {
    entries.sort_unstable_by_key(|(number, _)| *number);
    let size = entries.last().map_or(1, |(number, _)| *number as usize + 1);
    let mut all = vec![None; size];
    for (number, entry) in entries {
        all[number as usize] = Some(entry);
    }
    let free: Vec<u32> = (0..size as u32)
        .filter(|&n| all[n as usize].is_none())
        .collect();
    let mut out = Vec::with_capacity(size);
    let mut free_index = 0;
    for (number, entry) in all.into_iter().enumerate() {
        out.push(match entry {
            Some(entry) => entry,
            None => {
                // `free` is in ascending order and holds `number`.
                free_index += 1;
                let next = free.get(free_index).copied().unwrap_or(0);
                let generation = if number == 0 {
                    FREE_HEAD_GENERATION
                } else {
                    document.free_generation(number as u32)
                };
                XrefEntry::Free { next, generation }
            }
        });
    }
    out
}

/// The trailer with `Size` first, then the entries given.
pub(super) fn with_size(size: usize, trailer: Dictionary) -> Dictionary {
    let mut out = Dictionary::new();
    out.insert(Name::new(b"Size".as_slice()), integer(size));
    for (key, value) in trailer.iter() {
        out.insert(key.clone(), value.clone());
    }
    out
}

pub(super) fn end_of_file(out: &mut Vec<u8>, xref_offset: u64) {
    let _ = write!(out, "startxref\n{xref_offset}\n%%EOF\n");
}

/// One entry of a table, in 20 bytes (7.5.4).
pub(super) fn push_row(out: &mut Vec<u8>, entry: &XrefEntry) {
    let (field, generation, kind) = match *entry {
        XrefEntry::InUse { offset, generation } => (offset, generation, 'n'),
        XrefEntry::Free { next, generation } => (u64::from(next), generation, 'f'),
        // Neither is made in table form: every compressed object goes to a
        // file written with a cross-reference stream.
        XrefEntry::Compressed { .. } | XrefEntry::Null => (0, 0, 'f'),
    };
    let _ = write!(out, "{field:010} {generation:05} {kind}\r\n");
}

/// 7.5.4: the table, each entry in 20 bytes, then the trailer.
pub(super) fn table(
    mut writer: Writer<'_>,
    entries: Vec<(u32, XrefEntry)>,
    trailer: Dictionary,
) -> Vec<u8> {
    let entries = all_entries(writer.document, entries);
    let xref_offset = writer.offset();
    let out = &mut writer.out;
    let _ = write!(out, "xref\n0 {}\n", entries.len());
    for entry in &entries {
        push_row(out, entry);
    }
    out.extend_from_slice(b"trailer\n");
    let trailer = with_size(entries.len(), trailer);
    // The trailer was read from the file or built by edits within the
    // nesting limit, so writing it cannot pass it.
    let _ = write_object(&Object::new(Value::Dictionary(trailer)), out);
    out.push(b'\n');
    end_of_file(out, xref_offset);
    writer.out
}

/// The fewest bytes that hold `value`, and at least one.
pub(super) fn width(value: u64) -> usize {
    let bits = u64::BITS - value.leading_zeros();
    (bits as usize).div_ceil(8).max(1)
}

pub(super) fn push_field(data: &mut Vec<u8>, value: u64, width: usize) {
    data.extend_from_slice(&value.to_be_bytes()[8 - width..]);
}

/// The three fields of an entry of a cross-reference stream (Table 18).
fn fields(entry: &XrefEntry) -> (u8, u64, u64) {
    match *entry {
        XrefEntry::Free { next, generation } => (TYPE_FREE, u64::from(next), u64::from(generation)),
        XrefEntry::InUse { offset, generation } => (TYPE_IN_USE, offset, u64::from(generation)),
        XrefEntry::Compressed { stream, index } => {
            (TYPE_COMPRESSED, u64::from(stream), u64::from(index))
        }
        XrefEntry::Null => (TYPE_FREE, 0, 0),
    }
}

/// The data of a cross-reference stream holding `entries`, and the widths
/// of its second and third fields (the first is [`TYPE_WIDTH`]).
pub(super) fn pack(entries: &[XrefEntry]) -> (Vec<u8>, usize, usize) {
    let (second, third) = entries
        .iter()
        .map(fields)
        .fold((0, 0), |(second, third), (_, b, c)| {
            (second.max(b), third.max(c))
        });
    let (second, third) = (width(second), width(third));
    let mut data = Vec::with_capacity(entries.len() * (TYPE_WIDTH + second + third));
    for entry in entries {
        let (kind, b, c) = fields(entry);
        data.push(kind);
        push_field(&mut data, b, second);
        push_field(&mut data, c, third);
    }
    (data, second, third)
}

/// 7.5.8: the cross-reference stream, which is never encrypted (7.5.8.2)
/// and carries the trailer's entries in its dictionary.
pub(super) fn stream(
    mut writer: Writer<'_>,
    mut entries: Vec<(u32, XrefEntry)>,
    trailer: Dictionary,
) -> Result<Vec<u8>, PdfError> {
    let id = writer.take_number()?;
    let xref_offset = writer.offset();
    entries.push((
        id.number,
        XrefEntry::InUse {
            offset: xref_offset,
            generation: 0,
        },
    ));
    let entries = all_entries(writer.document, entries);
    let (data, second, third) = pack(&entries);
    let encoded = flate_encode(&data)?;
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type".as_slice()), name(b"XRef"));
    for (key, value) in with_size(entries.len(), trailer).iter() {
        dictionary.insert(key.clone(), value.clone());
    }
    dictionary.insert(
        Name::new(b"W".as_slice()),
        Object::new(Value::Array(vec![
            integer(TYPE_WIDTH),
            integer(second),
            integer(third),
        ])),
    );
    dictionary.insert(Name::new(b"Filter".as_slice()), name(b"FlateDecode"));
    writer.stream_object(id, dictionary, &encoded)?;
    end_of_file(&mut writer.out, xref_offset);
    Ok(writer.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn widths() {
        assert_eq!(width(0), 1);
        assert_eq!(width(255), 1);
        assert_eq!(width(256), 2);
        assert_eq!(width(65_535), 2);
        assert_eq!(width(1 << 32), 5);
    }
}
