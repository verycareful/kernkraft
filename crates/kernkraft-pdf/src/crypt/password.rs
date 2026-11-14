// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The bytes a password is checked as.
//!
//! Security handlers of revisions 2 to 4 take a password in
//! PDFDocEncoding (ISO 32000-1:2008, 7.6.3.3; the encoding is Table D.2).
//! Revisions 5 and 6 take its UTF-8 bytes after the SASLprep profile
//! (RFC 4013) of stringprep (RFC 3454), at most 127 of them (ISO
//! 32000-2:2020, 7.6.4.3.3).

use super::dictionary::Revision;
use super::saslprep::saslprep;
use crate::pdf_doc::pdf_doc_encode;

/// The most bytes of a password that revisions 5 and 6 use.
pub(crate) const MAX_UTF8_PASSWORD_BYTES: usize = 127;

/// The byte strings `password` is tried as, in order, each with whether it
/// is the form the revision defines: that form when the password has
/// one, then its UTF-8 bytes when they differ from it. Writers exist that
/// encrypt with the UTF-8 bytes whatever the revision.
pub(crate) fn candidates(revision: Revision, password: &str) -> Vec<(Vec<u8>, bool)> {
    let defined = if revision.uses_utf8_passwords() {
        saslprep(password).map(String::into_bytes)
    } else {
        pdf_doc_encode(password)
    };
    let typed = password.as_bytes().to_vec();
    match defined {
        Some(defined) if defined == typed => vec![(typed, true)],
        Some(defined) => vec![(defined, true), (typed, false)],
        None => vec![(typed, false)],
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn candidates_for_revisions_2_to_4() {
        let revision = Revision::R3;
        assert_eq!(candidates(revision, "user"), [(b"user".to_vec(), true)]);
        // One byte in PDFDocEncoding, three in UTF-8.
        assert_eq!(
            candidates(revision, "\u{20AC}"),
            [(vec![0xA0], true), ("\u{20AC}".as_bytes().to_vec(), false)]
        );
        // No PDFDocEncoding form: the UTF-8 bytes alone.
        assert_eq!(
            candidates(revision, "\u{0416}"),
            [("\u{0416}".as_bytes().to_vec(), false)]
        );
    }

    #[test]
    fn candidates_for_revisions_5_and_6() {
        let revision = Revision::R6;
        assert_eq!(candidates(revision, "user"), [(b"user".to_vec(), true)]);
        // SASLprep maps a no-break space to a space.
        assert_eq!(
            candidates(revision, "a\u{A0}b"),
            [
                (b"a b".to_vec(), true),
                ("a\u{A0}b".as_bytes().to_vec(), false)
            ]
        );
        // SASLprep forbids a control character.
        assert_eq!(candidates(revision, "a\u{7}"), [(b"a\x07".to_vec(), false)]);
    }
}
