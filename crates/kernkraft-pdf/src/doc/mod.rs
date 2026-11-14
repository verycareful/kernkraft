// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A PDF document: ISO 32000-1:2008, 7.7 (document structure), 7.9.6 and
//! 7.9.7 (name and number trees), 14.3.3 (document information).
//!
//! A [`Document`] reads its objects from the file when they are first
//! used, keeps a bounded number of them, and decrypts what an encrypted
//! file holds. It never changes: [`Document::apply`] gives a new
//! document, a snapshot that shares the file, the cache and every object
//! the edit did not touch, so readers of the old one carry on undisturbed.
//! A document is shared between threads freely.
//!
//! What reading tolerated is collected as [`Diagnostic`]s, once for each
//! object however often it is read, up to the number the caller allows.

mod cache;
mod changes;
mod log;
mod pages;
mod text;
mod tree;

use std::borrow::Cow;
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

pub use changes::{Changes, ObjectBuilder, StreamEncoding};
pub use pages::{Page, Rectangle};
pub use text::{decode_text_string, encode_text_string, Date, TextString};
pub use tree::{NameTree, NumberTree};

use crate::crypt::{decrypt_file, Decryptor, Encryption};
use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::file::{Body, Location, ObjectStream, PdfFile, ReadLimits, XrefEntry};
use crate::filter::{Decoded, Pipeline};
use crate::syntax::{Dictionary, Object, ObjectId, Span, Value};
use crate::{FileSource, MemorySource, PdfError, Source};
use cache::Lru;
use log::Log;
use pages::PageList;

const ROOT_KEY: &[u8] = b"Root";
const INFO_KEY: &[u8] = b"Info";
const SIZE_KEY: &[u8] = b"Size";
const FILTER_KEY: &[u8] = b"Filter";
const DECODE_PARMS_KEY: &[u8] = b"DecodeParms";

/// What opening a document may cost, set by the application: the format
/// sets no bound, and what is reasonable depends on the device.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpenOptions {
    /// The limits on reading the file.
    pub limits: ReadLimits,
    /// The most objects held in memory once read. Others are read again
    /// when used.
    pub cached_objects: usize,
    /// The most decoded object streams held in memory. Each is at most
    /// [`ReadLimits::max_object_stream_bytes`].
    pub cached_object_streams: usize,
    /// The most diagnostics kept. Past it they are counted
    /// ([`Document::dropped_diagnostics`]).
    pub max_diagnostics: usize,
}

/// An indirect object of a document.
#[derive(Debug, Clone, PartialEq)]
pub struct DocObject {
    pub id: ObjectId,
    pub content: Content,
    pub origin: Origin,
}

/// What an indirect object holds.
#[derive(Debug, Clone, PartialEq)]
pub enum Content {
    Object(Object),
    Stream(DocStream),
}

/// A stream object (7.3.8).
#[derive(Debug, Clone, PartialEq)]
pub struct DocStream {
    pub dictionary: Dictionary,
    pub data: StreamData,
}

/// Where a stream's data is. Either way it is encoded by the filters its
/// dictionary names.
#[derive(Debug, Clone, PartialEq)]
pub enum StreamData {
    /// In the file, at these bytes, encrypted when the file is.
    File(Span),
    /// In memory, never encrypted: data an edit gave.
    Memory(Arc<[u8]>),
}

/// Where an object came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    /// Read from the file. `verbatim` is true when reading it tolerated
    /// nothing, so that its bytes in the file say exactly what was read
    /// and can be written again as they are.
    File { location: Location, verbatim: bool },
    /// Given by an edit.
    Edited,
}

impl DocObject {
    /// The object, or `None` for a stream.
    pub fn object(&self) -> Option<&Object> {
        match &self.content {
            Content::Object(object) => Some(object),
            Content::Stream(_) => None,
        }
    }

    /// The dictionary the object is, or a stream's dictionary.
    pub fn dictionary(&self) -> Option<&Dictionary> {
        match &self.content {
            Content::Object(object) => match object.value() {
                Value::Dictionary(dictionary) => Some(dictionary),
                _ => None,
            },
            Content::Stream(stream) => Some(&stream.dictionary),
        }
    }

    pub fn stream(&self) -> Option<&DocStream> {
        match &self.content {
            Content::Stream(stream) => Some(stream),
            Content::Object(_) => None,
        }
    }
}

/// What an object stands for once a reference in it is followed: the
/// object itself, the object the reference names, or null when the
/// document has no such object (7.3.10).
#[derive(Debug, Clone)]
pub enum Resolved<'a> {
    Direct(&'a Object),
    Indirect(Arc<DocObject>),
    Null,
}

impl Resolved<'_> {
    /// The object. Never a stream: [`Document::resolve`] refuses one.
    pub fn object(&self) -> &Object {
        static NULL: OnceLock<Object> = OnceLock::new();
        let null = || NULL.get_or_init(|| Object::new(Value::Null));
        match self {
            Resolved::Direct(object) => object,
            Resolved::Indirect(loaded) => loaded.object().unwrap_or_else(null),
            Resolved::Null => null(),
        }
    }

    pub fn value(&self) -> &Value {
        self.object().value()
    }

    pub fn dictionary(&self) -> Option<&Dictionary> {
        match self.value() {
            Value::Dictionary(dictionary) => Some(dictionary),
            _ => None,
        }
    }

    pub fn array(&self) -> Option<&[Object]> {
        match self.value() {
            Value::Array(items) => Some(items),
            _ => None,
        }
    }

    /// The indirect object followed to, when there was one.
    pub fn id(&self) -> Option<ObjectId> {
        match self {
            Resolved::Indirect(loaded) => Some(loaded.id),
            _ => None,
        }
    }
}

/// What every snapshot of one opened file shares.
struct Base {
    file: PdfFile,
    decryptor: Option<Arc<Decryptor>>,
    options: OpenOptions,
    objects: Mutex<Lru<ObjectId, Arc<DocObject>>>,
    object_streams: Mutex<Lru<u32, Arc<ObjectStream>>>,
    log: Mutex<Log>,
}

/// What a snapshot holds beyond the file: the objects its edits set or
/// removed, its trailer, and the next free object number.
#[derive(Debug, Clone)]
pub(crate) struct Edits {
    pub(crate) objects: HashMap<u32, Edit>,
    pub(crate) trailer: Dictionary,
    pub(crate) next_number: u32,
}

#[derive(Debug, Clone)]
pub(crate) enum Edit {
    Set(Arc<DocObject>),
    /// The object was removed; it had this generation.
    Removed(u16),
}

/// An open PDF document. Cloning it is cheap and gives the same snapshot.
#[derive(Clone)]
pub struct Document {
    base: Arc<Base>,
    edits: Arc<Edits>,
    pages: Arc<Mutex<Option<Arc<PageList>>>>,
}

impl std::fmt::Debug for Document {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Document")
            .field("file", &self.base.file)
            .field("encrypted", &self.base.decryptor.is_some())
            .field("edited_objects", &self.edits.objects.len())
            .finish_non_exhaustive()
    }
}

/// Locks `mutex`. A thread that panicked while holding one of the
/// document's locks left a cache or a log that is still whole, since none
/// of them is left half changed by any step that can panic.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Document {
    /// Opens the PDF at `path`, reading it by offset as it is used. See
    /// [`Self::from_source`].
    pub fn open(path: &Path, options: &OpenOptions, password: &str) -> Result<Self, PdfError> {
        Self::from_source(Arc::new(FileSource::open(path)?), options, password)
    }

    /// Opens a PDF held in memory. See [`Self::from_source`].
    pub fn from_bytes(
        bytes: Vec<u8>,
        options: &OpenOptions,
        password: &str,
    ) -> Result<Self, PdfError> {
        Self::from_source(Arc::new(MemorySource::new(bytes)), options, password)
    }

    /// Opens the PDF `source` holds.
    ///
    /// An encrypted file is opened with `password`, which may be its user
    /// or its owner password; the empty string opens a file that has no
    /// user password. A password that opens neither is
    /// [`PdfError::WrongPassword`], and the caller asks for another. The
    /// password of a file that is not encrypted is not looked at.
    pub fn from_source(
        source: Arc<dyn Source>,
        options: &OpenOptions,
        password: &str,
    ) -> Result<Self, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let file = PdfFile::open(source, options.limits, &mut diagnostics)?;
        let (file, decryptor) = unlock(file, password, &mut diagnostics)?;
        Ok(Self::new(file, decryptor, options, diagnostics))
    }

    /// The same file opened without its cross-reference, read through for
    /// its objects as [`PdfFile::rebuild`] does, for a document whose
    /// cross-reference turned out wrong ([`PdfError::XrefMismatch`]). An
    /// encrypted file keeps the key it was opened with. Edits are not
    /// carried over.
    pub fn rebuilt(&self) -> Result<Self, PdfError> {
        let base = &self.base;
        let mut diagnostics = Diagnostics::new();
        let file = PdfFile::rebuild(
            base.file.source().clone(),
            base.options.limits,
            &mut diagnostics,
        )?;
        let file = match &base.decryptor {
            Some(decryptor) => decrypt_file(file, decryptor.clone(), &mut diagnostics)?,
            None => file,
        };
        Ok(Self::new(
            file,
            base.decryptor.clone(),
            &base.options,
            diagnostics,
        ))
    }

    fn new(
        file: PdfFile,
        decryptor: Option<Arc<Decryptor>>,
        options: &OpenOptions,
        diagnostics: Diagnostics,
    ) -> Self {
        let mut log = Log::new(options.max_diagnostics);
        log.record(diagnostics);
        let trailer = file.trailer().clone();
        let next_number = first_free_number(&file);
        Self {
            base: Arc::new(Base {
                file,
                decryptor,
                options: *options,
                objects: Mutex::new(Lru::new(options.cached_objects)),
                object_streams: Mutex::new(Lru::new(options.cached_object_streams)),
                log: Mutex::new(log),
            }),
            edits: Arc::new(Edits {
                objects: HashMap::new(),
                trailer,
                next_number,
            }),
            pages: Arc::new(Mutex::new(None)),
        }
    }

    /// The file the document was read from.
    pub fn file(&self) -> &PdfFile {
        &self.base.file
    }

    pub fn options(&self) -> &OpenOptions {
        &self.base.options
    }

    /// What decrypts the file, when it is encrypted.
    pub fn decryptor(&self) -> Option<&Arc<Decryptor>> {
        self.base.decryptor.as_ref()
    }

    /// The trailer as this snapshot has it: the file's newest trailer
    /// (7.5.5), with the entries edits set.
    pub fn trailer(&self) -> &Dictionary {
        &self.edits.trailer
    }

    /// The diagnostics kept so far, in the order they were raised. Reading
    /// more of the document may add to them.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        lock(&self.base.log).kept().to_vec()
    }

    /// How many diagnostics were raised past [`OpenOptions::max_diagnostics`].
    pub fn dropped_diagnostics(&self) -> u64 {
        lock(&self.base.log).dropped()
    }

    /// The object number the next new object would take.
    pub fn next_object_number(&self) -> u32 {
        self.edits.next_number
    }

    /// Whether this snapshot carries edits.
    pub fn is_edited(&self) -> bool {
        !self.edits.objects.is_empty() || self.edits.trailer != *self.base.file.trailer()
    }

    /// Object `id`, or `None` when the document has no such object: a
    /// reference to it is the null object (7.3.10).
    ///
    /// An object of the file that is not where its cross-reference places
    /// it is [`PdfError::XrefMismatch`]; [`Self::rebuilt`] opens such a
    /// file.
    pub fn object(&self, id: ObjectId) -> Result<Option<Arc<DocObject>>, PdfError> {
        match self.edits.objects.get(&id.number) {
            Some(Edit::Set(object)) => Ok((object.id == id).then(|| object.clone())),
            Some(Edit::Removed(_)) => Ok(None),
            None => self.load(id),
        }
    }

    /// As [`Self::object`], for an object that must be there.
    pub fn required(&self, id: ObjectId) -> Result<Arc<DocObject>, PdfError> {
        self.object(id)?
            .ok_or(PdfError::MissingObject { object: id })
    }

    /// `object` itself, or the object it refers to. A reference to a
    /// stream is [`PdfError::WrongKind`]: streams are reached with
    /// [`Self::object`].
    pub fn resolve<'a>(&self, object: &'a Object) -> Result<Resolved<'a>, PdfError> {
        match object.value() {
            Value::Reference(id) => match self.object(*id)? {
                Some(loaded) if loaded.object().is_some() => Ok(Resolved::Indirect(loaded)),
                Some(_) => Err(PdfError::WrongKind { object: *id }),
                None => Ok(Resolved::Null),
            },
            _ => Ok(Resolved::Direct(object)),
        }
    }

    /// The value of `key` in `dictionary`, followed when it is a
    /// reference. An absent entry is null.
    pub fn get<'a>(
        &self,
        dictionary: &'a Dictionary,
        key: &[u8],
    ) -> Result<Resolved<'a>, PdfError> {
        match dictionary.get(key) {
            Some(object) => self.resolve(object),
            None => Ok(Resolved::Null),
        }
    }

    /// The document catalog (7.7.2), which the trailer's `Root` names.
    pub fn catalog(&self) -> Result<Arc<DocObject>, PdfError> {
        let bad = PdfError::InvalidEntry {
            object: None,
            key: "Root",
        };
        let Some(Value::Reference(root)) = self.trailer().get(ROOT_KEY).map(Object::value) else {
            return Err(bad);
        };
        match self.object(*root)? {
            Some(catalog) if catalog.object().is_some() && catalog.dictionary().is_some() => {
                Ok(catalog)
            }
            _ => Err(bad),
        }
    }

    /// The document information dictionary (14.3.3), which the trailer's
    /// `Info` names, or `None` when it names none.
    pub fn info(&self) -> Result<Option<Dictionary>, PdfError> {
        let info = self.get(self.trailer(), INFO_KEY)?;
        match info.value() {
            Value::Dictionary(dictionary) => Ok(Some(dictionary.clone())),
            Value::Null => Ok(None),
            _ => Err(PdfError::InvalidEntry {
                object: None,
                key: "Info",
            }),
        }
    }

    /// The data of `stream`, object `id`, as its filters encoded it:
    /// decrypted when the file is encrypted.
    pub fn stream_data<'a>(
        &'a self,
        id: ObjectId,
        stream: &'a DocStream,
    ) -> Result<Cow<'a, [u8]>, PdfError> {
        let span = match &stream.data {
            StreamData::Memory(data) => return Ok(Cow::Borrowed(data)),
            StreamData::File(span) => *span,
        };
        let len = span.end.saturating_sub(span.start);
        let len = usize::try_from(len).map_err(|_| PdfError::OutOfBounds {
            offset: span.start,
            len,
            size: self.base.file.source().len(),
        })?;
        let stored = self.base.file.source().read(span.start, len)?;
        let Some(decryptor) = &self.base.decryptor else {
            return Ok(stored);
        };
        let mut diagnostics = Diagnostics::new();
        let plain = decryptor
            .decrypt_stream(
                id,
                &stream.dictionary,
                &stored,
                span.start,
                &mut diagnostics,
            )?
            .into_owned();
        lock(&self.base.log).record_decrypt(id, diagnostics);
        Ok(Cow::Owned(plain))
    }

    /// The data of `stream`, object `id`, decoded by its filters, of at
    /// most `max_output` bytes ([`PdfError::DecodedTooLarge`] past it).
    /// An indirect `Filter` or `DecodeParms`, or an indirect item of
    /// either, is followed.
    pub fn decode_stream(
        &self,
        id: ObjectId,
        stream: &DocStream,
        max_output: usize,
    ) -> Result<Decoded<'static>, PdfError> {
        let filter = self.direct_filter_entry(&stream.dictionary, FILTER_KEY)?;
        let parms = self.direct_filter_entry(&stream.dictionary, DECODE_PARMS_KEY)?;
        let pipeline = Pipeline::from_objects(filter.as_ref(), parms.as_ref())?;
        let data = self.stream_data(id, stream)?;
        let offset = match stream.data {
            StreamData::File(span) => span.start,
            StreamData::Memory(_) => 0,
        };
        let mut diagnostics = Diagnostics::new();
        diagnostics.set_object(Some(id));
        let decoded = pipeline.decode(&data, max_output, offset, &mut diagnostics);
        diagnostics.set_object(None);
        lock(&self.base.log).record_decode(id, diagnostics);
        let decoded = decoded?;
        Ok(Decoded {
            data: Cow::Owned(decoded.data.into_owned()),
            complete: decoded.complete,
        })
    }

    /// The entry `key` of a stream dictionary with every reference in it
    /// followed, one level into an array.
    fn direct_filter_entry(
        &self,
        dictionary: &Dictionary,
        key: &[u8],
    ) -> Result<Option<Object>, PdfError> {
        let Some(entry) = dictionary.get(key) else {
            return Ok(None);
        };
        let resolved = self.resolve(entry)?.object().clone();
        Ok(Some(match resolved.into_value() {
            Value::Array(items) => {
                let mut direct = Vec::with_capacity(items.len());
                for item in &items {
                    direct.push(self.resolve(item)?.object().clone());
                }
                Object::new(Value::Array(direct))
            }
            other => Object::new(other),
        }))
    }

    /// The object stream `number` of the file, decoded once while the
    /// cache holds it.
    pub(crate) fn object_stream(&self, number: u32) -> Result<Arc<ObjectStream>, PdfError> {
        if let Some(stream) = lock(&self.base.object_streams).get(&number) {
            return Ok(stream);
        }
        let mut diagnostics = Diagnostics::new();
        let stream = Arc::new(self.base.file.object_stream(number, &mut diagnostics)?);
        lock(&self.base.log).record_read(
            ObjectId {
                number,
                generation: 0,
            },
            diagnostics,
        );
        lock(&self.base.object_streams).insert(number, stream.clone());
        Ok(stream)
    }

    /// Object `id` as the file holds it.
    fn load(&self, id: ObjectId) -> Result<Option<Arc<DocObject>>, PdfError> {
        if let Some(object) = lock(&self.base.objects).get(&id) {
            return Ok(Some(object));
        }
        let mut diagnostics = Diagnostics::new();
        let loaded = match self.base.file.entry(id.number) {
            // 7.5.7: a compressed object has generation 0.
            Some(XrefEntry::Compressed { stream, index }) if id.generation == 0 => {
                let object = self.object_stream(stream)?.object(
                    index,
                    id.number,
                    self.base.options.limits.max_object_elements,
                    &mut diagnostics,
                )?;
                DocObject {
                    id,
                    content: Content::Object(object),
                    origin: Origin::File {
                        location: Location::ObjectStream { stream, index },
                        verbatim: diagnostics.is_empty(),
                    },
                }
            }
            Some(XrefEntry::InUse { .. }) => {
                let Some(mut object) = self.base.file.load(id, &mut diagnostics)? else {
                    return Ok(None);
                };
                if let Some(decryptor) = &self.base.decryptor {
                    decryptor.decrypt_object(&mut object, &mut diagnostics)?;
                }
                let origin = Origin::File {
                    location: object.location,
                    verbatim: diagnostics.is_empty(),
                };
                let content = match object.body {
                    Body::Object(object) => Content::Object(object),
                    Body::Stream(stream) => Content::Stream(DocStream {
                        dictionary: stream.dictionary,
                        data: StreamData::File(stream.data),
                    }),
                };
                DocObject {
                    id,
                    content,
                    origin,
                }
            }
            _ => return Ok(None),
        };
        lock(&self.base.log).record_read(id, diagnostics);
        let loaded = Arc::new(loaded);
        lock(&self.base.objects).insert(id, loaded.clone());
        Ok(Some(loaded))
    }

    /// The generation a free entry for object `number` takes: one past
    /// the generation of the object last there (7.5.4), or what the
    /// file's free entry already says.
    pub(crate) fn free_generation(&self, number: u32) -> u16 {
        let bump = |generation: u16| generation.saturating_add(1);
        match self.edits.objects.get(&number) {
            Some(Edit::Set(object)) => bump(object.id.generation),
            Some(Edit::Removed(generation)) => bump(*generation),
            None => match self.base.file.entry(number) {
                Some(XrefEntry::InUse { generation, .. }) => bump(generation),
                Some(XrefEntry::Compressed { .. }) => bump(0),
                Some(XrefEntry::Free { generation, .. }) => generation,
                Some(XrefEntry::Null) | None => 0,
            },
        }
    }

    /// A new snapshot with `changes` applied. This document is left as
    /// it is.
    pub fn apply(&self, changes: Changes) -> Result<Self, PdfError> {
        let mut edits = (*self.edits).clone();
        changes.apply_to(&mut edits)?;
        Ok(Self {
            base: self.base.clone(),
            edits: Arc::new(edits),
            pages: Arc::new(Mutex::new(None)),
        })
    }

    pub(crate) fn page_list(&self) -> Result<Arc<PageList>, PdfError> {
        if let Some(list) = lock(&self.pages).as_ref() {
            return Ok(list.clone());
        }
        let mut diagnostics = Diagnostics::new();
        let list = Arc::new(PageList::read(self, &mut diagnostics)?);
        lock(&self.base.log).record(diagnostics);
        *lock(&self.pages) = Some(list.clone());
        Ok(list)
    }
}

/// Reads the encryption dictionary of `file` and checks `password`
/// against it.
fn unlock(
    file: PdfFile,
    password: &str,
    diagnostics: &mut Diagnostics,
) -> Result<(PdfFile, Option<Arc<Decryptor>>), PdfError> {
    let Some(encryption) = Encryption::read(&file, diagnostics)? else {
        return Ok((file, None));
    };
    let decryptor = Arc::new(encryption.authenticate(password, diagnostics)?);
    let file = decrypt_file(file, decryptor.clone(), diagnostics)?;
    Ok((file, Some(decryptor)))
}

/// One past the highest object number the file has an entry for or its
/// trailer's `Size` allows, whichever is larger.
fn first_free_number(file: &PdfFile) -> u32 {
    let highest = file
        .sections()
        .iter()
        .flat_map(|section| section.entries().map(|(number, _)| number))
        .max()
        .map_or(1, |number| number.saturating_add(1));
    let size = match file.trailer().get(SIZE_KEY).map(Object::value) {
        Some(Value::Integer(size)) => u32::try_from(*size).unwrap_or(0),
        _ => 0,
    };
    highest.max(size).max(1)
}
