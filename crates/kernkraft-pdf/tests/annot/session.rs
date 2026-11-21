// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Annotations through an edit session: each change is one step that can be
//! undone and redone, and a save appends them to the bytes the file had.
//! ISO 32000-1, 7.5.6 (incremental updates).

use super::*;
use kernkraft_fs::{Expect, NoProgress};
use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::write::WriteOptions;
use kernkraft_pdf::{
    add_annotation, delete_annotation, edit_annotation, AnnotationEdit, AnnotationKind,
    EditSession, NewAnnotation, NewShape, SaveOptions,
};

const SEED: [u8; 32] = [3; 32];

fn options() -> SaveOptions {
    SaveOptions {
        write: WriteOptions { seed: SEED },
        modified: WHEN,
    }
}

fn highlight() -> NewAnnotation {
    NewAnnotation {
        shape: NewShape::Highlight {
            quads: vec![points_rect(100.0, 92.0, 400.0, 142.0)],
        },
        colour: Rgb {
            r: 1.0,
            g: 1.0,
            b: 0.0,
        },
        opacity: 1.0,
        contents: Some("keep".to_string()),
        author: None,
        modified: WHEN,
    }
}

#[test]
fn adding_editing_and_deleting_are_steps_that_undo_and_redo() {
    let mut session = EditSession::new(blank().open()).unwrap();
    let added = add_annotation(session.document(), 1, &highlight()).unwrap();
    assert!(session.push(added.document).unwrap());
    assert!(session.is_modified());
    assert_eq!(session.document().annotations(1).unwrap().len(), 1);

    let edited = edit_annotation(
        session.document(),
        1,
        added.id,
        &AnnotationEdit {
            contents: Some("changed".to_string()),
            ..Default::default()
        },
        &WHEN,
    )
    .unwrap();
    assert!(session.push(edited).unwrap());
    assert_eq!(
        session.document().annotations(1).unwrap()[0]
            .contents
            .as_deref(),
        Some("changed")
    );

    let deleted = delete_annotation(session.document(), 1, added.id).unwrap();
    assert!(session.push(deleted).unwrap());
    assert!(session.document().annotations(1).unwrap().is_empty());

    assert!(session.undo());
    assert_eq!(session.document().annotations(1).unwrap().len(), 1);
    assert!(session.undo());
    assert_eq!(
        session.document().annotations(1).unwrap()[0]
            .contents
            .as_deref(),
        Some("keep")
    );
    assert!(session.undo());
    assert!(session.document().annotations(1).unwrap().is_empty());
    assert!(!session.is_modified());
    assert!(session.redo());
    assert!(session.is_modified());
}

#[test]
fn a_save_appends_the_annotation_to_the_bytes_the_file_had_and_it_reads_back() {
    let fixture = document("0 1 1 rg 100 600 200 100 re f", "", "", &[]);
    let mut session = EditSession::new(fixture.open()).unwrap();
    let added = add_annotation(session.document(), 1, &highlight()).unwrap();
    session.push(added.document).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("annotated.pdf");
    session
        .save(&path, Expect::Anything, &options(), &NoProgress)
        .unwrap();
    let saved = std::fs::read(&path).unwrap();
    assert!(saved.starts_with(&fixture.bytes), "the original bytes stay");

    let reopened = Document::from_bytes(saved, &OPTIONS, "").unwrap();
    let listed = reopened.annotations(1).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].kind, AnnotationKind::Highlight);
    assert_eq!(listed[0].contents.as_deref(), Some("keep"));
    assert_rect(listed[0].rect, 100.0, 92.0, 400.0, 142.0, "rect");
    // The appearance came through the update: the cyan under the box is
    // drawn green by the multiply.
    assert_eq!(super::render::pixel(&reopened, 150, 120)[..3], [0, 255, 0]);
}

#[test]
fn an_annotation_deleted_after_a_save_is_deleted_by_the_next_update() {
    let mut session = EditSession::new(blank().open()).unwrap();
    let added = add_annotation(session.document(), 1, &highlight()).unwrap();
    session.push(added.document).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let first = dir.path().join("first.pdf");
    session
        .save(&first, Expect::Anything, &options(), &NoProgress)
        .unwrap();

    let deleted = delete_annotation(session.document(), 1, added.id).unwrap();
    session.push(deleted).unwrap();
    let second = dir.path().join("second.pdf");
    session
        .save(&second, Expect::Anything, &options(), &NoProgress)
        .unwrap();
    let reopened = Document::from_bytes(std::fs::read(&second).unwrap(), &OPTIONS, "").unwrap();
    assert!(reopened.annotations(1).unwrap().is_empty());
}
