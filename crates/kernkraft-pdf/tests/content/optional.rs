// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Optional content: ISO 32000-1, 8.11 (Tables 98, 99 and 101).

use kernkraft_pdf::doc::Document;
use kernkraft_pdf::DiagnosticKind;

use super::{
    content_fixture_with_catalog, has, paths, run_document, texts, Event, FONTS, FONT_OBJECTS,
};

/// Objects 30 (on), 31 (off by the `OFF` array) and 32, and the catalog
/// entry that sets the configuration to `config`.
fn run_oc(
    content: &str,
    config: &str,
    extra: &[(u32, &[u8])],
    resources: &str,
) -> (Document, Vec<Event>) {
    let mut objects: Vec<(u32, &[u8])> = vec![
        (30, b"<< /Type /OCG /Name (On) >>"),
        (31, b"<< /Type /OCG /Name (Off) >>"),
    ];
    objects.extend_from_slice(&FONT_OBJECTS);
    objects.extend_from_slice(extra);
    let catalog = format!("/OCProperties << /OCGs [30 0 R 31 0 R 32 0 R] /D << {config} >> >>");
    let resources =
        format!("{FONTS} /Properties << /L0 30 0 R /L1 31 0 R /M0 33 0 R >> {resources}");
    let document =
        content_fixture_with_catalog(content.as_bytes(), &resources, &objects, &catalog).open();
    let events = run_document(&document).unwrap();
    (document, events)
}

fn visible_texts(events: &[Event]) -> Vec<bool> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Text { visible, .. } => Some(*visible),
            _ => None,
        })
        .collect()
}

const SHOW: &str = "BT /F1 10 Tf (A) Tj ET";

#[test]
fn content_in_a_group_that_is_off_is_not_visible() {
    let (_, events) = run_oc(
        &format!("/OC /L1 BDC {SHOW} EMC /OC /L0 BDC {SHOW} EMC {SHOW}"),
        "/OFF [31 0 R]",
        &[],
        "",
    );
    // The hidden text is still reported, so that a text layer can see it.
    assert_eq!(visible_texts(&events), [false, true, true]);
}

#[test]
fn begin_marked_carries_the_visibility_and_the_hidden_flag() {
    let (_, events) = run_oc("/OC /L1 BDC EMC", "/OFF [31 0 R]", &[], "");
    let Event::BeginMarked { marked, visible } = &events[0] else {
        panic!()
    };
    assert!(!visible);
    assert!(marked[0].hidden);
    assert_eq!(marked[0].properties_id.map(|id| id.number), Some(31));
}

#[test]
fn hidden_content_hides_what_is_nested_in_it() {
    let (_, events) = run_oc(
        &format!("/OC /L1 BDC /Span BMC {SHOW} EMC EMC {SHOW}"),
        "/OFF [31 0 R]",
        &[],
        "",
    );
    assert_eq!(visible_texts(&events), [false, true]);
}

#[test]
fn a_base_state_of_off_needs_the_on_array() {
    let (_, events) = run_oc(
        &format!("/OC /L1 BDC {SHOW} EMC /OC /L0 BDC {SHOW} EMC"),
        "/BaseState /OFF /ON [30 0 R]",
        &[],
        "",
    );
    assert_eq!(visible_texts(&events), [false, true]);
}

#[test]
fn a_document_with_no_configuration_shows_every_group() {
    let objects: Vec<(u32, &[u8])> = vec![(30, b"<< /Type /OCG /Name (On) >>")];
    let mut all = objects;
    all.extend_from_slice(&FONT_OBJECTS);
    let resources = format!("{FONTS} /Properties << /L0 30 0 R >>");
    let document = content_fixture_with_catalog(
        format!("/OC /L0 BDC {SHOW} EMC").as_bytes(),
        &resources,
        &all,
        "",
    )
    .open();
    assert_eq!(visible_texts(&run_document(&document).unwrap()), [true]);
}

#[test]
fn a_view_state_in_a_groups_usage_applies_through_as() {
    let usage = b"<< /Type /OCG /Name (Usage) /Usage << /View << /ViewState /OFF >> >> >>";
    let (_, events) = run_oc(
        &format!("/OC /M0 BDC {SHOW} EMC"),
        "/AS [<< /Event /View /Category [/View] /OCGs [32 0 R] >>]",
        &[(32, usage), (33, b"<< /Type /OCMD /OCGs 32 0 R >>")],
        "",
    );
    // The membership dictionary M0 is over group 32, which AS turns off.
    assert_eq!(visible_texts(&events), [false]);
}

fn membership(body: &str) -> Vec<bool> {
    let object = format!("<< /Type /OCMD {body} >>");
    let (_, events) = run_oc(
        &format!("/OC /M0 BDC {SHOW} EMC"),
        "/OFF [31 0 R]",
        &[
            (32, b"<< /Type /OCG /Name (Other) >>"),
            (33, object.as_bytes()),
        ],
        "",
    );
    visible_texts(&events)
}

#[test]
fn a_membership_dictionary_follows_its_policy() {
    // 30 is on, 31 is off.
    assert_eq!(
        membership("/OCGs [30 0 R 31 0 R]"),
        [true],
        "AnyOn is the default"
    );
    assert_eq!(membership("/OCGs [30 0 R 31 0 R] /P /AnyOn"), [true]);
    assert_eq!(membership("/OCGs [30 0 R 31 0 R] /P /AllOn"), [false]);
    assert_eq!(membership("/OCGs [30 0 R 31 0 R] /P /AnyOff"), [true]);
    assert_eq!(membership("/OCGs [30 0 R 31 0 R] /P /AllOff"), [false]);
    assert_eq!(membership("/OCGs [31 0 R] /P /AllOff"), [true]);
    assert_eq!(membership("/OCGs 31 0 R"), [false], "a single group");
}

#[test]
fn a_membership_dictionary_with_no_groups_is_visible() {
    assert_eq!(membership(""), [true]);
    assert_eq!(membership("/OCGs []"), [true]);
}

#[test]
fn a_visibility_expression_overrides_the_policy() {
    assert_eq!(membership("/VE [/Not 31 0 R]"), [true]);
    assert_eq!(membership("/VE [/Not 30 0 R]"), [false]);
    assert_eq!(membership("/VE [/And 30 0 R 31 0 R]"), [false]);
    assert_eq!(membership("/VE [/Or 30 0 R 31 0 R]"), [true]);
    assert_eq!(membership("/VE [/And 30 0 R [/Not 31 0 R]]"), [true]);
    assert_eq!(
        membership("/VE [/Or [/And 30 0 R 31 0 R] [/Not 30 0 R]]"),
        [false]
    );
    // VE wins over P and OCGs.
    assert_eq!(
        membership("/OCGs [30 0 R] /P /AllOn /VE [/Not 30 0 R]"),
        [false]
    );
}

#[test]
fn a_damaged_expression_is_reported_and_shown() {
    let object = b"<< /Type /OCMD /VE [/Xor 30 0 R] >>";
    let (document, events) = run_oc(&format!("/OC /M0 BDC {SHOW} EMC"), "", &[(33, object)], "");
    assert_eq!(visible_texts(&events), [true]);
    assert!(has(&document, |k| *k == DiagnosticKind::OptionalContentInvalid));
}

#[test]
fn an_oc_properties_entry_that_is_not_a_group_is_reported_and_shown() {
    let (document, events) = run_oc(
        &format!("/OC /M0 BDC {SHOW} EMC"),
        "",
        &[(33, b"<< /Type /Font >>")],
        "",
    );
    assert_eq!(visible_texts(&events), [true]);
    assert!(has(&document, |k| *k == DiagnosticKind::OptionalContentInvalid));
}

#[test]
fn an_image_in_a_group_that_is_off_is_not_visible() {
    let image = super::stream(
        "/Type /XObject /Subtype /Image /Width 1 /Height 1 /BitsPerComponent 8 /ColorSpace /DeviceGray /OC 31 0 R",
        b"\x00",
    );
    let (_, events) = run_oc(
        "/Im1 Do",
        "/OFF [31 0 R]",
        &[(40, &image)],
        "/XObject << /Im1 40 0 R >>",
    );
    let Event::Image { visible, .. } = events
        .iter()
        .find(|e| matches!(e, Event::Image { .. }))
        .unwrap()
    else {
        panic!()
    };
    assert!(!visible);
}

#[test]
fn a_form_in_a_group_that_is_off_hides_everything_in_it() {
    let form = super::stream(
        "/Type /XObject /Subtype /Form /BBox [0 0 1 1] /OC 31 0 R",
        b"0 0 m 1 1 l S",
    );
    let (_, events) = run_oc(
        "/Fm1 Do 0 0 m 1 1 l S",
        "/OFF [31 0 R]",
        &[(40, &form)],
        "/XObject << /Fm1 40 0 R >>",
    );
    let visibility: Vec<bool> = paths(&events)
        .iter()
        .map(|e| match e {
            Event::Path { visible, .. } => *visible,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(visibility, [false, true]);
}

#[test]
fn text_helpers_agree() {
    let (_, events) = run_oc(SHOW, "", &[], "");
    assert_eq!(texts(&events).len(), 1);
}
