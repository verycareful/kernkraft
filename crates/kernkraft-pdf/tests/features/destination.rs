// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Destinations (`destination.rs`, 12.3.2), read through outline items.

use super::*;
use kernkraft_pdf::{Destination, DestinationPage, View};

/// A document of two pages whose outline has one item for each of
/// `dests`, its `Dest` entry.
fn outline_of(dests: &[&str], catalog: &str) -> Document {
    let mut extra: Vec<(u32, Vec<u8>)> = Vec::new();
    let first = 71;
    let last = first + dests.len() as u32 - 1;
    extra.push((70, format!("<< /First {first} 0 R >>").into_bytes()));
    for (index, dest) in dests.iter().enumerate() {
        let number = first + index as u32;
        let next = if number < last {
            format!("/Next {} 0 R", number + 1)
        } else {
            String::new()
        };
        extra.push((
            number,
            format!("<< /Title (item) {next} /Dest {dest} >>").into_bytes(),
        ));
    }
    document_with(2, &format!("/Outlines 70 0 R {catalog}"), &borrowed(&extra)).open()
}

fn destinations(document: &Document) -> Vec<Option<Destination>> {
    document
        .outline()
        .unwrap()
        .into_iter()
        .map(|item| item.destination)
        .collect()
}

#[test]
fn every_view_of_table_151() {
    let document = outline_of(
        &[
            "[10 0 R /XYZ null null 2]",
            "[10 0 R /FitV 30]",
            "[12 0 R /FitBH null]",
            "[12 0 R /FitBV 7.5]",
        ],
        "",
    );
    assert_eq!(
        destinations(&document),
        [
            Some(Destination {
                page: DestinationPage::Page(1),
                view: View::Xyz {
                    left: None,
                    top: None,
                    zoom: Some(2.0)
                }
            }),
            Some(Destination {
                page: DestinationPage::Page(1),
                view: View::FitV { left: Some(30.0) }
            }),
            Some(Destination {
                page: DestinationPage::Page(2),
                view: View::FitBH { top: None }
            }),
            Some(Destination {
                page: DestinationPage::Page(2),
                view: View::FitBV { left: Some(7.5) }
            }),
        ]
    );
    assert!(document.diagnostics().is_empty());
}

/// A reference to an object that is not a page is kept as what it names.
#[test]
fn a_page_the_document_does_not_have() {
    let document = outline_of(&["[3 0 R /Fit]"], "");
    assert_eq!(
        destinations(&document),
        [Some(Destination {
            page: DestinationPage::Missing(id(3)),
            view: View::Fit
        })]
    );
}

#[test]
fn malformed_destinations_lead_nowhere() {
    let document = outline_of(
        &[
            // XYZ takes three numbers.
            "[10 0 R /XYZ 0 792]",
            "[10 0 R /Zoom]",
            // FitR takes four, none of them null.
            "[10 0 R /FitR 0 0 null 1]",
            // A page of this document is a reference, not a number.
            "[0 /Fit]",
            "[]",
        ],
        "",
    );
    assert_eq!(destinations(&document), [None; 5]);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::InvalidDestination).len(),
        5
    );
}

#[test]
fn names_the_document_does_not_define() {
    let document = outline_of(
        &["/nowhere", "(nowhere)", "42"],
        "/Dests << /elsewhere [10 0 R /Fit] >>",
    );
    assert_eq!(destinations(&document), [None; 3]);
    assert_eq!(
        of_kind(&document, &DiagnosticKind::UnknownDestination).len(),
        3
    );
}

/// A named destination may be a dictionary whose `D` is the array, and
/// may be an indirect object.
#[test]
fn named_destinations_through_d_and_references() {
    let fixture = document_with(
        2,
        "/Outlines 70 0 R /Dests 80 0 R /Names << /Dests << /Names [(bystring) 81 0 R] >> >>",
        &[
            (70, b"<< /First 71 0 R >>"),
            (71, b"<< /Title (a) /Next 72 0 R /Dest /byname >>"),
            (72, b"<< /Title (b) /Dest (bystring) >>"),
            (80, b"<< /byname << /D [12 0 R /Fit] >> >>"),
            (81, b"[10 0 R /FitH 3]"),
        ],
    );
    assert_eq!(
        destinations(&fixture.open()),
        [
            Some(Destination {
                page: DestinationPage::Page(2),
                view: View::Fit
            }),
            Some(Destination {
                page: DestinationPage::Page(1),
                view: View::FitH { top: Some(3.0) }
            }),
        ]
    );
}
