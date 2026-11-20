// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Changing the document: `apply`, `undo` and `redo`.
//!
//! Every change gives a new snapshot that the session pushes; nothing is
//! written until a save. A change that changes nothing adds no step, and its
//! answer lists no changed pages.

use kernkraft_fs::NoProgress;
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::text::TextBlock;
use kernkraft_pdf::{
    add_annotation, delete_annotation, edit_annotation, edit_text, remove, reorder, rotate,
    AnnotationEdit, DiagnosticKind, NewAnnotation, NewShape, Rotation, TextEdit, TextStyle,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::{Changed, Engine, HandleRequest, Session};
use crate::error::WireError;
use crate::fields::Fields;
use crate::wire::{self, WireId};

/// The opacity of a new annotation that names none: a highlight multiplies
/// with the page, the others are opaque.
const DEFAULT_OPACITY: f64 = 1.0;

/// One change to a document.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ApplyRequest {
    pub handle: i64,
    pub op: Op,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "camelCase")]
pub enum MarkupKind {
    Highlight,
    Underline,
    StrikeOut,
}

/// One change, by `op`. Geometry is fractions of the displayed page; every
/// date is a PDF date string.
#[derive(Debug, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase", deny_unknown_fields)]
pub enum Op {
    /// Turns pages clockwise by 90, 180 or 270 degrees; no pages is all.
    Rotate {
        pages: Vec<u32>,
        degrees: u32,
    },
    /// The new order, listing every page once.
    Reorder {
        order: Vec<u32>,
    },
    Remove {
        pages: Vec<u32>,
    },
    AddMarkup {
        kind: MarkupKind,
        page: u32,
        quads: Vec<[f64; 4]>,
        colour: [f64; 3],
        opacity: Option<f64>,
        contents: Option<String>,
        author: Option<String>,
        modified: String,
    },
    AddInk {
        page: u32,
        strokes: Vec<Vec<(f64, f64)>>,
        width: f64,
        colour: [f64; 3],
        opacity: Option<f64>,
        author: Option<String>,
        modified: String,
    },
    AddNote {
        page: u32,
        at: (f64, f64),
        text: String,
        colour: [f64; 3],
        opacity: Option<f64>,
        author: Option<String>,
        modified: String,
    },
    EditAnnotation {
        page: u32,
        id: WireId,
        colour: Option<[f64; 3]>,
        opacity: Option<f64>,
        line_width: Option<f64>,
        contents: Option<String>,
        author: Option<String>,
        translate: Option<(f64, f64)>,
        modified: String,
    },
    DeleteAnnotation {
        page: u32,
        id: WireId,
    },
    /// Sets the text of a block read by `text` at `revision`. An empty text
    /// removes the block's text.
    EditText {
        page: u32,
        block: usize,
        revision: u64,
        text: String,
        family: Option<String>,
        size: Option<f64>,
        colour: Option<[f64; 3]>,
    },
}

/// What a change made: the new snapshot, the pages it changed and what the
/// answer adds.
struct Made {
    document: Document,
    changed: Changed,
    extra: Fields,
}

impl Made {
    fn new(document: Document, changed: Changed) -> Self {
        Self {
            document,
            changed,
            extra: Fields::new(),
        }
    }
}

impl Engine {
    pub fn apply(&self, request: ApplyRequest) -> Result<Fields, WireError> {
        let fonts = self.fonts();
        self.sessions.with(request.handle, |session| {
            let made = make(session, request.op, &fonts)?;
            let pushed = session.edit.push(made.document)?;
            let changed = if pushed {
                session.did(made.changed);
                session.last_changed()
            } else {
                Changed::Pages(Vec::new())
            };
            let mut fields = session.answer(&changed)?;
            fields.extend(made.extra);
            Ok(fields)
        })
    }

    pub fn undo(&self, request: HandleRequest) -> Result<Fields, WireError> {
        self.sessions.with(request.handle, |session| {
            let changed = session.undid().unwrap_or(Changed::Pages(Vec::new()));
            Ok(session.answer(&changed)?)
        })
    }

    pub fn redo(&self, request: HandleRequest) -> Result<Fields, WireError> {
        self.sessions.with(request.handle, |session| {
            let changed = session.redid().unwrap_or(Changed::Pages(Vec::new()));
            Ok(session.answer(&changed)?)
        })
    }
}

fn make(session: &Session, op: Op, fonts: &kernkraft_font::FontList) -> Result<Made, WireError> {
    let document = session.document();
    Ok(match op {
        Op::Rotate { pages, degrees } => {
            let by = match degrees {
                90 => Rotation::Cw90,
                180 => Rotation::Cw180,
                270 => Rotation::Cw270,
                other => {
                    return Err(WireError::bad_request(format!(
                        "a rotation is 90, 180 or 270 degrees, not {other}"
                    )))
                }
            };
            let changed = if pages.is_empty() {
                Changed::All
            } else {
                Changed::Pages(pages.clone())
            };
            Made::new(rotate(document, &pages, by)?, changed)
        }
        Op::Reorder { order } => Made::new(reorder(document, &order)?, Changed::All),
        Op::Remove { pages } => Made::new(remove(document, &pages)?, Changed::All),
        Op::AddMarkup {
            kind,
            page,
            quads,
            colour,
            opacity,
            contents,
            author,
            modified,
        } => {
            let quads = quads.into_iter().map(wire::rect_from).collect();
            let shape = match kind {
                MarkupKind::Highlight => NewShape::Highlight { quads },
                MarkupKind::Underline => NewShape::Underline { quads },
                MarkupKind::StrikeOut => NewShape::StrikeOut { quads },
            };
            added(
                document, page, shape, colour, opacity, contents, author, &modified,
            )?
        }
        Op::AddInk {
            page,
            strokes,
            width,
            colour,
            opacity,
            author,
            modified,
        } => added(
            document,
            page,
            NewShape::Ink { strokes, width },
            colour,
            opacity,
            None,
            author,
            &modified,
        )?,
        Op::AddNote {
            page,
            at,
            text,
            colour,
            opacity,
            author,
            modified,
        } => added(
            document,
            page,
            NewShape::Note { at },
            colour,
            opacity,
            Some(text),
            author,
            &modified,
        )?,
        Op::EditAnnotation {
            page,
            id,
            colour,
            opacity,
            line_width,
            contents,
            author,
            translate,
            modified,
        } => {
            let edit = AnnotationEdit {
                colour: colour.map(wire::colour_from),
                opacity,
                line_width,
                contents,
                author,
                translate,
            };
            let edited =
                edit_annotation(document, page, id.into(), &edit, &wire::date(&modified)?)?;
            Made::new(edited, Changed::one(page))
        }
        Op::DeleteAnnotation { page, id } => Made::new(
            delete_annotation(document, page, id.into())?,
            Changed::one(page),
        ),
        Op::EditText {
            page,
            block,
            revision,
            text,
            family,
            size,
            colour,
        } => {
            if revision != session.revision {
                return Err(WireError::stale_block(revision, session.revision));
            }
            let page_text = document.text_page(page, &NoProgress)?;
            let block: &TextBlock = page_text.blocks.get(block).ok_or_else(|| {
                WireError::bad_request(format!(
                    "page {page} has {} text blocks, not {}",
                    page_text.blocks.len(),
                    block + 1
                ))
            })?;
            let mut style = TextStyle::default();
            if let Some(family) = family {
                style = style.with_family(family);
            }
            if let Some(size) = size {
                style = style.with_size(size);
            }
            if let Some(colour) = colour {
                style = style.with_colour(wire::colour_from(colour));
            }
            let edit = TextEdit::new(text).with_style(style).with_fonts(fonts);
            let edited = edit_text(document, page, block, &edit, &NoProgress)?;
            let mut made = Made::new(edited.document, Changed::one(page));
            made.extra.insert("lines".to_owned(), json!(edited.lines));
            made.extra.insert(
                "notes".to_owned(),
                Value::Array(substitutions(document, &made.document)),
            );
            made
        }
    })
}

/// A new annotation, with its object in the answer.
#[allow(clippy::too_many_arguments)]
fn added(
    document: &Document,
    page: u32,
    shape: NewShape,
    colour: [f64; 3],
    opacity: Option<f64>,
    contents: Option<String>,
    author: Option<String>,
    modified: &str,
) -> Result<Made, WireError> {
    let new = NewAnnotation {
        shape,
        colour: wire::colour_from(colour),
        opacity: opacity.unwrap_or(DEFAULT_OPACITY),
        contents,
        author,
        modified: wire::date(modified)?,
    };
    let added = add_annotation(document, page, &new)?;
    let mut made = Made::new(added.document, Changed::one(page));
    made.extra
        .insert("annotation".to_owned(), wire::object(added.id));
    Ok(made)
}

/// The fonts the edit had to substitute, as notes for the user: the
/// diagnostics `after` has and `before` did not.
fn substitutions(before: &Document, after: &Document) -> Vec<Value> {
    let known: Vec<String> = before
        .diagnostics()
        .iter()
        .map(|d| format!("{:?}", d.kind))
        .collect();
    after
        .diagnostics()
        .iter()
        .filter(|d| !known.contains(&format!("{:?}", d.kind)))
        .filter_map(|d| match &d.kind {
            DiagnosticKind::FontSubstituted { name, family } => Some(json!(format!(
                "substitute font used for: {} ({family})",
                String::from_utf8_lossy(name.as_bytes())
            ))),
            _ => None,
        })
        .collect()
}
