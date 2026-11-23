// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The profile DeviceCMYK is converted through: ISO 32000-1:2008, 14.11.5
//! (output intents) and 8.6.5.5 (ICCBased), with the profile format of
//! ICC.1:2010.
//!
//! The format leaves DeviceCMYK device dependent (8.6.4.4), so no
//! conversion to sRGB is the format's own: 10.3.5's `1 - min(1, c + k)`
//! gives pure cyan as (0, 255, 255), far from any printed cyan. A document
//! takes its CMYK profile from, in order:
//!
//! 1. its output intents (14.11.5): the first whose `DestOutputProfile` is
//!    a CMYK profile that can be used. An intent whose profile is not CMYK
//!    (a PDF/A file's sRGB intent) is not a CMYK intent and is passed over;
//!    a CMYK profile that cannot be used, and an intent that is damaged
//!    (a missing object, a profile that is not a stream), are reported
//!    ([`DiagnosticKind::IccProfileUnusable`]) and the next intent is
//!    tried;
//! 2. the profile the application gave ([`Document::set_cmyk_profile`]);
//! 3. the profile bundled with the library ([`BUNDLED_PROFILE`]).
//!
//! A page's `DefaultCMYK` (8.6.5.6) is applied before any of these: it
//! replaces the device space, so it is not a choice among profiles.
//!
//! The intent is read once for a snapshot of the document; the chosen
//! profile is a [`CmykProfile`], which a [`ColourSpace::DeviceCmyk`](super::ColourSpace::DeviceCmyk)
//! holds.

use std::sync::{Arc, LazyLock};

use super::icc::{IccTransform, MAX_PROFILE_BYTES};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{lock, Document};
use crate::syntax::{Object, Value};
use crate::PdfError;

/// The CMYK profile the library bundles: FOGRA39L_coated.icc of the
/// colorprofiles package, unmodified. Its licence travels with it
/// (`assets/FOGRA39L_coated.NOTICE`).
pub const BUNDLED_PROFILE: &[u8] = include_bytes!("../../assets/FOGRA39L_coated.icc");

/// The component count of a CMYK colour.
const CMYK_COMPONENTS: usize = 4;
/// ICC.1:2010, 7.2.6: the data colour space signature is at bytes 16 to 19
/// of the header.
const SIGNATURE_OFFSET: usize = 16;
/// The data colour space signature of a CMYK profile (ICC.1:2010, 7.2.6,
/// Table 19).
const CMYK_SIGNATURE: &[u8; 4] = b"CMYK";
/// Table 29 of ISO 32000-1: the output intent's profile entry.
const DEST_OUTPUT_PROFILE_KEY: &[u8] = b"DestOutputProfile";
/// 14.11.5: the catalog's array of output intent dictionaries.
const OUTPUT_INTENTS_KEY: &[u8] = b"OutputIntents";

/// What reading the `DestOutputProfile` stream of an output intent came to,
/// kept for the file so that a snapshot made by an edit does not decode the
/// profile and build its transform again.
#[derive(Debug, Clone)]
pub(crate) enum IntentOutcome {
    /// Not a CMYK profile, or not a profile at all.
    NotCmyk,
    /// A CMYK profile that cannot be used (reported when first read).
    Unusable,
    Cmyk(CmykProfile),
}

impl IntentOutcome {
    fn profile(&self) -> Option<CmykProfile> {
        match self {
            Self::Cmyk(profile) => Some(profile.clone()),
            Self::NotCmyk | Self::Unusable => None,
        }
    }
}

/// Where a document's CMYK profile came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmykSource {
    /// An output intent of the file.
    OutputIntent,
    /// The application ([`Document::set_cmyk_profile`]).
    Application,
    /// The profile bundled with the library ([`BUNDLED_PROFILE`]).
    Bundled,
}

/// A CMYK profile's transform to sRGB, and where the profile came from.
/// Cloning it is cheap.
#[derive(Debug, Clone)]
pub struct CmykProfile {
    transform: Arc<IccTransform>,
    source: CmykSource,
}

impl CmykProfile {
    /// The profile `bytes`, taken to come from `source`. `None` when it is
    /// not a CMYK profile or no transform to sRGB can be built from it.
    pub(crate) fn new(bytes: &[u8], source: CmykSource) -> Option<Self> {
        let range = vec![[0.0, 1.0]; CMYK_COMPONENTS];
        Some(Self {
            transform: Arc::new(IccTransform::new(bytes, CMYK_COMPONENTS, range)?),
            source,
        })
    }

    /// The profile bundled with the library.
    pub fn bundled() -> Result<Self, PdfError> {
        static BUNDLED: LazyLock<Option<CmykProfile>> =
            LazyLock::new(|| CmykProfile::new(BUNDLED_PROFILE, CmykSource::Bundled));
        BUNDLED
            .clone()
            .ok_or_else(|| PdfError::Invalid("the bundled CMYK profile cannot be used".into()))
    }

    /// Where the profile came from.
    pub fn source(&self) -> CmykSource {
        self.source
    }

    /// The sRGB of each CMYK colour of `colours` (four numbers a colour, 0
    /// to 1), appended to `out`. `None` when the transform fails, which it
    /// does not for whole colours.
    pub(crate) fn convert(&self, colours: &[f64], out: &mut Vec<[f64; 3]>) -> Option<()> {
        self.transform.convert(colours, out)
    }

    /// The sRGB of the one CMYK colour `colour`, with no allocation.
    pub(crate) fn convert_one(&self, colour: &[f64]) -> Option<[f64; 3]> {
        self.transform.convert_one(colour)
    }
}

/// Whether `profile` states a CMYK data colour space (ICC.1:2010, 7.2.6).
fn is_cmyk(profile: &[u8]) -> bool {
    profile
        .get(SIGNATURE_OFFSET..SIGNATURE_OFFSET + CMYK_SIGNATURE.len())
        .is_some_and(|signature| signature == CMYK_SIGNATURE)
}

impl Document {
    /// The profile DeviceCMYK is converted through in this document: the
    /// file's output intent, else the application's profile, else the
    /// bundled one (see the module). Fails only when the bundled profile
    /// cannot be used, or the file cannot be read.
    pub fn cmyk_profile(&self) -> Result<CmykProfile, PdfError> {
        if let Some(intent) = self.output_intent_profile()? {
            return Ok(intent);
        }
        if let Some(profile) = lock(self.application_cmyk()).clone() {
            return Ok(profile);
        }
        CmykProfile::bundled()
    }

    /// Makes `profile`, an ICC profile of a CMYK colour space, the one
    /// DeviceCMYK is converted through where the file's output intents
    /// give none. It belongs to the opened file, so every snapshot of the
    /// document uses it. A profile that is not a CMYK profile, or that
    /// cannot be used, is [`PdfError::Invalid`] and changes nothing.
    pub fn set_cmyk_profile(&self, profile: &[u8]) -> Result<(), PdfError> {
        if profile.len() > MAX_PROFILE_BYTES {
            return Err(PdfError::Invalid(
                "the CMYK profile is larger than a profile may be".into(),
            ));
        }
        if !is_cmyk(profile) {
            return Err(PdfError::Invalid(
                "the profile is not for a CMYK colour space".into(),
            ));
        }
        let profile = CmykProfile::new(profile, CmykSource::Application)
            .ok_or_else(|| PdfError::Invalid("the CMYK profile cannot be used".into()))?;
        *lock(self.application_cmyk()) = Some(profile);
        Ok(())
    }

    /// The first usable CMYK profile of the output intents, read once for
    /// this snapshot.
    fn output_intent_profile(&self) -> Result<Option<CmykProfile>, PdfError> {
        let mut cached = lock(self.output_intent_cache());
        if let Some(read) = cached.as_ref() {
            return Ok(read.clone());
        }
        let mut diagnostics = Diagnostics::new();
        let read = self.read_output_intents(&mut diagnostics);
        self.record_diagnostics(diagnostics);
        let read = read?;
        *cached = Some(read.clone());
        Ok(read)
    }

    /// Reads the output intents. A damaged intent is reported
    /// ([`DiagnosticKind::IccProfileUnusable`]) and passed over, whatever
    /// the damage: DeviceCMYK is converted all over a document, and an
    /// optional hint must not stop it. Only a failure of the file system
    /// is an error.
    fn read_output_intents(
        &self,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<CmykProfile>, PdfError> {
        let intents = match self.output_intent_list() {
            Ok(intents) => intents,
            Err(PdfError::Fs(error)) => return Err(PdfError::Fs(error)),
            Err(_) => {
                diagnostics.set_object(None);
                diagnostics.push(DiagnosticKind::IccProfileUnusable, 0);
                return Ok(None);
            }
        };
        for intent in &intents {
            diagnostics.set_object(match intent.value() {
                Value::Reference(id) => Some(*id),
                _ => None,
            });
            match self.intent_profile(intent, diagnostics) {
                Ok(Some(profile)) => return Ok(Some(profile)),
                Ok(None) => {}
                Err(PdfError::Fs(error)) => return Err(PdfError::Fs(error)),
                Err(_) => diagnostics.push(DiagnosticKind::IccProfileUnusable, 0),
            }
        }
        Ok(None)
    }

    /// The catalog's `OutputIntents` entries. A file with no usable
    /// catalog, or none with the entry, has none; the page operations
    /// report the catalog.
    fn output_intent_list(&self) -> Result<Vec<Object>, PdfError> {
        let catalog = match self.catalog() {
            Ok(catalog) => catalog,
            Err(PdfError::InvalidEntry { .. }) => return Ok(Vec::new()),
            Err(error) => return Err(error),
        };
        let Some(catalog) = catalog.dictionary() else {
            return Ok(Vec::new());
        };
        Ok(match self.get(catalog, OUTPUT_INTENTS_KEY)?.value() {
            Value::Array(items) => items.to_vec(),
            _ => Vec::new(),
        })
    }

    /// The CMYK profile of the output intent `intent`: `None` when it has
    /// no `DestOutputProfile` (an intent that names a registered production
    /// condition, 14.11.5) or its profile is not for CMYK; an error when it
    /// is damaged; reported and `None` when the CMYK profile cannot be
    /// used.
    fn intent_profile(
        &self,
        intent: &Object,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<CmykProfile>, PdfError> {
        let Some(dictionary) = self.resolve(intent)?.dictionary().cloned() else {
            return Err(PdfError::InvalidEntry {
                object: None,
                key: "OutputIntents",
            });
        };
        let Some(entry) = dictionary.get(DEST_OUTPUT_PROFILE_KEY) else {
            return Ok(None);
        };
        let invalid = || PdfError::InvalidEntry {
            object: None,
            key: "DestOutputProfile",
        };
        let Value::Reference(id) = entry.value() else {
            return Err(invalid());
        };
        let object = self.required(*id)?;
        let stream = object.stream().ok_or_else(invalid)?;
        // An object of the file that no edit touched reads the same in
        // every snapshot, and the file's log has the report of it.
        let unedited = self.edit(id.number).is_none();
        if unedited {
            if let Some(outcome) = lock(self.intent_profile_cache()).get(id) {
                return Ok(outcome.profile());
            }
        }
        diagnostics.set_object(Some(*id));
        let outcome = match self.decode_stream(*id, stream, MAX_PROFILE_BYTES) {
            Ok(decoded) if !is_cmyk(&decoded.data) => IntentOutcome::NotCmyk,
            Ok(decoded) => match CmykProfile::new(&decoded.data, CmykSource::OutputIntent) {
                Some(profile) => IntentOutcome::Cmyk(profile),
                None => {
                    diagnostics.push(DiagnosticKind::IccProfileUnusable, 0);
                    IntentOutcome::Unusable
                }
            },
            Err(PdfError::Fs(error)) => return Err(PdfError::Fs(error)),
            Err(_) => {
                diagnostics.push(DiagnosticKind::IccProfileUnusable, 0);
                IntentOutcome::Unusable
            }
        };
        let profile = outcome.profile();
        if unedited {
            lock(self.intent_profile_cache()).insert(*id, outcome);
        }
        Ok(profile)
    }
}
