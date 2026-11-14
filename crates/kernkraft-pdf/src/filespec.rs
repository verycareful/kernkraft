// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! File specifications: ISO 32000-1:2008, 7.11.2 (strings), 7.11.3
//! (dictionaries, Table 44) and 7.11.5 (URL specifications).
//!
//! The name of the file is taken from `UF`, then `F`, then the obsolescent
//! `Unix`, `Mac` and `DOS`, as 7.11.3 asks readers to prefer `F` and `UF`.
//! 7.11.2 leaves the encoding of a file specification string to the
//! platform; it is read as a text string (7.9.2.2), which is what `UF`
//! is, and [`TextString::exact`](crate::doc::TextString) says when a byte
//! had no character.

use crate::doc::{decode_text_string, Document, Resolved, TextString};
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;

const FS_KEY: &[u8] = b"FS";
const URL_FILE_SYSTEM: &[u8] = b"URL";
const DESC_KEY: &[u8] = b"Desc";
const EF_KEY: &[u8] = b"EF";
/// Table 44: the entries that name the file, in the order a reader
/// prefers them.
const NAME_KEYS: [&[u8]; 5] = [b"UF", b"F", b"Unix", b"Mac", b"DOS"];

/// A file specification, string or dictionary.
#[derive(Debug, Clone, PartialEq)]
pub struct FileSpec {
    /// The file's name, or the URL when [`Self::url`] is set.
    pub name: Option<TextString>,
    /// Whether the file system is `URL` (7.11.5): [`Self::name`] is a
    /// uniform resource locator.
    pub url: bool,
    /// `Desc`.
    pub description: Option<TextString>,
    /// The embedded file stream of `EF`, taken from the same entry the
    /// name was, else from the first that `EF` has.
    pub embedded: Option<ObjectId>,
}

impl Document {
    /// The file specification `object` is, or `None` when it is neither
    /// a string nor a dictionary.
    pub(crate) fn file_spec(&self, object: &Object) -> Result<Option<FileSpec>, PdfError> {
        let resolved = self.resolve(object)?;
        match resolved.value() {
            Value::String(name) => Ok(Some(FileSpec {
                name: Some(decode_text_string(name.as_bytes())),
                url: false,
                description: None,
                embedded: None,
            })),
            Value::Dictionary(dictionary) => self.file_spec_dictionary(dictionary).map(Some),
            _ => Ok(None),
        }
    }

    fn file_spec_dictionary(&self, dictionary: &Dictionary) -> Result<FileSpec, PdfError> {
        let url = matches!(
            self.get(dictionary, FS_KEY)?.value(),
            Value::Name(name) if name.as_bytes() == URL_FILE_SYSTEM
        );
        let mut name = None;
        let mut name_key = None;
        for key in NAME_KEYS {
            if let Some(text) = self.text(dictionary, key)? {
                name = Some(text);
                name_key = Some(key);
                break;
            }
        }
        let description = self.text(dictionary, DESC_KEY)?;
        let embedded = self.embedded_stream(dictionary, name_key)?;
        Ok(FileSpec {
            name,
            url,
            description,
            embedded,
        })
    }

    /// The stream of `EF` under `preferred`, else under the first of
    /// [`NAME_KEYS`] that `EF` has.
    fn embedded_stream(
        &self,
        dictionary: &Dictionary,
        preferred: Option<&[u8]>,
    ) -> Result<Option<ObjectId>, PdfError> {
        let ef = self.get(dictionary, EF_KEY)?;
        let Some(ef) = ef.dictionary() else {
            return Ok(None);
        };
        let keys = preferred.into_iter().chain(NAME_KEYS);
        for key in keys {
            if let Some(Value::Reference(id)) = ef.get(key).map(Object::value) {
                return Ok(Some(*id));
            }
        }
        Ok(None)
    }

    /// The text string `key` of `dictionary`, when it is a string.
    pub(crate) fn text(
        &self,
        dictionary: &Dictionary,
        key: &[u8],
    ) -> Result<Option<TextString>, PdfError> {
        Ok(match self.get(dictionary, key)? {
            Resolved::Null => None,
            resolved => match resolved.value() {
                Value::String(string) => Some(decode_text_string(string.as_bytes())),
                _ => None,
            },
        })
    }
}
