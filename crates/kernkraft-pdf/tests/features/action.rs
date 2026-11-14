// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Actions (`action.rs`, 12.6), read from the catalog's `OpenAction`.

use super::*;
use kernkraft_pdf::syntax::Name;
use kernkraft_pdf::{Action, ActionKind, Destination, DestinationPage, View};

/// The actions the `OpenAction` of a document whose catalog has
/// `open_action` as its value gives.
fn open_actions(open_action: &str, extra: &[(u32, &[u8])]) -> Vec<Action> {
    let document = document_with(2, &format!("/OpenAction {open_action}"), extra).open();
    let catalog = document.catalog().unwrap();
    let open = catalog.dictionary().unwrap().get(b"OpenAction").unwrap();
    document.actions(open).unwrap()
}

fn kinds(actions: &[Action]) -> Vec<ActionKind> {
    actions.iter().map(|action| action.kind.clone()).collect()
}

/// 12.6.2: each action, then what its `Next` chains, depth first; an
/// action reached again is not performed again.
#[test]
fn the_order_next_performs() {
    let actions = open_actions(
        "<< /S /Named /N /NextPage /Next [80 0 R << /S /URI /URI (http://a) /Next 81 0 R >>] >>",
        &[
            (80, b"<< /S /GoTo /D [10 0 R /Fit] >>"),
            (
                81,
                b"<< /Type /Action /S /JavaScript /JS (app.alert(1)) /Next 80 0 R >>",
            ),
        ],
    );
    assert_eq!(
        kinds(&actions),
        [
            ActionKind::Named,
            ActionKind::GoTo,
            ActionKind::Uri,
            ActionKind::JavaScript
        ]
    );
    assert_eq!(actions[0].object, None);
    assert_eq!(actions[1].object, Some(id(80)));
    assert_eq!(
        actions[1].destination,
        Some(Destination {
            page: DestinationPage::Page(1),
            view: View::Fit
        })
    );
    assert_eq!(actions[2].uri.as_ref().unwrap().text, "http://a");
    assert!(actions[3].script);
}

/// An indirect `Next` array whose actions name it again is followed once.
#[test]
fn a_next_array_that_names_itself() {
    let actions = open_actions(
        "<< /S /Named /N /X /Next 82 0 R >>",
        &[(82, b"[<< /S /Hide /T (a) /Next 82 0 R >>]")],
    );
    assert_eq!(kinds(&actions), [ActionKind::Named, ActionKind::Hide]);
}

#[test]
fn where_actions_lead_out_of_the_document() {
    let remote = open_actions("<< /S /GoToR /F (other.pdf) /D [3 /Fit] >>", &[]);
    let file = remote[0].file.as_ref().unwrap();
    assert_eq!(file.name.as_ref().unwrap().text, "other.pdf");
    assert!(!file.url);
    // In another file the page is its index, counted from 0 (12.6.4.3).
    assert_eq!(
        remote[0].destination,
        Some(Destination {
            page: DestinationPage::Index(3),
            view: View::Fit
        })
    );

    let named = open_actions("<< /S /GoToR /F (other.pdf) /D /chapter >>", &[]);
    assert_eq!(named[0].destination, None);
    assert_eq!(named[0].destination_name.as_deref(), Some(&b"chapter"[..]));

    let launch = open_actions("<< /S /Launch /Win << /F (cmd.exe) >> >>", &[]);
    assert_eq!(launch[0].kind, ActionKind::Launch);
    assert_eq!(
        launch[0].file.as_ref().unwrap().name.as_ref().unwrap().text,
        "cmd.exe"
    );

    let submit = open_actions(
        "<< /S /SubmitForm /F << /FS /URL /F (https://example.invalid/form) >> >>",
        &[],
    );
    let file = submit[0].file.as_ref().unwrap();
    assert!(file.url);
    assert_eq!(
        file.name.as_ref().unwrap().text,
        "https://example.invalid/form"
    );

    let import = open_actions("<< /S /ImportData /F (data.fdf) >>", &[]);
    assert_eq!(
        import[0].file.as_ref().unwrap().name.as_ref().unwrap().text,
        "data.fdf"
    );
}

/// A URI is 7-bit ASCII; other bytes are U+FFFD and make it inexact.
#[test]
fn a_uri_outside_ascii() {
    let actions = open_actions("<< /S /URI /URI <68C3A9> >>", &[]);
    let uri = actions[0].uri.as_ref().unwrap();
    assert_eq!(uri.text, "h\u{fffd}\u{fffd}");
    assert!(!uri.exact);
}

#[test]
fn scripts_in_strings_streams_and_renditions() {
    let stream_script = stream("", b"app.alert(2)");
    let actions = open_actions(
        "<< /S /JavaScript /JS 83 0 R /Next [<< /S /Rendition /JS (x) >> << /S /Rendition /OP 0 >> << /S /Foo >>] >>",
        &[(83, &stream_script)],
    );
    assert_eq!(
        kinds(&actions),
        [
            ActionKind::JavaScript,
            ActionKind::Rendition,
            ActionKind::Rendition,
            ActionKind::Other(Name::new(&b"Foo"[..])),
        ]
    );
    let scripts: Vec<bool> = actions.iter().map(|action| action.script).collect();
    assert_eq!(scripts, [true, true, false, false]);
}

#[test]
fn a_dictionary_without_s_is_no_action() {
    assert!(open_actions("<< /D [10 0 R /Fit] >>", &[]).is_empty());
    // Nor is a destination array.
    assert!(open_actions("[10 0 R /Fit]", &[]).is_empty());
}
