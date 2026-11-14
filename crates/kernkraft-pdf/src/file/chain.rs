// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The chain of cross-reference sections: ISO 32000-1:2008, 7.5.5
//! (`startxref` and the trailer's `Prev`), 7.5.6 (incremental updates),
//! 7.5.8.1 (a cross-reference stream as a section) and 7.5.8.4
//! (`XRefStm` in a hybrid-reference file).

use std::collections::HashSet;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::load::{Body, LengthFrom, Reader};
use crate::file::scan::find_last_in;
use crate::file::xref::{
    read_table, size, unsigned, Budget, Entries, Section, SectionForm, StreamLayout,
};
use crate::file::ReadLimits;
use crate::filter::{FilterKind, Pipeline};
use crate::syntax::chars::is_whitespace;
use crate::syntax::{Dictionary, Lexer, TokenKind};
use crate::{PdfError, Source};

/// How far from the end of the file `startxref` is looked for. Adobe's
/// PDF Reference, sixth edition, Appendix H, implementation note 18 (on
/// 3.4.4, File Trailer): the end-of-file marker may appear anywhere
/// within the last 1024 bytes.
pub const TRAILER_SEARCH_BYTES: usize = 1024;

const STARTXREF: &[u8] = b"startxref";
const EOF_MARKER: &[u8] = b"%%EOF";
const PREV_KEY: &[u8] = b"Prev";
const HYBRID_KEY: &[u8] = b"XRefStm";

/// Reads every cross-reference section, from the one `startxref` names
/// back through each `Prev`: the newest first.
///
/// A fault in the cross-reference data is an error that
/// [`is_damage`](crate::file::is_damage) recognises.
pub(crate) fn read_chain(
    source: &dyn Source,
    limits: &ReadLimits,
    diagnostics: &mut Diagnostics,
) -> Result<Vec<Section>, PdfError> {
    let reader = Reader {
        source,
        sections: &[],
        limits,
        decrypt: None,
    };
    let mut budget = Budget::new(limits.max_xref_entries);
    let mut sections = Vec::new();
    let mut seen = HashSet::new();
    let mut next = Some(find_startxref(source, diagnostics)?);
    while let Some(offset) = next {
        // A section met twice means the chain runs in a circle.
        if !seen.insert(offset) {
            return Err(PdfError::BadXref { offset });
        }
        let section = read_section(&reader, offset, &mut budget, diagnostics)?;
        next = unsigned(&section.trailer, PREV_KEY).ok_or(PdfError::BadXref { offset })?;
        sections.push(section);
    }
    Ok(sections)
}

/// The offset that follows the last `startxref` of the file (7.5.5).
fn find_startxref(source: &dyn Source, diagnostics: &mut Diagnostics) -> Result<u64, PdfError> {
    let size = source.len();
    let len =
        usize::try_from(size).map_or(TRAILER_SEARCH_BYTES, |size| size.min(TRAILER_SEARCH_BYTES));
    // `len` is at most the size, and fits a u64.
    let tail_start = size - len as u64;
    let tail = source.read(tail_start, len)?;
    let Some(position) = find_last_in(&tail, STARTXREF) else {
        return Err(PdfError::BadXref { offset: size });
    };
    let keyword = tail_start + position as u64;

    let mut lexer = Lexer::new(source);
    lexer.seek(keyword + STARTXREF.len() as u64)?;
    // The offset is not a PDF integer object.
    let token = lexer.next_token(&mut Diagnostics::new())?;
    let offset = match token.kind {
        TokenKind::Integer(offset) => u64::try_from(offset).ok(),
        _ => None,
    }
    .ok_or(PdfError::BadXref { offset: keyword })?;

    let cursor = lexer.cursor();
    while cursor.peek()?.is_some_and(is_whitespace) {
        cursor.advance(1);
    }
    for (ahead, expected) in (0..).zip(EOF_MARKER) {
        if cursor.peek_ahead(ahead)? != Some(*expected) {
            diagnostics.push(DiagnosticKind::MissingEofMarker, token.span.end);
            break;
        }
    }
    Ok(offset)
}

/// The cross-reference section at `offset`: a table with its trailer, or
/// a cross-reference stream.
fn read_section(
    reader: &Reader<'_>,
    offset: u64,
    budget: &mut Budget,
    diagnostics: &mut Diagnostics,
) -> Result<Section, PdfError> {
    let bad = || PdfError::BadXref { offset };
    if offset >= reader.source.len() {
        return Err(bad());
    }
    let mut parser = reader.parser();
    parser.seek(offset)?;
    let token = parser.lexer().next_token(&mut Diagnostics::new())?;
    if token.span.start != offset {
        diagnostics.push(DiagnosticKind::WhitespaceAtOffset, offset);
    }
    let section = match token.kind {
        TokenKind::Keyword(keyword) if keyword == b"xref" => {
            let (entries, trailer) = read_table(&mut parser, offset, budget, diagnostics)?;
            let size = size(&trailer, offset)?;
            let hybrid = match unsigned(&trailer, HYBRID_KEY).ok_or_else(bad)? {
                None => None,
                Some(stream_offset) => {
                    let (entries, _) = read_stream(reader, stream_offset, budget, diagnostics)?;
                    Some((stream_offset, entries))
                }
            };
            Section {
                offset,
                form: SectionForm::Table,
                trailer,
                size,
                entries,
                hybrid,
            }
        }
        TokenKind::Integer(_) => {
            let (entries, trailer) = read_stream(reader, token.span.start, budget, diagnostics)?;
            let size = size(&trailer, offset)?;
            Section {
                offset,
                form: SectionForm::Stream,
                trailer,
                size,
                entries,
                hybrid: None,
            }
        }
        _ => return Err(bad()),
    };
    let hybrid_end = section
        .hybrid
        .as_ref()
        .map_or(0, |(_, entries)| entries.end());
    if section.entries.end().max(hybrid_end) > section.size {
        diagnostics.push(DiagnosticKind::EntryPastSize, offset);
    }
    Ok(section)
}

/// The entries and the dictionary of the cross-reference stream whose
/// object starts at `offset` (7.5.8).
fn read_stream(
    reader: &Reader<'_>,
    offset: u64,
    budget: &mut Budget,
    diagnostics: &mut Diagnostics,
) -> Result<(Entries, Dictionary), PdfError> {
    let bad = || PdfError::BadXref { offset };
    if offset >= reader.source.len() {
        return Err(bad());
    }
    let mut parser = reader.parser();
    parser.seek(offset)?;
    let indirect = parser.parse_indirect_object(diagnostics)?;
    let id = indirect.id;
    // No cross-reference is known yet, so a `Length` that is an indirect
    // reference cannot be followed.
    let object = reader.finish(indirect, LengthFrom::Nowhere, diagnostics)?;
    let Body::Stream(stream) = object.body else {
        return Err(bad());
    };
    let layout = StreamLayout::read(&stream.dictionary, offset)?;
    budget.take(layout.entry_count, offset)?;

    let encoded = reader.stream_data(&stream)?;
    // 7.5.8.2: `Filter` and `DecodeParms` are direct objects here, which
    // is how the pipeline reads them.
    // It is never encrypted and shall not name a Crypt filter.
    let pipeline = match Pipeline::from_dictionary(&stream.dictionary) {
        Ok(pipeline)
            if pipeline
                .filters()
                .iter()
                .any(|filter| filter.kind() == FilterKind::Crypt) =>
        {
            return Err(bad())
        }
        Ok(pipeline) => pipeline,
        Err(PdfError::UnknownFilter { .. } | PdfError::FilterParameters { .. }) => {
            return Err(bad())
        }
        Err(error) => return Err(error),
    };
    diagnostics.set_object(Some(id));
    let decoded = pipeline.decode(&encoded, layout.data_bytes, stream.data.start, diagnostics);
    diagnostics.set_object(None);
    let decoded = match decoded {
        Ok(decoded) if decoded.complete => decoded,
        // More data than the entries the dictionary declares, or data
        // that could not all be decoded.
        Ok(_) | Err(PdfError::DecodedTooLarge { .. }) => return Err(bad()),
        Err(error) => return Err(error),
    };
    let entries = layout.entries(&decoded.data, offset)?;
    Ok((entries, stream.dictionary))
}
