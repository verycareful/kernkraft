// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Editing the text of a page as Acrobat does: a block of the text layer is
//! taken out of the content stream and new text is set in its place.
//! ISO 32000-1:2008, 9.4 (text objects), 9.4.3 (text-showing operators),
//! 9.3 (text state parameters), 9.3.6 (text rendering mode) and 7.8.2
//! (content streams).
//!
//! An edit splices the decoded content stream at the byte ranges of the
//! operations it changes and copies every other byte as it was, so what is
//! not edited is not re-serialised. The changed stream is written as a new
//! object and the page's `Contents` is pointed at it, so a stream or a
//! `Resources` dictionary that other pages share is never changed under
//! them.
//!
//! The text layer leaves out the glyphs that lie wholly off the page, so no
//! block holds them. An edit of a block takes with it the glyphs of the
//! showing operations the block's glyphs are in that lie wholly off the page:
//! they are the rest of the line the block is. Text that is hidden by an
//! optional content group, or is shown by an operation none of the block's
//! glyphs are in, is other text, and stays. So does text that has no size:
//! the text layer holds it in a block of its own, which is refused
//! ([`EditBlocker::NoSize`]).
//!
//! [`Document::text_block_editability`] says whether a block of a
//! [`TextPage`](crate::text::TextPage) can be edited, and when it cannot,
//! why ([`EditBlocker`]). [`delete_text_block`] takes a block's text out
//! and [`edit_text`] puts new text in its place: set in the block's font
//! when that font can show it, else in a face of the font list handed in;
//! in the block's width, after the `ET` of its last text object.

mod add;
mod arrange;
mod collect;
mod face;
mod insert;
mod mirrored_table;
mod splice;
mod typeset;

pub use add::{add_text, block_style, blocks_by_distance, BlockStyle, Follow, NewText, TextAdded};

use std::collections::{HashMap, HashSet};

use kernkraft_fs::Progress;

use kernkraft_font::{FontList, Query};

use crate::build::ContentBuilder;
use crate::colour::Rgb;
use crate::content::{
    ContentParser, ContentStream, GraphicsState, InterpretOptions, Interpreter, Matrix, StreamId,
    DEFAULT_MAX_CONTENT_BYTES, GLYPH_UNITS_PER_EM,
};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::display::PageView;
use crate::doc::{Document, Page, StreamEncoding};
use crate::edit_text::collect::{FormUse, OpKey, ShowCollector, ShowOp, TextEnd};
use crate::edit_text::face::{Embedded, Face};
use crate::font::FontKind;
use crate::pages::rewrite::{direct, reference_object, Rewriter};
use crate::source::MemorySource;
use crate::syntax::{Dictionary, Name, ObjectId, Value};
use crate::text::collect::lies_off_page;
use crate::text::{GlyphSource, TextBlock};
use crate::PdfError;

/// Why a block cannot be edited. Each reason is a fact about the page, not
/// a limit of the editor that a later version lifts, except where it says
/// so.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditBlocker {
    /// The block has no glyph that was shown by an operator: it holds only
    /// spaces the layout read between glyphs.
    NoGlyphs,
    /// The block's text has no size (a font size of 0, or a text matrix
    /// that collapses it): nothing of it is drawn, so there is no text on
    /// the page to change, and the glyphs stay in the file.
    NoSize,
    /// An operation that showed the block's glyphs is not on the page as
    /// the document now has it: the block comes from another snapshot.
    NotOnPage,
    /// The text is set in a Type 3 font, whose glyphs are content streams
    /// (9.6.5): no character can be typeset in it.
    Type3Font,
    /// The render mode adds the glyphs to the clipping path (9.3.6, modes
    /// 4 to 7). Taking the glyphs out changes what every later object on
    /// the page is clipped to.
    ClippingRenderMode,
    /// The glyphs of the block were shown by operations in more than one
    /// content stream.
    SplitAcrossStreams,
    /// One operation that showed the block's glyphs starts in one stream
    /// of the page's `Contents` and ends in another (7.8.2 allows a break
    /// between any two tokens).
    StraddlesStreams,
    /// The text is written vertically (9.7.4.3); new text is set along the
    /// line.
    VerticalWriting,
    /// A text object of the block has no `ET` before the stream ends or
    /// another text object begins, so there is no place after it for new
    /// text.
    UnterminatedTextObject,
    /// The transparency, overprint or (for stroked text) line parameters,
    /// or the clipping path, differ between the block's first glyph and
    /// the `ET` that closes its last text object, so new text put after
    /// the `ET` would not look like the block did.
    StateChangedBeforeEnd,
    /// The block is in a form XObject and in marked content with an `MCID`.
    /// The structure tree names the form's content by the form object and
    /// the `MCID` (14.7.4.2, `Stm`), and an edited copy of the form is a new
    /// object the tree does not name.
    TaggedFormContent,
}

/// Whether a block can be edited.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editability {
    Editable,
    Blocked(EditBlocker),
}

/// What an edit of a block works from.
struct Inspected {
    /// The glyphs of the block, in the order the text layer gives them.
    sources: Vec<GlyphSource>,
    /// The showing operations they come from.
    ops: HashMap<OpKey, ShowOp>,
    /// For each of those operations, the glyphs it shows that lie wholly off
    /// the page, as `(operand, offset)` like a [`GlyphSource`]'s. The text
    /// layer leaves them out, so no block holds them; they are the rest of
    /// the line the block is, and an edit takes them with it.
    off_page: HashMap<OpKey, HashSet<(u32, u32)>>,
}

/// The key of the operation that showed a glyph.
fn key_of(source: &GlyphSource) -> OpKey {
    (source.span.stream, source.span.index, source.invocation)
}

impl Inspected {
    /// The forms the block's text is in, outermost first; empty for text
    /// in the page's own content.
    fn chain(&self) -> &[FormUse] {
        &self.ops[&key_of(&self.sources[0])].chain
    }
}

/// Finds the operations that showed `block` and checks that they can be
/// edited.
fn inspect(
    doc: &Document,
    page_number: u32,
    block: &TextBlock,
    progress: &dyn Progress,
) -> Result<Result<Inspected, EditBlocker>, PdfError> {
    if block
        .lines
        .iter()
        .any(|line| line.chars.iter().any(|ch| ch.sizeless))
    {
        return Ok(Err(EditBlocker::NoSize));
    }
    let page = doc.page(page_number)?;
    let sources: Vec<GlyphSource> = block
        .lines
        .iter()
        .flat_map(|line| line.visual_chars())
        .filter_map(|ch| ch.source)
        .collect();
    if sources.is_empty() {
        return Ok(Err(EditBlocker::NoGlyphs));
    }
    let wanted: HashSet<OpKey> = sources.iter().map(key_of).collect();
    let stream = sources[0].span.stream;
    if sources
        .iter()
        .any(|source| source.span.stream != stream || source.invocation != sources[0].invocation)
    {
        return Ok(Err(EditBlocker::SplitAcrossStreams));
    }
    let mut collector = ShowCollector::new(wanted.clone());
    Interpreter::new(doc, InterpretOptions::default())?.run_page(
        &page,
        &mut collector,
        progress,
    )?;
    let ops = collector.ops;

    let mut seen: Vec<&ShowOp> = Vec::new();
    for key in &wanted {
        match ops.get(key) {
            Some(op) => seen.push(op),
            None => return Ok(Err(EditBlocker::NotOnPage)),
        }
    }
    // The spans must still name the bytes they were read from.
    for source in &sources {
        let op = &ops[&key_of(source)];
        if op.span != source.span {
            return Ok(Err(EditBlocker::NotOnPage));
        }
    }
    for op in &seen {
        if op.kind == FontKind::Type3 {
            return Ok(Err(EditBlocker::Type3Font));
        }
        if op.render_mode.clips() {
            return Ok(Err(EditBlocker::ClippingRenderMode));
        }
        if op.tagged && !op.chain.is_empty() {
            return Ok(Err(EditBlocker::TaggedFormContent));
        }
    }
    // Every operation the edit changes, in the page's content: the
    // showing operations, and the `Do` that invokes the outermost form.
    if stream == StreamId::Page(page.id) || !seen[0].chain.is_empty() {
        let content = doc.content_stream(StreamId::Page(page.id), DEFAULT_MAX_CONTENT_BYTES)?;
        let spans = seen
            .iter()
            .filter(|op| op.span.stream == StreamId::Page(page.id))
            .map(|op| op.span)
            .chain(
                seen[0]
                    .chain
                    .first()
                    .map(|used| used.invoked_by)
                    .filter(|span| span.stream == StreamId::Page(page.id)),
            );
        for span in spans {
            let start = span.range.start as usize;
            let last = (span.range.end as usize).saturating_sub(1);
            if content.locate(start).map(|(object, _)| object)
                != content.locate(last).map(|(object, _)| object)
            {
                return Ok(Err(EditBlocker::StraddlesStreams));
            }
        }
    }
    let view = PageView::of(&page, &mut Diagnostics::new());
    let off_page = ops
        .iter()
        .map(|(key, op)| (*key, glyphs_off_page(&view, op)))
        .collect();
    Ok(Ok(Inspected {
        sources,
        ops,
        off_page,
    }))
}

/// The glyphs of `show` that lie wholly off the page, as `(operand,
/// offset)`.
fn glyphs_off_page(view: &PageView, show: &ShowOp) -> HashSet<(u32, u32)> {
    let mut off = HashSet::new();
    let mut at = (0, 0);
    for glyph in &show.run.glyphs {
        if glyph.operand != at.0 {
            at = (glyph.operand, 0);
        }
        if lies_off_page(view, &show.run, glyph) {
            off.insert(at);
        }
        at.1 += glyph.len as u32;
    }
    off
}

impl Document {
    /// Whether `block`, a block of the text of page number `page` read
    /// from this document, can be edited by [`edit_text`], and if not,
    /// why. [`delete_text_block`] needs less of the block: it does not
    /// refuse a block for the reasons that concern new text, which are
    /// [`EditBlocker::VerticalWriting`],
    /// [`EditBlocker::UnterminatedTextObject`] and
    /// [`EditBlocker::StateChangedBeforeEnd`].
    pub fn text_block_editability(
        &self,
        page: u32,
        block: &TextBlock,
        progress: &dyn Progress,
    ) -> Result<Editability, PdfError> {
        Ok(match inspect(self, page, block, progress)? {
            Ok(inspected) => match insertion(&inspected) {
                Ok(_) => Editability::Editable,
                Err(blocker) => Editability::Blocked(blocker),
            },
            Err(blocker) => Editability::Blocked(blocker),
        })
    }
}

/// The bytes `start..end` of a decoded content stream, replaced by `bytes`.
struct Splice {
    start: usize,
    end: usize,
    bytes: Vec<u8>,
}

/// The splices that take the glyphs of `inspected` out of `content`.
fn removals(inspected: &Inspected, content: &ContentStream) -> Result<Vec<Splice>, PdfError> {
    let mut by_op: HashMap<OpKey, HashSet<(u32, u32)>> = HashMap::new();
    for source in &inspected.sources {
        by_op
            .entry(key_of(source))
            .or_default()
            .insert((source.operand, source.offset));
    }
    let mut splices = Vec::with_capacity(by_op.len());
    for (key, removed) in &mut by_op {
        if let Some(off_page) = inspected.off_page.get(key) {
            removed.extend(off_page);
        }
    }
    for (key, removed) in by_op {
        let show = &inspected.ops[&key];
        let (start, end) = (show.span.range.start as usize, show.span.range.end as usize);
        let Some(bytes) = content.data.get(start..end) else {
            return Err(PdfError::Invalid(
                "the text to edit lies past its content stream".to_owned(),
            ));
        };
        let source = MemorySource::new(bytes.to_vec());
        let mut diagnostics = Diagnostics::new();
        let operation = ContentParser::new(&source).next_operation(&mut diagnostics)?;
        let Some(operation) = operation.filter(|operation| {
            operation.span.start == 0 && operation.span.end == bytes.len() as u64
        }) else {
            return Err(PdfError::Invalid(
                "the bytes of the text to edit are not one operation".to_owned(),
            ));
        };
        splices.push(Splice {
            start,
            end,
            bytes: splice::without_glyphs(show, &operation, &removed)?,
        });
    }
    splices.sort_by_key(|splice| splice.start);
    Ok(splices)
}

/// `content`'s bytes from `from` to `to`, with the `splices` inside them
/// made.
fn spliced(content: &ContentStream, from: usize, to: usize, splices: &[&Splice]) -> Vec<u8> {
    let mut data = Vec::with_capacity(to - from);
    let mut at = from;
    for splice in splices {
        data.extend_from_slice(&content.data[at..splice.start]);
        data.extend_from_slice(&splice.bytes);
        at = splice.end;
    }
    data.extend_from_slice(&content.data[at..to]);
    data
}

/// A new stream object that is the stream object `old` with `data` for its
/// content and, when given, `resources` for its `Resources`.
fn write_stream(
    rewriter: &mut Rewriter<'_>,
    old: ObjectId,
    data: Vec<u8>,
    resources: Option<Dictionary>,
) -> Result<ObjectId, PdfError> {
    let stream = rewriter.document.required(old)?;
    let Some(stream) = stream.stream() else {
        return Err(PdfError::WrongKind { object: old });
    };
    let mut dictionary = stream.dictionary.clone();
    dictionary.remove(b"DL");
    if let Some(resources) = resources {
        dictionary.insert(
            Name::new(&b"Resources"[..]),
            direct(Value::Dictionary(resources)),
        );
    }
    let id = ObjectId {
        number: rewriter.reserve_many(1)?,
        generation: 0,
    };
    rewriter
        .builder()
        .set_stream(id, dictionary, data, StreamEncoding::Flate)?;
    Ok(id)
}

/// Writes `splices` into the streams of the page's `Contents` they fall in,
/// each as a new object, and points the `Contents` of `page` at the new
/// streams; `resources`, when given, become the page's own. Only a stream
/// with a splice is written again.
fn write_page_splices(
    rewriter: &mut Rewriter<'_>,
    page: ObjectId,
    content: &ContentStream,
    splices: &[Splice],
    resources: Option<Dictionary>,
) -> Result<(), PdfError> {
    let mut replaced: HashMap<ObjectId, ObjectId> = HashMap::new();
    for part in &content.parts {
        let (from, to) = (part.start, part.start + part.len);
        let inside: Vec<&Splice> = splices
            .iter()
            .filter(|splice| splice.start >= from && splice.end <= to)
            .collect();
        if inside.is_empty() {
            continue;
        }
        let data = spliced(content, from, to, &inside);
        let id = write_stream(rewriter, part.object, data, None)?;
        replaced.insert(part.object, id);
    }
    if replaced.is_empty() {
        return Ok(());
    }
    let streams = rewriter.document.page_content_streams(page)?;
    let mut items: Vec<_> = streams
        .iter()
        .map(|id| reference_object(*replaced.get(id).unwrap_or(id)))
        .collect();
    let contents = if items.len() == 1 {
        items.remove(0)
    } else {
        direct(Value::Array(items))
    };
    rewriter.edit(page, |_, dictionary| {
        dictionary.insert(Name::new(&b"Contents"[..]), contents);
        if let Some(resources) = resources {
            dictionary.insert(
                Name::new(&b"Resources"[..]),
                direct(Value::Dictionary(resources)),
            );
        }
        Ok(())
    })?;
    Ok(())
}

/// The resources in force `depth` forms deep in the content of `page`:
/// `depth` 0 is the page's own content. A form without `Resources` uses
/// those of the content that invoked it (7.8.3).
fn effective_resources(
    document: &Document,
    page: &Page,
    chain: &[FormUse],
    depth: usize,
) -> Result<Dictionary, PdfError> {
    for used in chain[..depth].iter().rev() {
        let form = document.required(used.id)?;
        if let Some(resources) = form.stream().and_then(|s| s.dictionary.get(b"Resources")) {
            return Ok(document
                .resolve(resources)?
                .dictionary()
                .cloned()
                .unwrap_or_default());
        }
    }
    match &page.resources {
        Some(resources) => Ok(document
            .resolve(resources)?
            .dictionary()
            .cloned()
            .unwrap_or_default()),
        None => Ok(Dictionary::new()),
    }
}

/// Makes the changes `splices` stand for in `content`, the stream that
/// holds the block, and gives the page a way to reach them. Text in the
/// page's own content is written into the page's `Contents`. Text in a
/// form is written into a copy of the form, and each `Do` on the way from
/// the page to it is made to invoke the copy: the form itself, which other
/// pages or other invocations may share, is not changed.
fn commit(
    rewriter: &mut Rewriter<'_>,
    document: &Document,
    page: &Page,
    chain: &[FormUse],
    content: &ContentStream,
    splices: &[Splice],
    resources: Option<Dictionary>,
) -> Result<(), PdfError> {
    let Some(innermost) = chain.last() else {
        return write_page_splices(rewriter, page.id, content, splices, resources);
    };
    let inside: Vec<&Splice> = splices.iter().collect();
    let data = spliced(content, 0, content.data.len(), &inside);
    let mut child = write_stream(rewriter, innermost.id, data, resources)?;
    for depth in (0..chain.len()).rev() {
        let used = chain[depth];
        let parent = if depth == 0 {
            StreamId::Page(page.id)
        } else {
            StreamId::Object(chain[depth - 1].id)
        };
        let parent_content = document.content_stream(parent, DEFAULT_MAX_CONTENT_BYTES)?;
        let effective = effective_resources(document, page, chain, depth)?;
        let (resources, name) =
            insert::with_resource(rewriter, &effective, b"XObject", "KernForm", child)?;
        let mut call = ContentBuilder::new();
        call.xobject(name.as_bytes())?;
        let mut bytes = call.finish();
        while bytes.last().is_some_and(u8::is_ascii_whitespace) {
            bytes.pop();
        }
        let splice = Splice {
            start: used.invoked_by.range.start as usize,
            end: used.invoked_by.range.end as usize,
            bytes,
        };
        if depth == 0 {
            write_page_splices(
                rewriter,
                page.id,
                &parent_content,
                &[splice],
                Some(resources),
            )?;
        } else {
            let data = spliced(&parent_content, 0, parent_content.data.len(), &[&splice]);
            child = write_stream(rewriter, chain[depth - 1].id, data, Some(resources))?;
        }
    }
    Ok(())
}

/// Takes the text of `block` out of page `page` (counted from 1) and gives
/// the document without it. Everything else on the page stays where it is:
/// each glyph taken out is made up for in the operation that showed it, so
/// the text after it is not moved. The glyphs of those operations that lie
/// wholly off the page go with it (see the module).
///
/// A block that [`Document::text_block_editability`] refuses is
/// [`PdfError::TextNotEditable`].
pub fn delete_text_block(
    document: &Document,
    page: u32,
    block: &TextBlock,
    progress: &dyn Progress,
) -> Result<Document, PdfError> {
    let inspected = match inspect(document, page, block, progress)? {
        Ok(inspected) => inspected,
        Err(reason) => return Err(PdfError::TextNotEditable { reason }),
    };
    let page = document.page(page)?;
    let content =
        document.content_stream(inspected.sources[0].span.stream, DEFAULT_MAX_CONTENT_BYTES)?;
    let splices = removals(&inspected, &content)?;
    let mut rewriter = Rewriter::new(document);
    commit(
        &mut rewriter,
        document,
        &page,
        inspected.chain(),
        &content,
        &splices,
        None,
    )?;
    document.apply(rewriter.finish())
}

/// How new text looks. What is left out is the block's own: its font (or,
/// when that cannot show the text, the closest face of the font list), its
/// size and its colour.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct TextStyle {
    /// A family of the font list. The text is set in its closest face to
    /// the block's weight and slant, which is embedded.
    pub family: Option<String>,
    /// The font size, in text space units (points on an unscaled page).
    pub size: Option<f64>,
    /// The colour of the text, in DeviceRGB.
    pub colour: Option<Rgb>,
}

impl TextStyle {
    pub fn with_family(mut self, family: impl Into<String>) -> Self {
        self.family = Some(family.into());
        self
    }

    pub fn with_size(mut self, size: f64) -> Self {
        self.size = Some(size);
        self
    }

    pub fn with_colour(mut self, colour: Rgb) -> Self {
        self.colour = Some(colour);
        self
    }
}

/// The new text of a block.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct TextEdit<'f> {
    /// The text. A line feed ends a line; a line also ends where the next
    /// word would pass the width of the block. Empty text deletes the
    /// block.
    pub text: String,
    pub style: TextStyle,
    /// The faces a font can be taken from when the block's own font cannot
    /// show the text, or the style names a family. A face is embedded
    /// whole as a subset of the glyphs the text uses.
    pub fonts: Option<&'f FontList>,
}

impl<'f> TextEdit<'f> {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            style: TextStyle::default(),
            fonts: None,
        }
    }

    pub fn with_style(mut self, style: TextStyle) -> Self {
        self.style = style;
        self
    }

    pub fn with_fonts(mut self, fonts: &'f FontList) -> Self {
        self.fonts = Some(fonts);
        self
    }
}

/// The result of [`edit_text`].
#[derive(Debug, Clone)]
pub struct Edited {
    /// The document with the new text, for
    /// [`EditSession::push`](crate::EditSession::push).
    pub document: Document,
    /// How many lines the new text took.
    pub lines: usize,
    /// The fonts of the block that were replaced by one of the font list
    /// for this edit: empty when the text was set in the block's own font or
    /// the style named the family. Each is also a
    /// [`FontSubstituted`](crate::DiagnosticKind::FontSubstituted)
    /// diagnostic of `document`, but a diagnostic already logged is not
    /// logged again, so this is the answer for this edit alone.
    pub substitutions: Vec<FontSubstitution>,
}

/// A font of a block that an edit replaced with a face of the font list.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FontSubstitution {
    /// The `BaseFont` name of the font the block was shown in.
    pub name: Name,
    /// The family of the face that was used instead.
    pub family: String,
}

/// Whether two graphics states paint alike, in what a text object leaves
/// out of its own: the transparency and overprint parameters, and for
/// stroked text the line parameters.
fn same_painting(a: &GraphicsState, b: &GraphicsState) -> bool {
    let transparency = a.fill_alpha == b.fill_alpha
        && a.stroke_alpha == b.stroke_alpha
        && a.blend_mode == b.blend_mode
        && a.soft_mask == b.soft_mask
        && a.alpha_is_shape == b.alpha_is_shape
        && a.text_knockout == b.text_knockout
        && a.stroke_overprint == b.stroke_overprint
        && a.fill_overprint == b.fill_overprint
        && a.overprint_mode == b.overprint_mode;
    let lines = !a.text.render_mode.strokes()
        || (a.line_width == b.line_width
            && a.line_cap == b.line_cap
            && a.line_join == b.line_join
            && a.miter_limit == b.miter_limit
            && a.dash == b.dash);
    transparency && lines
}

/// Where and how new text goes.
struct Insertion {
    /// The offset in the decoded stream right after the `ET` of the text
    /// object that holds the block's last glyph.
    at: usize,
    /// The matrix to concatenate to the CTM there so that it is the CTM the
    /// block's first glyph was shown under (`cm` may stand between two text
    /// objects, 8.3.4); `None` when they are the same.
    cm: Option<Matrix>,
}

fn insertion(inspected: &Inspected) -> Result<Insertion, EditBlocker> {
    let ops = || inspected.ops.values();
    if ops().any(|op| op.run.font.is_vertical()) {
        return Err(EditBlocker::VerticalWriting);
    }
    if ops().any(|op| op.text_end.is_none()) {
        return Err(EditBlocker::UnterminatedTextObject);
    }
    let Some(last) = ops().max_by_key(|op| op.span.range.start) else {
        return Err(EditBlocker::NoGlyphs);
    };
    let Some(TextEnd { span, state, clips }) = &last.text_end else {
        return Err(EditBlocker::UnterminatedTextObject);
    };
    let first = &inspected.ops[&key_of(&inspected.sources[0])];
    if !same_painting(&first.state, state) || first.clips != *clips {
        return Err(EditBlocker::StateChangedBeforeEnd);
    }
    let cm = if first.state.ctm == state.ctm {
        None
    } else {
        // M x (CTM at the ET) = (CTM at the first glyph).
        let Some(inverse) = state.ctm.inverse() else {
            return Err(EditBlocker::StateChangedBeforeEnd);
        };
        Some(first.state.ctm.then(inverse))
    };
    Ok(Insertion {
        at: span.range.end as usize,
        cm,
    })
}

/// The glyph of `op` that starts at byte `offset` of string `operand`.
fn glyph_at(
    op: &ShowOp,
    operand: u32,
    offset: u32,
) -> Result<&crate::content::PlacedGlyph, PdfError> {
    let mut at = (u32::MAX, 0u32);
    for glyph in &op.run.glyphs {
        if glyph.operand != at.0 {
            at = (glyph.operand, 0);
        }
        if glyph.operand == operand && at.1 == offset {
            return Ok(glyph);
        }
        at.1 += glyph.len as u32;
    }
    Err(PdfError::Invalid(
        "a glyph of the text to edit is not in its operation".to_owned(),
    ))
}

/// Where and how wide the block is, in the text space of its first glyph.
struct Frame {
    /// The text matrix of the first glyph (9.4.2).
    first: Matrix,
    /// The width of the widest line, in text space units.
    width: f64,
    /// How far the block reaches to the right of the first glyph's origin,
    /// in text space units: where a right-to-left paragraph is set against.
    right: f64,
    /// The distance from the baseline of the first line to that of the
    /// second, along text space's `-y`, in text space units, when the block
    /// has two. It is negative when the second line is further along `+y`,
    /// as in text mirrored top to bottom, where lines read from the top of
    /// the page go against text space's `-y`; new lines keep that
    /// direction.
    pitch: Option<f64>,
    /// The size the glyphs were set at.
    size: f64,
}

fn frame_of(inspected: &Inspected, block: &TextBlock) -> Result<Frame, PdfError> {
    let invalid = |what: &str| PdfError::Invalid(format!("the text to edit: {what}"));
    let key = key_of;
    let first_source = inspected.sources[0];
    let op = &inspected.ops[&key(&first_source)];
    let first = glyph_at(op, first_source.operand, first_source.offset)?;
    let text = &op.state.text;
    let size = op.run.font_size;
    let sm = Matrix::new(
        size * text.horizontal_scaling,
        0.0,
        0.0,
        size,
        0.0,
        text.rise,
    );
    let ctm = op.state.ctm;
    let (Some(sm_inverse), Some(ctm_inverse)) = (sm.inverse(), ctm.inverse()) else {
        return Err(invalid("a text matrix that cannot be inverted"));
    };
    let tm = sm_inverse.then(first.matrix).then(ctm_inverse);
    let Some(to_text) = tm.then(ctm).inverse() else {
        return Err(invalid("a text matrix that cannot be inverted"));
    };
    let mut width = 0.0f64;
    let mut right = f64::NEG_INFINITY;
    let mut starts = Vec::new();
    for line in &block.lines {
        let mut sources = line.visual_chars().into_iter().filter_map(|ch| ch.source);
        let (Some(head), tail) = (sources.next(), sources.next_back()) else {
            continue;
        };
        let tail = tail.unwrap_or(head);
        let head_glyph = glyph_at(&inspected.ops[&key(&head)], head.operand, head.offset)?;
        let tail_glyph = glyph_at(&inspected.ops[&key(&tail)], tail.operand, tail.offset)?;
        let start = to_text.apply(
            head_glyph.matrix.apply(0.0, 0.0).0,
            head_glyph.matrix.apply(0.0, 0.0).1,
        );
        let end_point = tail_glyph.matrix.apply(tail_glyph.width / 1000.0, 0.0);
        let end = to_text.apply(end_point.0, end_point.1);
        width = width.max(end.0 - start.0);
        right = right.max(end.0);
        starts.push(start);
    }
    let pitch = match starts.as_slice() {
        [a, b, ..] if a.1 != b.1 && (a.1 - b.1).is_finite() => Some(a.1 - b.1),
        _ => None,
    };
    Ok(Frame {
        first: tm,
        width,
        right: if right.is_finite() { right } else { width },
        pitch,
        size,
    })
}

/// The face the new text is set in, and a note of it for the diagnostics.
/// `op` is the showing operation whose font and style the text follows;
/// with none (new text on a page that has no text) the face is the list's
/// closest to Helvetica, regular.
fn choose_face(
    document: &Document,
    op: Option<&ShowOp>,
    edit: &TextEdit<'_>,
    diagnostics: &mut Diagnostics,
) -> Result<Face, PdfError> {
    // A hard break ends a line and a soft hyphen is dropped: neither is drawn.
    // A bracket in a right-to-left run is drawn as its mirror image, so the
    // face needs that glyph too.
    let shown = arrange::with_mirror_images(&typeset::shown(&edit.text));
    if let (None, Some(op)) = (&edit.style.family, op) {
        if let Some(reused) = face::reuse(document, op, &shown, diagnostics)? {
            return Ok(Face::Reused(reused));
        }
    }
    let Some(list) = edit.fonts else {
        return Err(PdfError::FontListRequired);
    };
    let wanted = op.map_or_else(crate::render::glyphs::Wanted::plain_sans, |op| {
        crate::render::glyphs::wanted_style(&op.run.font)
    });
    let named;
    let families: &[&str] = match &edit.style.family {
        Some(family) => {
            named = [family.as_str()];
            &named
        }
        None => wanted.families,
    };
    let query = Query {
        families,
        weight: if wanted.bold {
            face::WEIGHT_BOLD
        } else {
            face::WEIGHT_REGULAR
        },
        italic: wanted.italic,
        ..Query::default()
    };
    let postscript = match (&edit.style.family, op) {
        (None, Some(op)) => face::postscript_name(op),
        _ => None,
    };
    let embedded = face::embed(list, &query, &shown, postscript.as_deref())?;
    if let (None, Some(op)) = (&edit.style.family, op) {
        diagnostics.push(
            DiagnosticKind::FontSubstituted {
                name: op.run.font_name.clone(),
                family: embedded.family.clone(),
            },
            op.span.range.start,
        );
    }
    Ok(Face::Embedded(embedded))
}

/// Where the new text goes, besides its offset: the resources in force
/// there, and the matrix that restores the CTM of the block's first glyph.
#[derive(Clone, Copy)]
struct Place<'a> {
    resources: &'a Dictionary,
    cm: Option<Matrix>,
}

/// What [`compose`] made: the bytes of the new text, the number of its
/// lines, the page resources with the font it needs added (when it needs
/// one), and the fonts it substituted.
struct Composed {
    bytes: Vec<u8>,
    lines: usize,
    /// The family of the face the text is set in: the list face's, or the
    /// name of the page's font that was used again.
    family: String,
    resources: Option<Dictionary>,
    substitutions: Vec<FontSubstitution>,
}

/// What the new text is set after: the showing operation whose font it
/// follows (`None` for a page with no text), the graphics state it takes
/// its spacing, scaling, rise and render mode from, the font size, and where
/// the lines go.
struct Model<'a> {
    op: Option<&'a ShowOp>,
    state: &'a GraphicsState,
    size: f64,
    frame: Frame,
    /// Where the first baseline is when it depends on the font: the top of
    /// the box on the displayed page, in points, and the matrix from the
    /// displayed page to user space. The text matrix of the first line is
    /// then made once the face is known, and `frame.first` is not used.
    top: Option<(Matrix, (f64, f64))>,
}

/// The new text as content bytes, with the font it needs added to the page.
fn compose(
    document: &Document,
    page: &Page,
    model: &Model<'_>,
    edit: &TextEdit<'_>,
    place: &Place<'_>,
    rewriter: &mut Rewriter<'_>,
) -> Result<Composed, PdfError> {
    let Place { resources, cm } = *place;
    let Model {
        op,
        state,
        size,
        frame,
        top,
    } = model;
    let (op, state, size) = (*op, *state, *size);
    if !(size.is_finite() && size > 0.0) {
        return Err(PdfError::Invalid(format!(
            "{size} is not a font size a text can be set at"
        )));
    }
    let mut diagnostics = Diagnostics::new();
    diagnostics.set_object(Some(page.id));
    let mut face = choose_face(document, op, edit, &mut diagnostics)?;
    let first = match top {
        // The text stands upright on the displayed page (text space's y
        // points up, the displayed page's down), its baseline the face's
        // ascent below the top of the box.
        Some((to_user, (x, top))) => {
            let baseline = top + face.ascent()? / GLYPH_UNITS_PER_EM * size;
            Matrix::new(1.0, 0.0, 0.0, -1.0, *x, baseline).then(*to_user)
        }
        None => frame.first,
    };

    let spacing = &state.text;
    let (tc, tw, th) = (
        spacing.char_spacing,
        spacing.word_spacing,
        spacing.horizontal_scaling,
    );
    let advance = |c: char| -> Result<f64, PdfError> {
        let word = if face.takes_word_spacing(c) { tw } else { 0.0 };
        Ok((face.width(c)? * size / 1000.0 + tc + word) * th)
    };
    let set = typeset::lines(&edit.text, frame.width, &advance)?;
    let arranged = arrange::arrange(&set, &advance)?;
    let pitch = match frame.pitch {
        Some(measured) => measured * size / frame.size,
        None => face.line_height()? / 1000.0 * size,
    };
    let mut codes: Vec<Vec<insert::Piece>> = Vec::with_capacity(arranged.len());
    for line in &arranged {
        // A right-to-left paragraph ends at the block's right edge, a
        // left-to-right one starts at its first glyph.
        let line_left = if line.right_to_left {
            frame.right - line.width
        } else {
            0.0
        };
        let mut row = Vec::with_capacity(line.pieces.len());
        for piece in &line.pieces {
            row.push(insert::Piece {
                left: line_left + piece.left,
                codes: face.encode(&piece.text)?,
                actual: piece.actual.clone(),
            });
        }
        codes.push(row);
    }

    let family = match &face {
        Face::Embedded(embedded) => embedded.family.clone(),
        Face::Reused(_) => op.and_then(face::postscript_name).unwrap_or_default(),
    };
    let (name, new_resources) = match &mut face {
        Face::Reused(reused) => (reused.name.clone(), None),
        Face::Embedded(Embedded { embedder, .. }) => {
            let font = embedder.write(rewriter.builder())?;
            let (copy, name) =
                insert::with_resource(rewriter, resources, b"Font", "KernFont", font)?;
            (name, Some(copy))
        }
    };
    let substitutions = substitutions_in(&diagnostics);
    document.record_diagnostics(diagnostics);
    let setting = insert::Setting {
        font: &name,
        size,
        state,
        colour: edit.style.colour,
        first,
        pitch,
        cm,
    };
    let mut bytes = vec![b'\n'];
    bytes.extend(insert::show(&setting, &codes)?);
    Ok(Composed {
        bytes,
        lines: set.lines.len(),
        family,
        resources: new_resources,
        substitutions,
    })
}

/// The fonts `diagnostics` says were substituted, in the order raised.
fn substitutions_in(diagnostics: &Diagnostics) -> Vec<FontSubstitution> {
    diagnostics
        .as_slice()
        .iter()
        .filter_map(|diagnostic| match &diagnostic.kind {
            DiagnosticKind::FontSubstituted { name, family } => Some(FontSubstitution {
                name: name.clone(),
                family: family.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Replaces the text of `block`, a block of page `page` (counted from 1)
/// read from `document`, with `edit`'s text, and gives the document with
/// it.
///
/// The block's glyphs are taken out as [`delete_text_block`] does, so the
/// text after them stays where it was. The new text is set in the block's
/// font when it is a simple font that can show every character (an
/// embedded program only if it has marks for each); otherwise in a face of
/// `edit.fonts`, embedded as a subset, which is reported as a
/// [`FontSubstituted`](crate::DiagnosticKind::FontSubstituted) diagnostic
/// unless the style named the family. It starts where the block's first
/// glyph did, is broken to the width of the block's widest line, and goes
/// after the `ET` of the block's last text object, in a `q ... Q` of its
/// own with the block's spacing, scaling, rise, render mode and colours.
/// Lines are as far apart as the block's first two were, or as the font's
/// ascent, descent and leading say.
///
/// The text is in logical order, as typed. A paragraph is right to left when
/// its first strong character is (UAX #9, P2 and P3); its lines are shown in
/// the order their levels give and are set against the block's right edge,
/// and the glyphs are written in the order typed, each piece where it
/// stands. Shaping is not done: one character is one glyph of the font's
/// Unicode map, so ligatures, kerning, combining marks and the joining of
/// Arabic letters are not formed. A character that is drawn mirrored in a
/// right-to-left run (a bracket) is set as the glyph of its mirror image,
/// with the character typed as the `ActualText` around it; one with no exact
/// mirror image is [`PdfError::MirroredGlyphNotSet`].
///
/// A block that [`Document::text_block_editability`] refuses is
/// [`PdfError::TextNotEditable`]; a character no usable font has is
/// [`PdfError::NoGlyphForCharacter`]; a font from the list when none was
/// given is [`PdfError::FontListRequired`].
pub fn edit_text(
    document: &Document,
    page: u32,
    block: &TextBlock,
    edit: &TextEdit<'_>,
    progress: &dyn Progress,
) -> Result<Edited, PdfError> {
    if edit.text.is_empty() {
        return Ok(Edited {
            document: delete_text_block(document, page, block, progress)?,
            lines: 0,
            substitutions: Vec::new(),
        });
    }
    let inspected = match inspect(document, page, block, progress)? {
        Ok(inspected) => inspected,
        Err(reason) => return Err(PdfError::TextNotEditable { reason }),
    };
    let insertion = match insertion(&inspected) {
        Ok(insertion) => insertion,
        Err(reason) => return Err(PdfError::TextNotEditable { reason }),
    };
    let page = document.page(page)?;
    let content =
        document.content_stream(inspected.sources[0].span.stream, DEFAULT_MAX_CONTENT_BYTES)?;
    let mut splices = removals(&inspected, &content)?;
    let mut rewriter = Rewriter::new(document);
    let chain = inspected.chain().to_vec();
    let resources = effective_resources(document, &page, &chain, chain.len())?;
    let first = &inspected.ops[&key_of(&inspected.sources[0])];
    let model = Model {
        op: Some(first),
        state: &first.state,
        size: edit.style.size.unwrap_or(first.run.font_size),
        frame: frame_of(&inspected, block)?,
        top: None,
    };
    let Composed {
        bytes,
        lines,
        resources: new_resources,
        substitutions,
        ..
    } = compose(
        document,
        &page,
        &model,
        edit,
        &Place {
            resources: &resources,
            cm: insertion.cm,
        },
        &mut rewriter,
    )?;
    splices.push(Splice {
        start: insertion.at,
        end: insertion.at,
        bytes,
    });
    splices.sort_by_key(|splice| splice.start);
    commit(
        &mut rewriter,
        document,
        &page,
        &chain,
        &content,
        &splices,
        new_resources,
    )?;
    Ok(Edited {
        document: document.apply(rewriter.finish())?,
        lines,
        substitutions,
    })
}
