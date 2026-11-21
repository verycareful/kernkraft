// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

#![allow(clippy::unwrap_used, clippy::expect_used)]

//! Error names as the application receives them.

use kernkraft_font::FontError;
use kernkraft_pdf::{EditSession, PdfError};
use kernkraft_pdf_engine::error::{kind_of, variant_name, WireError};

/// A session is held in a registry shared by every thread the application calls from,
/// so it has to be `Send`. This fails to compile when it is not.
const _: fn() = || {
    fn assert_send<T: Send>() {}
    assert_send::<EditSession>();
};

#[test]
fn a_pdf_error_is_named_by_its_variant() {
    assert_eq!(kind_of(&PdfError::Cancelled), "Cancelled");
    assert_eq!(
        kind_of(&PdfError::PageOutOfRange { page: 9, count: 3 }),
        "PageOutOfRange"
    );
    assert_eq!(kind_of(&PdfError::DuplicatePage(2)), "DuplicatePage");
    assert_eq!(kind_of(&PdfError::BadRange("x".into())), "BadRange");
}

#[test]
fn a_forwarding_variant_reports_the_inner_name_and_the_outer_message() {
    let font = PdfError::Font(FontError::NoVariationAxes);
    let wire = WireError::from(&font);

    assert_eq!(wire.kind, "NoVariationAxes");
    assert_eq!(wire.message, font.to_string());
}

#[test]
fn variant_name_stops_at_the_field_list() {
    assert_eq!(variant_name(&FontError::NoVariationAxes), "NoVariationAxes");
    assert_eq!(
        variant_name(&PdfError::PageOutOfRange { page: 1, count: 1 }),
        "PageOutOfRange"
    );
    assert_eq!(variant_name(&PdfError::DuplicatePage(1)), "DuplicatePage");
}
