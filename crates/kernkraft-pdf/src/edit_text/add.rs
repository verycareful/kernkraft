// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! New text on a page, where there is none to replace (ISO 32000-1:2008,
//! 9.3, 9.4 and 7.8.2).
//!
//! The text is set in a `q ... Q` of a content stream of its own, added to
//! the end of the page's `Contents`, so the page's own streams are not
//! changed. Its font, size and colour follow a block of text that is already
//! on the page (the nearest to the box, or one the caller names), so the new
//! text looks like its surroundings: the block's font is used again when it
//! can show the text, and otherwise the list face of the same family is
//! embedded, as an edit does.

use kernkraft_fs::Progress;

use kernkraft_font::FontList;

use crate::colour::Rgb;
use crate::content::{GraphicsState, Matrix, RenderMode};
use crate::diagnostic::Diagnostics;
use crate::display::PageView;
use crate::doc::{Document, StreamEncoding};
use crate::edit_text::{
    compose, effective_resources, glyph_at, inspect, key_of, EditBlocker, FontSubstitution, Frame,
    Model, Place, TextEdit, TextStyle,
};
use crate::pages::rewrite::{direct, reference_object, Rewriter};
use crate::syntax::{Dictionary, Name, ObjectId, Value};
use crate::text::{TextBlock, TextPage};
use crate::PdfError;

/// The colour of text with nothing to follow: black, the initial colour of
/// the graphics state (Table 52).
const BLACK: Rgb = Rgb {
    r: 0.0,
    g: 0.0,
    b: 0.0,
};

/// The block of text the new text follows.
#[derive(Clone, Copy)]
pub enum Follow<'a> {
    /// The block of the page nearest the box, among those whose text can be
    /// followed (a block of vertical writing, of invisible text or that
    /// cannot be read is passed over). A page with none has no text to
    /// follow.
    Nearest(&'a TextPage),
    /// This block of the page. One whose text cannot be followed is
    /// [`PdfError::TextNotEditable`].
    Block(&'a TextBlock),
}

/// New text and where it goes.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct NewText<'f> {
    /// The text. A line feed ends a line; a line also ends where the next
    /// word would pass `width`.
    pub text: String,
    /// The top left corner of the box the text is set in, as fractions of
    /// the displayed page's width and height, `y` growing downwards.
    pub at: (f64, f64),
    /// The box's width, as a fraction of the displayed page's width. The
    /// lines grow downwards from the top of the box.
    pub width: f64,
    /// What the block followed does not decide: a family, a size, a colour.
    /// A page with no text to follow needs the size.
    pub style: TextStyle,
    /// The faces a font can be taken from, as for [`TextEdit`].
    pub fonts: Option<&'f FontList>,
}

impl<'f> NewText<'f> {
    pub fn new(text: impl Into<String>, at: (f64, f64), width: f64) -> Self {
        Self {
            text: text.into(),
            at,
            width,
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

/// The result of [`add_text`].
#[derive(Debug, Clone)]
pub struct TextAdded {
    /// The document with the new text, for
    /// [`EditSession::push`](crate::EditSession::push).
    pub document: Document,
    /// How many lines the new text took.
    pub lines: usize,
    /// The family the text is set in: the list face's, or the name of the
    /// page's own font when it was used again.
    pub family: String,
    /// The index in [`TextPage::blocks`] of the block followed, when
    /// [`Follow::Nearest`] found one.
    pub followed: Option<usize>,
    /// The fonts of the block followed that a face of the list replaced.
    pub substitutions: Vec<FontSubstitution>,
}

/// What a block gives the text that follows it.
struct Template {
    op: crate::edit_text::collect::ShowOp,
    /// The font size as it shows on the displayed page.
    size: f64,
}

/// The size the first glyph of `op` is drawn at on the page: the font size
/// carried through the text matrix and the CTM, so that a page set under a
/// scaling matrix gives the size the reader sees.
fn shown_size(
    inspected: &crate::edit_text::Inspected,
    op: &crate::edit_text::collect::ShowOp,
) -> Result<f64, PdfError> {
    let first = inspected.sources[0];
    let glyph = glyph_at(op, first.operand, first.offset)?;
    let m = glyph.matrix;
    let scaling = op.state.text.horizontal_scaling;
    let area = (m.a * m.d - m.b * m.c).abs();
    if !(scaling.is_finite() && scaling > 0.0 && area.is_finite() && area > 0.0) {
        return Err(PdfError::Invalid(
            "the text to follow has no size on the page".to_owned(),
        ));
    }
    Ok((area / scaling).sqrt())
}

/// The template `block` gives, or why it gives none.
fn template_of(
    document: &Document,
    page: u32,
    block: &TextBlock,
    progress: &dyn Progress,
) -> Result<Result<Template, EditBlocker>, PdfError> {
    let inspected = match inspect(document, page, block, progress)? {
        Ok(inspected) => inspected,
        Err(reason) => return Ok(Err(reason)),
    };
    let op = inspected.ops[&key_of(&inspected.sources[0])].clone();
    if op.run.font.is_vertical() {
        return Ok(Err(EditBlocker::VerticalWriting));
    }
    let Ok(size) = shown_size(&inspected, &op) else {
        return Ok(Err(EditBlocker::NoSize));
    };
    Ok(Ok(Template { op, size }))
}

/// Whether text set after `template` is seen: the text of an invisible
/// render mode (the layer of a scanned page) is not.
fn followable(template: &Template) -> bool {
    template.op.render_mode != RenderMode::Invisible
}

/// The indices of `text`'s blocks, nearest first to the point `at` (fractions
/// of the displayed page), measured in points from the block's box; blocks
/// the same distance away stay in reading order.
pub fn blocks_by_distance(
    document: &Document,
    page: u32,
    text: &TextPage,
    at: (f64, f64),
) -> Result<Vec<usize>, PdfError> {
    let geometry = document.page_geometry(page)?;
    let (width, height) = (geometry.width, geometry.height);
    let (x, y) = (at.0 * width, at.1 * height);
    let mut order: Vec<(f64, usize)> = text
        .blocks
        .iter()
        .enumerate()
        .map(|(index, block)| {
            let rect = &block.rect;
            let in_points = crate::Rect {
                left: rect.left * width,
                top: rect.top * height,
                right: rect.right * width,
                bottom: rect.bottom * height,
            };
            (in_points.distance_to(x, y), index)
        })
        .collect();
    order.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)));
    Ok(order.into_iter().map(|(_, index)| index).collect())
}

/// The template of the block nearest `at` that gives one, and its index.
fn nearest(
    document: &Document,
    page: u32,
    text: &TextPage,
    at: (f64, f64),
    progress: &dyn Progress,
) -> Result<Option<(usize, Template)>, PdfError> {
    for index in blocks_by_distance(document, page, text, at)? {
        if let Ok(template) = template_of(document, page, &text.blocks[index], progress)? {
            if followable(&template) {
                return Ok(Some((index, template)));
            }
        }
    }
    Ok(None)
}

/// What new text takes from a block it follows.
#[derive(Debug, Clone, PartialEq)]
pub struct BlockStyle {
    /// The PostScript name of the block's font, without the subset tag that
    /// names a font embedded as a subset.
    pub font: String,
    /// Whether the font has its program in the file.
    pub embedded: bool,
    /// The font size as it shows on the displayed page, in points.
    pub size: f64,
    /// The colour of the text, `None` when it is painted with a pattern.
    pub colour: Option<Rgb>,
}

/// The style new text takes from `block`, or `None` when the block gives
/// none: it cannot be edited (see [`EditBlocker`]), is written vertically or
/// is invisible.
pub fn block_style(
    document: &Document,
    page: u32,
    block: &TextBlock,
    progress: &dyn Progress,
) -> Result<Option<BlockStyle>, PdfError> {
    let Ok(template) = template_of(document, page, block, progress)? else {
        return Ok(None);
    };
    if !followable(&template) {
        return Ok(None);
    }
    Ok(Some(BlockStyle {
        font: crate::edit_text::face::postscript_name(&template.op).unwrap_or_default(),
        embedded: template
            .op
            .run
            .font
            .embedded_program(document)
            .ok()
            .flatten()
            .is_some(),
        size: template.size,
        colour: colour_of(document, &template).ok(),
    }))
}

/// The colour of the text of `template`, as the reader sees it.
fn colour_of(document: &Document, template: &Template) -> Result<Rgb, PdfError> {
    let paint = &template.op.state.fill;
    if paint.pattern.is_some() {
        return Err(PdfError::Invalid(
            "the text to follow is painted with a pattern: name a colour".to_owned(),
        ));
    }
    document
        .paint_colour_space(&paint.space)?
        .to_rgb(&paint.components)
}

/// Puts `new` on page `page` (counted from 1) of `document` and gives the
/// document with it.
///
/// The text follows `follow`: its font is the block's when that can show the
/// text, else the list face of the block's family (a
/// [`FontSubstituted`](crate::DiagnosticKind::FontSubstituted) diagnostic,
/// and a `substitutions` entry), and its size and colour are the block's,
/// each unless `new.style` names one. `new.style.family` names a family of
/// the list instead of the block's font. With no block to follow (a page
/// with no text) the face is the list's closest to Helvetica, the colour
/// black, and the size must be given: [`PdfError::NoSizeForNewText`].
///
/// A face that is needed and no list was given is
/// [`PdfError::FontListRequired`]; a character no usable font has is
/// [`PdfError::NoGlyphForCharacter`]; a block named by [`Follow::Block`]
/// that cannot be followed is [`PdfError::TextNotEditable`].
pub fn add_text(
    document: &Document,
    page: u32,
    follow: Follow<'_>,
    new: &NewText<'_>,
    progress: &dyn Progress,
) -> Result<TextAdded, PdfError> {
    let (at, width) = (new.at, new.width);
    if !(at.0.is_finite() && at.1.is_finite() && width.is_finite() && width > 0.0) {
        return Err(PdfError::Invalid(format!(
            "a box at ({}, {}) {} wide is not on a page",
            at.0, at.1, width
        )));
    }
    let (template, followed) = match follow {
        Follow::Nearest(text) => match nearest(document, page, text, at, progress)? {
            Some((index, template)) => (Some(template), Some(index)),
            None => (None, None),
        },
        Follow::Block(block) => match template_of(document, page, block, progress)? {
            Ok(template) => (Some(template), None),
            Err(reason) => return Err(PdfError::TextNotEditable { reason }),
        },
    };
    let size = match (new.style.size, &template) {
        (Some(size), _) => size,
        (None, Some(template)) => template.size,
        (None, None) => return Err(PdfError::NoSizeForNewText),
    };
    let colour = match (new.style.colour, &template) {
        (Some(colour), _) => colour,
        (None, Some(template)) => colour_of(document, template)?,
        (None, None) => BLACK,
    };
    let mut style = new.style.clone();
    style.size = Some(size);
    style.colour = Some(colour);
    let edit = TextEdit {
        text: new.text.clone(),
        style,
        fonts: new.fonts,
    };

    let page_object = document.page(page)?;
    let view = PageView::of(&page_object, &mut Diagnostics::new());
    let Some(to_user) = view.matrix.inverse() else {
        return Err(PdfError::Invalid(
            "the page's size and rotation leave it no area".to_owned(),
        ));
    };
    let wide = width * view.width;
    let state = GraphicsState::initial(Matrix::IDENTITY);
    let model = Model {
        op: template.as_ref().map(|template| &template.op),
        state: &state,
        size,
        frame: Frame {
            first: Matrix::IDENTITY,
            width: wide,
            right: wide,
            pitch: None,
            size,
        },
        top: Some((to_user, (at.0 * view.width, at.1 * view.height))),
    };
    let resources = effective_resources(document, &page_object, &[], 0)?;
    let mut rewriter = Rewriter::new(document);
    let composed = compose(
        document,
        &page_object,
        &model,
        &edit,
        &Place {
            resources: &resources,
            cm: None,
        },
        &mut rewriter,
    )?;
    let stream = ObjectId {
        number: rewriter.reserve_many(1)?,
        generation: 0,
    };
    rewriter.builder().set_stream(
        stream,
        Dictionary::new(),
        composed.bytes,
        StreamEncoding::Flate,
    )?;
    let mut contents: Vec<_> = document
        .page_content_streams(page_object.id)?
        .into_iter()
        .map(reference_object)
        .collect();
    contents.push(reference_object(stream));
    let new_resources = composed.resources;
    rewriter.edit(page_object.id, |_, dictionary| {
        dictionary.insert(Name::new(&b"Contents"[..]), direct(Value::Array(contents)));
        if let Some(resources) = new_resources {
            dictionary.insert(
                Name::new(&b"Resources"[..]),
                direct(Value::Dictionary(resources)),
            );
        }
        Ok(())
    })?;
    Ok(TextAdded {
        document: document.apply(rewriter.finish())?,
        lines: composed.lines,
        family: composed.family,
        followed,
        substitutions: composed.substitutions,
    })
}
