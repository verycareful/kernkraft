// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Deterministic mutation of the feature fixtures: every truncation and
//! every substitution of one byte by a byte that means something to the
//! reader. Each mutated file must open or be refused with an error, and
//! every feature must read it or refuse it: a panic or a hang fails the
//! test.

use super::*;
use kernkraft_fs::NoProgress;

/// Bytes that change how the reader takes what follows: delimiters, white
/// space, digits at each end of the range, the letters of `R`, `n` and
/// `f`, and the extreme values.
const REPLACEMENTS: &[u8] = b"()<>[]/% \n\x0009Rnf\xFF";

fn labelled() -> Vec<u8> {
    document_with(
        3,
        "/PageLabels << /Nums [0 << /S /r >> 2 << /S /A /P (x) /St 3 >>] >>",
        &[],
    )
    .bytes
}

fn signed_and_updated() -> Vec<u8> {
    use super::signatures::{signed, whole, APPROVAL};
    let signed = signed("", APPROVAL, 4, whole, &[]);
    let (updated, _) = append_update(
        &signed,
        &[(3, b"<< /Title (Edited) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    updated.bytes
}

fn exercise(bytes: &[u8], password: &str) {
    let Ok(document) = Document::from_bytes(bytes.to_vec(), &OPTIONS, password) else {
        return;
    };
    let _ = document.page_labels();
    let _ = document.outline();
    if let Ok(attachments) = document.attachments() {
        for attachment in &attachments {
            let _ = document.attachment_data(attachment, 1 << 16);
        }
    }
    let _ = document.active_content(&NoProgress);
    if let Ok(revisions) = document.revisions() {
        for revision in &revisions {
            let _ = document.open_revision(revision.index);
        }
    }
    let _ = document.signatures();
    let _ = document.unlock_attachments("user");
}

fn truncations(fixture: &[u8]) {
    for length in 0..fixture.len() {
        exercise(&fixture[..length], "");
    }
}

fn substitutions(fixture: &[u8]) {
    let mut bytes = fixture.to_vec();
    for offset in 0..fixture.len() {
        for &replacement in REPLACEMENTS {
            bytes[offset] = replacement;
            exercise(&bytes, "");
        }
        bytes[offset] = fixture[offset];
    }
}

/// A truncation test and a substitution test for each fixture, so that
/// the harness runs them side by side.
macro_rules! mutated {
    ($($name:ident: $fixture:expr;)*) => {
        $(
            mod $name {
                use super::*;

                #[test]
                fn every_truncation_reads_or_is_refused() {
                    truncations(&$fixture);
                }

                #[test]
                fn every_single_byte_substitution_reads_or_is_refused() {
                    substitutions(&$fixture);
                }
            }
        )*
    };
}

mutated! {
    labels: labelled();
    outline: super::super::outline::chapters().bytes;
    attachments: super::super::attachments::attached().bytes;
    active: super::super::active::active().bytes;
    signatures: signed_and_updated();
    attachments_only: encrypted_fixture("attachments-only");
}
