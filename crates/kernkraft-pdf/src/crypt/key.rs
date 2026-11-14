// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The keys of the standard security handler and the password checks that
//! give them.
//!
//! Revisions 2 to 4: ISO 32000-1:2008, 7.6.2 (Algorithm 1, the key of one
//! object), 7.6.3.3 (Algorithm 2, the file key) and 7.6.3.4 (Algorithms 3
//! to 7). Revisions 5 and 6: ISO 32000-2:2020, 7.6.4.3.3 and 7.6.4.4
//! (Algorithms 2.A and 2.B, 11 and 12); revision 5 hashes once where
//! revision 6 runs Algorithm 2.B.

use md5::{Digest, Md5};
use sha2::{Sha256, Sha384, Sha512};

use super::cipher::{cbc_decrypt, cbc_encrypt_128, rc4, AesKey, AES_BLOCK};
use super::dictionary::{Encryption, Revision, HASH_BYTES_R4, KEY_BYTES_256};
use super::password::MAX_UTF8_PASSWORD_BYTES;
use crate::syntax::ObjectId;
use crate::PdfError;

/// The padding string of Algorithm 2, step a.
const PADDING: [u8; 32] = [
    0x28, 0xBF, 0x4E, 0x5E, 0x4E, 0x75, 0x8A, 0x41, 0x64, 0x00, 0x4E, 0x56, 0xFF, 0xFA, 0x01, 0x08,
    0x2E, 0x2E, 0x00, 0xB6, 0xD0, 0x68, 0x3E, 0x80, 0x2F, 0x0C, 0xA9, 0xFE, 0x64, 0x53, 0x69, 0x7A,
];
/// The bytes Algorithm 1, step b, adds for AES: "sAlT".
const AES_SALT: [u8; 4] = [0x73, 0x41, 0x6C, 0x54];
/// The bytes of the object number and of the generation that Algorithm 1
/// adds to the key, low-order byte first.
const OBJECT_NUMBER_BYTES: usize = 3;
const GENERATION_BYTES: usize = 2;
/// The longest key of one object: an MD5 hash (Algorithm 1, step d).
const MAX_OBJECT_KEY_BYTES: usize = 16;
/// The rehashings of Algorithm 2, step h, and Algorithm 3, step c.
const REHASHES: usize = 50;
/// The further RC4 passes of Algorithm 3, step g, and Algorithm 5, step e.
const RC4_PASSES: u8 = 19;
/// The bytes of `U` that Algorithm 6 compares for revision 3 or greater.
const U_CHECK_BYTES_R3: usize = 16;
/// Algorithm 2, step f: what is hashed when metadata is not encrypted.
const UNENCRYPTED_METADATA: [u8; 4] = [0xFF; 4];

/// Revisions 5 and 6: the parts of `O` and `U`.
const HASH_END: usize = 32;
const VALIDATION_SALT_END: usize = 40;
const KEY_SALT_END: usize = 48;
/// Algorithm 2.B: the rounds always run, the repetitions of the input of
/// each, and the bytes of `E` read as a number to choose the next hash.
const MIN_ROUNDS: usize = 64;
const ROUND_INPUT_REPEATS: usize = 64;
const ROUND_STOP_OFFSET: usize = 32;
const SELECTOR_BYTES: usize = 16;
const HASH_CHOICES: u32 = 3;

fn md5(parts: &[&[u8]]) -> [u8; 16] {
    let mut hash = Md5::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().into()
}

/// Algorithm 2, step a: the first 32 bytes of `password`, filled up from
/// the padding string.
fn pad(password: &[u8]) -> [u8; 32] {
    let mut padded = PADDING;
    let used = password.len().min(padded.len());
    padded[..used].copy_from_slice(&password[..used]);
    if let Some(rest) = padded.get_mut(used..) {
        rest.copy_from_slice(&PADDING[..PADDING.len() - used]);
    }
    padded
}

/// `key` with each byte XORed with `counter` (Algorithms 3, 5 and 7).
fn xor_key(key: &[u8], counter: u8) -> Vec<u8> {
    key.iter().map(|byte| byte ^ counter).collect()
}

/// Algorithm 2: the file key computed from a padded password.
fn file_key(encryption: &Encryption, padded: &[u8; 32]) -> Vec<u8> {
    let n = encryption.key_length;
    let mut hash = Md5::new();
    hash.update(padded);
    hash.update(&encryption.owner[..HASH_BYTES_R4]);
    hash.update(encryption.permissions.to_le_bytes());
    hash.update(&encryption.file_id);
    if encryption.revision == Revision::R4 && !encryption.encrypt_metadata {
        hash.update(UNENCRYPTED_METADATA);
    }
    let mut digest: [u8; 16] = hash.finalize().into();
    if encryption.revision != Revision::R2 {
        for _ in 0..REHASHES {
            digest = md5(&[&digest[..n]]);
        }
    }
    digest[..n].to_vec()
}

/// Algorithms 4 and 5 up to their last step, then Algorithm 6: the file
/// key when `padded` is the user password.
fn check_user(encryption: &Encryption, padded: &[u8; 32]) -> Result<Option<Vec<u8>>, PdfError> {
    let key = file_key(encryption, padded);
    let matches = if encryption.revision == Revision::R2 {
        let mut value = PADDING;
        rc4(&key, &mut value)?;
        value[..] == encryption.user[..HASH_BYTES_R4]
    } else {
        let mut value = md5(&[&PADDING, &encryption.file_id]);
        rc4(&key, &mut value)?;
        for counter in 1..=RC4_PASSES {
            rc4(&xor_key(&key, counter), &mut value)?;
        }
        value[..U_CHECK_BYTES_R3] == encryption.user[..U_CHECK_BYTES_R3]
    };
    Ok(matches.then_some(key))
}

/// Algorithm 7: the file key when `padded` is the owner password. The
/// owner password decrypts `O` to the user password, which Algorithm 6
/// then checks.
fn check_owner(encryption: &Encryption, padded: &[u8; 32]) -> Result<Option<Vec<u8>>, PdfError> {
    let n = encryption.key_length;
    let mut digest = md5(&[padded]);
    if encryption.revision != Revision::R2 {
        for _ in 0..REHASHES {
            digest = md5(&[&digest]);
        }
    }
    let key = &digest[..n];
    let mut user: [u8; 32] = PADDING;
    user.copy_from_slice(&encryption.owner[..HASH_BYTES_R4]);
    if encryption.revision == Revision::R2 {
        rc4(key, &mut user)?;
    } else {
        for counter in (0..=RC4_PASSES).rev() {
            rc4(&xor_key(key, counter), &mut user)?;
        }
    }
    check_user(encryption, &user)
}

/// Which password a check accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// The owner password: the file's permissions do not bind (7.6.3.1).
    Owner,
    /// The user password, or the empty one when the file has none.
    User,
}

/// The file key `password` gives, and which password it is; `None` when
/// it is neither. The owner password is tried first, so a password that is
/// both is the owner's.
pub(crate) fn authenticate(
    encryption: &Encryption,
    password: &[u8],
) -> Result<Option<(Vec<u8>, Access)>, PdfError> {
    if encryption.revision.uses_utf8_passwords() {
        let password = &password[..password.len().min(MAX_UTF8_PASSWORD_BYTES)];
        return Ok(check_owner_r6(encryption, password)
            .map(|key| (key, Access::Owner))
            .or_else(|| check_user_r6(encryption, password).map(|key| (key, Access::User))));
    }
    let padded = pad(password);
    if let Some(key) = check_owner(encryption, &padded)? {
        return Ok(Some((key, Access::Owner)));
    }
    Ok(check_user(encryption, &padded)?.map(|key| (key, Access::User)))
}

/// Algorithm 2.B (revision 6), or the single SHA-256 that stands for it in
/// revision 5: the hash of a password with a salt and, for the owner
/// password, the 48 bytes of `U`.
fn hash_r6(revision: Revision, password: &[u8], salt: &[u8], user: &[u8]) -> [u8; 32] {
    let mut k: Vec<u8> = Sha256::new()
        .chain_update(password)
        .chain_update(salt)
        .chain_update(user)
        .finalize()
        .to_vec();
    if revision == Revision::R5 {
        return first_32(&k);
    }
    let mut round = 0;
    loop {
        let mut e: Vec<u8> =
            Vec::with_capacity((password.len() + k.len() + user.len()) * ROUND_INPUT_REPEATS);
        for _ in 0..ROUND_INPUT_REPEATS {
            e.extend_from_slice(password);
            e.extend_from_slice(&k);
            e.extend_from_slice(user);
        }
        let aes_key: [u8; 16] = k[..16].try_into().unwrap_or_default();
        let iv: [u8; AES_BLOCK] = k[16..32].try_into().unwrap_or_default();
        // Every part is repeated 64 times, so `e` is whole blocks.
        cbc_encrypt_128(aes_key, iv, &mut e);
        // The first 16 bytes of E as a number modulo 3: 256 is 1 modulo
        // 3, so the sum of the bytes has the same remainder.
        let selector: u32 = e[..SELECTOR_BYTES]
            .iter()
            .map(|&byte| u32::from(byte))
            .sum();
        k = match selector % HASH_CHOICES {
            0 => Sha256::digest(&e).to_vec(),
            1 => Sha384::digest(&e).to_vec(),
            _ => Sha512::digest(&e).to_vec(),
        };
        round += 1;
        let last = e.last().copied().map_or(0, usize::from);
        if round >= MIN_ROUNDS && last + ROUND_STOP_OFFSET <= round {
            break;
        }
    }
    first_32(&k)
}

fn first_32(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0u8; 32];
    out.copy_from_slice(&bytes[..32]);
    out
}

/// Decrypts a 32-byte wrapped file key (`OE` or `UE`) with AES-256 in CBC
/// mode, a zero vector and no padding.
fn unwrap_key(intermediate: [u8; 32], wrapped: &[u8]) -> Vec<u8> {
    let mut key = wrapped[..KEY_BYTES_256].to_vec();
    cbc_decrypt(&AesKey::Aes256(intermediate), [0; AES_BLOCK], &mut key);
    key
}

/// Algorithm 11, and Algorithm 2.A for the key: the file key when
/// `password` is the user password.
fn check_user_r6(encryption: &Encryption, password: &[u8]) -> Option<Vec<u8>> {
    let user = &encryption.user;
    let revision = encryption.revision;
    let hash = hash_r6(
        revision,
        password,
        &user[HASH_END..VALIDATION_SALT_END],
        &[],
    );
    if hash[..] != user[..HASH_END] {
        return None;
    }
    let intermediate = hash_r6(
        revision,
        password,
        &user[VALIDATION_SALT_END..KEY_SALT_END],
        &[],
    );
    Some(unwrap_key(intermediate, &encryption.user_key))
}

/// Algorithm 12, and Algorithm 2.A for the key: the file key when
/// `password` is the owner password.
fn check_owner_r6(encryption: &Encryption, password: &[u8]) -> Option<Vec<u8>> {
    let owner = &encryption.owner;
    let user = &encryption.user[..KEY_SALT_END];
    let revision = encryption.revision;
    let hash = hash_r6(
        revision,
        password,
        &owner[HASH_END..VALIDATION_SALT_END],
        user,
    );
    if hash[..] != owner[..HASH_END] {
        return None;
    }
    let intermediate = hash_r6(
        revision,
        password,
        &owner[VALIDATION_SALT_END..KEY_SALT_END],
        user,
    );
    Some(unwrap_key(intermediate, &encryption.owner_key))
}

/// Algorithm 1, steps a to d: the key that encrypts the strings and
/// streams of object `id` under a file key of revision 2 to 4.
pub(crate) fn object_key(file_key: &[u8], id: ObjectId, aes: bool) -> Vec<u8> {
    let mut hash = Md5::new();
    hash.update(file_key);
    hash.update(&id.number.to_le_bytes()[..OBJECT_NUMBER_BYTES]);
    hash.update(&id.generation.to_le_bytes()[..GENERATION_BYTES]);
    if aes {
        hash.update(AES_SALT);
    }
    let digest: [u8; 16] = hash.finalize().into();
    let length =
        (file_key.len() + OBJECT_NUMBER_BYTES + GENERATION_BYTES).min(MAX_OBJECT_KEY_BYTES);
    digest[..length].to_vec()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn pad_fills_from_the_padding_string() {
        assert_eq!(pad(b""), PADDING);
        let padded = pad(b"abc");
        assert_eq!(&padded[..3], b"abc");
        assert_eq!(&padded[3..], &PADDING[..29]);
        let long = [b'x'; 40];
        assert_eq!(pad(&long), [b'x'; 32]);
    }

    /// RFC 1321, A.5: the MD5 test suite's first entries.
    #[test]
    fn md5_of_rfc_1321() {
        assert_eq!(
            md5(&[b""]),
            [
                0xd4, 0x1d, 0x8c, 0xd9, 0x8f, 0x00, 0xb2, 0x04, 0xe9, 0x80, 0x09, 0x98, 0xec, 0xf8,
                0x42, 0x7e
            ]
        );
        assert_eq!(md5(&[b"a", b"bc"]), md5(&[b"abc"]));
    }

    /// Algorithm 1 for object 0x0A0B0C of generation 0x0D0E: the key, then
    /// the number's three low bytes and the generation's two, low-order
    /// first, then "sAlT" for AES, all hashed; n + 5 bytes of the hash,
    /// at most 16.
    #[test]
    fn object_key_of_algorithm_1() {
        let id = ObjectId {
            number: 0xFF0A_0B0C,
            generation: 0x0D0E,
        };
        let key = [1u8, 2, 3, 4, 5];
        let expected = md5(&[&key, &[0x0C, 0x0B, 0x0A, 0x0E, 0x0D]]);
        assert_eq!(object_key(&key, id, false), expected[..10]);
        let expected = md5(&[&key, &[0x0C, 0x0B, 0x0A, 0x0E, 0x0D], b"sAlT"]);
        assert_eq!(object_key(&key, id, true), expected[..10]);
        assert_eq!(object_key(&[7u8; 16], id, false).len(), 16);
        assert_eq!(object_key(&[7u8; 11], id, false).len(), 16);
        assert_eq!(object_key(&[7u8; 10], id, false).len(), 15);
    }
}
