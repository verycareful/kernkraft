// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Encrypted files: ISO 32000-1:2008, 7.6, with revisions 5 and 6 of the
//! standard security handler from ISO 32000-2:2020, 7.6.4.
//!
//! [`Encryption::read`] reads the encryption dictionary a file's trailer
//! names. [`Encryption::authenticate`] checks a password against it and
//! gives a [`Decryptor`], or [`PdfError::WrongPassword`]; a caller first
//! tries the empty password, which opens a file that has no user
//! password. [`decrypt_file`] hands the decryptor to the file, which then
//! reads its object streams, and [`Decryptor::decrypt_object`] and
//! [`Decryptor::decrypt_stream`] decrypt what is loaded.
//!
//! The standard security handler is read in all its revisions, 2 to 6:
//! RC4 with keys of 40 to 128 bits, AES-128 and AES-256. Every other
//! handler, the public-key handlers of 7.6.4 among them, is refused.
//!
//! The permissions a file states ([`Permissions`]) are reported and never
//! enforced here: what to allow is the application's decision.

mod cipher;
mod dictionary;
mod key;
mod password;
mod permissions;
mod saslprep;
mod saslprep_tables;

use std::borrow::Cow;
use std::sync::{Arc, OnceLock};

pub use dictionary::{CryptMethod, Encryption, Revision};
pub use key::Access;
pub use permissions::Permissions;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::{Body, FileObject, Location, PdfFile, StreamDecrypt};
use crate::filter::crypt_filter_name;
use crate::syntax::{Dictionary, Object, ObjectId, Value};
use crate::PdfError;
use cipher::{decrypt_block_256, decrypt_message, encrypt_message, rc4, AesKey, AES_BLOCK};
use dictionary::KEY_BYTES_256;

const TYPE_KEY: &[u8] = b"Type";
const ROOT_KEY: &[u8] = b"Root";
const METADATA_KEY: &[u8] = b"Metadata";
const CONTENTS_KEY: &[u8] = b"Contents";
const BYTE_RANGE_KEY: &[u8] = b"ByteRange";
/// The stream types whose data a decryptor treats apart: cross-reference
/// streams are never encrypted (7.5.8.2), embedded files may have their
/// own crypt filter (Table 20, `EFF`) and the document's metadata may be
/// left plain (Table 21, `EncryptMetadata`).
const XREF_TYPE: &[u8] = b"XRef";
const EMBEDDED_FILE_TYPE: &[u8] = b"EmbeddedFile";
/// The types of a signature dictionary (12.8.1, Table 252) and of a
/// document timestamp dictionary (ISO 32000-2:2020, 12.8.5).
const SIGNATURE_TYPES: [&[u8]; 2] = [b"Sig", b"DocTimeStamp"];

/// `Perms` decrypted (ISO 32000-2:2020, 7.6.4.4, Algorithm 13): `P` in
/// bytes 0 to 3, low-order first; `T` or `F` for `EncryptMetadata` in byte
/// 8; `adb` in bytes 9 to 11.
const PERMS_P: std::ops::Range<usize> = 0..4;
const PERMS_METADATA: usize = 8;
const PERMS_MARK: std::ops::Range<usize> = 9..12;
const PERMS_MARK_BYTES: &[u8] = b"adb";
const PERMS_TRUE: u8 = b'T';
const PERMS_FALSE: u8 = b'F';

impl Encryption {
    /// Checks `password` as the owner password, then as the user password,
    /// and gives the decryptor of the file. The empty password opens a
    /// file whose user password is empty.
    ///
    /// The password is tried in the form the revision defines
    /// (PDFDocEncoding for revisions 2 to 4, SASLprep for 5 and 6) and
    /// then as its UTF-8 bytes; a password accepted in the second form
    /// raises [`DiagnosticKind::PasswordEncoding`]. A password that is
    /// neither the owner's nor the user's is [`PdfError::WrongPassword`].
    pub fn authenticate(
        &self,
        password: &str,
        diagnostics: &mut Diagnostics,
    ) -> Result<Decryptor, PdfError> {
        for (bytes, defined) in password::candidates(self.revision, password) {
            let Some((file_key, access)) = key::authenticate(self, &bytes)? else {
                continue;
            };
            if !defined {
                diagnostics.push(DiagnosticKind::PasswordEncoding, self.offset);
            }
            if self.revision.uses_utf8_passwords() && !self.perms_agree(&file_key) {
                diagnostics.push(DiagnosticKind::PermissionsUnverified, self.offset);
            }
            return Ok(Decryptor {
                file_key,
                access,
                permissions: Permissions::new(self.permissions, self.revision),
                encryption: self.clone(),
                metadata: OnceLock::new(),
            });
        }
        Err(PdfError::WrongPassword)
    }

    /// Whether `Perms` decrypts to `P` and `EncryptMetadata` as the
    /// dictionary states them. A missing `Perms` does not agree.
    fn perms_agree(&self, file_key: &[u8]) -> bool {
        let (Some(perms), Ok(key)) = (self.perms, <[u8; KEY_BYTES_256]>::try_from(file_key)) else {
            return false;
        };
        let plain = decrypt_block_256(key, perms);
        let metadata = if self.encrypt_metadata {
            PERMS_TRUE
        } else {
            PERMS_FALSE
        };
        plain[PERMS_P] == self.permissions.to_le_bytes()
            && plain[PERMS_METADATA] == metadata
            && &plain[PERMS_MARK] == PERMS_MARK_BYTES
    }
}

/// Decrypts the strings and streams of one encrypted file, with the key a
/// password gave.
pub struct Decryptor {
    file_key: Vec<u8>,
    access: Access,
    permissions: Permissions,
    encryption: Encryption,
    /// The document's metadata stream (Table 28, `Metadata`), set by
    /// [`decrypt_file`] when `EncryptMetadata` is false.
    metadata: OnceLock<Option<ObjectId>>,
}

impl std::fmt::Debug for Decryptor {
    // The key is left out.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Decryptor")
            .field("access", &self.access)
            .field("permissions", &self.permissions)
            .field("encryption", &self.encryption)
            .finish_non_exhaustive()
    }
}

/// `file` reading its object streams through `decryptor`, which knows from
/// here on which stream is the document's metadata. As
/// [`PdfFile::decrypting`] says, `diagnostics` is the collector the file
/// was opened with.
///
/// A decryptor belongs to the file whose encryption dictionary gave it.
pub fn decrypt_file(
    file: PdfFile,
    decryptor: Arc<Decryptor>,
    diagnostics: &mut Diagnostics,
) -> Result<PdfFile, PdfError> {
    let file = file.decrypting(decryptor.clone(), diagnostics)?;
    let metadata = if decryptor.encryption.encrypt_metadata {
        None
    } else {
        document_metadata(&file)?
    };
    // A decryptor handed to its file a second time keeps what it was told
    // the first time, which is the same.
    let _ = decryptor.metadata.set(metadata);
    Ok(file)
}

/// The object the catalog's `Metadata` names. The catalog is read again
/// when the document loads it, which is when its diagnostics are raised.
fn document_metadata(file: &PdfFile) -> Result<Option<ObjectId>, PdfError> {
    let Some(Value::Reference(root)) = file.trailer().get(ROOT_KEY).map(Object::value) else {
        return Ok(None);
    };
    let mut unkept = Diagnostics::new();
    let Some(catalog) = file.load(*root, &mut unkept)? else {
        return Ok(None);
    };
    Ok(match catalog.body {
        Body::Object(object) => match object.value() {
            Value::Dictionary(catalog) => match catalog.get(METADATA_KEY).map(Object::value) {
                Some(Value::Reference(metadata)) => Some(*metadata),
                _ => None,
            },
            _ => None,
        },
        Body::Stream(_) => None,
    })
}

impl Decryptor {
    /// Which password opened the file.
    pub fn access(&self) -> Access {
        self.access
    }

    /// What the file allows someone who opened it with the user password.
    pub fn permissions(&self) -> Permissions {
        self.permissions
    }

    pub fn encryption(&self) -> &Encryption {
        &self.encryption
    }

    /// Decrypts the strings of `object` in place.
    ///
    /// Nothing is done to an object read from an object stream, which was
    /// decrypted with its stream, to the encryption dictionary, or to the
    /// dictionary of a cross-reference stream (7.6.1, 7.5.8.2); the value
    /// of `Contents` in a signature dictionary is left as it is, since it
    /// is not encrypted (ISO 32000-2:2020, 7.6.1). A decrypted string, and
    /// every array and dictionary that holds one, loses its span: the
    /// bytes it was read from do not say what it now holds.
    /// [`FileObject::location`] still names the bytes of the whole object.
    pub fn decrypt_object(
        &self,
        object: &mut FileObject,
        diagnostics: &mut Diagnostics,
    ) -> Result<(), PdfError> {
        let Location::File(span) = object.location else {
            return Ok(());
        };
        if Some(object.id) == self.encryption.dictionary_id {
            return Ok(());
        }
        let method = self.encryption.strings;
        let id = object.id;
        diagnostics.set_object(Some(id));
        let result = match &mut object.body {
            Body::Object(value) => self.decrypt_strings(value, id, method, span.start, diagnostics),
            Body::Stream(stream) if has_type(&stream.dictionary, XREF_TYPE) => Ok(false),
            Body::Stream(stream) => self
                .decrypt_entries(&mut stream.dictionary, id, method, span.start, diagnostics)
                .map(|_| false),
        };
        diagnostics.set_object(None);
        result.map(|_| ())
    }

    /// The data of stream `id`, decrypted and still encoded by the
    /// stream's filters: with the crypt filter the stream names
    /// (7.4.10), else `EFF` for an embedded file, else the default for
    /// streams. A cross-reference stream, and the document's metadata
    /// when `EncryptMetadata` is false, are returned as they are.
    ///
    /// `offset` is where the data starts in the file, for diagnostics.
    pub fn decrypt_stream<'a>(
        &self,
        id: ObjectId,
        dictionary: &Dictionary,
        data: &'a [u8],
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Cow<'a, [u8]>, PdfError> {
        let (method, named) = self.stream_method(id, dictionary)?;
        diagnostics.set_object(Some(id));
        if named && matches!(method, CryptMethod::Rc4 | CryptMethod::Aes128) {
            diagnostics.push(DiagnosticKind::CryptFilterObjectKey, offset);
        }
        let result = self.apply(method, id, data, offset, diagnostics);
        diagnostics.set_object(None);
        result
    }

    /// The method that decrypts stream `id`, and whether the stream
    /// names it with a crypt filter (7.4.10).
    fn stream_method(
        &self,
        id: ObjectId,
        dictionary: &Dictionary,
    ) -> Result<(CryptMethod, bool), PdfError> {
        self.method_for_stream(id, dictionary, self.metadata.get().copied().flatten())
    }

    /// As [`Self::stream_method`], with `metadata` the document's metadata
    /// stream.
    fn method_for_stream(
        &self,
        id: ObjectId,
        dictionary: &Dictionary,
        metadata: Option<ObjectId>,
    ) -> Result<(CryptMethod, bool), PdfError> {
        let encryption = &self.encryption;
        if has_type(dictionary, XREF_TYPE) {
            return Ok((CryptMethod::Identity, false));
        }
        if let Some(name) = crypt_filter_name(dictionary)? {
            return Ok((encryption.filter(&name)?, true));
        }
        if !encryption.encrypt_metadata && metadata == Some(id) {
            return Ok((CryptMethod::Identity, false));
        }
        let method = match encryption.embedded_files {
            Some(method) if has_type(dictionary, EMBEDDED_FILE_TYPE) => method,
            _ => encryption.streams,
        };
        Ok((method, false))
    }

    /// Decrypts `data`, a string or the data of a stream of object `id`.
    ///
    /// For revisions 2 to 4 the key of each object follows Algorithm 1,
    /// whether the method comes from the default or from a crypt filter
    /// the stream names: 7.4.10 says a named filter takes the file key as
    /// it is, but files written with crypt filters are encrypted with the
    /// key of each object, and qpdf, MuPDF and pdf.js decrypt them so.
    /// [`DiagnosticKind::CryptFilterObjectKey`] reports each such stream.
    fn apply<'a>(
        &self,
        method: CryptMethod,
        id: ObjectId,
        data: &'a [u8],
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Cow<'a, [u8]>, PdfError> {
        let aes_key = match method {
            CryptMethod::Identity => return Ok(Cow::Borrowed(data)),
            CryptMethod::Rc4 => {
                let mut plain = data.to_vec();
                rc4(&key::object_key(&self.file_key, id, false), &mut plain)?;
                return Ok(Cow::Owned(plain));
            }
            CryptMethod::Aes128 => {
                let key = key::object_key(&self.file_key, id, true);
                AesKey::Aes128(
                    key.try_into()
                        .map_err(|_| PdfError::EncryptionDictionary { key: "Length" })?,
                )
            }
            CryptMethod::Aes256 => AesKey::Aes256(
                self.file_key
                    .clone()
                    .try_into()
                    .map_err(|_| PdfError::EncryptionDictionary { key: "Length" })?,
            ),
        };
        let (plain, framed) = decrypt_message(&aes_key, data);
        if !framed {
            diagnostics.push(DiagnosticKind::EncryptedDataDamaged, offset);
        }
        Ok(Cow::Owned(plain))
    }

    /// Decrypts every string inside `object`. Returns whether one was
    /// changed, which drops the span of `object`.
    fn decrypt_strings(
        &self,
        object: &mut Object,
        id: ObjectId,
        method: CryptMethod,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<bool, PdfError> {
        let (value, span) = object.parts_mut();
        let offset = span.map_or(offset, |span| span.start);
        let changed = match value {
            Value::String(string) => {
                let plain = self.apply(method, id, string.as_bytes(), offset, diagnostics)?;
                if let Cow::Owned(plain) = plain {
                    *string = crate::syntax::PdfString::new(plain);
                    true
                } else {
                    false
                }
            }
            Value::Array(items) => {
                let mut changed = false;
                for item in items {
                    changed |= self.decrypt_strings(item, id, method, offset, diagnostics)?;
                }
                changed
            }
            Value::Dictionary(dictionary) => {
                self.decrypt_entries(dictionary, id, method, offset, diagnostics)?
            }
            _ => false,
        };
        if changed {
            *span = None;
        }
        Ok(changed)
    }

    fn decrypt_entries(
        &self,
        dictionary: &mut Dictionary,
        id: ObjectId,
        method: CryptMethod,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<bool, PdfError> {
        let signature = is_signature(dictionary);
        let mut changed = false;
        for (key, value) in dictionary.iter_mut() {
            if signature && key.as_bytes() == CONTENTS_KEY {
                continue;
            }
            changed |= self.decrypt_strings(value, id, method, offset, diagnostics)?;
        }
        Ok(changed)
    }
}

/// The writer's side: what [`Decryptor::decrypt_object`] and
/// [`Decryptor::decrypt_stream`] undo, done with the same key and the same
/// exceptions.
impl Decryptor {
    /// Encrypts every string of `object`, the value of object `id`, in
    /// place, but for those [`Self::decrypt_object`] leaves alone: none
    /// of the encryption dictionary, and not the `Contents` of a
    /// signature dictionary.
    pub(crate) fn encrypt_object(
        &self,
        id: ObjectId,
        object: &mut Object,
        ivs: &mut IvSource,
    ) -> Result<(), PdfError> {
        if Some(id) == self.encryption.dictionary_id {
            return Ok(());
        }
        let method = self.encryption.strings;
        self.encrypt_strings(object, id, method, ivs)
    }

    /// Encrypts the strings of the dictionary of stream `id` in place; a
    /// cross-reference stream's are left alone (7.5.8.2).
    pub(crate) fn encrypt_stream_dictionary(
        &self,
        id: ObjectId,
        dictionary: &mut Dictionary,
        ivs: &mut IvSource,
    ) -> Result<(), PdfError> {
        if has_type(dictionary, XREF_TYPE) {
            return Ok(());
        }
        let method = self.encryption.strings;
        self.encrypt_entries(dictionary, id, method, ivs)
    }

    /// The data of stream `id`, still encoded by its filters, encrypted
    /// with the method [`Self::decrypt_stream`] would decrypt it with.
    /// `metadata` is the document's metadata stream, which stays plain
    /// when `EncryptMetadata` is false. `dictionary` is the stream's
    /// dictionary before its strings are encrypted.
    pub(crate) fn encrypt_stream(
        &self,
        id: ObjectId,
        dictionary: &Dictionary,
        data: &[u8],
        metadata: Option<ObjectId>,
        ivs: &mut IvSource,
    ) -> Result<Vec<u8>, PdfError> {
        let (method, _) = self.method_for_stream(id, dictionary, metadata)?;
        self.encrypt_bytes(method, id, data, ivs)
    }

    fn encrypt_bytes(
        &self,
        method: CryptMethod,
        id: ObjectId,
        data: &[u8],
        ivs: &mut IvSource,
    ) -> Result<Vec<u8>, PdfError> {
        let aes_key = match method {
            CryptMethod::Identity => return Ok(data.to_vec()),
            CryptMethod::Rc4 => {
                let mut encrypted = data.to_vec();
                rc4(&key::object_key(&self.file_key, id, false), &mut encrypted)?;
                return Ok(encrypted);
            }
            CryptMethod::Aes128 => AesKey::Aes128(
                key::object_key(&self.file_key, id, true)
                    .try_into()
                    .map_err(|_| PdfError::EncryptionDictionary { key: "Length" })?,
            ),
            CryptMethod::Aes256 => AesKey::Aes256(
                self.file_key
                    .clone()
                    .try_into()
                    .map_err(|_| PdfError::EncryptionDictionary { key: "Length" })?,
            ),
        };
        Ok(encrypt_message(&aes_key, ivs.next_iv(), data))
    }

    fn encrypt_strings(
        &self,
        object: &mut Object,
        id: ObjectId,
        method: CryptMethod,
        ivs: &mut IvSource,
    ) -> Result<(), PdfError> {
        match object.value_mut() {
            Value::String(string) => {
                let encrypted = self.encrypt_bytes(method, id, string.as_bytes(), ivs)?;
                *string = crate::syntax::PdfString::new(encrypted);
            }
            Value::Array(items) => {
                for item in items {
                    self.encrypt_strings(item, id, method, ivs)?;
                }
            }
            Value::Dictionary(dictionary) => self.encrypt_entries(dictionary, id, method, ivs)?,
            _ => {}
        }
        Ok(())
    }

    fn encrypt_entries(
        &self,
        dictionary: &mut Dictionary,
        id: ObjectId,
        method: CryptMethod,
        ivs: &mut IvSource,
    ) -> Result<(), PdfError> {
        let signature = is_signature(dictionary);
        for (key, value) in dictionary.iter_mut() {
            if signature && key.as_bytes() == CONTENTS_KEY {
                continue;
            }
            self.encrypt_strings(value, id, method, ivs)?;
        }
        Ok(())
    }
}

/// The initialization vectors of the AES-encrypted strings and streams of
/// one written file (7.6.2). Each is the first 16 bytes of SHA-256 over
/// the caller's seed and a counter, so the vectors cannot be told in
/// advance by anyone who does not know the seed, and the same seed gives
/// the same file. A caller seeds each save with fresh random bytes.
pub(crate) struct IvSource {
    seed: [u8; SEED_BYTES],
    counter: u64,
}

/// The bytes of the seed a writer is given.
pub(crate) const SEED_BYTES: usize = 32;

impl IvSource {
    pub(crate) fn new(seed: [u8; SEED_BYTES]) -> Self {
        Self { seed, counter: 0 }
    }

    pub(crate) fn next_iv(&mut self) -> [u8; AES_BLOCK] {
        use sha2::Digest;
        let mut hash = sha2::Sha256::new();
        hash.update(self.seed);
        hash.update(self.counter.to_le_bytes());
        self.counter += 1;
        let digest = hash.finalize();
        let mut iv = [0u8; AES_BLOCK];
        iv.copy_from_slice(&digest[..AES_BLOCK]);
        iv
    }
}

impl StreamDecrypt for Decryptor {
    fn decrypt_stream<'a>(
        &self,
        id: ObjectId,
        dictionary: &Dictionary,
        data: &'a [u8],
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Cow<'a, [u8]>, PdfError> {
        Decryptor::decrypt_stream(self, id, dictionary, data, offset, diagnostics)
    }
}

fn has_type(dictionary: &Dictionary, expected: &[u8]) -> bool {
    matches!(
        dictionary.get(TYPE_KEY).map(Object::value),
        Some(Value::Name(name)) if name.as_bytes() == expected
    )
}

/// Whether `dictionary` is a signature dictionary: its `Type` says so, or,
/// since Table 252 makes `Type` optional, it has the `ByteRange` that
/// every signature dictionary has.
fn is_signature(dictionary: &Dictionary) -> bool {
    SIGNATURE_TYPES
        .iter()
        .any(|signature| has_type(dictionary, signature))
        || matches!(
            dictionary.get(BYTE_RANGE_KEY).map(Object::value),
            Some(Value::Array(_))
        )
}
