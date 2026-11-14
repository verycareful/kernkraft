// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The diagnostics of one open document.
//!
//! Objects are read when they are first used, and read again after the
//! cache has dropped them; what reading one finds is the same each time
//! and is recorded the first time only. The number kept is bounded by the
//! caller; past it, diagnostics are counted and not kept.

use std::collections::HashSet;

use crate::diagnostic::{Diagnostic, Diagnostics};
use crate::syntax::ObjectId;

#[derive(Debug)]
pub(crate) struct Log {
    max: usize,
    kept: Vec<Diagnostic>,
    dropped: u64,
    /// The objects whose reading has been recorded.
    read: HashSet<ObjectId>,
    /// The streams whose data's decrypting has been recorded.
    decrypted: HashSet<ObjectId>,
    /// The streams whose data's decoding has been recorded.
    decoded: HashSet<ObjectId>,
}

impl Log {
    pub(crate) fn new(max: usize) -> Self {
        Self {
            max,
            kept: Vec::new(),
            dropped: 0,
            read: HashSet::new(),
            decrypted: HashSet::new(),
            decoded: HashSet::new(),
        }
    }

    /// Records `diagnostics`, raised by something other than the reading
    /// of one object: opening the file, a page tree walk.
    pub(crate) fn record(&mut self, diagnostics: Diagnostics) {
        for diagnostic in diagnostics.into_vec() {
            if self.kept.len() < self.max {
                self.kept.push(diagnostic);
            } else {
                self.dropped += 1;
            }
        }
    }

    /// Records what reading object `id` raised, unless its reading was
    /// recorded before.
    pub(crate) fn record_read(&mut self, id: ObjectId, diagnostics: Diagnostics) {
        if self.read.insert(id) {
            self.record(diagnostics);
        }
    }

    /// Records what decrypting the data of stream `id` raised, unless it
    /// was recorded before.
    pub(crate) fn record_decrypt(&mut self, id: ObjectId, diagnostics: Diagnostics) {
        if self.decrypted.insert(id) {
            self.record(diagnostics);
        }
    }

    /// Records what decoding the data of stream `id` raised, unless it was
    /// recorded before.
    pub(crate) fn record_decode(&mut self, id: ObjectId, diagnostics: Diagnostics) {
        if self.decoded.insert(id) {
            self.record(diagnostics);
        }
    }

    pub(crate) fn kept(&self) -> &[Diagnostic] {
        &self.kept
    }

    pub(crate) fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;
    use crate::diagnostic::DiagnosticKind;

    const ID: ObjectId = ObjectId {
        number: 3,
        generation: 0,
    };

    fn one(offset: u64) -> Diagnostics {
        let mut diagnostics = Diagnostics::new();
        diagnostics.push(DiagnosticKind::MissingEndobj, offset);
        diagnostics
    }

    #[test]
    fn a_read_is_recorded_once() {
        let mut log = Log::new(10);
        log.record_read(ID, one(5));
        log.record_read(ID, one(5));
        assert_eq!(log.kept().len(), 1);
        assert_eq!(log.dropped(), 0);
    }

    #[test]
    fn past_the_bound_diagnostics_are_counted() {
        let mut log = Log::new(2);
        for offset in 0..5 {
            log.record(one(offset));
        }
        assert_eq!(
            log.kept().iter().map(|d| d.offset).collect::<Vec<_>>(),
            [0, 1]
        );
        assert_eq!(log.dropped(), 3);
    }
}
