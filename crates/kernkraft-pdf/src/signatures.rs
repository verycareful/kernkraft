// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Signatures and what their byte ranges cover: ISO 32000-1:2008, 12.8.1
//! (Table 252), 12.8.2.2 (DocMDP, Tables 253 and 254), 12.8.4 (the
//! permissions dictionary, Table 258), 12.7.3 and 12.7.4.5 (signature
//! fields); the document timestamp of ISO 32000-2:2020, 12.8.5.
//!
//! No certificate is read and no digest is computed: a signature is
//! reported with what its dictionary states and with where its byte range
//! ends, so that the application can tell whether the file has changed
//! since it was signed, and whether an edit would fall after what was
//! signed (an incremental update) or break it (a compacted copy).
//!
//! The signatures are the values of the signature fields of the
//! interactive form, found by walking its `Fields` and their `Kids`, and
//! the usage rights signature of the permissions dictionary.

use std::collections::HashSet;

use crate::doc::{Date, Document, Origin, Resolved, TextString};
use crate::file::Location;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Span, Value};
use crate::PdfError;

const ACRO_FORM_KEY: &[u8] = b"AcroForm";
const FIELDS_KEY: &[u8] = b"Fields";
const KIDS_KEY: &[u8] = b"Kids";
const FT_KEY: &[u8] = b"FT";
const SIG_FIELD: &[u8] = b"Sig";
const T_KEY: &[u8] = b"T";
const V_KEY: &[u8] = b"V";
const TYPE_KEY: &[u8] = b"Type";
const DOC_TIME_STAMP: &[u8] = b"DocTimeStamp";
const FILTER_KEY: &[u8] = b"Filter";
const SUB_FILTER_KEY: &[u8] = b"SubFilter";
const NAME_KEY: &[u8] = b"Name";
const M_KEY: &[u8] = b"M";
const REASON_KEY: &[u8] = b"Reason";
const LOCATION_KEY: &[u8] = b"Location";
const CONTACT_INFO_KEY: &[u8] = b"ContactInfo";
const BYTE_RANGE_KEY: &[u8] = b"ByteRange";
const CONTENTS_KEY: &[u8] = b"Contents";
const REFERENCE_KEY: &[u8] = b"Reference";
const TRANSFORM_METHOD_KEY: &[u8] = b"TransformMethod";
const TRANSFORM_PARAMS_KEY: &[u8] = b"TransformParams";
const DOC_MDP: &[u8] = b"DocMDP";
const P_KEY: &[u8] = b"P";
const PERMS_KEY: &[u8] = b"Perms";
const UR3_KEY: &[u8] = b"UR3";
/// Table 254: the permissions a DocMDP signature grants when `P` is
/// absent.
const DEFAULT_DOC_MDP: u8 = 2;
/// The field-name separator of 12.7.3.2.
const NAME_SEPARATOR: &str = ".";

/// What a signature is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignatureKind {
    /// An approval signature in a signature field (12.8.1).
    Approval,
    /// A certification signature: one with a DocMDP transform (12.8.2.2).
    /// `permissions` is its `P`: 1 allows no change, 2 form filling and
    /// signing, 3 also annotations.
    Certification { permissions: u8 },
    /// A document timestamp (ISO 32000-2:2020, 12.8.5).
    DocumentTimestamp,
    /// A usage rights signature of the permissions dictionary (12.8.2.3).
    UsageRights,
}

/// What a signature's byte range covers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// `ByteRange` is not pairs of integers that lie inside the file.
    Invalid,
    /// The ranges do not run from the first byte of the file to their end
    /// leaving out the signature's own `Contents` string and nothing else:
    /// the signature does not vouch for every byte before its end.
    Gaps,
    /// The ranges cover every byte from the first to `end` but the
    /// signature's `Contents` string.
    Signed {
        end: u64,
        /// The revision whose length is `end`: the file as it was signed.
        revision: Option<usize>,
        /// Whether the file has bytes after `end`: it was changed, or had
        /// bytes appended, after it was signed.
        changed_after: bool,
    },
}

/// One signature.
#[derive(Debug, Clone, PartialEq)]
pub struct Signature {
    pub kind: SignatureKind,
    /// The signature field, for a signature in one.
    pub field: Option<ObjectId>,
    /// The field's fully qualified name (12.7.3.2).
    pub field_name: Option<TextString>,
    /// The signature dictionary, when it is an indirect object.
    pub dictionary: Option<ObjectId>,
    /// `Filter`: the signature handler.
    pub filter: Option<Name>,
    /// `SubFilter`: the encoding of the signature value.
    pub sub_filter: Option<Name>,
    /// `Name`, `M`, `Reason`, `Location` and `ContactInfo`, as the
    /// signer stated them; none of them is verified.
    pub signer: Option<TextString>,
    pub signed_at: Option<Date>,
    pub reason: Option<TextString>,
    pub location: Option<TextString>,
    pub contact: Option<TextString>,
    /// `ByteRange` as offset and length pairs.
    pub byte_range: Vec<(u64, u64)>,
    pub coverage: Coverage,
}

/// Where a signature dictionary was found.
struct Found<'a> {
    kind_hint: Option<SignatureKind>,
    field: Option<ObjectId>,
    field_name: Option<TextString>,
    value: &'a Object,
    /// The indirect object the value lies in, whose bytes the spans
    /// inside it are offsets into when it was read from the file.
    holder: Option<ObjectId>,
}

impl Document {
    /// The document's signatures: those of its signature fields in the
    /// order of the form's fields, then its usage rights signature.
    pub fn signatures(&self) -> Result<Vec<Signature>, PdfError> {
        let mut out = Vec::new();
        let catalog = self.catalog()?;
        let Some(catalog_dictionary) = catalog.dictionary() else {
            return Ok(out);
        };
        let lengths: Vec<u64> = self
            .revisions()?
            .iter()
            .map(|revision| revision.length.unwrap_or(u64::MAX))
            .collect();
        for (field, name) in self.signature_fields(catalog_dictionary)? {
            let field_object = self.required(field)?;
            let Some(dictionary) = field_object.dictionary() else {
                continue;
            };
            let Some(value) = dictionary.get(V_KEY) else {
                continue;
            };
            let found = Found {
                kind_hint: None,
                field: Some(field),
                field_name: name,
                value,
                holder: Some(field),
            };
            if let Some(signature) = self.signature(&found, &lengths)? {
                out.push(signature);
            }
        }
        let perms = self.get(catalog_dictionary, PERMS_KEY)?;
        if let Some(perms_dictionary) = perms.dictionary() {
            if let Some(value) = perms_dictionary.get(UR3_KEY) {
                let found = Found {
                    kind_hint: Some(SignatureKind::UsageRights),
                    field: None,
                    field_name: None,
                    value,
                    holder: perms_holder(&perms, catalog.id),
                };
                if let Some(signature) = self.signature(&found, &lengths)? {
                    out.push(signature);
                }
            }
        }
        Ok(out)
    }

    /// The signature fields with a `V` of their own, in the order of
    /// `Fields` and `Kids`: each field's object and fully qualified name.
    fn signature_fields(
        &self,
        catalog: &Dictionary,
    ) -> Result<Vec<(ObjectId, Option<TextString>)>, PdfError> {
        let form = self.get(catalog, ACRO_FORM_KEY)?;
        let Some(form) = form.dictionary() else {
            return Ok(Vec::new());
        };
        let fields = self.get(form, FIELDS_KEY)?;
        let mut out = Vec::new();
        let mut seen = HashSet::new();
        // A field, its inherited `FT`, and the names of its ancestors.
        let mut pending: Vec<(ObjectId, Option<Name>, Vec<String>)> = fields
            .array()
            .unwrap_or_default()
            .iter()
            .rev()
            .filter_map(|field| match field.value() {
                Value::Reference(id) => Some((*id, None, Vec::new())),
                _ => None,
            })
            .collect();
        while let Some((id, inherited, mut names)) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            let Some(object) = self.object(id)? else {
                continue;
            };
            let Some(dictionary) = object.object().and(object.dictionary()) else {
                continue;
            };
            let field_type = match self.get(dictionary, FT_KEY)?.value() {
                Value::Name(name) => Some(name.clone()),
                _ => inherited,
            };
            if let Some(partial) = self.text(dictionary, T_KEY)? {
                names.push(partial.text);
            }
            let is_signature = field_type
                .as_ref()
                .is_some_and(|name| name.as_bytes() == SIG_FIELD);
            if is_signature && dictionary.get(V_KEY).is_some() {
                let name = (!names.is_empty()).then(|| TextString {
                    text: names.join(NAME_SEPARATOR),
                    language: None,
                    exact: true,
                });
                out.push((id, name));
            }
            let kids = self.get(dictionary, KIDS_KEY)?;
            for kid in kids.array().unwrap_or_default().iter().rev() {
                if let Value::Reference(kid) = kid.value() {
                    pending.push((*kid, field_type.clone(), names.clone()));
                }
            }
        }
        Ok(out)
    }

    fn signature(&self, found: &Found<'_>, lengths: &[u64]) -> Result<Option<Signature>, PdfError> {
        let resolved = self.resolve(found.value)?;
        let Some(dictionary) = resolved.dictionary() else {
            return Ok(None);
        };
        // The bytes the spans in `dictionary` count from: the object it is
        // when indirect, else the one holding it, when read from the file.
        let holder = resolved.id().or(found.holder);
        let in_file = match holder.map(|id| self.object(id)).transpose()?.flatten() {
            Some(object) => matches!(
                object.origin,
                Origin::File {
                    location: Location::File(_),
                    ..
                }
            ),
            None => false,
        };
        let name_of = |key| -> Result<Option<Name>, PdfError> {
            Ok(match self.get(dictionary, key)?.value() {
                Value::Name(name) => Some(name.clone()),
                _ => None,
            })
        };
        let kind = match found.kind_hint {
            Some(kind) => kind,
            None if is_type(dictionary, DOC_TIME_STAMP) => SignatureKind::DocumentTimestamp,
            None => match self.doc_mdp(dictionary)? {
                Some(permissions) => SignatureKind::Certification { permissions },
                None => SignatureKind::Approval,
            },
        };
        let signed_at = match self.text(dictionary, M_KEY)? {
            Some(date) => Date::parse(&date.text),
            None => None,
        };
        let (byte_range, coverage) = self.coverage(dictionary, in_file, lengths)?;
        Ok(Some(Signature {
            kind,
            field: found.field,
            field_name: found.field_name.clone(),
            dictionary: resolved.id(),
            filter: name_of(FILTER_KEY)?,
            sub_filter: name_of(SUB_FILTER_KEY)?,
            signer: self.text(dictionary, NAME_KEY)?,
            signed_at,
            reason: self.text(dictionary, REASON_KEY)?,
            location: self.text(dictionary, LOCATION_KEY)?,
            contact: self.text(dictionary, CONTACT_INFO_KEY)?,
            byte_range,
            coverage,
        }))
    }

    /// The `P` of the DocMDP transform among the signature's references,
    /// or `None` when it has none.
    fn doc_mdp(&self, dictionary: &Dictionary) -> Result<Option<u8>, PdfError> {
        let references = self.get(dictionary, REFERENCE_KEY)?;
        for reference in references.array().unwrap_or_default() {
            let reference = self.resolve(reference)?;
            let Some(reference) = reference.dictionary() else {
                continue;
            };
            let is_doc_mdp = matches!(
                self.get(reference, TRANSFORM_METHOD_KEY)?.value(),
                Value::Name(method) if method.as_bytes() == DOC_MDP
            );
            if !is_doc_mdp {
                continue;
            }
            let params = self.get(reference, TRANSFORM_PARAMS_KEY)?;
            let permissions = match params.dictionary() {
                Some(params) => match self.get(params, P_KEY)?.value() {
                    Value::Integer(p @ 1..=3) => *p as u8,
                    _ => DEFAULT_DOC_MDP,
                },
                None => DEFAULT_DOC_MDP,
            };
            return Ok(Some(permissions));
        }
        Ok(None)
    }

    /// `ByteRange` and what it covers, against the file's size and the
    /// lengths of its revisions (`u64::MAX` for one not known).
    fn coverage(
        &self,
        dictionary: &Dictionary,
        in_file: bool,
        lengths: &[u64],
    ) -> Result<(Vec<(u64, u64)>, Coverage), PdfError> {
        let size = self.file().source().len();
        let items = match self.get(dictionary, BYTE_RANGE_KEY)? {
            Resolved::Null => return Ok((Vec::new(), Coverage::Invalid)),
            resolved => resolved.array().map(<[Object]>::to_vec).unwrap_or_default(),
        };
        let mut ranges = Vec::with_capacity(items.len() / 2);
        let pairs = items.as_chunks::<2>();
        if !pairs.1.is_empty() || pairs.0.is_empty() {
            return Ok((ranges, Coverage::Invalid));
        }
        for [offset, length] in pairs.0 {
            let number = |object: &Object| -> Result<Option<u64>, PdfError> {
                Ok(match self.resolve(object)?.value() {
                    Value::Integer(value) => u64::try_from(*value).ok(),
                    _ => None,
                })
            };
            let (Some(offset), Some(length)) = (number(offset)?, number(length)?) else {
                return Ok((ranges, Coverage::Invalid));
            };
            match offset.checked_add(length) {
                Some(end) if end <= size => ranges.push((offset, length)),
                _ => return Ok((ranges, Coverage::Invalid)),
            }
        }
        let contents = match dictionary.get(CONTENTS_KEY) {
            Some(contents) if in_file => contents.span(),
            _ => None,
        };
        let mut sorted = ranges.clone();
        sorted.sort_unstable();
        let mut covered = 0u64;
        let mut gaps_ok = true;
        for (offset, length) in &sorted {
            if *offset > covered {
                let gap = Span {
                    start: covered,
                    end: *offset,
                };
                gaps_ok &= Some(gap) == contents;
            } else if *offset < covered {
                // Ranges that overlap say nothing a reader can trust.
                gaps_ok = false;
            }
            covered = covered.max(offset + length);
        }
        // Exactly the one gap, the `Contents` string.
        let gap_count = sorted
            .windows(2)
            .filter(|pair| pair[1].0 > pair[0].0 + pair[0].1)
            .count();
        let starts_at_zero = sorted.first().is_some_and(|(offset, _)| *offset == 0);
        if !gaps_ok || gap_count != 1 || !starts_at_zero {
            return Ok((ranges, Coverage::Gaps));
        }
        let revision = lengths.iter().position(|length| *length == covered);
        Ok((
            ranges,
            Coverage::Signed {
                end: covered,
                revision,
                changed_after: covered < size,
            },
        ))
    }
}

/// The indirect object the permissions dictionary is, or the catalog when
/// it is direct there.
fn perms_holder(perms: &Resolved<'_>, catalog: ObjectId) -> Option<ObjectId> {
    perms.id().or(Some(catalog))
}

fn is_type(dictionary: &Dictionary, expected: &[u8]) -> bool {
    matches!(
        dictionary.get(TYPE_KEY).map(Object::value),
        Some(Value::Name(name)) if name.as_bytes() == expected
    )
}
