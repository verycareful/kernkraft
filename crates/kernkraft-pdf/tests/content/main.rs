// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Tests of the content interpreter, one module per area of `src/content/`.
//! Files are written by `pdf_fixture`, byte by byte, and each test's
//! expectations come from how its content is built and from the arithmetic
//! of ISO 32000-1 (8.3.4, 9.4.4), never from a run.

#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

#[path = "../pdf_fixture/mod.rs"]
mod pdf_fixture;

mod forms;
mod load;
mod marked;
mod matrix;
mod operation;
mod optional;
mod paths;
mod robustness;
mod spans;
mod state;
mod text;

use kernkraft_fs::NoProgress;
use kernkraft_pdf::content::{
    Context, Device, ExtGStateInfo, FillRule, FormInfo, GraphicsState, ImageSource,
    InterpretOptions, Interpreter, MarkedContent, Matrix, OperatorSpan, Path, PathPaint, TextRun,
};
use kernkraft_pdf::doc::Document;
use kernkraft_pdf::syntax::{Name, ObjectId};
use kernkraft_pdf::{Diagnostic, DiagnosticKind, PdfError};

pub use pdf_fixture::*;

pub fn id(number: u32) -> ObjectId {
    ObjectId {
        number,
        generation: 0,
    }
}

pub fn name(text: &str) -> Name {
    Name::new(text.as_bytes())
}

/// What a device was told, with the pieces of the context a test looks at.
#[derive(Debug, Clone)]
pub enum Event {
    Save,
    Restore,
    Path {
        path: Path,
        paint: PathPaint,
        state: GraphicsState,
        span: OperatorSpan,
        construction: OperatorSpan,
        visible: bool,
    },
    Clip {
        path: Path,
        rule: FillRule,
    },
    BeginText,
    EndText,
    Text {
        run: TextRun,
        state: GraphicsState,
        span: OperatorSpan,
        visible: bool,
        marked: Vec<MarkedContent>,
    },
    Image {
        image: ImageSource,
        state: GraphicsState,
        visible: bool,
    },
    Shading {
        name: Name,
    },
    BeginForm(FormInfo, GraphicsState),
    EndForm(FormInfo),
    BeginMarked {
        marked: Vec<MarkedContent>,
        visible: bool,
    },
    EndMarked(MarkedContent),
    Point {
        tag: Name,
    },
    ExtGState {
        name: Name,
        state: GraphicsState,
    },
    Compatibility(bool),
}

/// A device that records everything.
#[derive(Default)]
pub struct Recorder {
    pub events: Vec<Event>,
}

impl Device for Recorder {
    fn save(&mut self, _: &Context<'_>) {
        self.events.push(Event::Save);
    }
    fn restore(&mut self, _: &Context<'_>) {
        self.events.push(Event::Restore);
    }
    fn path(
        &mut self,
        ctx: &Context<'_>,
        path: &Path,
        paint: PathPaint,
        construction: OperatorSpan,
    ) {
        self.events.push(Event::Path {
            path: path.clone(),
            paint,
            state: ctx.state.clone(),
            span: ctx.span,
            construction,
            visible: ctx.visible,
        });
    }
    fn clip(&mut self, _: &Context<'_>, path: &Path, rule: FillRule) {
        self.events.push(Event::Clip {
            path: path.clone(),
            rule,
        });
    }
    fn begin_text(&mut self, _: &Context<'_>) {
        self.events.push(Event::BeginText);
    }
    fn end_text(&mut self, _: &Context<'_>) {
        self.events.push(Event::EndText);
    }
    fn text(&mut self, ctx: &Context<'_>, run: &TextRun) {
        self.events.push(Event::Text {
            run: run.clone(),
            state: ctx.state.clone(),
            span: ctx.span,
            visible: ctx.visible,
            marked: ctx.marked.to_vec(),
        });
    }
    fn image(&mut self, ctx: &Context<'_>, image: &ImageSource) {
        self.events.push(Event::Image {
            image: image.clone(),
            state: ctx.state.clone(),
            visible: ctx.visible,
        });
    }
    fn shading(&mut self, _: &Context<'_>, name: &Name, _: &kernkraft_pdf::syntax::Object) {
        self.events.push(Event::Shading { name: name.clone() });
    }
    fn begin_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {
        self.events
            .push(Event::BeginForm(form.clone(), ctx.state.clone()));
    }
    fn end_form(&mut self, _: &Context<'_>, form: &FormInfo) {
        self.events.push(Event::EndForm(form.clone()));
    }
    fn begin_marked(&mut self, ctx: &Context<'_>) {
        self.events.push(Event::BeginMarked {
            marked: ctx.marked.to_vec(),
            visible: ctx.visible,
        });
    }
    fn end_marked(&mut self, _: &Context<'_>, closed: &MarkedContent) {
        self.events.push(Event::EndMarked(closed.clone()));
    }
    fn marked_point(
        &mut self,
        _: &Context<'_>,
        tag: &Name,
        _: Option<&kernkraft_pdf::syntax::Dictionary>,
    ) {
        self.events.push(Event::Point { tag: tag.clone() });
    }
    fn ext_gstate(&mut self, ctx: &Context<'_>, info: &ExtGStateInfo) {
        self.events.push(Event::ExtGState {
            name: info.name.clone(),
            state: ctx.state.clone(),
        });
    }
    fn compatibility(&mut self, _: &Context<'_>, begin: bool) {
        self.events.push(Event::Compatibility(begin));
    }
}

/// A fixture of a one-page document whose page (object 10) has `content`
/// as its content stream (object 11), `resources` as its resource
/// dictionary body, and the extra objects.
pub fn content_fixture(content: &[u8], resources: &str, extra: &[(u32, &[u8])]) -> Fixture {
    content_fixture_with_catalog(content, resources, extra, "")
}

/// As [`content_fixture`], with `catalog` added to the catalog's entries.
pub fn content_fixture_with_catalog(
    content: &[u8],
    resources: &str,
    extra: &[(u32, &[u8])],
    catalog: &str,
) -> Fixture {
    let mut objects: Vec<(u32, Vec<u8>)> = vec![
        (
            1,
            format!("<< /Type /Catalog /Pages 2 0 R {catalog} >>").into_bytes(),
        ),
        (
            2,
            b"<< /Type /Pages /Kids [10 0 R] /Count 1 /MediaBox [0 0 612 792] >>".to_vec(),
        ),
        (3, b"<< /Producer (kernkraft tests) >>".to_vec()),
        (
            10,
            format!(
                "<< /Type /Page /Parent 2 0 R /Contents 11 0 R /Resources << {resources} >> >>"
            )
            .into_bytes(),
        ),
        (11, stream("", content)),
    ];
    for (number, body) in extra {
        objects.push((*number, body.to_vec()));
    }
    table_file(&borrowed(&objects), "/Root 1 0 R /Info 3 0 R")
}

/// Runs page 1 of `document` into a recorder.
pub fn run_document(document: &Document) -> Result<Vec<Event>, PdfError> {
    run_document_with(document, InterpretOptions::default())
}

pub fn run_document_with(
    document: &Document,
    options: InterpretOptions,
) -> Result<Vec<Event>, PdfError> {
    let page = document.page(1)?;
    let mut interpreter = Interpreter::new(document, options)?;
    let mut recorder = Recorder::default();
    interpreter.run_page(&page, &mut recorder, &NoProgress)?;
    Ok(recorder.events)
}

/// Runs `content` as the only page of a document with `resources`.
pub fn run(content: &str, resources: &str, extra: &[(u32, &[u8])]) -> (Document, Vec<Event>) {
    let document = content_fixture(content.as_bytes(), resources, extra).open();
    let events = run_document(&document).expect("the page runs");
    (document, events)
}

/// The diagnostics of one kind.
pub fn of_kind(document: &Document, kind: &DiagnosticKind) -> Vec<Diagnostic> {
    document
        .diagnostics()
        .into_iter()
        .filter(|diagnostic| &diagnostic.kind == kind)
        .collect()
}

/// Whether the document recorded any diagnostic matching `test`.
pub fn has(document: &Document, test: impl Fn(&DiagnosticKind) -> bool) -> bool {
    document
        .diagnostics()
        .iter()
        .any(|diagnostic| test(&diagnostic.kind))
}

/// The path events of a run.
pub fn paths(events: &[Event]) -> Vec<&Event> {
    events
        .iter()
        .filter(|event| matches!(event, Event::Path { .. }))
        .collect()
}

pub fn texts(events: &[Event]) -> Vec<&TextRun> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::Text { run, .. } => Some(run),
            _ => None,
        })
        .collect()
}

/// Whether two numbers are equal to within rounding.
pub fn near(a: f64, b: f64) -> bool {
    (a - b).abs() < 1e-9
}

pub fn assert_matrix(actual: Matrix, expected: [f64; 6]) {
    let actual = actual.to_array();
    assert!(
        actual.iter().zip(expected).all(|(a, b)| near(*a, b)),
        "matrix {actual:?} is not {expected:?}"
    );
}

/// A Helvetica font whose widths are given for codes 65 to 67 (600, 700,
/// 800), and a second whose widths are given for codes 32 and 33 (250, 300).
pub const FONTS: &str = "/Font << /F1 60 0 R /F2 61 0 R >>";
pub const FONT_OBJECTS: [(u32, &[u8]); 2] = [
    (
        60,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 65 /LastChar 67 /Widths [600 700 800] /Encoding /WinAnsiEncoding >>",
    ),
    (
        61,
        b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica /FirstChar 32 /LastChar 33 /Widths [250 300] /Encoding /WinAnsiEncoding >>",
    ),
];
