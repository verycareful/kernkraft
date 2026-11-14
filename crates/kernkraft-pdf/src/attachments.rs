// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Attachments: ISO 32000-1:2008, 7.11.4 (embedded file streams, Tables 45
//! and 46), 7.7.4 (the `EmbeddedFiles` name tree of the name dictionary)
//! and 12.5.6.15 (file attachment annotations).
//!
//! A document's attachments are the file specifications of its
//! `EmbeddedFiles` name tree and those of the file attachment annotations
//! of its pages, each with the embedded file stream its `EF` names. A file
//! specification that embeds no file has nothing to extract; it is left
//! out and reported ([`DiagnosticKind::AttachmentNotEmbedded`]).
//!
//! Extracting an attachment decodes its stream within a size the caller
//! sets, and checks the result against the size and the MD5 checksum the
//! stream's parameters state, when they state them. [`Document::save_attachment`]
//! writes an attachment to a file only when it decoded whole and matches
//! both.

use std::path::Path;

use kernkraft_fs::{write_bytes_atomic, Expect, FileStamp};
use md5::{Digest, Md5};

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{decode_text_string, Date, Document, NameTree, TextString};
use crate::filespec::FileSpec;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const NAMES_KEY: &[u8] = b"Names";
const EMBEDDED_FILES_KEY: &[u8] = b"EmbeddedFiles";
const ANNOTS_KEY: &[u8] = b"Annots";
const SUBTYPE_KEY: &[u8] = b"Subtype";
const FILE_ATTACHMENT: &[u8] = b"FileAttachment";
const FS_KEY: &[u8] = b"FS";
const PARAMS_KEY: &[u8] = b"Params";
const SIZE_KEY: &[u8] = b"Size";
const CREATION_DATE_KEY: &[u8] = b"CreationDate";
const MOD_DATE_KEY: &[u8] = b"ModDate";
const CHECKSUM_KEY: &[u8] = b"CheckSum";
/// Table 46: `CheckSum` is a 16-byte MD5 digest (RFC 1321).
const CHECKSUM_BYTES: usize = 16;

/// Where an attachment is attached.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachmentPlace {
    /// To the document, in the `EmbeddedFiles` name tree.
    Document,
    /// To a page, by a file attachment annotation.
    /// `annotation` is the annotation's object, when it is an indirect
    /// one.
    Page {
        page: u32,
        annotation: Option<ObjectId>,
    },
}

/// One embedded file.
#[derive(Debug, Clone, PartialEq)]
pub struct Attachment {
    pub place: AttachmentPlace,
    /// The key of the `EmbeddedFiles` name tree, for an attachment of the
    /// document: a string the tree orders by its bytes, shown as text
    /// (7.9.6).
    pub key: Option<TextString>,
    /// The file specification: the file's name and description.
    pub file: FileSpec,
    /// The embedded file stream.
    pub stream: ObjectId,
    /// `Subtype`: the media type (RFC 2046), as its name's bytes.
    pub media_type: Option<Name>,
    /// `Params`' `Size`: the decoded size the file states.
    pub size: Option<u64>,
    pub created: Option<Date>,
    pub modified: Option<Date>,
    /// `Params`' `CheckSum`: the MD5 digest of the decoded file.
    pub checksum: Option<[u8; CHECKSUM_BYTES]>,
}

/// How an extracted file compares with what its parameters state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Check {
    /// The parameters state nothing to compare with.
    Absent,
    Matches,
    Differs,
}

/// An attachment's decoded data and how far it can be trusted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub data: Vec<u8>,
    /// False when the stream's data could not all be decoded; `data` is
    /// what came before the damage.
    pub complete: bool,
    pub size: Check,
    pub checksum: Check,
}

impl Extracted {
    /// Whether the data decoded whole and agrees with every size and
    /// checksum stated for it.
    pub fn is_sound(&self) -> bool {
        self.complete && self.size != Check::Differs && self.checksum != Check::Differs
    }
}

impl Document {
    /// Every attachment: those of the document in the name tree's order,
    /// then those of each page's annotations in page order.
    pub fn attachments(&self) -> Result<Vec<Attachment>, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let mut out = Vec::new();
        let catalog = self.catalog()?;
        if let Some(catalog) = catalog.dictionary() {
            let names = self.get(catalog, NAMES_KEY)?;
            if let Some(root) = names.dictionary().and_then(|n| n.get(EMBEDDED_FILES_KEY)) {
                for (key, spec) in NameTree::new(self, root.clone()).entries()? {
                    let place = AttachmentPlace::Document;
                    let key = Some(decode_text_string(&key));
                    if let Some(found) = self.attachment(place, key, &spec, &mut diagnostics)? {
                        out.push(found);
                    }
                }
            }
        }
        for number in 1..=self.page_count()? {
            let page = self.required(self.page_id(number)?)?;
            let Some(dictionary) = page.dictionary() else {
                continue;
            };
            let annots = self.get(dictionary, ANNOTS_KEY)?;
            for annot in annots.array().unwrap_or_default() {
                let resolved = self.resolve(annot)?;
                let Some(annot_dictionary) = resolved.dictionary() else {
                    continue;
                };
                let is_attachment = matches!(
                    self.get(annot_dictionary, SUBTYPE_KEY)?.value(),
                    Value::Name(subtype) if subtype.as_bytes() == FILE_ATTACHMENT
                );
                let Some(spec) = annot_dictionary.get(FS_KEY).filter(|_| is_attachment) else {
                    continue;
                };
                let place = AttachmentPlace::Page {
                    page: number,
                    annotation: resolved.id(),
                };
                if let Some(found) = self.attachment(place, None, spec, &mut diagnostics)? {
                    out.push(found);
                }
            }
        }
        self.record_diagnostics(diagnostics);
        Ok(out)
    }

    fn attachment(
        &self,
        place: AttachmentPlace,
        key: Option<TextString>,
        spec: &Object,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Attachment>, PdfError> {
        let spec_id = match spec.value() {
            Value::Reference(id) => Some(*id),
            _ => None,
        };
        let file = self.file_spec(spec)?;
        let Some((file, stream)) = file.and_then(|file| file.embedded.map(|id| (file, id))) else {
            diagnostics.set_object(spec_id);
            diagnostics.push(
                DiagnosticKind::AttachmentNotEmbedded,
                self.offset_of(spec_id),
            );
            diagnostics.set_object(None);
            return Ok(None);
        };
        let mut attachment = Attachment {
            place,
            key,
            file,
            stream,
            media_type: None,
            size: None,
            created: None,
            modified: None,
            checksum: None,
        };
        let Some(object) = self.object(stream)? else {
            return Ok(Some(attachment));
        };
        let Some(stream_dictionary) = object.stream().map(|s| &s.dictionary) else {
            return Ok(Some(attachment));
        };
        if let Value::Name(subtype) = self.get(stream_dictionary, SUBTYPE_KEY)?.value() {
            attachment.media_type = Some(subtype.clone());
        }
        let params = self.get(stream_dictionary, PARAMS_KEY)?;
        if let Some(params_dictionary) = params.dictionary() {
            let holder = params.id().or(Some(stream));
            diagnostics.set_object(holder);
            let offset = self.offset_of(holder);
            self.read_params(params_dictionary, &mut attachment, offset, diagnostics)?;
            diagnostics.set_object(None);
        }
        Ok(Some(attachment))
    }

    /// Reads Table 46's `Size`, dates and `CheckSum`. An entry that is not
    /// what the table defines is left out and reported.
    fn read_params(
        &self,
        params: &Dictionary,
        attachment: &mut Attachment,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<(), PdfError> {
        let mut ignored = |key: &[u8]| {
            diagnostics.push(
                DiagnosticKind::IgnoredEntry {
                    key: Name::new(key),
                },
                offset,
            )
        };
        match self.get(params, SIZE_KEY)?.value() {
            Value::Null => {}
            Value::Integer(size) if *size >= 0 => attachment.size = Some(size.unsigned_abs()),
            _ => ignored(SIZE_KEY),
        }
        for (key, slot) in [
            (CREATION_DATE_KEY, &mut attachment.created),
            (MOD_DATE_KEY, &mut attachment.modified),
        ] {
            match self.get(params, key)?.value() {
                Value::Null => {}
                Value::String(date) => {
                    *slot = Date::parse(&decode_text_string(date.as_bytes()).text);
                    if slot.is_none() {
                        ignored(key);
                    }
                }
                _ => ignored(key),
            }
        }
        match self.get(params, CHECKSUM_KEY)?.value() {
            Value::Null => {}
            Value::String(checksum) => match checksum.as_bytes().try_into() {
                Ok(checksum) => attachment.checksum = Some(checksum),
                Err(_) => ignored(CHECKSUM_KEY),
            },
            _ => ignored(CHECKSUM_KEY),
        }
        Ok(())
    }

    /// The decoded data of `attachment`, of at most `max_bytes` bytes
    /// ([`PdfError::DecodedTooLarge`] past it), with how it compares to
    /// the size and checksum its parameters state. A stream that is not
    /// there, or is not a stream, is [`PdfError::MissingObject`] or
    /// [`PdfError::WrongKind`].
    pub fn attachment_data(
        &self,
        attachment: &Attachment,
        max_bytes: usize,
    ) -> Result<Extracted, PdfError> {
        let object = self.required(attachment.stream)?;
        let stream = object.stream().ok_or(PdfError::WrongKind {
            object: attachment.stream,
        })?;
        let decoded = self.decode_stream(attachment.stream, stream, max_bytes)?;
        let data = decoded.data.into_owned();
        let size = match attachment.size {
            None => Check::Absent,
            Some(size) if size == data.len() as u64 => Check::Matches,
            Some(_) => Check::Differs,
        };
        let checksum = match attachment.checksum {
            None => Check::Absent,
            Some(expected) => {
                let digest: [u8; CHECKSUM_BYTES] = Md5::digest(&data).into();
                if digest == expected {
                    Check::Matches
                } else {
                    Check::Differs
                }
            }
        };
        Ok(Extracted {
            data,
            complete: decoded.complete,
            size,
            checksum,
        })
    }

    /// Writes the data of `attachment` to `path` through the atomic
    /// writer, as `expect` allows, when it is sound
    /// ([`Extracted::is_sound`]). Data that is not is
    /// [`PdfError::AttachmentDamaged`] and nothing is written; the
    /// application can still take it from [`Self::attachment_data`].
    pub fn save_attachment(
        &self,
        attachment: &Attachment,
        max_bytes: usize,
        path: &Path,
        expect: Expect<'_>,
    ) -> Result<FileStamp, PdfError> {
        let extracted = self.attachment_data(attachment, max_bytes)?;
        if !extracted.is_sound() {
            return Err(PdfError::AttachmentDamaged {
                stream: attachment.stream,
            });
        }
        Ok(write_bytes_atomic(path, expect, &extracted.data)?)
    }
}
