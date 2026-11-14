// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The encryption dictionary of the standard security handler: ISO
//! 32000-1:2008, 7.6.1 (Table 20), 7.6.3.2 (Table 21) and 7.6.5 (Tables
//! 25 and 26). Revisions 5 and 6 and the `AESV3` method, with the `OE`,
//! `UE` and `Perms` entries, are those of ISO 32000-2:2020, 7.6.4.

use std::collections::HashMap;

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::{Body, Location, PdfFile, XrefEntry};
use crate::syntax::lexer::shown;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const ENCRYPT_KEY: &str = "Encrypt";
const ID_KEY: &[u8] = b"ID";
const FILTER_KEY: &str = "Filter";
const V_KEY: &str = "V";
const R_KEY: &str = "R";
const LENGTH_KEY: &str = "Length";
const O_KEY: &str = "O";
const U_KEY: &str = "U";
const OE_KEY: &str = "OE";
const UE_KEY: &str = "UE";
const PERMS_KEY: &str = "Perms";
const P_KEY: &str = "P";
const ENCRYPT_METADATA_KEY: &str = "EncryptMetadata";
const CF_KEY: &str = "CF";
const STMF_KEY: &str = "StmF";
const STRF_KEY: &str = "StrF";
const EFF_KEY: &str = "EFF";
const CFM_KEY: &[u8] = b"CFM";

/// The name of the standard security handler (Table 20, `Filter`).
const STANDARD: &[u8] = b"Standard";
/// The standard crypt filter that leaves data as it is (Table 26).
pub(crate) const IDENTITY: &[u8] = b"Identity";
/// The crypt filter methods of Table 25, and `AESV3` of ISO 32000-2.
const CFM_V2: &[u8] = b"V2";
const CFM_AESV2: &[u8] = b"AESV2";
const CFM_AESV3: &[u8] = b"AESV3";

/// The bytes of `O` and `U` that revisions 2 to 4 use (Table 21).
pub(crate) const HASH_BYTES_R4: usize = 32;
/// The bytes of `O` and `U` that revisions 5 and 6 use: a 32-byte hash,
/// an 8-byte validation salt and an 8-byte key salt.
pub(crate) const HASH_BYTES_R6: usize = 48;
/// The bytes of `OE` and `UE`: the file key, encrypted.
pub(crate) const WRAPPED_KEY_BYTES: usize = 32;
/// The bytes of `Perms`: one AES block.
pub(crate) const PERMS_BYTES: usize = 16;

/// Key lengths of Table 20, `Length`: a multiple of 8 bits from 40 to 128.
const MIN_KEY_BITS: i64 = 40;
const MAX_KEY_BITS: i64 = 128;
const BITS_PER_BYTE: i64 = 8;
/// The key length of `V` 1 (Table 20) and of revision 2 (7.6.3.3,
/// Algorithm 2, step i): 40 bits.
const KEY_BYTES_40: usize = 5;
/// The key length `V` 4 takes when no `Length` gives one.
const KEY_BYTES_128: usize = 16;
/// The key length of `V` 5: 256 bits.
pub(crate) const KEY_BYTES_256: usize = 32;

/// A revision of the standard security handler (Table 21, `R`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revision {
    R2,
    R3,
    R4,
    R5,
    R6,
}

impl Revision {
    pub fn number(self) -> u8 {
        match self {
            Revision::R2 => 2,
            Revision::R3 => 3,
            Revision::R4 => 4,
            Revision::R5 => 5,
            Revision::R6 => 6,
        }
    }

    /// Whether passwords are UTF-8 and the key is not derived from them
    /// (revisions 5 and 6).
    pub(crate) fn uses_utf8_passwords(self) -> bool {
        matches!(self, Revision::R5 | Revision::R6)
    }
}

/// How a string or the data of a stream is decrypted: a crypt filter
/// method (Table 25), or the Identity filter (Table 26).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptMethod {
    /// Left as it is.
    Identity,
    /// RC4 with a key for each object (7.6.2, Algorithm 1).
    Rc4,
    /// AES-128 in CBC mode with a key for each object (7.6.2, Algorithm 1).
    Aes128,
    /// AES-256 in CBC mode with the file key itself (ISO 32000-2).
    Aes256,
}

/// The encryption dictionary of a file, read and checked, and the first
/// element of its file identifier.
#[derive(Debug, Clone)]
pub struct Encryption {
    pub(crate) version: u8,
    pub(crate) revision: Revision,
    /// The bytes of the file key.
    pub(crate) key_length: usize,
    pub(crate) owner: Vec<u8>,
    pub(crate) user: Vec<u8>,
    /// `OE`, `UE` and `Perms`, for revisions 5 and 6.
    pub(crate) owner_key: Vec<u8>,
    pub(crate) user_key: Vec<u8>,
    pub(crate) perms: Option<[u8; PERMS_BYTES]>,
    pub(crate) permissions: u32,
    pub(crate) encrypt_metadata: bool,
    pub(crate) file_id: Vec<u8>,
    /// The crypt filters of `CF` by name, each with its method, or `None`
    /// for one that is not supported.
    pub(crate) filters: HashMap<Name, Option<CryptMethod>>,
    pub(crate) streams: CryptMethod,
    pub(crate) strings: CryptMethod,
    /// `EFF`, for embedded files with no crypt filter of their own.
    pub(crate) embedded_files: Option<CryptMethod>,
    /// The encryption dictionary's own object, whose strings are never
    /// encrypted (7.6.1).
    pub(crate) dictionary_id: Option<ObjectId>,
    /// Where the dictionary is in the file, for diagnostics.
    pub(crate) offset: u64,
}

/// What a missing or ill-formed entry of the dictionary is.
fn invalid(key: &'static str) -> PdfError {
    PdfError::EncryptionDictionary { key }
}

impl Encryption {
    /// Reads the encryption dictionary that the trailer of `file` names
    /// (Table 15, `Encrypt`). `None` for a file that is not encrypted.
    ///
    /// A handler other than `Standard` is
    /// [`PdfError::UnsupportedSecurityHandler`]; an algorithm and revision
    /// not read together, [`PdfError::UnsupportedEncryption`]; an entry
    /// missing or not as its table defines it,
    /// [`PdfError::EncryptionDictionary`]. Entries are read as direct
    /// objects: 7.6.1 requires it of strings, and the dictionary is read
    /// before anything that could resolve a reference in it is known.
    pub fn read(file: &PdfFile, diagnostics: &mut Diagnostics) -> Result<Option<Self>, PdfError> {
        let trailer = file.trailer();
        let (dictionary, dictionary_id, offset) =
            match trailer.get(ENCRYPT_KEY.as_bytes()).map(Object::value) {
                None | Some(Value::Null) => return Ok(None),
                Some(Value::Dictionary(dictionary)) => (dictionary.clone(), None, 0),
                Some(Value::Reference(id)) => {
                    let (dictionary, offset) = load_dictionary(file, *id, diagnostics)?;
                    (dictionary, Some(*id), offset)
                }
                Some(_) => return Err(invalid(ENCRYPT_KEY)),
            };
        let mut encryption = Self::from_dictionary(&dictionary)?;
        encryption.dictionary_id = dictionary_id;
        encryption.offset = offset;
        encryption.file_id = match first_id(trailer) {
            Some(id) => id,
            None => {
                // Revisions 5 and 6 do not use the identifier.
                if !encryption.revision.uses_utf8_passwords() {
                    diagnostics.push(DiagnosticKind::MissingFileId, offset);
                }
                Vec::new()
            }
        };
        Ok(Some(encryption))
    }

    fn from_dictionary(dictionary: &Dictionary) -> Result<Self, PdfError> {
        match name(dictionary, FILTER_KEY)? {
            Some(handler) if handler.as_bytes() == STANDARD => {}
            Some(handler) => {
                return Err(PdfError::UnsupportedSecurityHandler {
                    name: shown(handler.as_bytes()),
                })
            }
            None => return Err(invalid(FILTER_KEY)),
        }
        // Table 20: `V` is 0 when absent, and 0 is an undocumented
        // algorithm that shall not be used.
        let version = integer(dictionary, V_KEY)?.unwrap_or(0);
        let revision_number = integer(dictionary, R_KEY)?.ok_or(invalid(R_KEY))?;
        let unsupported = || PdfError::UnsupportedEncryption {
            version,
            revision: revision_number,
        };
        // Table 21: revision 2 or 3 with `V` 1 or 2, revision 4 with `V`
        // 4; ISO 32000-2: revision 5 (deprecated) or 6 with `V` 5. `V` 3
        // is an unpublished algorithm (Table 20).
        let revision = match (version, revision_number) {
            (1 | 2, 2) => Revision::R2,
            (1 | 2, 3) => Revision::R3,
            (4, 4) => Revision::R4,
            (5, 5) => Revision::R5,
            (5, 6) => Revision::R6,
            _ => return Err(unsupported()),
        };
        // `version` is one of 1, 2, 4 and 5.
        let version = u8::try_from(version).map_err(|_| unsupported())?;

        let hash_bytes = if revision.uses_utf8_passwords() {
            HASH_BYTES_R6
        } else {
            HASH_BYTES_R4
        };
        let owner = string_prefix(dictionary, O_KEY, hash_bytes)?;
        let user = string_prefix(dictionary, U_KEY, hash_bytes)?;
        let (owner_key, user_key, perms) = if revision.uses_utf8_passwords() {
            let perms = match string(dictionary, PERMS_KEY)? {
                Some(perms) => Some(
                    perms
                        .get(..PERMS_BYTES)
                        .and_then(|perms| perms.try_into().ok())
                        .ok_or(invalid(PERMS_KEY))?,
                ),
                None => None,
            };
            (
                string_prefix(dictionary, OE_KEY, WRAPPED_KEY_BYTES)?,
                string_prefix(dictionary, UE_KEY, WRAPPED_KEY_BYTES)?,
                perms,
            )
        } else {
            (Vec::new(), Vec::new(), None)
        };
        let permissions = permission_bits(dictionary)?;

        let uses_filters = version >= 4;
        // `EncryptMetadata` is meaningful only with crypt filters (Table
        // 21); without them every stream is encrypted.
        let encrypt_metadata = match boolean(dictionary, ENCRYPT_METADATA_KEY)? {
            Some(value) if uses_filters => value,
            _ => true,
        };

        let key_length = match (version, revision) {
            (_, Revision::R2) | (1, _) => KEY_BYTES_40,
            (2, _) => key_bytes(dictionary, LENGTH_KEY)?.unwrap_or(KEY_BYTES_40),
            (4, _) => match key_bytes(dictionary, LENGTH_KEY)? {
                Some(length) => length,
                None => default_filter_length(dictionary)?,
            },
            _ => KEY_BYTES_256,
        };

        let (filters, streams, strings, embedded_files) = if uses_filters {
            let filters = crypt_filters(dictionary, version, key_length)?;
            let method = |key| -> Result<Option<CryptMethod>, PdfError> {
                match name(dictionary, key)? {
                    None => Ok(None),
                    Some(filter) => resolve(&filters, &filter).map(Some),
                }
            };
            // Table 20: `StmF` and `StrF` are Identity when absent.
            let streams = method(STMF_KEY)?.unwrap_or(CryptMethod::Identity);
            let strings = method(STRF_KEY)?.unwrap_or(CryptMethod::Identity);
            let embedded_files = method(EFF_KEY)?;
            (filters, streams, strings, embedded_files)
        } else {
            (HashMap::new(), CryptMethod::Rc4, CryptMethod::Rc4, None)
        };

        Ok(Self {
            version,
            revision,
            key_length,
            owner,
            user,
            owner_key,
            user_key,
            perms,
            permissions,
            encrypt_metadata,
            file_id: Vec::new(),
            filters,
            streams,
            strings,
            embedded_files,
            dictionary_id: None,
            offset: 0,
        })
    }

    /// The algorithm (Table 20, `V`).
    pub fn version(&self) -> u8 {
        self.version
    }

    pub fn revision(&self) -> Revision {
        self.revision
    }

    /// The length of the file key in bits.
    pub fn key_bits(&self) -> usize {
        self.key_length * BITS_PER_BYTE as usize
    }

    /// How strings are decrypted.
    pub fn string_method(&self) -> CryptMethod {
        self.strings
    }

    /// How streams are decrypted, unless a stream names its own crypt
    /// filter.
    pub fn stream_method(&self) -> CryptMethod {
        self.streams
    }

    /// Whether the document-level metadata stream is encrypted (Table 21,
    /// `EncryptMetadata`).
    pub fn encrypt_metadata(&self) -> bool {
        self.encrypt_metadata
    }

    /// The method of crypt filter `name`: the Identity filter, or one of
    /// `CF`. Any other name, or a filter whose method is not supported, is
    /// [`PdfError::UnsupportedCryptFilter`].
    pub(crate) fn filter(&self, name: &Name) -> Result<CryptMethod, PdfError> {
        resolve(&self.filters, name)
    }
}

/// Loads the encryption dictionary, object `id`. 7.5.7 keeps it out of
/// object streams, and one there could not be read before the key is
/// known.
fn load_dictionary(
    file: &PdfFile,
    id: ObjectId,
    diagnostics: &mut Diagnostics,
) -> Result<(Dictionary, u64), PdfError> {
    if let Some(XrefEntry::Compressed { .. }) = file.entry(id.number) {
        return Err(invalid(ENCRYPT_KEY));
    }
    let object = file.load(id, diagnostics)?.ok_or(invalid(ENCRYPT_KEY))?;
    let offset = match object.location {
        Location::File(span) => span.start,
        Location::ObjectStream { .. } => return Err(invalid(ENCRYPT_KEY)),
    };
    match object.body {
        Body::Object(object) => match object.into_value() {
            Value::Dictionary(dictionary) => Ok((dictionary, offset)),
            _ => Err(invalid(ENCRYPT_KEY)),
        },
        Body::Stream(_) => Err(invalid(ENCRYPT_KEY)),
    }
}

/// The first element of the trailer's `ID` (Table 15), when it is a
/// string.
fn first_id(trailer: &Dictionary) -> Option<Vec<u8>> {
    match trailer.get(ID_KEY).map(Object::value) {
        Some(Value::Array(items)) => match items.first().map(Object::value) {
            Some(Value::String(id)) => Some(id.as_bytes().to_vec()),
            _ => None,
        },
        _ => None,
    }
}

/// The value of `key` when present and not null.
fn entry<'a>(dictionary: &'a Dictionary, key: &str) -> Option<&'a Value> {
    match dictionary.get(key.as_bytes()).map(Object::value) {
        None | Some(Value::Null) => None,
        value => value,
    }
}

fn name(dictionary: &Dictionary, key: &'static str) -> Result<Option<Name>, PdfError> {
    match entry(dictionary, key) {
        None => Ok(None),
        Some(Value::Name(name)) => Ok(Some(name.clone())),
        Some(_) => Err(invalid(key)),
    }
}

fn integer(dictionary: &Dictionary, key: &'static str) -> Result<Option<i64>, PdfError> {
    match entry(dictionary, key) {
        None => Ok(None),
        Some(Value::Integer(value)) => Ok(Some(*value)),
        Some(_) => Err(invalid(key)),
    }
}

fn boolean(dictionary: &Dictionary, key: &'static str) -> Result<Option<bool>, PdfError> {
    match entry(dictionary, key) {
        None => Ok(None),
        Some(Value::Boolean(value)) => Ok(Some(*value)),
        Some(_) => Err(invalid(key)),
    }
}

fn string<'a>(dictionary: &'a Dictionary, key: &'static str) -> Result<Option<&'a [u8]>, PdfError> {
    match entry(dictionary, key) {
        None => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_bytes())),
        Some(_) => Err(invalid(key)),
    }
}

/// The first `length` bytes of a required string. Writers pad some of
/// these strings past their defined length; a shorter string is refused.
fn string_prefix(
    dictionary: &Dictionary,
    key: &'static str,
    length: usize,
) -> Result<Vec<u8>, PdfError> {
    string(dictionary, key)?
        .and_then(|bytes| bytes.get(..length))
        .map(<[u8]>::to_vec)
        .ok_or(invalid(key))
}

/// `P` as the unsigned 32-bit quantity 7.6.3.2 reads it as. Writers give
/// it as a signed or an unsigned 32-bit integer; both are taken.
fn permission_bits(dictionary: &Dictionary) -> Result<u32, PdfError> {
    let value = integer(dictionary, P_KEY)?.ok_or(invalid(P_KEY))?;
    if let Ok(signed) = i32::try_from(value) {
        return Ok(u32::from_ne_bytes(signed.to_ne_bytes()));
    }
    u32::try_from(value).map_err(|_| invalid(P_KEY))
}

/// A key length in bits (Table 20, `Length`) as bytes: a multiple of 8
/// from 40 to 128.
fn key_bytes(dictionary: &Dictionary, key: &'static str) -> Result<Option<usize>, PdfError> {
    match integer(dictionary, key)? {
        None => Ok(None),
        Some(bits)
            if (MIN_KEY_BITS..=MAX_KEY_BITS).contains(&bits) && bits % BITS_PER_BYTE == 0 =>
        {
            usize::try_from(bits / BITS_PER_BYTE)
                .map(Some)
                .map_err(|_| invalid(key))
        }
        Some(_) => Err(invalid(key)),
    }
}

/// The key length of `V` 4 when the dictionary gives no `Length`: that
/// of the crypt filter `StmF` names. Table 25 says the standard handler
/// gives a filter's `Length` in bytes ("16 means 128"), and writers give
/// it in bits too: a value below the 40-bit minimum is read as bytes.
/// With neither, 128 bits, the only length `AESV2` can use.
fn default_filter_length(dictionary: &Dictionary) -> Result<usize, PdfError> {
    let filter = match (entry(dictionary, CF_KEY), name(dictionary, STMF_KEY)?) {
        (Some(Value::Dictionary(filters)), Some(stream_filter)) => {
            match filters.get(stream_filter.as_bytes()).map(Object::value) {
                Some(Value::Dictionary(filter)) => Some(filter),
                _ => None,
            }
        }
        _ => None,
    };
    let Some(length) = filter
        .map(|filter| integer(filter, LENGTH_KEY))
        .transpose()?
    else {
        return Ok(KEY_BYTES_128);
    };
    let bits = match length {
        None => return Ok(KEY_BYTES_128),
        Some(bytes) if bytes < MIN_KEY_BITS => bytes.saturating_mul(BITS_PER_BYTE),
        Some(bits) => bits,
    };
    if (MIN_KEY_BITS..=MAX_KEY_BITS).contains(&bits) && bits % BITS_PER_BYTE == 0 {
        usize::try_from(bits / BITS_PER_BYTE).map_err(|_| invalid(LENGTH_KEY))
    } else {
        Err(invalid(LENGTH_KEY))
    }
}

/// The crypt filters of `CF` (Table 20) with their methods. Entries named
/// as a standard filter are ignored, as Table 20 says. A method is
/// supported when the algorithm allows it: `V2` and `AESV2` under `V` 4,
/// `AESV2` only with a 128-bit key, and `AESV3` under `V` 5.
fn crypt_filters(
    dictionary: &Dictionary,
    version: u8,
    key_length: usize,
) -> Result<HashMap<Name, Option<CryptMethod>>, PdfError> {
    let filters = match entry(dictionary, CF_KEY) {
        None => return Ok(HashMap::new()),
        Some(Value::Dictionary(filters)) => filters,
        Some(_) => return Err(invalid(CF_KEY)),
    };
    let mut methods = HashMap::new();
    for (filter_name, filter) in filters.iter() {
        if filter_name.as_bytes() == IDENTITY {
            continue;
        }
        let Value::Dictionary(filter) = filter.value() else {
            return Err(invalid(CF_KEY));
        };
        // Table 25: `CFM` is None when absent, which hands decryption to
        // a handler of the application's own.
        let method = match filter.get(CFM_KEY).map(Object::value) {
            Some(Value::Name(method)) => match (method.as_bytes(), version) {
                (CFM_V2, 4) => Some(CryptMethod::Rc4),
                (CFM_AESV2, 4) if key_length == KEY_BYTES_128 => Some(CryptMethod::Aes128),
                (CFM_AESV3, 5) => Some(CryptMethod::Aes256),
                _ => None,
            },
            None | Some(Value::Null) => None,
            Some(_) => return Err(invalid(CF_KEY)),
        };
        methods.insert(filter_name.clone(), method);
    }
    Ok(methods)
}

fn resolve(
    filters: &HashMap<Name, Option<CryptMethod>>,
    name: &Name,
) -> Result<CryptMethod, PdfError> {
    if name.as_bytes() == IDENTITY {
        return Ok(CryptMethod::Identity);
    }
    filters
        .get(name)
        .copied()
        .flatten()
        .ok_or_else(|| PdfError::UnsupportedCryptFilter {
            name: shown(name.as_bytes()),
        })
}
