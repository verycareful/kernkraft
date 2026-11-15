// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The incremental update: ISO 32000-1:2008, 7.5.6 (incremental updates),
//! 7.5.4 (the free list), 7.5.5 (the trailer), 7.5.8 (cross-reference
//! streams).
//!
//! An update is appended to a file and leaves its bytes as they are. It is
//! the difference between two states of one file: the state the bytes
//! before it hold ([`Tail`]) and the document to be saved. Its
//! cross-reference section has an entry for each object that differs, in
//! subsections (7.5.4), and its trailer has every entry of the document's
//! trailer, a new second file identifier and `Prev`, the offset of the
//! section before it (7.5.6).
//!
//! What differs is read from the edits of the two snapshots, not from the
//! bytes of their objects:
//!
//! - an object the document sets is written;
//! - an object the document removed gets a free entry, one generation on
//!   (7.5.4);
//! - an object the earlier state had changed and the document has not gets
//!   the entry the file itself has, so that the original bytes are read
//!   again and nothing is written for it.
//!
//! The free entries of the update are linked into the free list: entry 0
//! names the first free object, each free entry the next, the last 0
//! (7.5.4). A newly freed object is put at the head of the list, in
//! ascending order; an object that is in use again is taken out of it. The
//! entries the list passes through are not touched but for the one whose
//! `next` changes.
//!
//! The section is a cross-reference stream when the newest section of the
//! file is one, and a table otherwise (7.5.8.4: a hybrid file's table
//! stays a table). Objects are never put in object streams here. An
//! encrypted file stays encrypted: the objects the update writes are
//! encrypted with the key the file has.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Write as _;
use std::sync::Arc;

use kernkraft_fs::Progress;

use super::objects::{integer, name, Writer};
use super::xref::{end_of_file, pack, push_row, with_size, FREE_HEAD_GENERATION, TYPE_WIDTH};
use super::{written_trailer, WriteOptions};
use crate::crypt::IvSource;
use crate::doc::{DocObject, Document, Edit};
use crate::file::{SectionForm, XrefEntry};
use crate::filter::flate_encode;
use crate::syntax::{write_object, Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const PREV_KEY: &[u8] = b"Prev";

/// What the bytes of a file hold after the updates appended so far: the
/// document they say, where the file ends and where its newest
/// cross-reference section starts.
#[derive(Debug, Clone)]
pub(crate) struct Tail {
    /// The length of the file.
    length: u64,
    /// Where the newest cross-reference section starts: the `Prev` of the
    /// next update.
    xref_offset: u64,
    /// Whether the last byte of the file ends a line, so that an update
    /// can start at once.
    ends_in_line: bool,
    /// The document the file holds.
    state: Document,
    /// The entries the updates appended wrote, the latest of each number:
    /// together with the file's own entries they say what the file holds.
    written: BTreeMap<u32, XrefEntry>,
}

/// An update to append, and what the file holds once it is.
#[derive(Debug)]
pub(crate) struct Update {
    pub(crate) bytes: Vec<u8>,
    pub(crate) tail: Tail,
}

impl Tail {
    /// The file `document` was read from, as it is: no update appended.
    /// A file whose cross-reference was rebuilt has no section to append
    /// after ([`PdfError::RebuiltNotEditable`]).
    pub(crate) fn of_file(document: &Document) -> Result<Self, PdfError> {
        let file = document.file();
        let newest = match file.sections().first() {
            Some(newest) if !file.xref_rebuilt() => newest,
            _ => return Err(PdfError::RebuiltNotEditable),
        };
        let length = file.source().len();
        let ends_in_line = match length.checked_sub(1) {
            Some(last) => matches!(file.source().read(last, 1)?.first(), Some(b'\n' | b'\r')),
            None => true,
        };
        Ok(Self {
            length,
            xref_offset: newest.offset(),
            ends_in_line,
            state: document.unedited(),
            written: BTreeMap::new(),
        })
    }

    /// The length of the file.
    pub(crate) fn length(&self) -> u64 {
        self.length
    }

    /// What the file says of object `number`.
    fn entry(&self, number: u32) -> Option<XrefEntry> {
        self.written
            .get(&number)
            .copied()
            .or_else(|| self.state.file().entry(number))
    }

    /// Whether `document` is a state other than the one the file holds.
    pub(crate) fn differs(&self, document: &Document) -> bool {
        !self.state.same_edits(document)
    }
}

/// The objects an update writes, the entries it holds without writing
/// anything, and the numbers it frees.
struct Plan {
    /// Objects to write, in order of number.
    writes: Vec<Arc<DocObject>>,
    /// Entries that need no object written.
    entries: BTreeMap<u32, XrefEntry>,
    /// Numbers that are free from now on, and the generation of the free
    /// entry.
    freed: BTreeMap<u32, u16>,
}

/// The update that turns the file `from` holds into `to`, a snapshot of
/// the same file. `progress` counts the objects written; a cancelled
/// write is [`PdfError::Cancelled`].
pub(crate) fn incremental(
    from: &Tail,
    to: &Document,
    options: &WriteOptions,
    progress: &dyn Progress,
) -> Result<Update, PdfError> {
    if !from.state.same_file(to) {
        return Err(PdfError::Invalid(
            "the document to save is not a snapshot of the file the update is appended to"
                .to_string(),
        ));
    }
    let streams = to
        .file()
        .sections()
        .first()
        .is_some_and(|newest| newest.form() == SectionForm::Stream);
    let Plan {
        writes,
        mut entries,
        freed,
    } = plan(from, to, streams)?;
    link_free_entries(from, &writes, &mut entries, &freed);

    let mut ivs = IvSource::new(options.seed);
    let mut writer = Writer::new(to, &mut ivs)?;
    writer.start = from.length;
    if !from.ends_in_line {
        writer.out.push(b'\n');
    }
    let total = writes.len() as u64;
    for (done, object) in writes.iter().enumerate() {
        if progress.is_cancelled() {
            return Err(PdfError::Cancelled);
        }
        progress.report(done as u64, total, "objects");
        let offset = writer.write_at_end(object)?;
        entries.insert(
            object.id.number,
            XrefEntry::InUse {
                offset,
                generation: object.id.generation,
            },
        );
    }
    progress.report(total, total, "objects");

    let trailer = written_trailer(to, to.trailer(), options);
    let xref_offset = if streams {
        stream_section(&mut writer, &mut entries, trailer, from.xref_offset)?
    } else {
        table_section(&mut writer, &entries, trailer, from.xref_offset)
    };
    let mut written = from.written.clone();
    written.extend(entries);
    let length = from.length + writer.out.len() as u64;
    Ok(Update {
        bytes: writer.out,
        tail: Tail {
            length,
            xref_offset,
            ends_in_line: true,
            state: to.clone(),
            written,
        },
    })
}

/// What differs between the file `from` holds and `to`.
fn plan(from: &Tail, to: &Document, streams: bool) -> Result<Plan, PdfError> {
    let file = to.file();
    let candidates: BTreeSet<u32> = from
        .state
        .edited_numbers()
        .chain(to.edited_numbers())
        .collect();
    let mut plan = Plan {
        writes: Vec::new(),
        entries: BTreeMap::new(),
        freed: BTreeMap::new(),
    };
    for number in candidates {
        let after = to.edit(number);
        if Edit::same(from.state.edit(number), after) {
            continue;
        }
        match after {
            Some(Edit::Set(object)) => plan.writes.push(object.clone()),
            Some(Edit::Removed(generation)) => {
                plan.freed.insert(number, generation.saturating_add(1));
            }
            // The document does not edit it: the file's own object is
            // there again.
            None => match file.entry(number) {
                Some(entry @ XrefEntry::InUse { .. }) => {
                    plan.entries.insert(number, entry);
                }
                Some(entry @ XrefEntry::Compressed { .. }) if streams => {
                    plan.entries.insert(number, entry);
                }
                // A table cannot name an object in an object stream (Table
                // 18 is the stream's): the object is written.
                Some(XrefEntry::Compressed { .. }) => {
                    plan.writes.push(to.required(ObjectId {
                        number,
                        generation: 0,
                    })?);
                }
                Some(XrefEntry::Free { generation, .. }) => {
                    plan.freed.insert(number, generation);
                }
                // An object the earlier state had and the file never did.
                Some(XrefEntry::Null) | None => {
                    plan.freed
                        .insert(number, from.state.free_generation(number));
                }
            },
        }
    }
    Ok(plan)
}

/// Adds to `entries` the free entries the update holds, and entry 0.
fn link_free_entries(
    from: &Tail,
    writes: &[Arc<DocObject>],
    entries: &mut BTreeMap<u32, XrefEntry>,
    freed: &BTreeMap<u32, u16>,
) {
    let is_free = |number: u32| matches!(from.entry(number), Some(XrefEntry::Free { .. }));
    // Free in the file and in use after the update.
    let in_use_again: HashSet<u32> = writes
        .iter()
        .map(|object| object.id.number)
        .chain(entries.keys().copied())
        .filter(|&number| is_free(number))
        .collect();
    let newly_free: Vec<u32> = freed
        .keys()
        .copied()
        .filter(|&number| !is_free(number))
        .collect();
    let mut chain = newly_free;
    chain.extend(
        free_list(from)
            .into_iter()
            .filter(|number| !in_use_again.contains(number)),
    );
    let generation_of = |number: u32| match freed.get(&number) {
        Some(generation) => *generation,
        None => match from.entry(number) {
            Some(XrefEntry::Free { generation, .. }) => generation,
            _ => 0,
        },
    };
    for (index, &number) in chain.iter().enumerate() {
        let entry = XrefEntry::Free {
            next: chain.get(index + 1).copied().unwrap_or(0),
            generation: generation_of(number),
        };
        if from.entry(number) != Some(entry) {
            entries.insert(number, entry);
        }
    }
    // A number that was free already and stays off the list keeps its
    // place, with the generation it now has.
    let listed: HashSet<u32> = chain.iter().copied().collect();
    for (&number, &generation) in freed {
        if let Some(XrefEntry::Free {
            next,
            generation: old,
        }) = from.entry(number)
        {
            if !listed.contains(&number) && old != generation {
                entries.insert(number, XrefEntry::Free { next, generation });
            }
        }
    }
    let head = chain.first().copied().unwrap_or(0);
    let old_head = match from.entry(0) {
        Some(XrefEntry::Free { next, .. }) => Some(next),
        _ => None,
    };
    if old_head != Some(head) && !(old_head.is_none() && head == 0) {
        entries.insert(
            0,
            XrefEntry::Free {
                next: head,
                generation: FREE_HEAD_GENERATION,
            },
        );
    }
}

/// The free list of the file as it is: the numbers entry 0 leads to, in
/// order. The list ends at 0, at an entry that is not free, and where it
/// would come back to a number it has.
fn free_list(from: &Tail) -> Vec<u32> {
    let mut list = Vec::new();
    let mut seen = HashSet::new();
    let mut at = match from.entry(0) {
        Some(XrefEntry::Free { next, .. }) => next,
        _ => 0,
    };
    while at != 0 && seen.insert(at) {
        match from.entry(at) {
            Some(XrefEntry::Free { next, .. }) => {
                list.push(at);
                at = next;
            }
            _ => break,
        }
    }
    list
}

/// The entries in subsections: each run of consecutive numbers (7.5.4).
fn subsections(entries: &BTreeMap<u32, XrefEntry>) -> Vec<(u32, Vec<XrefEntry>)> {
    let mut runs: Vec<(u32, Vec<XrefEntry>)> = Vec::new();
    for (&number, &entry) in entries {
        match runs.last_mut() {
            Some((first, run)) if u64::from(*first) + run.len() as u64 == u64::from(number) => {
                run.push(entry);
            }
            _ => runs.push((number, vec![entry])),
        }
    }
    runs
}

/// `Size` and `Prev` added to `trailer`: one more than the highest object
/// number of the file (Table 15), and the section before this one.
fn with_size_and_prev(size: u64, trailer: Dictionary, prev: u64) -> Dictionary {
    // A count of entries and an offset in a file are far below i64::MAX.
    let mut trailer = with_size(size as usize, trailer);
    trailer.insert(
        Name::new(PREV_KEY),
        Object::new(Value::Integer(prev as i64)),
    );
    trailer
}

/// The size an update gives the file: past every number it has an entry
/// for and past the next number a new object would take.
fn size_of(entries: &BTreeMap<u32, XrefEntry>, next_number: u32) -> u64 {
    let highest = entries.keys().next_back().map_or(0, |&n| u64::from(n) + 1);
    highest.max(u64::from(next_number))
}

/// 7.5.4, 7.5.5: the table with its subsections and the trailer. Gives the
/// offset of the section.
fn table_section(
    writer: &mut Writer<'_>,
    entries: &BTreeMap<u32, XrefEntry>,
    trailer: Dictionary,
    prev: u64,
) -> u64 {
    let xref_offset = writer.offset();
    let size = size_of(entries, writer.next_number);
    let out = &mut writer.out;
    out.extend_from_slice(b"xref\n");
    for (first, run) in subsections(entries) {
        let _ = writeln!(out, "{first} {}", run.len());
        for entry in &run {
            push_row(out, entry);
        }
    }
    out.extend_from_slice(b"trailer\n");
    let trailer = with_size_and_prev(size, trailer, prev);
    // The trailer was read from the file or built by edits within the
    // nesting limit, so writing it cannot pass it.
    let _ = write_object(&Object::new(Value::Dictionary(trailer)), out);
    out.push(b'\n');
    end_of_file(out, xref_offset);
    xref_offset
}

/// 7.5.8: the cross-reference stream, which is never encrypted (7.5.8.2),
/// with an `Index` of the subsections. Gives the offset of the section.
fn stream_section(
    writer: &mut Writer<'_>,
    entries: &mut BTreeMap<u32, XrefEntry>,
    trailer: Dictionary,
    prev: u64,
) -> Result<u64, PdfError> {
    let id = writer.take_number()?;
    let xref_offset = writer.offset();
    entries.insert(
        id.number,
        XrefEntry::InUse {
            offset: xref_offset,
            generation: 0,
        },
    );
    let size = size_of(entries, writer.next_number);
    let runs = subsections(entries);
    let rows: Vec<XrefEntry> = runs
        .iter()
        .flat_map(|(_, run)| run.iter().copied())
        .collect();
    let (data, second, third) = pack(&rows);
    let encoded = flate_encode(&data)?;
    let mut dictionary = Dictionary::new();
    dictionary.insert(Name::new(b"Type".as_slice()), name(b"XRef"));
    for (key, value) in with_size_and_prev(size, trailer, prev).iter() {
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
    let index = runs
        .iter()
        .flat_map(|(first, run)| [integer(*first as usize), integer(run.len())])
        .collect();
    dictionary.insert(
        Name::new(b"Index".as_slice()),
        Object::new(Value::Array(index)),
    );
    dictionary.insert(Name::new(b"Filter".as_slice()), name(b"FlateDecode"));
    writer.stream_object(id, dictionary, &encoded)?;
    end_of_file(&mut writer.out, xref_offset);
    Ok(xref_offset)
}

#[cfg(test)]
mod tests {
    use super::*;

    const FREE: XrefEntry = XrefEntry::Free {
        next: 0,
        generation: 1,
    };

    fn entries(numbers: &[u32]) -> BTreeMap<u32, XrefEntry> {
        numbers.iter().map(|&number| (number, FREE)).collect()
    }

    #[test]
    fn consecutive_numbers_share_a_subsection() {
        let runs = subsections(&entries(&[0, 3, 4, 5, 9, 10]));
        let shape: Vec<(u32, usize)> = runs
            .iter()
            .map(|(first, run)| (*first, run.len()))
            .collect();
        assert_eq!(shape, [(0, 1), (3, 3), (9, 2)]);
    }

    #[test]
    fn no_entries_are_no_subsections() {
        assert!(subsections(&BTreeMap::new()).is_empty());
    }

    #[test]
    fn the_size_is_past_every_entry_and_the_next_number() {
        assert_eq!(size_of(&entries(&[0, 7]), 5), 8);
        assert_eq!(size_of(&entries(&[0, 7]), 20), 20);
        assert_eq!(size_of(&BTreeMap::new(), 4), 4);
    }

    #[test]
    fn a_subsection_at_the_top_of_the_number_range_does_not_overflow() {
        let runs = subsections(&entries(&[u32::MAX - 1, u32::MAX]));
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].1.len(), 2);
        assert_eq!(size_of(&entries(&[u32::MAX]), 1), u64::from(u32::MAX) + 1);
    }
}
