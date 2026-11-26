// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! `apply`, `undo` and `redo` through the engine, on the two-page sample.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;
mod fixture;
#[path = "../../kernkraft-font/tests/common/mod.rs"]
mod fontkit;

use common::{failure_kind, ok};
use fixture::{lines_page, open, DATE};
use kernkraft_jni::bridge::Bridge;
use serde_json::{json, Value};

fn apply(engine: &Bridge, handle: i64, op: Value) -> String {
    engine.apply(&json!({ "handle": handle, "op": op }).to_string())
}

fn step(engine: &Bridge, operation: fn(&Bridge, &str) -> String, handle: i64) -> Value {
    ok(&operation(engine, &json!({ "handle": handle }).to_string()))
}

fn rotation_of(engine: &Bridge, handle: i64, page: u32) -> u64 {
    let info = ok(&engine.page_info(&json!({ "handle": handle, "page": page }).to_string()));
    info["rotate"].as_u64().unwrap()
}

fn annotations(engine: &Bridge, handle: i64) -> Vec<Value> {
    let answer = ok(&engine.annotations(&json!({ "handle": handle, "page": 1 }).to_string()));
    answer["annotations"].as_array().unwrap().clone()
}

#[test]
fn a_rotation_reports_the_page_and_undo_and_redo_move_between_the_states() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [1], "degrees": 90 }),
    ));
    assert_eq!(done["revision"], 1);
    assert_eq!(done["changed_pages"], json!([1]));
    assert_eq!(done["pages"], 2);
    assert_eq!(done["can_undo"], true);
    assert_eq!(done["can_redo"], false);
    assert_eq!(done["modified"], true);
    assert_eq!(rotation_of(&engine, handle, 1), 90);
    assert_eq!(rotation_of(&engine, handle, 2), 0);

    let undone = step(&engine, Bridge::undo, handle);
    assert_eq!(undone["revision"], 2);
    assert_eq!(undone["changed_pages"], json!([1]));
    assert_eq!(undone["modified"], false);
    assert_eq!(undone["can_redo"], true);
    assert_eq!(rotation_of(&engine, handle, 1), 0);

    let redone = step(&engine, Bridge::redo, handle);
    assert_eq!(redone["revision"], 3);
    assert_eq!(redone["changed_pages"], json!([1]));
    assert_eq!(rotation_of(&engine, handle, 1), 90);
}

#[test]
fn undo_with_nothing_to_undo_changes_nothing() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let undone = step(&engine, Bridge::undo, handle);

    assert_eq!(undone["revision"], 0);
    assert_eq!(undone["changed_pages"], json!([]));
    assert_eq!(undone["can_undo"], false);
    assert_eq!(
        step(&engine, Bridge::redo, handle)["changed_pages"],
        json!([])
    );
}

#[test]
fn rotating_no_pages_rotates_them_all() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [], "degrees": 180 }),
    ));

    assert_eq!(done["changed_pages"], json!([1, 2]));
    assert_eq!(rotation_of(&engine, handle, 2), 180);
}

#[test]
fn a_rotation_that_is_not_a_quarter_turn_is_bad() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [1], "degrees": 45 }),
    );

    assert_eq!(failure_kind(&answer), "BadRequest");
}

#[test]
fn removing_a_page_changes_the_page_tree_and_undo_brings_it_back() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let removed = ok(&apply(
        &engine,
        handle,
        json!({ "op": "remove", "pages": [2] }),
    ));
    assert_eq!(removed["pages"], 1);
    assert_eq!(removed["changed_pages"], json!([1]));

    let undone = step(&engine, Bridge::undo, handle);
    assert_eq!(undone["pages"], 2);
    assert_eq!(undone["changed_pages"], json!([1, 2]));
}

#[test]
fn removing_every_page_is_refused_and_leaves_the_document() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let answer = apply(&engine, handle, json!({ "op": "remove", "pages": [1, 2] }));

    assert_eq!(failure_kind(&answer), "Invalid");
    let after = step(&engine, Bridge::undo, handle);
    assert_eq!(after["pages"], 2);
    assert_eq!(after["revision"], 0);
}

#[test]
fn reordering_reports_every_page() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "reorder", "order": [2, 1] }),
    ));

    assert_eq!(done["changed_pages"], json!([1, 2]));
    assert_eq!(done["revision"], 1);
}

#[test]
fn a_new_change_after_an_undo_drops_what_could_be_redone() {
    let engine = Bridge::new();
    let handle = open(&engine);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [1], "degrees": 90 }),
    ));
    step(&engine, Bridge::undo, handle);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [2], "degrees": 270 }),
    ));

    assert_eq!(done["can_redo"], false);
    assert_eq!(done["changed_pages"], json!([2]));
    assert_eq!(rotation_of(&engine, handle, 1), 0);
    assert_eq!(rotation_of(&engine, handle, 2), 270);
}

#[test]
fn an_op_the_engine_does_not_know_is_bad() {
    let engine = Bridge::new();
    let handle = open(&engine);

    assert_eq!(
        failure_kind(&apply(
            &engine,
            handle,
            json!({ "op": "addText", "page": 1 })
        )),
        "BadRequest"
    );
    assert_eq!(
        failure_kind(&apply(&engine, handle, json!({ "pages": [1] }))),
        "BadRequest"
    );
}

#[test]
fn a_highlight_is_listed_with_its_id_and_can_be_recoloured_and_deleted() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let added = ok(&apply(
        &engine,
        handle,
        json!({ "op": "addMarkup", "kind": "highlight", "page": 1,
                "quads": [[0.1, 0.1, 0.5, 0.15]], "colour": [1.0, 1.0, 0.0],
                "modified": DATE }),
    ));
    assert_eq!(added["changed_pages"], json!([1]));
    let id = added["annotation"].clone();

    let listed = annotations(&engine, handle);
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["kind"], "Highlight");
    assert_eq!(listed[0]["id"], id);
    assert_eq!(listed[0]["colour"], json!([1.0, 1.0, 0.0]));

    ok(&apply(
        &engine,
        handle,
        json!({ "op": "editAnnotation", "page": 1, "id": id, "colour": [0.0, 1.0, 0.0],
                "modified": DATE }),
    ));
    assert_eq!(
        annotations(&engine, handle)[0]["colour"],
        json!([0.0, 1.0, 0.0])
    );

    ok(&apply(
        &engine,
        handle,
        json!({ "op": "deleteAnnotation", "page": 1, "id": id }),
    ));
    assert!(annotations(&engine, handle).is_empty());

    step(&engine, Bridge::undo, handle);
    assert_eq!(annotations(&engine, handle).len(), 1);
}

#[test]
fn ink_and_a_note_are_listed_by_kind() {
    let engine = Bridge::new();
    let handle = open(&engine);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "addInk", "page": 1, "strokes": [[[0.1, 0.1], [0.2, 0.2]]],
                "width": 2.0, "colour": [1.0, 0.0, 0.0], "modified": DATE }),
    ));
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "addNote", "page": 1, "at": [0.5, 0.5], "text": "check this",
                "colour": [1.0, 0.8, 0.0], "modified": DATE }),
    ));

    let listed = annotations(&engine, handle);

    let kinds: Vec<&str> = listed.iter().map(|a| a["kind"].as_str().unwrap()).collect();
    assert_eq!(kinds, ["Ink", "Note"]);
    assert_eq!(listed[1]["contents"], "check this");
    assert_eq!(listed[0]["line_width"], 2.0);
}

#[test]
fn an_annotation_dated_wrongly_is_bad_and_adds_nothing() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "addNote", "page": 1, "at": [0.5, 0.5], "text": "x",
                "colour": [0.0, 0.0, 0.0], "modified": "yesterday" }),
    );

    assert_eq!(failure_kind(&answer), "BadRequest");
    assert!(annotations(&engine, handle).is_empty());
}

#[test]
fn deleting_an_annotation_that_is_not_there_is_refused() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "deleteAnnotation", "page": 1, "id": { "object": 9999, "generation": 0 } }),
    );

    assert_eq!(failure_kind(&answer), "AnnotationNotOnPage");
}

fn text_of(engine: &Bridge, handle: i64) -> (u64, String) {
    let text = ok(&engine.text(&json!({ "handle": handle, "page": 1 }).to_string()));
    let all: Vec<&str> = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l["text"].as_str().unwrap())
        .collect();
    (text["revision"].as_u64().unwrap(), all.join("\n"))
}

fn heading_block(engine: &Bridge, handle: i64) -> u64 {
    let text = ok(&engine.text(&json!({ "handle": handle, "page": 1 }).to_string()));
    let line = text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .find(|l| l["text"] == "Kern -- Project Architecture Brief")
        .unwrap();
    line["block"].as_u64().unwrap()
}

#[test]
fn editing_a_block_replaces_its_text() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "Kern Brief" }),
    ));

    assert_eq!(done["changed_pages"], json!([1]));
    assert_eq!(done["lines"], 1);
    let (_, text) = text_of(&engine, handle);
    assert!(text.contains("Kern Brief"), "{text}");
    assert!(!text.contains("Project Architecture"), "{text}");
}

#[test]
fn editing_a_block_takes_a_left_and_a_width_as_json() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);

    // Three two-letter words in a box 0.01 of the page across take a line
    // each or more; the same text in a box 0.95 across takes one.
    let narrow = ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "ab cd ef", "left": 0.25, "width": 0.01 }),
    ));
    assert!(narrow["lines"].as_u64().unwrap() >= 3, "{narrow}");
    step(&engine, Bridge::undo, handle);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);
    let wide = ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "ab cd ef", "left": 0.25, "width": 0.95 }),
    ));
    assert_eq!(wide["lines"], 1);
}

#[test]
fn a_box_off_the_page_is_refused_and_an_unknown_field_is_a_bad_request() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);

    let off = apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "X", "width": 0.0 }),
    );
    assert_eq!(failure_kind(&off), "Invalid");
    let unknown = apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "X", "right": 0.9 }),
    );
    assert_eq!(failure_kind(&unknown), "BadRequest");
    assert!(text_of(&engine, handle).1.contains("Project Architecture"));
}

#[test]
fn a_block_read_before_a_change_is_stale() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [2], "degrees": 90 }),
    ));

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision, "text": "X" }),
    );

    assert_eq!(failure_kind(&answer), "StaleBlock");
    assert!(text_of(&engine, handle).1.contains("Project Architecture"));
}

#[test]
fn a_block_past_the_last_is_bad() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": 9999, "revision": 0, "text": "X" }),
    );

    assert_eq!(failure_kind(&answer), "BadRequest");
}

#[test]
fn an_empty_text_removes_the_blocks_text() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);

    ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": 0, "text": "" }),
    ));

    assert!(!text_of(&engine, handle).1.contains("Project Architecture"));
}

/// A font file of the space and the Cyrillic letter U+0416, which the
/// sample's heading font cannot show, written to `dir`.
fn cyrillic_font(dir: &std::path::Path) -> std::path::PathBuf {
    use fontkit::sfnt::{self, Glyph, SfntSpec};
    let glyph = |name: &'static str, advance: u16, contours| Glyph {
        name,
        advance,
        contours,
    };
    let square = vec![vec![
        (0, 0, true),
        (500, 0, true),
        (500, 500, true),
        (0, 500, true),
    ]];
    let mut spec = SfntSpec::new(vec![
        glyph(".notdef", 500, vec![]),
        glyph("uni0416", 600, square),
        glyph("space", 250, vec![]),
    ]);
    spec.unicode = vec![(0x416, 1), (0x20, 2)];
    spec.family = "Cyrillic Sans";
    spec.full = "Cyrillic Sans Regular";
    spec.postscript = "CyrillicSans-Regular";
    let path = dir.join("cyrillic-sans.ttf");
    std::fs::write(&path, sfnt::build(&spec)).unwrap();
    path
}

fn notes_of(done: &Value) -> Vec<String> {
    done["notes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|note| note.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn an_edit_that_needs_a_face_of_the_font_list_answers_a_note_each_time() {
    let dir = tempfile::tempdir().unwrap();
    let engine = Bridge::new();
    let fonts =
        ok(&engine.set_system_fonts(&json!({ "paths": [cyrillic_font(dir.path())] }).to_string()));
    assert_eq!(fonts["faces"], 1);
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);
    let edit = json!({ "op": "editText", "page": 1, "block": block, "revision": revision, "text": "\u{416}" });

    let first = ok(&apply(&engine, handle, edit.clone()));
    let notes = notes_of(&first);
    assert_eq!(notes.len(), 1, "{first}");
    assert!(notes[0].ends_with("(Cyrillic Sans)"), "{notes:?}");

    // An undo and the same edit again make the same substitution on the same
    // block, which the document's log holds once: the answer still names it.
    step(&engine, Bridge::undo, handle);
    let (revision, _) = text_of(&engine, handle);
    let again = ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision, "text": "\u{416}" }),
    ));
    assert_eq!(notes_of(&again), notes, "{again}");
}

#[test]
fn an_edit_in_the_blocks_own_font_answers_no_note() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let block = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "editText", "page": 1, "block": block, "revision": revision,
                "text": "Kern Brief" }),
    ));

    assert_eq!(done["notes"], json!([]));
}

/// The `wrapped` of each line of page 1 of `path`, with the text of its block.
fn wrapped_lines(engine: &Bridge, path: &std::path::Path) -> Vec<(u64, String, bool)> {
    let opened = ok(&engine.open(&json!({ "path": path }).to_string()));
    let handle = opened["handle"].as_i64().unwrap();
    let text = ok(&engine.text(&json!({ "handle": handle, "page": 1 }).to_string()));
    text["lines"]
        .as_array()
        .unwrap()
        .iter()
        .map(|line| {
            (
                line["block"].as_u64().unwrap(),
                line["text"].as_str().unwrap().to_owned(),
                line["wrapped"].as_bool().unwrap(),
            )
        })
        .collect()
}

#[test]
fn the_text_says_for_each_line_whether_it_wrapped() {
    let dir = tempfile::tempdir().unwrap();
    let paragraph = lines_page(
        dir.path(),
        &[
            "alpha beta gamma delta epsilon",
            "zeta eta theta iota kappa",
            "lambda mu.",
        ],
    );
    let engine = Bridge::new();

    let lines = wrapped_lines(&engine, &paragraph);

    // Greedy to a column of 30 characters: "lambda" and a space do not fit
    // the 5 characters left after the second line.
    assert_eq!(
        lines,
        vec![
            (0, "alpha beta gamma delta epsilon".to_owned(), true),
            (0, "zeta eta theta iota kappa".to_owned(), true),
            (0, "lambda mu.".to_owned(), false),
        ]
    );
}

#[test]
fn the_lines_of_an_address_did_not_wrap() {
    let dir = tempfile::tempdir().unwrap();
    let address = lines_page(
        dir.path(),
        &["Jane Doe", "12 Long Street Name", "Springfield"],
    );
    let engine = Bridge::new();

    let wrapped: Vec<bool> = wrapped_lines(&engine, &address)
        .into_iter()
        .map(|(_, _, wrapped)| wrapped)
        .collect();

    assert_eq!(wrapped, vec![false, false, false]);
}

fn page_fonts(engine: &Bridge, handle: i64, near: Option<[f64; 2]>) -> Value {
    let mut request = json!({ "handle": handle, "page": 1 });
    if let Some(near) = near {
        request["near"] = json!(near);
    }
    ok(&engine.page_fonts(&request.to_string()))
}

#[test]
fn the_fonts_of_the_sample_are_listed_with_the_nearest_marked() {
    let engine = Bridge::new();
    let handle = open(&engine);

    // The heading is at 4.7 to 7.4 percent of the page's height, set in
    // Helvetica-Bold at 24 points in black.
    let answer = page_fonts(&engine, handle, Some([0.5, 0.06]));

    assert_eq!(answer["revision"], 0);
    let fonts = answer["fonts"].as_array().unwrap();
    let heading = fonts
        .iter()
        .find(|font| font["font"] == "Helvetica-Bold" && font["size"] == 24.0)
        .unwrap_or_else(|| panic!("no heading font in {fonts:?}"));
    assert_eq!(heading["embedded"], false);
    assert_eq!(heading["colour"], json!([0.0, 0.0, 0.0]));
    assert_eq!(heading["nearest"], true);
    assert_eq!(answer["nearest"], heading["blocks"][0]);
    assert!(fonts.iter().filter(|font| font["nearest"] == true).count() == 1);
    // Most used first.
    let chars: Vec<u64> = fonts.iter().map(|f| f["chars"].as_u64().unwrap()).collect();
    assert!(chars.windows(2).all(|pair| pair[0] >= pair[1]), "{chars:?}");
}

#[test]
fn a_paragraph_of_one_font_is_one_entry_with_the_characters_it_has() {
    let dir = tempfile::tempdir().unwrap();
    let path = lines_page(
        dir.path(),
        &[
            "alpha beta gamma delta epsilon",
            "zeta eta theta iota kappa",
            "lambda mu.",
        ],
    );
    let engine = Bridge::new();
    let opened = ok(&engine.open(&json!({ "path": path }).to_string()));
    let handle = opened["handle"].as_i64().unwrap();

    let answer = page_fonts(&engine, handle, None);

    // 30 + 25 + 10 characters of Courier at 10 points, painted black.
    assert_eq!(
        answer["fonts"],
        json!([{ "font": "Courier", "embedded": false, "size": 10.0,
                 "colour": [0.0, 0.0, 0.0], "blocks": [0], "chars": 65,
                 "nearest": false }])
    );
    assert_eq!(answer["nearest"], Value::Null);
}

#[test]
fn new_text_on_the_sample_follows_the_nearest_block_and_one_undo_takes_it_off() {
    let engine = Bridge::new();
    let handle = open(&engine);

    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "addText", "page": 1, "at": [0.3, 0.06], "width": 0.4,
                "text": "Added" }),
    ));

    assert_eq!(done["changed_pages"], json!([1]));
    assert_eq!(done["lines"], 1);
    assert_eq!(done["family"], "Helvetica-Bold");
    assert_eq!(
        done["followed"],
        page_fonts(&engine, handle, Some([0.3, 0.06]))["nearest"]
    );
    assert_eq!(done["notes"], json!([]));
    assert!(text_of(&engine, handle).1.contains("Added"));

    let undone = step(&engine, Bridge::undo, handle);
    assert_eq!(undone["changed_pages"], json!([1]));
    assert!(!text_of(&engine, handle).1.contains("Added"));
    step(&engine, Bridge::redo, handle);
    assert!(text_of(&engine, handle).1.contains("Added"));
}

#[test]
fn a_named_block_is_followed_and_each_part_of_the_style_can_be_replaced() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let heading = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);

    // Far from the heading, but the heading is named: its font again, at 12
    // points in blue.
    let done = ok(&apply(
        &engine,
        handle,
        json!({ "op": "addText", "page": 1, "at": [0.1, 0.9], "width": 0.4,
                "text": "Named", "like": { "block": heading, "revision": revision },
                "size": 12.0, "colour": [0.0, 0.0, 1.0] }),
    ));

    assert_eq!(done["family"], "Helvetica-Bold");
    assert_eq!(done["followed"], heading);
    let fonts = page_fonts(&engine, handle, None);
    let named = fonts["fonts"]
        .as_array()
        .unwrap()
        .iter()
        .find(|font| font["size"] == 12.0 && font["colour"] == json!([0.0, 0.0, 1.0]))
        .unwrap_or_else(|| panic!("{fonts}"));
    assert_eq!(named["font"], "Helvetica-Bold");
}

#[test]
fn a_block_read_before_a_change_is_stale_for_new_text_too() {
    let engine = Bridge::new();
    let handle = open(&engine);
    let heading = heading_block(&engine, handle);
    let (revision, _) = text_of(&engine, handle);
    ok(&apply(
        &engine,
        handle,
        json!({ "op": "rotate", "pages": [2], "degrees": 90 }),
    ));

    let answer = apply(
        &engine,
        handle,
        json!({ "op": "addText", "page": 1, "at": [0.1, 0.1], "width": 0.4, "text": "X",
                "like": { "block": heading, "revision": revision } }),
    );

    assert_eq!(failure_kind(&answer), "StaleBlock");
}

#[test]
fn a_page_with_no_text_needs_a_size_and_then_takes_the_font_list_face() {
    let dir = tempfile::tempdir().unwrap();
    let path = lines_page(dir.path(), &[]);
    let engine = Bridge::new();
    let opened = ok(&engine.open(&json!({ "path": path }).to_string()));
    let handle = opened["handle"].as_i64().unwrap();
    let new = |extra: Value| {
        let mut op = json!({ "op": "addText", "page": 1, "at": [0.2, 0.2], "width": 0.5,
                             "text": "\u{416}" });
        op.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        op
    };

    assert_eq!(
        page_fonts(&engine, handle, Some([0.5, 0.5]))["fonts"],
        json!([])
    );
    assert_eq!(
        failure_kind(&apply(&engine, handle, new(json!({})))),
        "NoSizeForNewText"
    );
    // The engine always has a font list, empty until the application sets
    // one, so no font is a character no face has.
    assert_eq!(
        failure_kind(&apply(&engine, handle, new(json!({ "size": 12.0 })))),
        "NoGlyphForCharacter"
    );

    let dir = tempfile::tempdir().unwrap();
    ok(&engine.set_system_fonts(&json!({ "paths": [cyrillic_font(dir.path())] }).to_string()));
    let done = ok(&apply(&engine, handle, new(json!({ "size": 12.0 }))));
    assert_eq!(done["family"], "Cyrillic Sans");
    assert_eq!(done["followed"], Value::Null);
    assert_eq!(done["notes"], json!([]));
}
