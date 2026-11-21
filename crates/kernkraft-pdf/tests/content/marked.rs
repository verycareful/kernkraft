// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Marked content: ISO 32000-1, 14.6 (Table 352) and 14.7.4.4 (`MCID`).

use kernkraft_pdf::DiagnosticKind;

use super::{has, name, run, Event, FONTS, FONT_OBJECTS};

#[test]
fn bmc_and_emc_bracket_a_sequence() {
    let (document, events) = run("/Span BMC 0 0 m 1 1 l S EMC", "", &[]);
    let Event::BeginMarked { marked, visible } = &events[0] else {
        panic!()
    };
    assert_eq!(marked.len(), 1);
    assert_eq!(marked[0].tag, name("Span"));
    assert!(marked[0].properties.is_none());
    assert!(*visible);
    let Event::EndMarked(closed) = events.last().unwrap() else {
        panic!()
    };
    assert_eq!(closed.tag, name("Span"));
    assert!(document.diagnostics().is_empty());
}

#[test]
fn bdc_with_a_dictionary_in_place_gives_the_mcid() {
    let (_, events) = run("/P <</MCID 7>> BDC EMC", "", &[]);
    let Event::BeginMarked { marked, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(marked[0].mcid(), Some(7));
}

#[test]
fn bdc_with_a_properties_name_resolves_the_resource() {
    let resources = "/Properties << /MC0 40 0 R >>";
    let extra: [(u32, &[u8]); 1] = [(40, b"<< /MCID 3 /Lang (en) >>")];
    let (_, events) = run("/P /MC0 BDC EMC", resources, &extra);
    let Event::BeginMarked { marked, .. } = &events[0] else {
        panic!()
    };
    assert_eq!(marked[0].mcid(), Some(3));
    assert_eq!(marked[0].properties_name, Some(name("MC0")));
    assert_eq!(marked[0].properties_id.map(|id| id.number), Some(40));
}

#[test]
fn nested_sequences_are_all_in_the_context() {
    let (_, events) = run(
        "/A BMC /B <</MCID 1>> BDC BT /F1 10 Tf (A) Tj ET EMC EMC",
        FONTS,
        &FONT_OBJECTS,
    );
    let Event::Text { marked, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Text { .. }))
        .unwrap()
    else {
        panic!()
    };
    let tags: Vec<_> = marked.iter().map(|m| m.tag.clone()).collect();
    assert_eq!(tags, [name("A"), name("B")]);
}

#[test]
fn emc_closes_the_innermost_sequence() {
    let (_, events) = run("/A BMC /B BMC EMC EMC", "", &[]);
    let closed: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::EndMarked(m) => Some(m.tag.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(closed, [name("B"), name("A")]);
}

#[test]
fn an_emc_with_nothing_open_is_reported_and_ignored() {
    let (document, events) = run("EMC", "", &[]);
    assert!(events.is_empty());
    assert!(has(&document, |k| *k == DiagnosticKind::MarkedContentUnbalanced));
}

#[test]
fn a_sequence_left_open_at_the_end_is_reported() {
    let (document, _) = run("/A BMC", "", &[]);
    assert!(has(&document, |k| *k == DiagnosticKind::MarkedContentUnbalanced));
}

#[test]
fn mp_and_dp_are_points() {
    let (_, events) = run("/Here MP /There <</K 1>> DP", "", &[]);
    let tags: Vec<_> = events
        .iter()
        .filter_map(|e| match e {
            Event::Point { tag } => Some(tag.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(tags, [name("Here"), name("There")]);
}

#[test]
fn a_missing_properties_resource_is_reported_and_no_sequence_opens() {
    let (document, events) = run("/P /Nope BDC EMC", "", &[]);
    assert!(!events
        .iter()
        .any(|e| matches!(e, Event::BeginMarked { .. })));
    assert!(has(
        &document,
        |k| matches!(k, DiagnosticKind::ResourceMissing { category, .. } if category.as_bytes() == b"Properties")
    ));
}

#[test]
fn bdc_with_one_operand_is_invalid() {
    let (document, _) = run("/P BDC", "", &[]);
    assert!(has(&document, |k| matches!(
        k,
        DiagnosticKind::OperandsInvalid { .. }
    )));
}
