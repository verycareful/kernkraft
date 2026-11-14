// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Object streams: ISO 32000-1:2008, 7.5.7 (Table 16).

use std::borrow::Cow;

use crate::diagnostic::Diagnostics;
use crate::file::is_damage;
use crate::file::xref::unsigned;
use crate::syntax::{Dictionary, Lexer, Object, ObjectId, Parser, TokenKind, Value};
use crate::{MemorySource, PdfError, Source};

const TYPE_KEY: &[u8] = b"Type";
const OBJECT_STREAM_TYPE: &[u8] = b"ObjStm";
const COUNT_KEY: &[u8] = b"N";
const FIRST_KEY: &[u8] = b"First";

/// The decoded data of one object stream and where each object in it
/// starts.
#[derive(Debug, Clone)]
pub struct ObjectStream {
    number: u32,
    data: MemorySource,
    /// The object number of each object and the offset in `data` where it
    /// starts, in the order the stream lists them.
    objects: Vec<(u32, u64)>,
}

impl ObjectStream {
    /// Reads the table at the start of `data`, the decoded data of object
    /// stream `number`, as `dictionary` describes it: `N` pairs of an
    /// object number and an offset from `First`, the offsets increasing.
    ///
    /// Anything else is [`PdfError::ObjectStream`].
    pub fn parse(number: u32, dictionary: &Dictionary, data: Vec<u8>) -> Result<Self, PdfError> {
        let bad = || PdfError::ObjectStream { stream: number };
        match dictionary.get(TYPE_KEY).map(Object::value) {
            Some(Value::Name(name)) if name.as_bytes() == OBJECT_STREAM_TYPE => {}
            _ => return Err(bad()),
        }
        let required = |key| unsigned(dictionary, key).flatten().ok_or_else(bad);
        let count = required(COUNT_KEY)?;
        let first = required(FIRST_KEY)?;
        // A slice is at most isize::MAX bytes, which fits a u64.
        let size = data.len() as u64;
        if first > size {
            return Err(bad());
        }

        let data = MemorySource::new(data);
        let mut lexer = Lexer::new(&data);
        // The lexer judges every number as a PDF integer; the numbers of
        // the table are not objects, so what it says of them is not kept.
        let mut unkept = Diagnostics::new();
        let mut objects = Vec::new();
        let mut previous = None;
        for _ in 0..count {
            let mut field = || -> Result<u64, PdfError> {
                let token = lexer.next_token(&mut unkept).map_err(|error| {
                    if is_damage(&error) {
                        bad()
                    } else {
                        error
                    }
                })?;
                match token.kind {
                    TokenKind::Integer(value) if token.span.end <= first => {
                        u64::try_from(value).map_err(|_| bad())
                    }
                    _ => Err(bad()),
                }
            };
            let object_number = u32::try_from(field()?).map_err(|_| bad())?;
            let offset = field()?;
            if previous.is_some_and(|previous| offset <= previous) {
                return Err(bad());
            }
            previous = Some(offset);
            let start = first
                .checked_add(offset)
                .filter(|&start| start < size)
                .ok_or_else(bad)?;
            objects.push((object_number, start));
        }
        Ok(Self {
            number,
            data,
            objects,
        })
    }

    /// The object number of the stream itself.
    pub fn number(&self) -> u32 {
        self.number
    }

    /// How many objects the stream holds.
    pub fn len(&self) -> usize {
        self.objects.len()
    }

    pub fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    /// The object number of each object, in the order the stream lists
    /// them: the position of a number is the index a cross-reference
    /// entry names.
    pub fn object_numbers(&self) -> impl Iterator<Item = u32> + '_ {
        self.objects.iter().map(|&(number, _)| number)
    }

    /// The decoded data. The spans of an object read from the stream are
    /// offsets into it.
    pub fn data(&self) -> Result<Cow<'_, [u8]>, PdfError> {
        let len = usize::try_from(self.data.len()).unwrap_or(usize::MAX);
        self.data.read(0, len)
    }

    /// The object at `index`, which must be object `number`. Its spans and
    /// the offsets of its diagnostics are positions in [`Self::data`], and
    /// its diagnostics name the object.
    ///
    /// `max_elements` is the most array items and dictionary entries the
    /// object may hold. An index the stream does not have, another object
    /// number at that index, an object that cannot be read and an object
    /// that is only an indirect reference are [`PdfError::ObjectStream`].
    pub fn object(
        &self,
        index: u32,
        number: u32,
        max_elements: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Object, PdfError> {
        let bad = || PdfError::ObjectStream {
            stream: self.number,
        };
        let &(found, start) = usize::try_from(index)
            .ok()
            .and_then(|index| self.objects.get(index))
            .ok_or_else(bad)?;
        if found != number {
            return Err(bad());
        }
        let mut parser = Parser::new(&self.data).with_max_elements(max_elements);
        parser.seek(start)?;
        // 7.5.7: a compressed object has generation 0.
        diagnostics.set_object(Some(ObjectId {
            number,
            generation: 0,
        }));
        let result = parser.parse_object(diagnostics);
        diagnostics.set_object(None);
        match result {
            Ok(object) if matches!(object.value(), Value::Reference(_)) => Err(bad()),
            Ok(object) => Ok(object),
            Err(error) if is_damage(&error) => Err(bad()),
            Err(error) => Err(error),
        }
    }
}
