// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The device that finds the showing operations of a block again:
//! ISO 32000-1:2008, 9.4.3 (the text-showing operators), 9.3 (text state
//! parameters), 9.3.6 (text rendering mode) and 9.4.1 (text objects).
//!
//! The text layer keeps, for each glyph, only where in the stream the
//! operation that showed it lies ([`GlyphSource`](crate::text::GlyphSource)).
//! What an editor needs of that operation (the font and its size, the
//! spacing, the render mode, the advance of every glyph, the graphics
//! state, the `ET` that closes its text object) is read here, by running
//! the page again and keeping the operations it was told to.

use std::collections::{HashMap, HashSet};

use crate::content::{
    Context, Device, FillRule, FormInfo, GraphicsState, OperatorSpan, Path, RenderMode, StreamId,
    TextRun,
};
use crate::font::FontKind;
use crate::syntax::ObjectId;

/// A showing operation, told apart from the same operation in another
/// invocation of its form: its stream, its place in it and the invocation
/// ([`GlyphSource::invocation`](crate::text::GlyphSource::invocation)).
pub(super) type OpKey = (StreamId, u32, u32);

/// A form XObject the content invoked to reach an operation.
#[derive(Debug, Clone, Copy)]
pub(super) struct FormUse {
    /// The `Do` that invoked the form, in the stream that holds it.
    pub invoked_by: OperatorSpan,
    pub id: ObjectId,
}

/// A showing operation of the page, as the interpreter saw it.
#[derive(Debug, Clone)]
pub(super) struct ShowOp {
    pub span: OperatorSpan,
    pub run: TextRun,
    pub kind: FontKind,
    pub render_mode: RenderMode,
    /// `Tz` as a fraction (9.3.4).
    pub horizontal_scaling: f64,
    /// The forms open when the operation ran, outermost first.
    pub chain: Vec<FormUse>,
    /// The graphics state as the operation found it.
    pub state: GraphicsState,
    /// How many clipping paths were in force, counted through `q` and `Q`.
    pub clips: usize,
    /// Whether the operation is in marked content that has an `MCID`.
    pub tagged: bool,
    /// The `ET` that ends the text object the operation is in. `None` when
    /// the stream ends, or another text object begins, first.
    pub text_end: Option<TextEnd>,
}

/// The `ET` of a text object, and what was in force at it.
#[derive(Debug, Clone)]
pub(super) struct TextEnd {
    pub span: OperatorSpan,
    pub state: GraphicsState,
    pub clips: usize,
}

pub(super) struct ShowCollector {
    wanted: HashSet<OpKey>,
    invoked: u32,
    /// The forms open, innermost last, with the number of each invocation.
    forms: Vec<(u32, FormUse)>,
    /// The wanted operations of the text object now open.
    open: Vec<OpKey>,
    /// The clipping paths in force and, for each `q` open, how many there
    /// were when it saved the state.
    clips: usize,
    saved_clips: Vec<usize>,
    pub(super) ops: HashMap<OpKey, ShowOp>,
}

impl ShowCollector {
    pub(super) fn new(wanted: HashSet<OpKey>) -> Self {
        Self {
            wanted,
            invoked: 0,
            forms: Vec::new(),
            open: Vec::new(),
            clips: 0,
            saved_clips: Vec::new(),
            ops: HashMap::new(),
        }
    }
}

impl Device for ShowCollector {
    fn begin_form(&mut self, ctx: &Context<'_>, form: &FormInfo) {
        self.invoked += 1;
        self.forms.push((
            self.invoked,
            FormUse {
                invoked_by: ctx.span,
                id: form.id,
            },
        ));
    }

    fn end_form(&mut self, _ctx: &Context<'_>, _form: &FormInfo) {
        self.forms.pop();
    }

    fn save(&mut self, _ctx: &Context<'_>) {
        self.saved_clips.push(self.clips);
    }

    fn restore(&mut self, _ctx: &Context<'_>) {
        if let Some(clips) = self.saved_clips.pop() {
            self.clips = clips;
        }
    }

    fn clip(&mut self, _ctx: &Context<'_>, _path: &Path, _rule: FillRule) {
        self.clips += 1;
    }

    fn begin_text(&mut self, _ctx: &Context<'_>) {
        self.open.clear();
    }

    fn end_text(&mut self, ctx: &Context<'_>) {
        for key in self.open.drain(..) {
            if let Some(op) = self.ops.get_mut(&key) {
                op.text_end = Some(TextEnd {
                    span: ctx.span,
                    state: ctx.state.clone(),
                    clips: self.clips,
                });
            }
        }
    }

    fn text(&mut self, ctx: &Context<'_>, run: &TextRun) {
        let invocation = self.forms.last().map_or(0, |(number, _)| *number);
        let key = (ctx.span.stream, ctx.span.index, invocation);
        if !self.wanted.contains(&key) {
            return;
        }
        self.open.push(key);
        self.ops.insert(
            key,
            ShowOp {
                span: ctx.span,
                run: run.clone(),
                kind: run.font.kind(),
                render_mode: ctx.state.text.render_mode,
                horizontal_scaling: ctx.state.text.horizontal_scaling,
                chain: self.forms.iter().map(|(_, used)| *used).collect(),
                state: ctx.state.clone(),
                clips: self.clips,
                tagged: ctx.marked.iter().any(|marked| marked.mcid().is_some()),
                text_end: None,
            },
        );
    }
}
