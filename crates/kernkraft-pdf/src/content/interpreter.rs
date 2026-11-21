// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The content-stream interpreter: ISO 32000-1:2008, 7.8.2 (content
//! streams), 8.2 (graphics objects), 8.4 (graphics state), 8.5 (paths),
//! 8.6 (colour), 9.3 and 9.4 (text), 8.10 (form XObjects) and 14.6
//! (marked content), driving a [`Device`].
//!
//! The interpreter reads each operation of a stream, keeps the graphics
//! state, the text object, the path being built and the marked-content
//! stack, and tells the device what is painted. It paints nothing itself.
//! Everything it tolerates (an operator it does not know, operands of the
//! wrong kind, a missing resource) is a [`Diagnostic`](crate::Diagnostic),
//! recorded on the document when the run ends, and the operation is
//! skipped.

use std::collections::HashMap;
use std::sync::Arc;

use kernkraft_fs::Progress;

use crate::content::device::{
    Context, Device, ExtGStateInfo, FillRule, ImageSource, MarkedContent, OperatorSpan, Path,
    PathPaint, PathSegment, StreamId,
};
use crate::content::load::DEFAULT_MAX_CONTENT_BYTES;
use crate::content::matrix::Matrix;
use crate::content::operation::{operand_number, ContentParser, InlineImage, Operation};
use crate::content::optional::OptionalContent;
use crate::content::state::{
    ColorSpace, Dash, GraphicsState, LineCap, LineJoin, Paint, RenderMode, SoftMask, TextFont,
};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{Document, Page};
use crate::error::{Limit, PdfError};
use crate::font::Font;
use crate::source::MemorySource;
use crate::syntax::limits::MAX_GRAPHICS_STATE_DEPTH;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Span, Value};

/// How many operations run between two looks at the caller's
/// cancellation flag.
const CANCEL_CHECK_INTERVAL: u64 = 256;

/// The operations a run may execute when the caller does not say. The
/// densest real pages (maps, CAD drawings) run to a few million; this
/// leaves room above them while ending a run that multiplies forms within
/// seconds.
pub const DEFAULT_MAX_OPERATIONS: u64 = 1 << 27;

/// What the caller sets for a run.
#[derive(Debug, Clone)]
pub struct InterpretOptions {
    /// The matrix the CTM starts as: the identity gives default user space;
    /// a renderer passes the matrix from there to its device space.
    pub base_matrix: Matrix,
    /// The most bytes one content stream may decode to
    /// ([`PdfError::DecodedTooLarge`] past it).
    pub max_stream_bytes: usize,
    /// The most operations one run executes, those of every form it runs
    /// included ([`Limit::Operations`]). Forms that invoke forms multiply:
    /// a chain of 50 forms that each paint the next twice is 2^50
    /// operations from a few hundred bytes, so the count is bounded rather
    /// than the nesting alone.
    pub max_operations: u64,
}

impl Default for InterpretOptions {
    fn default() -> Self {
        Self {
            base_matrix: Matrix::IDENTITY,
            max_stream_bytes: DEFAULT_MAX_CONTENT_BYTES,
            max_operations: DEFAULT_MAX_OPERATIONS,
        }
    }
}

/// What a run needs from the caller beside the document.
pub(super) struct Env<'a> {
    pub(super) device: &'a mut dyn Device,
    pub(super) progress: &'a dyn Progress,
}

/// The text object (9.4.1): the text matrix and the text line matrix.
#[derive(Debug, Clone, Copy)]
pub(super) struct TextObject {
    pub(super) active: bool,
    pub(super) tm: Matrix,
    pub(super) tlm: Matrix,
}

impl Default for TextObject {
    fn default() -> Self {
        Self {
            active: false,
            tm: Matrix::IDENTITY,
            tlm: Matrix::IDENTITY,
        }
    }
}

/// The path being built (8.5.2).
#[derive(Debug, Default)]
pub(super) struct PathBuilder {
    pub(super) path: Path,
    /// The current point, when a subpath is open.
    pub(super) current: Option<(f64, f64)>,
    /// The start of the current subpath, the point `h` returns to.
    pub(super) start: Option<(f64, f64)>,
    /// The first construction operator, for the span of the whole path.
    pub(super) first: Option<OperatorSpan>,
    /// The clip rule `W` or `W*` asked for, applied by the next painting
    /// operator.
    pub(super) clip: Option<FillRule>,
}

/// What belongs to the stream being run, saved while a nested stream runs.
struct StreamFrame {
    stream: StreamId,
    index: u32,
    compat: u32,
    base_saved: usize,
    base_marked: usize,
    text: TextObject,
    path: PathBuilder,
}

/// The blend modes of 11.3.5 (Table 136), which the interpreter's consumers
/// are expected to know.
const KNOWN_BLEND_MODES: [&[u8]; 17] = [
    b"Normal",
    b"Compatible",
    b"Multiply",
    b"Screen",
    b"Overlay",
    b"Darken",
    b"Lighten",
    b"ColorDodge",
    b"ColorBurn",
    b"HardLight",
    b"SoftLight",
    b"Difference",
    b"Exclusion",
    b"Hue",
    b"Saturation",
    b"Color",
    b"Luminosity",
];

/// A tiling pattern's cell to run (8.7.3).
#[derive(Debug, Clone, Copy)]
pub struct PatternCell<'a> {
    /// The pattern's stream.
    pub id: ObjectId,
    /// The pattern's own resources.
    pub resources: &'a Dictionary,
    /// Pattern space to the space the device draws in.
    pub matrix: Matrix,
    /// `BBox`, as `[left bottom right top]`: the cell is clipped to it.
    pub bbox: [f64; 4],
    /// The colour of an uncoloured pattern (`PaintType` 2).
    pub colour: Option<&'a Paint>,
}

/// Interprets content streams against a [`Document`]. It holds its own
/// handle on the document (a clone is the same snapshot), so a caller can
/// keep one interpreter, and the fonts it has read, across many runs.
pub struct Interpreter {
    pub(super) doc: Document,
    pub(super) options: InterpretOptions,
    pub(super) diagnostics: Diagnostics,
    pub(super) optional: OptionalContent,
    pub(super) fonts: HashMap<ObjectId, Arc<Font>>,
    pub(super) state: GraphicsState,
    pub(super) saved: Vec<GraphicsState>,
    pub(super) marked: Vec<MarkedContent>,
    pub(super) text: TextObject,
    pub(super) path: PathBuilder,
    /// The form XObjects open, outermost first.
    pub(super) forms: Vec<ObjectId>,
    /// How many forms open hide their content by optional content.
    pub(super) hidden_forms: usize,
    /// The resource dictionaries of the streams open, innermost last.
    pub(super) resources: Vec<Dictionary>,
    pub(super) stream: StreamId,
    pub(super) index: u32,
    /// The `BX` sections open (7.8.2): unknown operators in one are
    /// ignored.
    pub(super) compat: u32,
    /// The saved-state count a `Q` may not go below: where the stream being
    /// run began.
    pub(super) base_saved: usize,
    pub(super) base_marked: usize,
    /// Whether the stream being run is a Type 3 glyph description (9.6.5),
    /// the only place `d0` and `d1` belong.
    pub(super) in_glyph: bool,
    /// Set by `d1`: the glyph takes the colour of the text that shows it,
    /// so the colour operators of the description are ignored (9.6.5.1).
    pub(super) uncoloured: bool,
    operations: u64,
}

impl Interpreter {
    /// An interpreter for `doc`. It reads the document's optional-content
    /// configuration once.
    pub fn new(doc: &Document, options: InterpretOptions) -> Result<Self, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let optional = OptionalContent::read(doc, &mut diagnostics)?;
        let state = GraphicsState::initial(options.base_matrix);
        Ok(Self {
            doc: doc.clone(),
            options,
            diagnostics,
            optional,
            fonts: HashMap::new(),
            state,
            saved: Vec::new(),
            marked: Vec::new(),
            text: TextObject::default(),
            path: PathBuilder::default(),
            forms: Vec::new(),
            hidden_forms: 0,
            resources: Vec::new(),
            stream: StreamId::Page(ObjectId {
                number: 0,
                generation: 0,
            }),
            index: 0,
            compat: 0,
            base_saved: 0,
            base_marked: 0,
            in_glyph: false,
            uncoloured: false,
            operations: 0,
        })
    }

    /// Runs the page's content: its `Contents` streams, as one, with the
    /// page's resources. The graphics state starts as 8.4.1 says, with the
    /// CTM the options give.
    pub fn run_page(
        &mut self,
        page: &Page,
        device: &mut dyn Device,
        progress: &dyn Progress,
    ) -> Result<(), PdfError> {
        let result = self.run_page_inner(page, &mut Env { device, progress });
        self.flush();
        result
    }

    /// Runs the form XObject `id` as a page would run its content, in the
    /// state `matrix` gives for the CTM: for an annotation's appearance
    /// stream, a pattern cell or a soft mask. The form's `Matrix` and
    /// `BBox` apply as 8.10.2 says.
    pub fn run_form(
        &mut self,
        id: ObjectId,
        matrix: Matrix,
        device: &mut dyn Device,
        progress: &dyn Progress,
    ) -> Result<(), PdfError> {
        self.reset(matrix);
        let span = OperatorSpan {
            stream: StreamId::Object(id),
            range: Span { start: 0, end: 0 },
            index: 0,
        };
        let mut env = Env { device, progress };
        let result = self.invoke_form(id, None, span, &mut env);
        self.flush();
        result
    }

    /// Runs the glyph description `procedure` of a Type 3 font (9.6.5) with
    /// `matrix` taking glyph space to the space the device draws in.
    ///
    /// The description starts in the state of the text that shows it,
    /// `outer`: its colour as both fill and stroke colour, its constant
    /// alpha, blend mode and soft mask. Its `d0` or `d1` is accepted; after
    /// `d1` the colour operators of the description are ignored (9.6.5.1).
    /// `resources` are the font's, or the page's when it has none.
    pub fn run_glyph(
        &mut self,
        procedure: ObjectId,
        resources: &Dictionary,
        matrix: Matrix,
        outer: &GraphicsState,
        device: &mut dyn Device,
        progress: &dyn Progress,
    ) -> Result<(), PdfError> {
        self.reset(matrix);
        self.state.fill = outer.fill.clone();
        self.state.stroke = outer.fill.clone();
        self.state.fill_alpha = outer.fill_alpha;
        self.state.stroke_alpha = outer.fill_alpha;
        self.state.blend_mode = outer.blend_mode.clone();
        self.state.soft_mask = outer.soft_mask.clone();
        self.in_glyph = true;
        let result = self.run_object_stream(procedure, resources, &mut Env { device, progress });
        self.in_glyph = false;
        self.uncoloured = false;
        self.flush();
        result
    }

    /// Runs the cell of the tiling pattern `pattern` (8.7.3) with `matrix`
    /// taking pattern space to the space the device draws in, clipped to
    /// the pattern's `bbox`. `resources` are the pattern's own.
    ///
    /// A coloured pattern (`PaintType` 1) starts in the initial state and
    /// sets its own colours. For an uncoloured one (`PaintType` 2) `colour`
    /// is the colour it is painted in: it is both the fill and the stroke
    /// colour, and the colour operators of the cell are ignored (8.7.3.3).
    pub fn run_pattern(
        &mut self,
        cell: &PatternCell<'_>,
        device: &mut dyn Device,
        progress: &dyn Progress,
    ) -> Result<(), PdfError> {
        let PatternCell {
            id: pattern,
            resources,
            matrix,
            bbox,
            colour,
        } = *cell;
        self.reset(matrix);
        if let Some(colour) = colour {
            self.state.fill = colour.clone();
            self.state.stroke = colour.clone();
            self.uncoloured = true;
        }
        let stream = StreamId::Object(pattern);
        let span = OperatorSpan {
            stream,
            range: Span { start: 0, end: 0 },
            index: 0,
        };
        let [llx, lly, urx, ury] = bbox;
        let clip = Path {
            segments: vec![
                PathSegment::MoveTo(llx, lly),
                PathSegment::LineTo(urx, lly),
                PathSegment::LineTo(urx, ury),
                PathSegment::LineTo(llx, ury),
                PathSegment::Close,
            ],
        };
        let mut env = Env { device, progress };
        let ctx = self.context(span);
        env.device.clip(&ctx, &clip, FillRule::NonZeroWinding);
        let result = self.run_object_stream(pattern, resources, &mut env);
        self.uncoloured = false;
        self.flush();
        result
    }

    /// Runs the content stream of the object `procedure` with `resources`.
    fn run_object_stream(
        &mut self,
        procedure: ObjectId,
        resources: &Dictionary,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let content = self.doc.load_content_stream(
            StreamId::Object(procedure),
            self.options.max_stream_bytes,
            &mut self.diagnostics,
        )?;
        self.resources.push(resources.clone());
        self.diagnostics.set_object(Some(procedure));
        let result = self.run_content(StreamId::Object(procedure), content.data, env);
        self.diagnostics.set_object(None);
        self.resources.pop();
        result
    }

    /// Hands the diagnostics of the run to the document.
    fn flush(&mut self) {
        let diagnostics = std::mem::take(&mut self.diagnostics);
        self.doc.record_diagnostics(diagnostics);
    }

    fn reset(&mut self, ctm: Matrix) {
        self.state = GraphicsState::initial(ctm);
        self.saved.clear();
        self.marked.clear();
        self.text = TextObject::default();
        self.path = PathBuilder::default();
        self.forms.clear();
        self.hidden_forms = 0;
        self.resources.clear();
        self.compat = 0;
        self.base_saved = 0;
        self.base_marked = 0;
        self.in_glyph = false;
        self.uncoloured = false;
        self.operations = 0;
    }

    fn run_page_inner(&mut self, page: &Page, env: &mut Env<'_>) -> Result<(), PdfError> {
        self.reset(self.options.base_matrix);
        let resources = match &page.resources {
            Some(object) => self
                .doc
                .resolve(object)?
                .dictionary()
                .cloned()
                .unwrap_or_default(),
            None => Dictionary::new(),
        };
        let id = StreamId::Page(page.id);
        self.diagnostics.set_object(Some(page.id));
        let content = self.doc.load_content_stream(
            id,
            self.options.max_stream_bytes,
            &mut self.diagnostics,
        )?;
        self.resources.push(resources);
        let result = self.run_content(id, content.data, env);
        self.resources.pop();
        self.diagnostics.set_object(None);
        result
    }

    /// Runs the operations of one content stream, `data`, in the state the
    /// interpreter is in. The `q`, text object and marked-content
    /// sequences it leaves open are closed as it ends.
    pub(super) fn run_content(
        &mut self,
        stream: StreamId,
        data: Vec<u8>,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let frame = StreamFrame {
            stream: self.stream,
            index: self.index,
            compat: self.compat,
            base_saved: self.base_saved,
            base_marked: self.base_marked,
            text: std::mem::take(&mut self.text),
            path: std::mem::take(&mut self.path),
        };
        self.stream = stream;
        self.index = 0;
        self.compat = 0;
        self.base_saved = self.saved.len();
        self.base_marked = self.marked.len();
        let result = self.run_operations(data, env);
        let unclosed = self.saved.len() > self.base_saved;
        if unclosed {
            self.diagnostics
                .push(DiagnosticKind::GraphicsStateUnclosed, 0);
            while self.saved.len() > self.base_saved {
                self.restore_state(None, env);
            }
        }
        if self.marked.len() > self.base_marked {
            self.diagnostics
                .push(DiagnosticKind::MarkedContentUnbalanced, 0);
            self.marked.truncate(self.base_marked);
        }
        self.stream = frame.stream;
        self.index = frame.index;
        self.compat = frame.compat;
        self.base_saved = frame.base_saved;
        self.base_marked = frame.base_marked;
        self.text = frame.text;
        self.path = frame.path;
        result
    }

    fn run_operations(&mut self, data: Vec<u8>, env: &mut Env<'_>) -> Result<(), PdfError> {
        let source = MemorySource::new(data);
        let mut parser = ContentParser::new(&source);
        while let Some(operation) = parser.next_operation(&mut self.diagnostics)? {
            self.operations += 1;
            if self.operations > self.options.max_operations {
                return Err(PdfError::LimitExceeded {
                    offset: operation.span.start,
                    limit: Limit::Operations {
                        max: self.options.max_operations,
                    },
                });
            }
            if self.operations.is_multiple_of(CANCEL_CHECK_INTERVAL) && env.progress.is_cancelled()
            {
                return Err(PdfError::Cancelled);
            }
            let span = OperatorSpan {
                stream: self.stream,
                range: operation.span,
                index: self.index,
            };
            self.index = self.index.saturating_add(1);
            self.execute(&operation, span, env)?;
        }
        Ok(())
    }

    /// The context an event is sent with.
    pub(super) fn context(&self, span: OperatorSpan) -> Context<'_> {
        Context {
            span,
            state: &self.state,
            marked: &self.marked,
            visible: self.hidden_forms == 0 && !self.marked.iter().any(|marked| marked.hidden),
        }
    }

    // --- diagnostics ---

    pub(super) fn invalid(&mut self, operation: &Operation) {
        self.diagnostics.push(
            DiagnosticKind::OperandsInvalid {
                operator: Name::new(operation.operator.clone()),
            },
            operation.span.start,
        );
    }

    /// `d0` and `d1` (9.6.5, Table 113): the first operator of a Type 3
    /// glyph description. Anywhere else they are misplaced.
    fn glyph_metrics(&mut self, operation: &Operation) {
        if !self.in_glyph {
            self.misplaced(operation);
            return;
        }
        let uncoloured = operation.operator == b"d1";
        let wanted = if uncoloured { 6 } else { 2 };
        if operation.operands.len() != wanted
            || operation
                .operands
                .iter()
                .any(|operand| operand_number(operand).is_none())
        {
            self.invalid(operation);
            return;
        }
        if uncoloured {
            self.uncoloured = true;
        }
    }

    pub(super) fn misplaced(&mut self, operation: &Operation) {
        self.diagnostics.push(
            DiagnosticKind::OperatorMisplaced {
                operator: Name::new(operation.operator.clone()),
            },
            operation.span.start,
        );
    }

    pub(super) fn missing(&mut self, category: &[u8], name: &Name, operation: &Operation) {
        self.diagnostics.push(
            DiagnosticKind::ResourceMissing {
                category: Name::new(category),
                name: name.clone(),
            },
            operation.span.start,
        );
    }

    // --- operands ---

    /// The operands as `N` numbers, when there are exactly `N`.
    pub(super) fn numbers<const N: usize>(&mut self, operation: &Operation) -> Option<[f64; N]> {
        let numbers = (operation.operands.len() == N)
            .then(|| {
                let mut values = [0.0; N];
                for (slot, operand) in values.iter_mut().zip(&operation.operands) {
                    *slot = operand_number(operand)?;
                }
                Some(values)
            })
            .flatten();
        if numbers.is_none() {
            self.invalid(operation);
        }
        numbers
    }

    /// The only operand, as a name.
    pub(super) fn name_operand(&mut self, operation: &Operation) -> Option<Name> {
        let name = match operation.operands.as_slice() {
            [only] => match only.value() {
                Value::Name(name) => Some(name.clone()),
                _ => None,
            },
            _ => None,
        };
        if name.is_none() {
            self.invalid(operation);
        }
        name
    }

    // --- dispatch ---

    fn execute(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        match operation.operator.as_slice() {
            // General graphics state (8.4.4, Table 57).
            b"w" => {
                if let Some([width]) = self.numbers(operation) {
                    self.state.line_width = width;
                }
            }
            b"J" => {
                if let Some([code]) = self.numbers::<1>(operation) {
                    match LineCap::from_code(code as i64) {
                        Some(cap) => self.state.line_cap = cap,
                        None => self.invalid(operation),
                    }
                }
            }
            b"j" => {
                if let Some([code]) = self.numbers::<1>(operation) {
                    match LineJoin::from_code(code as i64) {
                        Some(join) => self.state.line_join = join,
                        None => self.invalid(operation),
                    }
                }
            }
            b"M" => {
                if let Some([limit]) = self.numbers(operation) {
                    self.state.miter_limit = limit;
                }
            }
            b"d" => self.set_dash(operation),
            b"ri" => {
                if let Some(name) = self.name_operand(operation) {
                    self.state.rendering_intent = name;
                }
            }
            b"i" => {
                if let Some([flatness]) = self.numbers(operation) {
                    self.state.flatness = flatness;
                }
            }
            b"gs" => self.set_ext_gstate(operation, span, env)?,
            // Special graphics state (8.4.4).
            b"q" => self.save_state(operation, span, env)?,
            b"Q" => self.restore_state(Some((operation, span)), env),
            b"cm" => {
                if let Some(values) = self.numbers::<6>(operation) {
                    self.state.ctm = Matrix::from_array(values).then(self.state.ctm);
                }
            }
            // Path construction (8.5.2, Table 59).
            b"m" | b"l" | b"c" | b"v" | b"y" | b"h" | b"re" => self.construct_path(operation, span),
            // Path painting (8.5.3) and clipping (8.5.4).
            b"S" | b"s" | b"f" | b"F" | b"f*" | b"B" | b"B*" | b"b" | b"b*" | b"n" => {
                self.paint_path(operation, span, env)
            }
            b"W" | b"W*" => {
                if !operation.operands.is_empty() {
                    self.invalid(operation);
                } else {
                    self.path.clip = Some(if operation.operator == b"W" {
                        FillRule::NonZeroWinding
                    } else {
                        FillRule::EvenOdd
                    });
                }
            }
            // Text objects (9.4.1) and text state (9.3).
            b"BT" => self.begin_text(operation, span, env),
            b"ET" => self.end_text(operation, span, env),
            b"Tc" | b"Tw" | b"Tz" | b"TL" | b"Tf" | b"Tr" | b"Ts" => {
                self.set_text_state(operation)?
            }
            // Text positioning (9.4.2) and showing (9.4.3).
            b"Td" | b"TD" | b"Tm" | b"T*" => self.position_text(operation),
            b"Tj" | b"TJ" | b"'" | b"\"" => self.show_text(operation, span, env)?,
            // Type 3 fonts (9.6.5): only a glyph description may use these.
            b"d0" | b"d1" => self.glyph_metrics(operation),
            // Colour (8.6.8).
            b"CS" | b"cs" | b"SC" | b"SCN" | b"sc" | b"scn" | b"G" | b"g" | b"RG" | b"rg"
            | b"K" | b"k"
                if self.uncoloured => {}
            b"CS" | b"cs" => self.set_colour_space(operation),
            b"SC" | b"SCN" | b"sc" | b"scn" => self.set_colour(operation)?,
            b"G" | b"g" | b"RG" | b"rg" | b"K" | b"k" => self.set_device_colour(operation),
            // Shading (8.7.4.2).
            b"sh" => self.paint_shading(operation, span, env)?,
            // Inline images (8.9.7).
            b"BI" => match &operation.inline_image {
                Some(image) => {
                    let image = ImageSource::Inline(self.inline_in_resources(image)?);
                    let ctx = self.context(span);
                    env.device.image(&ctx, &image);
                }
                None => self
                    .diagnostics
                    .push(DiagnosticKind::InlineImageDamaged, operation.span.start),
            },
            b"ID" | b"EI" => self.diagnostics.push(
                DiagnosticKind::OperatorUnknown {
                    operator: Name::new(operation.operator.clone()),
                },
                operation.span.start,
            ),
            // External objects (8.8).
            b"Do" => self.do_xobject(operation, span, env)?,
            // Marked content (14.6.2).
            b"MP" | b"DP" | b"BMC" | b"BDC" | b"EMC" => {
                self.marked_content(operation, span, env)?
            }
            // Compatibility (7.8.2).
            b"BX" => {
                self.compat += 1;
                let ctx = self.context(span);
                env.device.compatibility(&ctx, true);
            }
            b"EX" => {
                if self.compat == 0 {
                    self.misplaced(operation);
                } else {
                    self.compat -= 1;
                    let ctx = self.context(span);
                    env.device.compatibility(&ctx, false);
                }
            }
            _ => {
                if self.compat == 0 {
                    self.diagnostics.push(
                        DiagnosticKind::OperatorUnknown {
                            operator: Name::new(operation.operator.clone()),
                        },
                        operation.span.start,
                    );
                }
            }
        }
        Ok(())
    }

    // --- graphics state ---

    fn set_dash(&mut self, operation: &Operation) {
        let [array, phase] = operation.operands.as_slice() else {
            return self.invalid(operation);
        };
        let (Value::Array(items), Some(phase)) = (array.value(), operand_number(phase)) else {
            return self.invalid(operation);
        };
        let numbers: Option<Vec<f64>> = items.iter().map(operand_number).collect();
        match numbers {
            Some(array) if array.iter().all(|length| *length >= 0.0) => {
                self.state.dash = Dash { array, phase };
            }
            _ => self.invalid(operation),
        }
    }

    pub(super) fn save_state(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        if !operation.operands.is_empty() {
            self.invalid(operation);
            return Ok(());
        }
        self.push_state(operation.span.start)?;
        let ctx = self.context(span);
        env.device.save(&ctx);
        Ok(())
    }

    /// Saves the state on the stack: `q`, and a form's start.
    pub(super) fn push_state(&mut self, offset: u64) -> Result<(), PdfError> {
        if self.saved.len() >= MAX_GRAPHICS_STATE_DEPTH {
            return Err(PdfError::LimitExceeded {
                offset,
                limit: Limit::GraphicsStateDepth,
            });
        }
        self.saved.push(self.state.clone());
        Ok(())
    }

    /// `Q`, or the restore that ends a form or a stream that left a `q`
    /// open. `operation` is `None` for the latter two, which have no
    /// operator to name.
    pub(super) fn restore_state(
        &mut self,
        operation: Option<(&Operation, OperatorSpan)>,
        env: &mut Env<'_>,
    ) {
        if let Some((operation, _)) = operation {
            if !operation.operands.is_empty() {
                return self.invalid(operation);
            }
        }
        if self.saved.len() <= self.base_saved && operation.is_some() {
            let offset = operation.map_or(0, |(operation, _)| operation.span.start);
            self.diagnostics
                .push(DiagnosticKind::GraphicsStateUnderflow, offset);
            return;
        }
        let Some(state) = self.saved.pop() else {
            return;
        };
        self.state = state;
        let span = operation.map_or(
            OperatorSpan {
                stream: self.stream,
                range: Span { start: 0, end: 0 },
                index: self.index,
            },
            |(_, span)| span,
        );
        let ctx = self.context(span);
        env.device.restore(&ctx);
    }

    fn set_ext_gstate(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let Some(name) = self.name_operand(operation) else {
            return Ok(());
        };
        let Some(entry) = self.lookup(b"ExtGState", &name)? else {
            self.missing(b"ExtGState", &name, operation);
            return Ok(());
        };
        let resolved = self.doc.resolve(&entry)?;
        let Some(dictionary) = resolved.dictionary().cloned() else {
            self.invalid(operation);
            return Ok(());
        };
        for (key, value) in dictionary.iter() {
            self.apply_ext_gstate_entry(key, value, operation)?;
        }
        let info = ExtGStateInfo { name, dictionary };
        let ctx = self.context(span);
        env.device.ext_gstate(&ctx, &info);
        Ok(())
    }

    /// One entry of an extended graphics state dictionary (Table 58).
    fn apply_ext_gstate_entry(
        &mut self,
        key: &Name,
        value: &Object,
        operation: &Operation,
    ) -> Result<(), PdfError> {
        // The transfer, halftone, black-generation and colour-removal entries
        // (8.4.5) are not acted on, and are often functions, which are
        // streams that `resolve` does not follow: they are not read at all.
        if matches!(
            key.as_bytes(),
            b"BG" | b"BG2" | b"UCR" | b"UCR2" | b"TR" | b"TR2" | b"HT" | b"SM"
        ) {
            return Ok(());
        }
        let doc = self.doc.clone();
        let resolved = doc.resolve(value)?;
        let object = resolved.object();
        let number = operand_number(object);
        let flag = match object.value() {
            Value::Boolean(flag) => Some(*flag),
            _ => None,
        };
        let applied = match key.as_bytes() {
            b"LW" => number.map(|value| self.state.line_width = value).is_some(),
            b"LC" => number
                .and_then(|code| LineCap::from_code(code as i64))
                .map(|cap| self.state.line_cap = cap)
                .is_some(),
            b"LJ" => number
                .and_then(|code| LineJoin::from_code(code as i64))
                .map(|join| self.state.line_join = join)
                .is_some(),
            b"ML" => number.map(|value| self.state.miter_limit = value).is_some(),
            b"D" => self.ext_dash(object)?,
            b"RI" => match object.value() {
                Value::Name(name) => {
                    self.state.rendering_intent = name.clone();
                    true
                }
                _ => false,
            },
            b"FL" => number.map(|value| self.state.flatness = value).is_some(),
            b"SA" => flag
                .map(|value| self.state.stroke_adjustment = value)
                .is_some(),
            b"CA" => number
                .map(|value| self.state.stroke_alpha = value.clamp(0.0, 1.0))
                .is_some(),
            b"ca" => number
                .map(|value| self.state.fill_alpha = value.clamp(0.0, 1.0))
                .is_some(),
            b"AIS" => flag
                .map(|value| self.state.alpha_is_shape = value)
                .is_some(),
            b"TK" => flag.map(|value| self.state.text_knockout = value).is_some(),
            b"OP" => flag
                .map(|value| {
                    self.state.stroke_overprint = value;
                })
                .is_some(),
            b"op" => flag
                .map(|value| self.state.fill_overprint = value)
                .is_some(),
            b"OPM" => number
                .map(|value| self.state.overprint_mode = value as i64)
                .is_some(),
            b"BM" => self.ext_blend_mode(object)?,
            b"SMask" => match object.value() {
                Value::Name(name) if name.as_bytes() == b"None" => {
                    self.state.soft_mask = SoftMask::None;
                    true
                }
                Value::Dictionary(_) => {
                    self.state.soft_mask = SoftMask::Dictionary(value.clone());
                    true
                }
                _ => false,
            },
            b"Font" => self.ext_font(object)?,
            // Type, and the entries a renderer does not act on (the
            // transfer, halftone and colour-removal functions, 8.4.5).
            _ => true,
        };
        if !applied {
            self.invalid(operation);
        }
        Ok(())
    }

    fn ext_dash(&mut self, object: &Object) -> Result<bool, PdfError> {
        let Value::Array(items) = object.value() else {
            return Ok(false);
        };
        let [array, phase] = items.as_slice() else {
            return Ok(false);
        };
        let array = self.doc.resolve(array)?;
        let Some(items) = array.array() else {
            return Ok(false);
        };
        let numbers: Option<Vec<f64>> = items.iter().map(operand_number).collect();
        match (numbers, operand_number(phase)) {
            (Some(array), Some(phase)) if array.iter().all(|length| *length >= 0.0) => {
                self.state.dash = Dash { array, phase };
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn ext_blend_mode(&mut self, object: &Object) -> Result<bool, PdfError> {
        let chosen = match object.value() {
            Value::Name(name) => Some(name.clone()),
            // An array lists modes in order of preference (11.6.3): the
            // first one this consumer knows is used, and the first when it
            // knows none.
            Value::Array(items) => {
                let mut names = Vec::with_capacity(items.len());
                for item in items {
                    if let Value::Name(name) = self.doc.resolve(item)?.value() {
                        names.push(name.clone());
                    }
                }
                names
                    .iter()
                    .find(|name| KNOWN_BLEND_MODES.contains(&name.as_bytes()))
                    .or(names.first())
                    .cloned()
            }
            _ => None,
        };
        match chosen {
            Some(name) => {
                self.state.blend_mode = name;
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// `Font` of an extended graphics state: an array of a font and a size.
    fn ext_font(&mut self, object: &Object) -> Result<bool, PdfError> {
        let Value::Array(items) = object.value() else {
            return Ok(false);
        };
        let [font, size] = items.as_slice() else {
            return Ok(false);
        };
        let Some(size) = operand_number(self.doc.resolve(size)?.object()) else {
            return Ok(false);
        };
        let Some(loaded) = self.load_font(font, None)? else {
            return Ok(false);
        };
        self.state.text.font = Some(TextFont {
            name: Name::new(Vec::new()),
            font: loaded,
            size,
        });
        Ok(true)
    }

    // --- resources ---

    /// The entry `name` of the resource category `category`, as the
    /// resource dictionaries give it (maybe a reference). The innermost
    /// stream's resources are looked in first; a form without a resource
    /// dictionary of its own uses the enclosing one's (7.8.3).
    pub(super) fn lookup(&self, category: &[u8], name: &Name) -> Result<Option<Object>, PdfError> {
        let Some(resources) = self.resources.last() else {
            return Ok(None);
        };
        let Some(entry) = resources.get(category) else {
            return Ok(None);
        };
        let resolved = self.doc.resolve(entry)?;
        let Some(dictionary) = resolved.dictionary() else {
            return Ok(None);
        };
        Ok(dictionary.get(name.as_bytes()).cloned())
    }

    /// An inline image whose `ColorSpace` is the name of a colour space
    /// resource (8.9.7, Table 93) with that resource in its place, so that
    /// the device need not know which resources were current.
    fn inline_in_resources(&self, image: &InlineImage) -> Result<InlineImage, PdfError> {
        let mut image = image.clone();
        let Some(Value::Name(name)) = image.dictionary.get(b"ColorSpace").map(Object::value) else {
            return Ok(image);
        };
        if matches!(
            name.as_bytes(),
            b"DeviceGray" | b"DeviceRGB" | b"DeviceCMYK" | b"Pattern"
        ) {
            return Ok(image);
        }
        let name = name.clone();
        if let Some(object) = self.lookup(b"ColorSpace", &name)? {
            image
                .dictionary
                .insert(Name::new(b"ColorSpace".to_vec()), object);
        }
        Ok(image)
    }

    /// Reads the font `object` names or gives, once for each indirect
    /// font. A font that cannot be read is `None` and a diagnostic.
    pub(super) fn load_font(
        &mut self,
        object: &Object,
        name: Option<&Name>,
    ) -> Result<Option<Arc<Font>>, PdfError> {
        let id = match object.value() {
            Value::Reference(id) => Some(*id),
            _ => None,
        };
        if let Some(font) = id.and_then(|id| self.fonts.get(&id)) {
            return Ok(Some(font.clone()));
        }
        match self.doc.font(object) {
            Ok(font) => {
                let font = Arc::new(font);
                if let Some(id) = id {
                    self.fonts.insert(id, font.clone());
                }
                Ok(Some(font))
            }
            Err(error @ (PdfError::Fs(_) | PdfError::Cancelled)) => Err(error),
            Err(_) => {
                self.diagnostics.push(
                    DiagnosticKind::FontUnreadable {
                        name: name.cloned().unwrap_or_else(|| Name::new(Vec::new())),
                    },
                    0,
                );
                Ok(None)
            }
        }
    }

    // --- path ---

    fn construct_path(&mut self, operation: &Operation, span: OperatorSpan) {
        let operator = operation.operator.as_slice();
        let points = match operator {
            b"h" => {
                if !operation.operands.is_empty() {
                    return self.invalid(operation);
                }
                if self.path.current.is_none() {
                    return self.misplaced(operation);
                }
                self.path.first.get_or_insert(span);
                self.path.path.segments.push(PathSegment::Close);
                self.path.current = self.path.start;
                return;
            }
            b"m" | b"l" => self.numbers::<2>(operation).map(|p| p.to_vec()),
            b"v" | b"y" => self.numbers::<4>(operation).map(|p| p.to_vec()),
            b"c" => self.numbers::<6>(operation).map(|p| p.to_vec()),
            _ => self.numbers::<4>(operation).map(|p| p.to_vec()),
        };
        let Some(p) = points else {
            return;
        };
        self.path.first.get_or_insert(span);
        let segments = &mut self.path.path.segments;
        match operator {
            b"m" => {
                segments.push(PathSegment::MoveTo(p[0], p[1]));
                self.path.current = Some((p[0], p[1]));
                self.path.start = self.path.current;
            }
            b"re" => {
                let (x, y, w, h) = (p[0], p[1], p[2], p[3]);
                segments.push(PathSegment::MoveTo(x, y));
                segments.push(PathSegment::LineTo(x + w, y));
                segments.push(PathSegment::LineTo(x + w, y + h));
                segments.push(PathSegment::LineTo(x, y + h));
                segments.push(PathSegment::Close);
                self.path.current = Some((x, y));
                self.path.start = self.path.current;
            }
            _ => {
                let Some((cx, cy)) = self.path.current else {
                    // A segment needs a current point (8.5.2.1).
                    self.path.first = if self.path.path.is_empty() {
                        None
                    } else {
                        self.path.first
                    };
                    return self.misplaced(operation);
                };
                match operator {
                    b"l" => {
                        segments.push(PathSegment::LineTo(p[0], p[1]));
                        self.path.current = Some((p[0], p[1]));
                    }
                    b"c" => {
                        segments.push(PathSegment::CurveTo(p[0], p[1], p[2], p[3], p[4], p[5]));
                        self.path.current = Some((p[4], p[5]));
                    }
                    // `v`: the first control point is the current point.
                    b"v" => {
                        segments.push(PathSegment::CurveTo(cx, cy, p[0], p[1], p[2], p[3]));
                        self.path.current = Some((p[2], p[3]));
                    }
                    // `y`: the second control point is the end point.
                    _ => {
                        segments.push(PathSegment::CurveTo(p[0], p[1], p[2], p[3], p[2], p[3]));
                        self.path.current = Some((p[2], p[3]));
                    }
                }
            }
        }
    }

    fn paint_path(&mut self, operation: &Operation, span: OperatorSpan, env: &mut Env<'_>) {
        if !operation.operands.is_empty() {
            return self.invalid(operation);
        }
        let operator = operation.operator.as_slice();
        let close = matches!(operator, b"s" | b"b" | b"b*");
        if close && self.path.current.is_some() {
            self.path.path.segments.push(PathSegment::Close);
        }
        let paint = match operator {
            b"S" | b"s" => PathPaint {
                fill: None,
                stroke: true,
            },
            b"f" | b"F" => PathPaint {
                fill: Some(FillRule::NonZeroWinding),
                stroke: false,
            },
            b"f*" => PathPaint {
                fill: Some(FillRule::EvenOdd),
                stroke: false,
            },
            b"B" | b"b" => PathPaint {
                fill: Some(FillRule::NonZeroWinding),
                stroke: true,
            },
            b"B*" | b"b*" => PathPaint {
                fill: Some(FillRule::EvenOdd),
                stroke: true,
            },
            _ => PathPaint {
                fill: None,
                stroke: false,
            },
        };
        let builder = std::mem::take(&mut self.path);
        if builder.path.is_empty() {
            return;
        }
        let first = builder.first.unwrap_or(span);
        let construction = OperatorSpan {
            stream: span.stream,
            range: Span {
                start: first.range.start,
                end: span.range.end,
            },
            index: first.index,
        };
        if paint.fill.is_some() || paint.stroke {
            let ctx = self.context(span);
            env.device.path(&ctx, &builder.path, paint, construction);
        }
        if let Some(rule) = builder.clip {
            let ctx = self.context(span);
            env.device.clip(&ctx, &builder.path, rule);
        }
    }

    // --- shading ---

    fn paint_shading(
        &mut self,
        operation: &Operation,
        span: OperatorSpan,
        env: &mut Env<'_>,
    ) -> Result<(), PdfError> {
        let Some(name) = self.name_operand(operation) else {
            return Ok(());
        };
        let Some(object) = self.lookup(b"Shading", &name)? else {
            self.missing(b"Shading", &name, operation);
            return Ok(());
        };
        let ctx = self.context(span);
        env.device.shading(&ctx, &name, &object);
        Ok(())
    }

    // --- colour ---

    fn set_colour_space(&mut self, operation: &Operation) {
        let stroke = operation.operator == b"CS";
        let Some(name) = self.name_operand(operation) else {
            return;
        };
        let space = match name.as_bytes() {
            b"DeviceGray" | b"G" => ColorSpace::DeviceGray,
            b"DeviceRGB" | b"RGB" => ColorSpace::DeviceRGB,
            b"DeviceCMYK" | b"CMYK" => ColorSpace::DeviceCMYK,
            b"Pattern" => ColorSpace::Pattern,
            _ => match self.lookup(b"ColorSpace", &name) {
                Ok(Some(object)) => ColorSpace::Resource { name, object },
                Ok(None) => return self.missing(b"ColorSpace", &name, operation),
                Err(_) => return self.invalid(operation),
            },
        };
        let components = self.initial_components(&space);
        let paint = Paint {
            space,
            components,
            pattern: None,
        };
        if stroke {
            self.state.stroke = paint;
        } else {
            self.state.fill = paint;
        }
    }

    /// The colour a new colour space starts with (8.6.5.1 to 8.6.6.5): zero
    /// in each component, except DeviceCMYK's black and the full tint
    /// (1.0) of a Separation or DeviceN space. A space whose component
    /// count is not told by its definition has none yet.
    fn initial_components(&self, space: &ColorSpace) -> Vec<f64> {
        match space {
            ColorSpace::DeviceGray => vec![0.0],
            ColorSpace::DeviceRGB => vec![0.0; 3],
            ColorSpace::DeviceCMYK => vec![0.0, 0.0, 0.0, 1.0],
            ColorSpace::Pattern => Vec::new(),
            ColorSpace::Resource { object, .. } => {
                self.definition_components(object).unwrap_or_default()
            }
        }
    }

    fn definition_components(&self, object: &Object) -> Option<Vec<f64>> {
        let resolved = self.doc.resolve(object).ok()?;
        match resolved.value() {
            Value::Name(name) => match name.as_bytes() {
                b"DeviceGray" | b"CalGray" => Some(vec![0.0]),
                b"DeviceRGB" | b"CalRGB" => Some(vec![0.0; 3]),
                b"DeviceCMYK" => Some(vec![0.0, 0.0, 0.0, 1.0]),
                _ => None,
            },
            Value::Array(items) => {
                let family = match self.doc.resolve(items.first()?).ok()?.value() {
                    Value::Name(name) => name.clone(),
                    _ => return None,
                };
                match family.as_bytes() {
                    b"CalGray" | b"Indexed" | b"I" => Some(vec![0.0]),
                    b"CalRGB" | b"Lab" => Some(vec![0.0; 3]),
                    b"Separation" => Some(vec![1.0]),
                    b"DeviceN" => {
                        let names = self.doc.resolve(items.get(1)?).ok()?;
                        Some(vec![1.0; names.array()?.len()])
                    }
                    b"ICCBased" => {
                        let Value::Reference(id) = items.get(1)?.value() else {
                            return None;
                        };
                        let stream = self.doc.object(*id).ok()??;
                        let count = match stream.dictionary()?.get(b"N")?.value() {
                            Value::Integer(count) => usize::try_from(*count).ok()?,
                            _ => return None,
                        };
                        Some(vec![0.0; count])
                    }
                    b"DeviceGray" | b"G" => Some(vec![0.0]),
                    b"DeviceRGB" | b"RGB" => Some(vec![0.0; 3]),
                    b"DeviceCMYK" | b"CMYK" => Some(vec![0.0, 0.0, 0.0, 1.0]),
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn set_colour(&mut self, operation: &Operation) -> Result<(), PdfError> {
        let stroke = matches!(operation.operator.as_slice(), b"SC" | b"SCN");
        let is_n = matches!(operation.operator.as_slice(), b"SCN" | b"scn");
        let mut operands = operation.operands.as_slice();
        let mut pattern_name = None;
        if let Some((last, rest)) = operands.split_last() {
            if let Value::Name(name) = last.value() {
                if !is_n {
                    self.invalid(operation);
                    return Ok(());
                }
                pattern_name = Some(name.clone());
                operands = rest;
            }
        }
        let components: Option<Vec<f64>> = operands.iter().map(operand_number).collect();
        let Some(components) = components else {
            self.invalid(operation);
            return Ok(());
        };
        let current = if stroke {
            &self.state.stroke
        } else {
            &self.state.fill
        };
        let space = current.space.clone();
        let pattern = match pattern_name {
            Some(name) => {
                if !matches!(space, ColorSpace::Pattern) && !is_pattern_resource(&space) {
                    self.misplaced(operation);
                    return Ok(());
                }
                match self.lookup(b"Pattern", &name)? {
                    Some(object) => Some((name, object)),
                    None => {
                        self.missing(b"Pattern", &name, operation);
                        return Ok(());
                    }
                }
            }
            None => None,
        };
        let paint = Paint {
            space,
            components,
            pattern,
        };
        if stroke {
            self.state.stroke = paint;
        } else {
            self.state.fill = paint;
        }
        Ok(())
    }

    fn set_device_colour(&mut self, operation: &Operation) {
        let (stroke, space, count) = match operation.operator.as_slice() {
            b"G" => (true, ColorSpace::DeviceGray, 1),
            b"g" => (false, ColorSpace::DeviceGray, 1),
            b"RG" => (true, ColorSpace::DeviceRGB, 3),
            b"rg" => (false, ColorSpace::DeviceRGB, 3),
            b"K" => (true, ColorSpace::DeviceCMYK, 4),
            _ => (false, ColorSpace::DeviceCMYK, 4),
        };
        let components: Option<Vec<f64>> = (operation.operands.len() == count)
            .then(|| operation.operands.iter().map(operand_number).collect())
            .flatten();
        let Some(components) = components else {
            return self.invalid(operation);
        };
        let paint = Paint {
            space,
            components,
            pattern: None,
        };
        if stroke {
            self.state.stroke = paint;
        } else {
            self.state.fill = paint;
        }
    }
}

/// Whether a resource colour space is a Pattern space with an underlying
/// space (`[/Pattern base]`, 8.6.6.2).
fn is_pattern_resource(space: &ColorSpace) -> bool {
    let ColorSpace::Resource { object, .. } = space else {
        return false;
    };
    match object.value() {
        Value::Array(items) => matches!(
            items.first().map(Object::value),
            Some(Value::Name(name)) if name.as_bytes() == b"Pattern"
        ),
        _ => false,
    }
}

/// The render mode named by a `Tr` operand.
pub(super) fn render_mode_of(code: f64) -> Option<RenderMode> {
    RenderMode::from_code(code as i64)
}
