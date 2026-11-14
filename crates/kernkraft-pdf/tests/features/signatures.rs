// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Signatures and what their byte ranges cover (`signatures.rs`, 12.8).

use super::*;
use kernkraft_pdf::syntax::Name;
use kernkraft_pdf::{Coverage, SignatureKind};

/// Each `ByteRange` number is written this wide, so that the file can be
/// written once with zeros and the numbers filled in after.
const WIDTH: usize = 10;
/// The bytes of the `Contents` value: 64 hexadecimal digits, as a
/// signature handler reserves room for the signature before signing.
const CONTENTS: &str = "<0000000000000000000000000000000000000000000000000000000000000000>";

fn number(value: i64) -> String {
    if value < 0 {
        format!("-{:0width$}", -value, width = WIDTH - 1)
    } else {
        format!("{value:0WIDTH$}")
    }
}

/// Where the `Contents` string of the signature dictionary, object 61,
/// lies in `bytes`: from `<` to just past `>`. A page's `Contents` is a
/// reference, so the first `Contents` followed by a string is this one.
fn contents_span(bytes: &[u8]) -> (usize, usize) {
    let key = b"/Contents <";
    let at = bytes.windows(key.len()).position(|w| w == key).unwrap() + key.len() - 1;
    (at, at + CONTENTS.len())
}

/// A one-page document with signature field 60 whose value, object 61,
/// is a signature dictionary of `entries` and a `Contents` string, and
/// whose `ByteRange` is what `ranges` makes of where `Contents` starts and
/// ends and how long the file is. `ranges` gives `count` numbers.
pub fn signed(
    catalog: &str,
    entries: &str,
    count: usize,
    ranges: impl Fn(i64, i64, i64) -> Vec<i64>,
    extra: &[(u32, &[u8])],
) -> Fixture {
    let placeholder = vec![number(0); count].join(" ");
    let signature = format!("<< {entries} /Contents {CONTENTS} /ByteRange [{placeholder}] >>");
    let mut all: Vec<(u32, Vec<u8>)> = vec![
        (
            60,
            b"<< /FT /Sig /T (sig1) /V 61 0 R /Type /Annot /Subtype /Widget /Rect [0 0 0 0] /P 10 0 R >>"
                .to_vec(),
        ),
        (61, signature.into_bytes()),
    ];
    for (n, body) in extra {
        all.push((*n, body.to_vec()));
    }
    let catalog = format!("/AcroForm << /Fields [60 0 R] /SigFlags 3 >> {catalog}");
    let mut fixture = document_with(1, &catalog, &borrowed(&all));
    let (start, end) = contents_span(&fixture.bytes);
    let len = fixture.bytes.len();
    let values = ranges(start as i64, end as i64, len as i64);
    assert_eq!(values.len(), count);
    let text = values
        .iter()
        .map(|v| number(*v))
        .collect::<Vec<_>>()
        .join(" ");
    let key = b"/ByteRange [";
    let at = fixture
        .bytes
        .windows(key.len())
        .position(|w| w == key)
        .unwrap()
        + key.len();
    fixture.bytes[at..at + text.len()].copy_from_slice(text.as_bytes());
    fixture
}

/// What makes a `ByteRange` of where `Contents` starts and ends and how
/// long the file is.
type Ranges = Box<dyn Fn(i64, i64, i64) -> Vec<i64>>;

/// The byte range a signature handler writes: everything but `Contents`.
pub fn whole(start: i64, end: i64, len: i64) -> Vec<i64> {
    vec![0, start, end, len - end]
}

pub const APPROVAL: &str = "/Type /Sig /Filter /Adobe.PPKLite /SubFilter /adbe.pkcs7.detached \
    /Name (Signer) /M (D:20260101120000Z) /Reason (approved) /Location (here) /ContactInfo (me)";

#[test]
fn a_signature_over_the_whole_file() {
    let fixture = signed("", APPROVAL, 4, whole, &[]);
    let document = fixture.open();
    let signatures = document.signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    let signature = &signatures[0];
    assert_eq!(signature.kind, SignatureKind::Approval);
    assert_eq!(signature.field, Some(id(60)));
    assert_eq!(signature.field_name.as_ref().unwrap().text, "sig1");
    assert_eq!(signature.dictionary, Some(id(61)));
    assert_eq!(signature.filter, Some(Name::new(&b"Adobe.PPKLite"[..])));
    assert_eq!(
        signature.sub_filter,
        Some(Name::new(&b"adbe.pkcs7.detached"[..]))
    );
    assert_eq!(signature.signer.as_ref().unwrap().text, "Signer");
    assert_eq!(signature.reason.as_ref().unwrap().text, "approved");
    assert_eq!(signature.location.as_ref().unwrap().text, "here");
    assert_eq!(signature.contact.as_ref().unwrap().text, "me");
    assert_eq!(
        signature.signed_at.map(|date| (date.year, date.hour)),
        Some((2026, 12))
    );
    let (start, end) = contents_span(&fixture.bytes);
    let len = fixture.bytes.len() as u64;
    assert_eq!(
        signature.byte_range,
        [(0, start as u64), (end as u64, len - end as u64)]
    );
    assert_eq!(
        signature.coverage,
        Coverage::Signed {
            end: len,
            revision: Some(0),
            changed_after: false,
        }
    );
}

/// An update appended after signing leaves the signed revision whole and
/// shows that the file changed after it was signed.
#[test]
fn an_update_after_signing() {
    let fixture = signed("", APPROVAL, 4, whole, &[]);
    let (updated, _) = append_update(
        &fixture,
        &[(3, b"<< /Title (Edited) >>")],
        "/Root 1 0 R /Info 3 0 R",
    );
    let document = updated.open();
    assert_eq!(document.revisions().unwrap().len(), 2);
    let signatures = document.signatures().unwrap();
    assert_eq!(
        signatures[0].coverage,
        Coverage::Signed {
            end: fixture.bytes.len() as u64,
            revision: Some(0),
            changed_after: true,
        }
    );
}

#[test]
fn ranges_that_leave_more_out_than_the_signature() {
    let cases: [(usize, Ranges); 4] = [
        // The gap starts a byte before `Contents`.
        (4, Box::new(|s, e, l| vec![0, s - 1, e, l - e])),
        // Two gaps.
        (6, Box::new(|s, e, l| vec![0, 10, 20, s - 20, e, l - e])),
        // Not from the first byte.
        (4, Box::new(|s, e, l| vec![1, s - 1, e, l - e])),
        // No gap at all: the signature covers its own value.
        (2, Box::new(|_, _, l| vec![0, l])),
    ];
    for (index, (count, ranges)) in cases.into_iter().enumerate() {
        let document = signed("", APPROVAL, count, ranges, &[]).open();
        assert_eq!(
            document.signatures().unwrap()[0].coverage,
            Coverage::Gaps,
            "case {index}"
        );
    }
}

#[test]
fn ranges_that_are_not_ranges_of_the_file() {
    let cases: [(usize, Ranges); 3] = [
        // Past the end of the file.
        (4, Box::new(|s, e, l| vec![0, s, e, l - e + 1])),
        // An odd count.
        (3, Box::new(|s, e, _| vec![0, s, e])),
        // A negative length.
        (4, Box::new(|s, e, _| vec![0, s, e, -1])),
    ];
    for (index, (count, ranges)) in cases.into_iter().enumerate() {
        let document = signed("", APPROVAL, count, ranges, &[]).open();
        assert_eq!(
            document.signatures().unwrap()[0].coverage,
            Coverage::Invalid,
            "case {index}"
        );
    }
}

#[test]
fn certification_and_timestamp_signatures() {
    let certified = signed(
        "",
        "/Type /Sig /Filter /Adobe.PPKLite /Reference [<< /Type /SigRef /TransformMethod /DocMDP \
         /TransformParams << /Type /TransformParams /P 1 /V /1.2 >> >>]",
        4,
        whole,
        &[],
    );
    assert_eq!(
        certified.open().signatures().unwrap()[0].kind,
        SignatureKind::Certification { permissions: 1 }
    );
    // Table 254: `P` is 2 when absent.
    let default = signed(
        "",
        "/Filter /Adobe.PPKLite /Reference [<< /TransformMethod /DocMDP >>]",
        4,
        whole,
        &[],
    );
    assert_eq!(
        default.open().signatures().unwrap()[0].kind,
        SignatureKind::Certification { permissions: 2 }
    );
    let timestamp = signed(
        "",
        "/Type /DocTimeStamp /Filter /Adobe.PPKLite /SubFilter /ETSI.RFC3161",
        4,
        whole,
        &[],
    );
    assert_eq!(
        timestamp.open().signatures().unwrap()[0].kind,
        SignatureKind::DocumentTimestamp
    );
}

/// The usage rights signature of the permissions dictionary (Table 258).
#[test]
fn a_usage_rights_signature() {
    let fixture = signed(
        "/Perms << /UR3 62 0 R >>",
        APPROVAL,
        4,
        whole,
        &[(
            62,
            b"<< /Type /Sig /Filter /Adobe.PPKLite /Contents <00> /ByteRange [0 1 2 3] >>",
        )],
    );
    let signatures = fixture.open().signatures().unwrap();
    assert_eq!(signatures.len(), 2);
    assert_eq!(signatures[1].kind, SignatureKind::UsageRights);
    assert_eq!(signatures[1].field, None);
    assert_eq!(signatures[1].dictionary, Some(id(62)));
    assert_eq!(signatures[1].coverage, Coverage::Gaps);
}

/// A field's name is its own partial name after its ancestors' (12.7.3.2),
/// and `FT` is inherited.
#[test]
fn a_signature_field_below_another_field() {
    let mut objects = pages_document(1);
    objects[0].1 = b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [63 0 R] >> >>".to_vec();
    objects.push((
        63,
        b"<< /T (form) /FT /Sig /Kids [64 0 R 65 0 R] >>".to_vec(),
    ));
    objects.push((64, b"<< /T (sig) /Parent 63 0 R /V 66 0 R >>".to_vec()));
    // A field with no value of its own is not signed.
    objects.push((65, b"<< /T (empty) /Parent 63 0 R >>".to_vec()));
    objects.push((
        66,
        b"<< /Type /Sig /Filter /Adobe.PPKLite /Contents <00> /ByteRange [0 1 2 3] >>".to_vec(),
    ));
    let document = table_file(&borrowed(&objects), "/Root 1 0 R").open();
    let signatures = document.signatures().unwrap();
    assert_eq!(signatures.len(), 1);
    assert_eq!(signatures[0].field, Some(id(64)));
    assert_eq!(signatures[0].field_name.as_ref().unwrap().text, "form.sig");
}

/// A signature dictionary in an object stream has no bytes in the file
/// that its range could leave out.
#[test]
fn a_signature_in_an_object_stream() {
    let fixture = stream_file(
        &[],
        &[
            (
                1,
                b"<< /Type /Catalog /Pages 2 0 R /AcroForm << /Fields [60 0 R] >> >>",
            ),
            (2, b"<< /Type /Pages /Kids [] /Count 0 >>"),
            (60, b"<< /FT /Sig /T (s) /V 61 0 R >>"),
            (
                61,
                b"<< /Type /Sig /Contents <00> /ByteRange [0 10 20 30] >>",
            ),
        ],
        "/Root 1 0 R",
    );
    let signatures = fixture.open().signatures().unwrap();
    assert_eq!(signatures[0].coverage, Coverage::Gaps);
}

#[test]
fn a_document_without_a_form_has_no_signatures() {
    assert!(document_with(1, "", &[])
        .open()
        .signatures()
        .unwrap()
        .is_empty());
}
