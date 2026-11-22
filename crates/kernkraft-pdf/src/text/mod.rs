// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The text layer of a page: ISO 32000-1:2008, 9.4 (text objects), 9.10
//! (extraction of text content) and 14.8.2.3 (page content order).
//!
//! A [`TextExtractor`] runs a page's content through the interpreter and
//! gives a [`TextPage`]: the page's text as blocks of lines of characters,
//! each with the rectangle it covers on the displayed page (as fractions
//! of its width and height, see [`Rect`]), in reading order. When the file
//! is tagged, the structure tree gives the order of the blocks and keeps
//! the text of one paragraph, heading or table cell together; otherwise
//! the order comes from the page's geometry ([`layout`]).
//!
//! Text in the invisible render mode is text of the page: it is how a
//! scanned page carries its recognised text. Text that optional content
//! hides, that has no size, or that lies wholly off the displayed page is
//! not. Lines are in visual order, left to right for text that flows that
//! way: a right-to-left script is not reordered into its logical order.
//!
//! A [`TextPage`] is searched with [`TextPage::search`], and a document
//! with [`Document::search`].

pub(crate) mod collect;
mod layout;
mod search;
mod structure;

use kernkraft_fs::Progress;

use crate::content::{InterpretOptions, Interpreter, Matrix, OperatorSpan};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::display::{PageView, Rect};
use crate::doc::Document;
use crate::error::{Limit, PdfError};
use crate::text::collect::Collector;
use crate::text::structure::StructureIndex;

pub use search::{Match, SearchOptions};

/// The glyphs one page may show before reading its text is refused
/// ([`Limit::TextGlyphs`]): a page of a printed book has a few thousand and
/// a densely set poster some hundred thousand, and each glyph held costs
/// about 200 bytes.
pub const DEFAULT_MAX_GLYPHS: usize = 1 << 21;

/// Where in a content stream a glyph was shown, so that an editor can find
/// the bytes that painted it (9.4.3: the operand of `Tj`, `'` and `\"`, or
/// one string of the array of `TJ`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlyphSource {
    /// The showing operation.
    pub span: OperatorSpan,
    /// Which string of the operation: 0 for `Tj`, `'` and `\"`, the index
    /// in the array operand for `TJ` (the numbering of
    /// [`PlacedGlyph::operand`](crate::content::PlacedGlyph::operand)).
    pub operand: u32,
    /// Where the glyph's code starts in that string, in bytes of the
    /// decoded string (escapes and hexadecimal digits of the source
    /// resolved).
    pub offset: u32,
    /// How many bytes the code takes.
    pub len: u32,
    /// Which run of the operation this is: 0 for page content, and for
    /// text in a form XObject the number of the form's invocation, counted
    /// from 1 in the order the page's content invokes forms. A form
    /// invoked twice has two numbers for the one operation.
    pub invocation: u32,
}

/// One character of the text of a page.
#[derive(Debug, Clone, PartialEq)]
pub struct TextChar {
    /// Its Unicode text: one character, or more for a ligature the font
    /// maps to several.
    pub text: String,
    /// The box the glyph covers: the smallest upright rectangle that holds
    /// [`quad`](Self::quad).
    pub rect: Rect,
    /// The glyph's own box as it stands on the displayed page, as fractions
    /// of the page's width and height with `y` growing downwards, like
    /// [`rect`](Self::rect): bottom left, bottom right, top right and top
    /// left of the glyph, "bottom" and "left" being the glyph's own. A
    /// glyph drawn upright has the corners of `rect`; one drawn under a
    /// text matrix that skews or turns it is a parallelogram that `rect`
    /// only envelops. For a synthetic space it is the gap between the
    /// neighbouring glyphs, its corners in the same order.
    pub quad: [(f64, f64); 4],
    /// Whether the character is a space the layout read between two
    /// glyphs that no space glyph separates, so that it covers no glyph.
    pub synthetic: bool,
    /// The glyph the character stands for; `None` for a synthetic space.
    pub source: Option<GlyphSource>,
}

/// A line of text: characters that share a baseline.
#[derive(Debug, Clone, PartialEq)]
pub struct TextLine {
    pub rect: Rect,
    pub chars: Vec<TextChar>,
}

impl TextLine {
    pub fn text(&self) -> String {
        self.chars.iter().map(|ch| ch.text.as_str()).collect()
    }
}

/// A block of text: lines stacked one below the next, a paragraph, a
/// heading, a table cell.
#[derive(Debug, Clone, PartialEq)]
pub struct TextBlock {
    pub rect: Rect,
    pub lines: Vec<TextLine>,
}

/// Where a character of a page is: its block, line and place in the line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CharPlace {
    pub block: usize,
    pub line: usize,
    pub chars: usize,
}

/// Text a user selected.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// The text, a line feed after each line and a blank line between
    /// blocks.
    pub text: String,
    /// The part of each line selected.
    pub rects: Vec<Rect>,
}

/// The text of a page, in reading order.
#[derive(Debug, Clone, PartialEq)]
pub struct TextPage {
    /// The page's number, counted from 1.
    pub page: u32,
    pub blocks: Vec<TextBlock>,
    /// Whether the structure tree gave the order of any block.
    pub tagged: bool,
    places: Vec<CharPlace>,
}

impl TextPage {
    fn new(page: u32, blocks: Vec<TextBlock>, tagged: bool) -> Self {
        let mut places = Vec::new();
        for (block, text_block) in blocks.iter().enumerate() {
            for (line, text_line) in text_block.lines.iter().enumerate() {
                for chars in 0..text_line.chars.len() {
                    places.push(CharPlace { block, line, chars });
                }
            }
        }
        Self {
            page,
            blocks,
            tagged,
            places,
        }
    }

    /// How many characters the page has, those a layout read between
    /// glyphs included.
    pub fn len(&self) -> usize {
        self.places.len()
    }

    pub fn is_empty(&self) -> bool {
        self.places.is_empty()
    }

    /// Where character `index` of the page is, counting in reading order.
    pub(crate) fn positions(&self) -> &[CharPlace] {
        &self.places
    }

    /// Character `index` of the page, counting in reading order.
    pub fn char(&self, index: usize) -> Option<&TextChar> {
        let place = self.places.get(index)?;
        Some(&self.blocks[place.block].lines[place.line].chars[place.chars])
    }

    /// The text of the page: a line feed after each line and a blank line
    /// between blocks.
    pub fn text(&self) -> String {
        self.text_of(0, self.places.len())
    }

    /// The text of characters `start` to `end` (exclusive).
    fn text_of(&self, start: usize, end: usize) -> String {
        let mut text = String::new();
        let mut before: Option<CharPlace> = None;
        for place in &self.places[start.min(self.places.len())..end.min(self.places.len())] {
            if let Some(before) = before {
                if before.block != place.block {
                    text.push_str("\n\n");
                } else if before.line != place.line {
                    text.push('\n');
                }
            }
            text.push_str(&self.blocks[place.block].lines[place.line].chars[place.chars].text);
            before = Some(*place);
        }
        text
    }

    /// The part of each line that characters `start` to `end` (exclusive)
    /// cover.
    pub(crate) fn rects_of(&self, start: usize, end: usize) -> Vec<Rect> {
        let mut rects: Vec<Rect> = Vec::new();
        let mut before: Option<(usize, usize)> = None;
        for place in &self.places[start.min(self.places.len())..end.min(self.places.len())] {
            let rect = self.blocks[place.block].lines[place.line].chars[place.chars].rect;
            match (before, rects.last_mut()) {
                (Some(line), Some(last)) if line == (place.block, place.line) => {
                    *last = last.union(&rect);
                }
                _ => rects.push(rect),
            }
            before = Some((place.block, place.line));
        }
        rects
    }

    /// The character whose box holds the point, given as fractions of the
    /// page. The first, when boxes overlap.
    pub fn char_at(&self, x: f64, y: f64) -> Option<usize> {
        (0..self.places.len()).find(|&index| {
            self.char(index)
                .is_some_and(|ch| !ch.synthetic && ch.rect.contains(x, y))
        })
    }

    /// The character nearest the point, given as fractions of the page:
    /// the one the point is in, or the one whose box is closest. The
    /// first, when two are as close.
    pub fn nearest_char(&self, x: f64, y: f64) -> Option<usize> {
        let mut best: Option<(usize, f64)> = None;
        for index in 0..self.places.len() {
            let Some(ch) = self.char(index).filter(|ch| !ch.synthetic) else {
                continue;
            };
            let distance = ch.rect.distance_to(x, y);
            if best.is_none_or(|(_, nearest)| distance < nearest) {
                best = Some((index, distance));
            }
        }
        best.map(|(index, _)| index)
    }

    /// The text from character `from` to character `to`, both included,
    /// whichever comes first in reading order. `None` when either is not
    /// a character of the page.
    pub fn selection(&self, from: usize, to: usize) -> Option<Selection> {
        if from >= self.places.len() || to >= self.places.len() {
            return None;
        }
        let (start, end) = (from.min(to), from.max(to) + 1);
        Some(Selection {
            text: self.text_of(start, end),
            rects: self.rects_of(start, end),
        })
    }
}

/// What a text extraction is allowed.
#[derive(Debug, Clone)]
pub struct TextOptions {
    /// How the page's content is interpreted. Its `base_matrix` is not
    /// used: the text is placed on the displayed page.
    pub interpret: InterpretOptions,
    /// The most glyphs one page may show ([`Limit::TextGlyphs`]).
    pub max_glyphs: usize,
}

impl Default for TextOptions {
    fn default() -> Self {
        Self {
            interpret: InterpretOptions::default(),
            max_glyphs: DEFAULT_MAX_GLYPHS,
        }
    }
}

/// Reads the text of the pages of one document. It keeps what is the same
/// for every page (the fonts it has read, the structure tree), so that one
/// extractor serves a whole document. It holds its own handle on the
/// snapshot it was made from, so it may outlive the borrow it was made
/// with, and a session can keep one for as long as the snapshot stands.
pub struct TextExtractor {
    doc: Document,
    interpreter: Interpreter,
    structure: StructureIndex,
    max_glyphs: usize,
}

impl TextExtractor {
    pub fn new(doc: &Document) -> Result<Self, PdfError> {
        Self::with_options(doc, TextOptions::default())
    }

    pub fn with_options(doc: &Document, options: TextOptions) -> Result<Self, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let structure = StructureIndex::read(doc, &mut diagnostics)?;
        doc.record_diagnostics(diagnostics);
        let interpret = InterpretOptions {
            base_matrix: Matrix::IDENTITY,
            ..options.interpret
        };
        Ok(Self {
            doc: doc.clone(),
            interpreter: Interpreter::new(doc, interpret)?,
            structure,
            max_glyphs: options.max_glyphs,
        })
    }

    /// The text of page `number`, counted from 1.
    pub fn page(&mut self, number: u32, progress: &dyn Progress) -> Result<TextPage, PdfError> {
        let page = self.doc.page(number)?;
        let mut diagnostics = Diagnostics::new();
        diagnostics.set_object(Some(page.id));
        let view = PageView::of(&page, &mut diagnostics);
        let mut collector = Collector::new(&view, &self.structure, self.max_glyphs);
        let run = self.interpreter.run_page(&page, &mut collector, progress);
        let collected = collector.finish();
        let result = run.and(match collected.overflow {
            Some(span) => Err(PdfError::LimitExceeded {
                offset: span.range.start,
                limit: Limit::TextGlyphs {
                    max: self.max_glyphs as u64,
                },
            }),
            None => Ok(()),
        });
        if let Some(span) = collected.unmapped {
            diagnostics.push(DiagnosticKind::TextUnmapped, span.range.start);
        }
        diagnostics.set_object(None);
        self.doc.record_diagnostics(diagnostics);
        result?;
        let laid = layout::lay_out(&collected.glyphs, &collected.rules, &view);
        Ok(TextPage::new(number, laid.blocks, laid.tagged))
    }
}

/// One match in a document.
#[derive(Debug, Clone, PartialEq)]
pub struct SearchHit {
    /// The page, counted from 1.
    pub page: u32,
    pub found: Match,
}

impl Document {
    /// The text of page `number`, counted from 1. To read several pages,
    /// use one [`TextExtractor`].
    pub fn text_page(&self, number: u32, progress: &dyn Progress) -> Result<TextPage, PdfError> {
        TextExtractor::new(self)?.page(number, progress)
    }

    /// Every match of `query` in the document, page by page. The search
    /// stops with [`PdfError::Cancelled`] when `progress` is cancelled.
    pub fn search(
        &self,
        query: &str,
        options: &SearchOptions,
        progress: &dyn Progress,
    ) -> Result<Vec<SearchHit>, PdfError> {
        let count = self.page_count()?;
        let mut extractor = TextExtractor::new(self)?;
        let mut hits = Vec::new();
        for number in 1..=count {
            if progress.is_cancelled() {
                return Err(PdfError::Cancelled);
            }
            progress.report(u64::from(number - 1), u64::from(count), "search");
            let page = extractor.page(number, progress)?;
            hits.extend(
                page.search(query, options)
                    .into_iter()
                    .map(|found| SearchHit {
                        page: number,
                        found,
                    }),
            );
        }
        progress.report(u64::from(count), u64::from(count), "search");
        Ok(hits)
    }
}
