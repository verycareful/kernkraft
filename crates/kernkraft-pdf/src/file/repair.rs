// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A cross-reference rebuilt from the body of the file, for a file whose
//! own cannot be read.
//!
//! ISO 32000 does not describe this: it is what a reader does with a
//! file that 7.5.4 and 7.5.5 call invalid. The file is read through once
//! for the headers of indirect objects (7.3.10) and for trailers
//! (7.5.5). Of two objects with one number, the later in the file
//! counts, as it would after an incremental update (7.5.6). The objects
//! of every object stream found (7.5.7) count as defined where their
//! stream lies. In an encrypted file (7.6) the object streams are
//! encrypted, so their objects are found only once the file's key is
//! known.

use std::collections::{BTreeMap, HashMap};

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::load::{Body, FileObject, LengthFrom, Location, Reader};
use crate::file::xref::{Budget, Entries, Section, SectionForm, Subsection, XrefEntry};
use crate::file::{is_damage, is_not_syntax, names_root, ReadLimits, StreamDecrypt, ROOT_KEY};
use crate::syntax::chars::{is_regular, is_whitespace};
use crate::syntax::{Cursor, Dictionary, Name, Object, ObjectId, Value};
use crate::{PdfError, Source};

/// The longest run of regular characters the search keeps whole: the ten
/// digits of the largest object number, which is longer than the two
/// keywords looked for.
const MAX_RUN_BYTES: usize = 10;

const TYPE_KEY: &[u8] = b"Type";
const SIZE_KEY: &[u8] = b"Size";
const CATALOG_TYPE: &[u8] = b"Catalog";
const OBJECT_STREAM_TYPE: &[u8] = b"ObjStm";
const XREF_TYPE: &[u8] = b"XRef";
/// The entries of a trailer (Table 15) that do not describe the
/// cross-reference it belonged to.
const KEPT_TRAILER_KEYS: [&[u8]; 4] = [ROOT_KEY, b"Info", b"ID", b"Encrypt"];

/// An indirect object found in the file.
struct Found {
    id: ObjectId,
    offset: u64,
    is_object_stream: bool,
    is_catalog: bool,
}

/// What reading the file through found, in file order.
#[derive(Default)]
struct Survey {
    objects: Vec<Found>,
    /// Trailer dictionaries and the dictionaries of cross-reference
    /// streams.
    trailers: Vec<Dictionary>,
}

/// A decimal number that stood alone between white space.
#[derive(Clone, Copy)]
struct Number {
    start: u64,
    value: u64,
}

/// Builds the one section of a file whose cross-reference is not used.
/// `start` is where the header is. Object streams are read through
/// `decrypt` when there is one; without it those of an encrypted file
/// cannot be read and are reported.
pub(crate) fn rebuild(
    source: &dyn Source,
    start: u64,
    limits: &ReadLimits,
    decrypt: Option<&dyn StreamDecrypt>,
    diagnostics: &mut Diagnostics,
) -> Result<Section, PdfError> {
    let mut budget = Budget::new(limits.max_xref_entries);
    let survey = survey(source, start, limits, &mut budget)?;

    // Every object that lies in the file itself, for following a `Length`
    // that is an indirect reference.
    let mut in_file = BTreeMap::new();
    for found in &survey.objects {
        in_file.insert(found.id.number, in_use(found));
    }
    let provisional = [section(&in_file, Dictionary::new())?];
    let reader = Reader {
        source,
        sections: &provisional,
        limits,
        decrypt,
    };

    let rooted = survey.trailers.iter().rev().find(|t| names_root(t));
    let mut table = BTreeMap::new();
    // The offset of the object stream each compressed entry came from.
    let mut origins = HashMap::new();
    // The offset of the last object stream read under each object number.
    let mut stream_offsets = HashMap::new();
    // Objects that say they are the catalog, with their entry.
    let mut catalogs = Vec::new();
    for found in &survey.objects {
        let number = found.id.number;
        table.insert(number, in_use(found));
        if found.is_catalog {
            catalogs.push((number, in_use(found)));
        }
        if !found.is_object_stream || found.id.generation != 0 {
            continue;
        }
        // The diagnostics of an object are raised when it is loaded, not
        // while it is looked for.
        let mut unkept = Diagnostics::new();
        let stream = reader
            .object_at(found.offset, found.id, LengthFrom::File, &mut unkept)
            .and_then(|object| reader.decode_object_stream(&object, &mut unkept));
        let stream = match stream {
            Ok(stream) => stream,
            Err(error) if is_damage(&error) => {
                diagnostics.push(
                    DiagnosticKind::ObjectStreamUnreadable { stream: number },
                    found.offset,
                );
                continue;
            }
            Err(error) => return Err(error),
        };
        stream_offsets.insert(number, found.offset);
        for (index, compressed) in (0..).zip(stream.object_numbers()) {
            budget.take(1, found.offset)?;
            let entry = XrefEntry::Compressed {
                stream: number,
                index,
            };
            table.insert(compressed, entry);
            origins.insert(compressed, found.offset);
            if rooted.is_none() {
                let object =
                    stream.object(index, compressed, limits.max_object_elements, &mut unkept);
                match object {
                    Ok(object) if has_type(&object, CATALOG_TYPE) => {
                        catalogs.push((compressed, entry));
                    }
                    Ok(_) => {}
                    Err(error) if is_damage(&error) => {}
                    Err(error) => return Err(error),
                }
            }
        }
    }
    // A compressed entry counts only while the object stream it came from
    // is still the object its number names.
    let standing: HashMap<u32, u64> = stream_offsets
        .into_iter()
        .filter(|&(stream, offset)| {
            matches!(table.get(&stream), Some(XrefEntry::InUse { offset: at, .. }) if *at == offset)
        })
        .collect();
    table.retain(|number, entry| match *entry {
        XrefEntry::Compressed { stream, .. } => standing
            .get(&stream)
            .is_some_and(|offset| origins.get(number) == Some(offset)),
        _ => true,
    });

    let mut trailer = Dictionary::new();
    match rooted {
        Some(rooted) => {
            for key in KEPT_TRAILER_KEYS {
                if let Some(value) = rooted.get(key) {
                    trailer.insert(Name::new(key), Object::new(value.value().clone()));
                }
            }
        }
        None => {
            let (number, entry) = catalogs
                .iter()
                .rev()
                .find(|(number, entry)| table.get(number) == Some(entry))
                .ok_or(PdfError::NoCatalog)?;
            let generation = match entry {
                XrefEntry::InUse { generation, .. } => *generation,
                _ => 0,
            };
            let root = ObjectId {
                number: *number,
                generation,
            };
            trailer.insert(Name::new(ROOT_KEY), Value::Reference(root).into());
        }
    }
    section(&table, trailer)
}

fn in_use(found: &Found) -> XrefEntry {
    XrefEntry::InUse {
        offset: found.offset,
        generation: found.id.generation,
    }
}

/// The rebuilt section that holds `table`. `Size` is one more than the
/// highest object number (Table 15).
fn section(table: &BTreeMap<u32, XrefEntry>, mut trailer: Dictionary) -> Result<Section, PdfError> {
    let mut subsections: Vec<Subsection> = Vec::new();
    for (&number, &entry) in table {
        match subsections.last_mut() {
            Some(last)
                if u64::from(last.first) + last.entries.len() as u64 == u64::from(number) =>
            {
                last.entries.push(entry);
            }
            _ => subsections.push(Subsection {
                first: number,
                entries: vec![entry],
            }),
        }
    }
    let size = table
        .last_key_value()
        .map_or(0, |(&highest, _)| u64::from(highest) + 1);
    // One more than a u32 fits an i64.
    let size_value = Value::Integer(i64::try_from(size).unwrap_or(i64::MAX));
    trailer.insert(Name::new(SIZE_KEY), size_value.into());
    Ok(Section {
        offset: 0,
        form: SectionForm::Rebuilt,
        trailer,
        size,
        entries: Entries::new(subsections, 0)?,
        hybrid: None,
    })
}

fn has_type(object: &Object, expected: &[u8]) -> bool {
    match object.value() {
        Value::Dictionary(dictionary) => dictionary_has_type(dictionary, expected),
        _ => false,
    }
}

fn dictionary_has_type(dictionary: &Dictionary, expected: &[u8]) -> bool {
    matches!(
        dictionary.get(TYPE_KEY).map(Object::value),
        Some(Value::Name(name)) if name.as_bytes() == expected
    )
}

/// Reads the file through from `start` for indirect objects and trailers.
///
/// An object header is two numbers and `obj`, each a run of regular
/// characters with only white space between them. What follows a header
/// must read as an object, and reading goes on after it, so that nothing
/// inside an object or in the data of a stream is taken for a header.
fn survey(
    source: &dyn Source,
    start: u64,
    limits: &ReadLimits,
    budget: &mut Budget,
) -> Result<Survey, PdfError> {
    let reader = Reader {
        source,
        sections: &[],
        limits,
        decrypt: None,
    };
    let mut survey = Survey::default();
    let mut cursor = Cursor::new(source);
    cursor.seek(start.min(source.len()))?;

    let mut run: Vec<u8> = Vec::with_capacity(MAX_RUN_BYTES);
    let mut run_start = 0;
    let mut run_too_long = false;
    // The two numbers before the run being read, when only white space
    // lies between them and it.
    let mut numbers: [Option<Number>; 2] = [None, None];
    loop {
        let position = cursor.position();
        let byte = cursor.next()?;
        if let Some(byte) = byte.filter(|&byte| is_regular(byte)) {
            if run.is_empty() {
                run_start = position;
            }
            if run.len() < MAX_RUN_BYTES {
                run.push(byte);
            } else {
                run_too_long = true;
            }
            continue;
        }

        // The run, if there was one, ends before this byte.
        let ended = std::mem::take(&mut run);
        let whole = !std::mem::take(&mut run_too_long);
        // Any run takes the place of the numbers before it; white space
        // alone leaves them standing.
        let before = if ended.is_empty() {
            [None, None]
        } else {
            std::mem::take(&mut numbers)
        };
        if whole && !ended.is_empty() {
            if let Some(value) = decimal(&ended) {
                numbers = [
                    before[1],
                    Some(Number {
                        start: run_start,
                        value,
                    }),
                ];
            } else if ended == b"obj" {
                if let [Some(number), Some(generation)] = before {
                    let id = u32::try_from(number.value)
                        .ok()
                        .zip(u16::try_from(generation.value).ok())
                        .map(|(number, generation)| ObjectId { number, generation });
                    let read = match id {
                        Some(_) => read_object(&reader, number.start)?,
                        None => None,
                    };
                    if let Some((found, end, trailer)) = read {
                        budget.take(1, found.offset)?;
                        survey.objects.push(found);
                        survey.trailers.extend(trailer);
                        cursor.seek(end)?;
                        continue;
                    }
                }
            } else if ended == b"trailer" {
                if let Some((trailer, end)) = read_trailer(&reader, position)? {
                    survey.trailers.push(trailer);
                    cursor.seek(end)?;
                    continue;
                }
            }
        }

        match byte {
            None => return Ok(survey),
            // 7.2.3: a comment runs to the end of its line.
            Some(b'%') => {
                numbers = [None, None];
                while !matches!(cursor.peek()?, None | Some(b'\r' | b'\n')) {
                    cursor.advance(1);
                }
            }
            Some(byte) if is_whitespace(byte) => {}
            Some(_) => numbers = [None, None],
        }
    }
}

/// The value of a run that is only decimal digits.
fn decimal(run: &[u8]) -> Option<u64> {
    if !run.iter().all(u8::is_ascii_digit) {
        return None;
    }
    std::str::from_utf8(run).ok()?.parse().ok()
}

/// Reads the indirect object whose header starts at `start`. Returns what
/// it is, where it ends, and its dictionary when it is a cross-reference
/// stream. `None` when the bytes do not read as an object.
fn read_object(
    reader: &Reader<'_>,
    start: u64,
) -> Result<Option<(Found, u64, Option<Dictionary>)>, PdfError> {
    // The diagnostics of an object are raised when it is loaded, not
    // while it is looked for.
    let mut unkept = Diagnostics::new();
    let mut parser = reader.parser();
    parser.seek(start)?;
    let object = parser
        .parse_indirect_object(&mut unkept)
        .and_then(|indirect| reader.finish(indirect, LengthFrom::Nowhere, &mut unkept));
    let FileObject { id, body, location } = match object {
        Ok(object) => object,
        Err(error) if is_not_syntax(&error) => return Ok(None),
        Err(error) => return Err(error),
    };
    let Location::File(span) = location else {
        return Ok(None);
    };
    let mut found = Found {
        id,
        offset: start,
        is_object_stream: false,
        is_catalog: false,
    };
    let mut trailer = None;
    match body {
        Body::Object(object) => found.is_catalog = has_type(&object, CATALOG_TYPE),
        Body::Stream(stream) => {
            found.is_object_stream = dictionary_has_type(&stream.dictionary, OBJECT_STREAM_TYPE);
            if dictionary_has_type(&stream.dictionary, XREF_TYPE) {
                trailer = Some(stream.dictionary);
            }
        }
    }
    Ok(Some((found, span.end, trailer)))
}

/// Reads the dictionary that follows the `trailer` keyword ending at
/// `after`. Returns it and where it ends, or `None` when no dictionary
/// follows.
fn read_trailer(reader: &Reader<'_>, after: u64) -> Result<Option<(Dictionary, u64)>, PdfError> {
    let mut parser = reader.parser();
    parser.seek(after)?;
    match parser.parse_object(&mut Diagnostics::new()) {
        Ok(object) => {
            let end = parser.position();
            Ok(match object.into_value() {
                Value::Dictionary(dictionary) => Some((dictionary, end)),
                _ => None,
            })
        }
        Err(error) if is_not_syntax(&error) => Ok(None),
        Err(error) => Err(error),
    }
}
