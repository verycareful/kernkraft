// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The ciphers of ISO 32000-1:2008, 7.6.2: RC4, and AES in cipher block
//! chaining mode with the padding of RFC 2898 (PKCS #5).
//!
//! The ciphers themselves are the `rc4` and `aes` crates and the chaining
//! is the `cbc` crate. What is here is how PDF frames encrypted data: an
//! AES string or stream is a 16-byte initialization vector, then whole
//! blocks, the last of them padded.

use aes::cipher::{
    Array, BlockCipherDecrypt, BlockModeDecrypt, BlockModeEncrypt, KeyInit, KeyIvInit, StreamCipher,
};

use crate::PdfError;

/// The block size of AES (FIPS 197), which is also the size of the
/// initialization vector that starts encrypted data.
pub(crate) const AES_BLOCK: usize = 16;

/// The key of an AES cipher: 128 bits for the `AESV2` crypt filter method
/// (Table 25), 256 bits for `AESV3`.
#[derive(Clone, Copy)]
pub(crate) enum AesKey {
    Aes128([u8; 16]),
    Aes256([u8; 32]),
}

/// Encrypts or decrypts `data` in place with RC4, which is its own
/// inverse. A key that RC4 cannot take (none of 1 to 256 bytes) is a
/// fault of the `Length` the key's size came from.
pub(crate) fn rc4(key: &[u8], data: &mut [u8]) -> Result<(), PdfError> {
    let mut cipher = rc4::Rc4::new_from_slice(key)
        .map_err(|_| PdfError::EncryptionDictionary { key: "Length" })?;
    cipher.apply_keystream(data);
    Ok(())
}

/// Decrypts the whole blocks of `data` in place, chained from `iv`. Bytes
/// after the last whole block are left as they are.
pub(crate) fn cbc_decrypt(key: &AesKey, iv: [u8; AES_BLOCK], data: &mut [u8]) {
    let (blocks, _) = Array::slice_as_chunks_mut(data);
    match key {
        AesKey::Aes128(key) => {
            cbc::Decryptor::<aes::Aes128>::new(&(*key).into(), &iv.into()).decrypt_blocks(blocks);
        }
        AesKey::Aes256(key) => {
            cbc::Decryptor::<aes::Aes256>::new(&(*key).into(), &iv.into()).decrypt_blocks(blocks);
        }
    }
}

/// Encrypts the whole blocks of `data` in place with AES-128, chained
/// from `iv`.
pub(crate) fn cbc_encrypt_128(key: [u8; 16], iv: [u8; AES_BLOCK], data: &mut [u8]) {
    let (blocks, _) = Array::slice_as_chunks_mut(data);
    cbc::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into()).encrypt_blocks(blocks);
}

/// Decrypts one block with AES-256 and no chaining.
pub(crate) fn decrypt_block_256(key: [u8; 32], block: [u8; AES_BLOCK]) -> [u8; AES_BLOCK] {
    let mut block = block.into();
    aes::Aes256::new(&key.into()).decrypt_block(&mut block);
    block.into()
}

/// Decrypts a string or the data of a stream as 7.6.2 frames it: the
/// first 16 bytes are the initialization vector, whole blocks follow, and
/// the last block ends in 1 to 16 bytes that each hold their count.
///
/// Returns the plain bytes, and whether the data was framed so. When it
/// was not, the whole blocks after the vector are decrypted and returned
/// with nothing removed from their end, and bytes that make no whole block
/// are dropped.
pub(crate) fn decrypt_message(key: &AesKey, data: &[u8]) -> (Vec<u8>, bool) {
    let Some((iv, body)) = data.split_first_chunk::<AES_BLOCK>() else {
        return (Vec::new(), false);
    };
    let whole = body.len() - body.len() % AES_BLOCK;
    let mut plain = body.get(..whole).unwrap_or_default().to_vec();
    cbc_decrypt(key, *iv, &mut plain);
    let padding = match plain.last() {
        Some(&count) if whole == body.len() => usize::from(count),
        _ => return (plain, false),
    };
    let unpadded = plain.len().checked_sub(padding);
    match unpadded.filter(|_| (1..=AES_BLOCK).contains(&padding)) {
        Some(unpadded)
            if plain
                .get(unpadded..)
                .is_some_and(|pad| pad.iter().all(|&byte| usize::from(byte) == padding)) =>
        {
            plain.truncate(unpadded);
            (plain, true)
        }
        _ => (plain, false),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn hex(text: &str) -> Vec<u8> {
        let digits: Vec<u8> = text.bytes().filter(u8::is_ascii_hexdigit).collect();
        digits
            .chunks(2)
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect()
    }

    /// The first 16 bytes of the keystream of two keys of RFC 6229,
    /// section 2: 40 bits and 128 bits, the lengths PDF uses.
    #[test]
    fn rc4_keystream_of_rfc_6229() {
        let cases = [
            ("0102030405", "b2396305f03dc027ccc3524a0a1118a8"),
            (
                "0102030405060708090a0b0c0d0e0f10",
                "9ac7cc9a609d1ef7b2932899cde41b97",
            ),
        ];
        for (key, keystream) in cases {
            let mut data = [0u8; 16];
            rc4(&hex(key), &mut data).unwrap();
            assert_eq!(data.to_vec(), hex(keystream), "key {key}");
            // Applying the keystream again gives the zeros back.
            rc4(&hex(key), &mut data).unwrap();
            assert_eq!(data, [0u8; 16]);
        }
    }

    #[test]
    fn rc4_refuses_an_empty_key() {
        assert!(matches!(
            rc4(&[], &mut [0u8; 4]),
            Err(PdfError::EncryptionDictionary { key: "Length" })
        ));
    }

    /// NIST SP 800-38A, F.2.1 and F.2.2 (CBC-AES128) and F.2.5 and F.2.6
    /// (CBC-AES256): the first two blocks of each.
    const IV: &str = "000102030405060708090a0b0c0d0e0f";
    const PLAIN: &str = "6bc1bee22e409f96e93d7e117393172a ae2d8a571e03ac9c9eb76fac45af8e51";
    const KEY_128: &str = "2b7e151628aed2a6abf7158809cf4f3c";
    const CIPHER_128: &str = "7649abac8119b246cee98e9b12e9197d 5086cb9b507219ee95db113a917678b2";
    const KEY_256: &str = "603deb1015ca71be2b73aef0857d7781 1f352c073b6108d72d9810a30914dff4";
    const CIPHER_256: &str = "f58c4c04d6e5f1ba779eabfb5f7bfbd6 9cfc4e967edb808d679f777bc6702c7d";

    fn array<const N: usize>(text: &str) -> [u8; N] {
        hex(text).try_into().unwrap()
    }

    #[test]
    fn cbc_matches_sp_800_38a() {
        let mut data = hex(PLAIN);
        cbc_encrypt_128(array(KEY_128), array(IV), &mut data);
        assert_eq!(data, hex(CIPHER_128));
        cbc_decrypt(&AesKey::Aes128(array(KEY_128)), array(IV), &mut data);
        assert_eq!(data, hex(PLAIN));

        let mut data = hex(CIPHER_256);
        cbc_decrypt(&AesKey::Aes256(array(KEY_256)), array(IV), &mut data);
        assert_eq!(data, hex(PLAIN));
    }

    /// With a zero vector the first block of a chain is the block cipher
    /// alone, so F.2.6's first block gives a single-block case: the cipher
    /// block decrypts to the plain block XOR the vector of that example.
    #[test]
    fn one_block_without_chaining() {
        let cipher: [u8; 16] = array(&CIPHER_256[..32]);
        let plain: [u8; 16] = array(&PLAIN[..32]);
        let iv: [u8; 16] = array(IV);
        let expected: [u8; 16] = std::array::from_fn(|i| plain[i] ^ iv[i]);
        assert_eq!(decrypt_block_256(array(KEY_256), cipher), expected);
    }

    /// A message of `plain` framed as 7.6.2 says, under AES-128.
    fn framed(plain: &[u8]) -> Vec<u8> {
        let padding = AES_BLOCK - plain.len() % AES_BLOCK;
        let mut body = plain.to_vec();
        body.extend(std::iter::repeat_n(padding as u8, padding));
        cbc_encrypt_128(array(KEY_128), array(IV), &mut body);
        let mut message = hex(IV);
        message.extend(body);
        message
    }

    #[test]
    fn message_of_every_length_up_to_two_blocks() {
        let key = AesKey::Aes128(array(KEY_128));
        for length in 0..=2 * AES_BLOCK {
            let plain: Vec<u8> = (0..length as u8).collect();
            let message = framed(&plain);
            // 7.6.2: the pad is 16 - (M mod 16) bytes, a whole block when
            // M is a multiple of 16.
            assert_eq!(
                message.len(),
                AES_BLOCK + (length / AES_BLOCK + 1) * AES_BLOCK
            );
            assert_eq!(decrypt_message(&key, &message), (plain, true));
        }
    }

    #[test]
    fn message_that_is_not_framed() {
        let key = AesKey::Aes128(array(KEY_128));
        let plain = b"nine byte";
        let message = framed(plain);

        // Shorter than a vector: nothing to decrypt.
        for length in [0, 1, AES_BLOCK - 1] {
            assert_eq!(
                decrypt_message(&key, &message[..length]),
                (Vec::new(), false)
            );
        }
        // A vector and no block.
        assert_eq!(
            decrypt_message(&key, &message[..AES_BLOCK]),
            (Vec::new(), false)
        );
        // Bytes after the last whole block are dropped and the padding is
        // not looked for.
        let mut padded = plain.to_vec();
        padded.extend([7u8; 7]);
        let mut longer = message.clone();
        longer.extend([0xAA; 5]);
        assert_eq!(decrypt_message(&key, &longer), (padded, false));

        // A last byte that is no padding count: 0, and more than a block.
        for last in [0u8, 17] {
            let mut body = [3u8; AES_BLOCK];
            body[AES_BLOCK - 1] = last;
            let expected = body.to_vec();
            cbc_encrypt_128(array(KEY_128), array(IV), &mut body);
            let mut message = hex(IV);
            message.extend(body);
            assert_eq!(decrypt_message(&key, &message), (expected, false));
        }
        // A count that the bytes before it do not repeat.
        let mut body = [3u8; AES_BLOCK];
        body[AES_BLOCK - 1] = 4;
        let expected = body.to_vec();
        cbc_encrypt_128(array(KEY_128), array(IV), &mut body);
        let mut message = hex(IV);
        message.extend(body);
        assert_eq!(decrypt_message(&key, &message), (expected, false));
    }
}
