// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The Type 1 cipher (Adobe Type 1 Font Format, the encryption chapter:
//! eexec encryption and charstring encryption). One cipher, two keys.

/// Key of the `eexec` section.
pub(crate) const EEXEC_KEY: u16 = 55665;
/// Key of each charstring and subroutine.
pub(crate) const CHARSTRING_KEY: u16 = 4330;
/// The multiplier of the cipher's linear congruence.
const C1: u16 = 52845;
/// The increment of the cipher's linear congruence.
const C2: u16 = 22719;

/// Decrypts `cipher` with `key` and drops the first `skip` plaintext bytes,
/// which are random padding (4 for `eexec`; `lenIV` for a charstring).
pub(crate) fn decrypt(cipher: &[u8], key: u16, skip: usize) -> Vec<u8> {
    let mut r = key;
    let mut plain = Vec::with_capacity(cipher.len().saturating_sub(skip));
    for (i, &c) in cipher.iter().enumerate() {
        let p = c ^ (r >> 8) as u8;
        r = u16::from(c)
            .wrapping_add(r)
            .wrapping_mul(C1)
            .wrapping_add(C2);
        if i >= skip {
            plain.push(p);
        }
    }
    plain
}

/// The inverse of [`decrypt`], with `pad` as the leading random bytes. Used
/// by tests to build fonts.
#[cfg(test)]
pub(crate) fn encrypt(plain: &[u8], key: u16, pad: &[u8]) -> Vec<u8> {
    let mut r = key;
    let mut out = Vec::with_capacity(plain.len() + pad.len());
    for &p in pad.iter().chain(plain) {
        let c = p ^ (r >> 8) as u8;
        r = u16::from(c)
            .wrapping_add(r)
            .wrapping_mul(C1)
            .wrapping_add(C2);
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn round_trip() {
        let plain = b"/CharStrings 3 dict dup begin";
        let cipher = encrypt(plain, EEXEC_KEY, &[0, 0, 0, 0]);
        assert_ne!(&cipher[4..], plain);
        assert_eq!(decrypt(&cipher, EEXEC_KEY, 4), plain);
    }

    #[test]
    fn the_first_byte_is_derived_by_hand() {
        // c = p ^ (r >> 8): with r = 55665 = 0xD971 the key byte is 0xD9, so a
        // plaintext 0x00 encrypts to 0xD9.
        assert_eq!(encrypt(&[0], EEXEC_KEY, &[]), vec![0xD9]);
        // r' = (c + r) * 52845 + 22719 mod 65536, then the second key byte is r' >> 8.
        let r = (0xD9u32 + 55665) * 52845 + 22719;
        let key2 = ((r & 0xFFFF) >> 8) as u8;
        assert_eq!(encrypt(&[0, 0], EEXEC_KEY, &[])[1], key2);
    }

    #[test]
    fn skip_longer_than_the_input_is_empty() {
        assert!(decrypt(&[1, 2], CHARSTRING_KEY, 4).is_empty());
    }
}
