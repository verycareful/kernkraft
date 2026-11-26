// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening and closing a document, and reading a page: its size, text,
//! search matches, links and annotations.

use kernkraft_pdf::text::SearchOptions;
use kernkraft_pdf::{
    block_style, blocks_by_distance, AnnotationKind, BlockStyle, EditSession, Editability,
};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::{Engine, HandleRequest, PageRequest, Session};
use crate::error::{variant_name, WireError};
use crate::fields::{field, Fields};
use crate::jobs::JobId;
use crate::open::Source;
use crate::wire;

/// The matches of `query` on one page.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRequest {
    pub handle: i64,
    pub page: u32,
    pub query: String,
    #[serde(default)]
    pub case_sensitive: bool,
    #[serde(default)]
    pub whole_word: bool,
    /// The job that can cancel this request (see [`crate::jobs`]).
    #[serde(default)]
    pub job: Option<JobId>,
}

/// A request for a page's text.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TextRequest {
    pub handle: i64,
    pub page: u32,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]); `done` counts text blocks and the page's own work.
    #[serde(default)]
    pub job: Option<JobId>,
}

/// A request for the fonts of a page's text.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageFontsRequest {
    pub handle: i64,
    pub page: u32,
    /// A point of the displayed page, as fractions of its width and height:
    /// the entry of the block new text added there would follow is marked
    /// `nearest`.
    #[serde(default)]
    pub near: Option<(f64, f64)>,
    /// The job that can cancel this request and watch it (see
    /// [`crate::jobs`]).
    #[serde(default)]
    pub job: Option<JobId>,
}

/// Font sizes are grouped to a hundredth of a point: the size a block shows
/// on the page is the font size carried through its matrices, which leaves
/// a few millionths of rounding in a size that is 10.5 on the page.
const SIZE_STEP: f64 = 0.01;

/// What a group of blocks of a page share: the font, the size and the colour.
struct StyleGroup {
    style: BlockStyle,
    blocks: Vec<usize>,
    chars: usize,
}

impl Engine {
    /// Opens the PDF at `path` for editing. The document is read where it
    /// is: the application has already copied it into its own cache.
    pub fn open(&self, source: Source) -> Result<Fields, WireError> {
        let document = source.open(&self.options)?;
        if let Some(profile) = self.cmyk_profile() {
            document.use_cmyk_profile(profile);
        }
        let pages = document.page_count()?;
        let session = Session::new(EditSession::new(document)?);
        let handle = self.sessions.insert(session);
        let mut fields = field("handle", handle);
        fields.insert("pages".to_owned(), json!(pages));
        fields.insert("revision".to_owned(), json!(0));
        Ok(fields)
    }

    /// Closes the session; its handle is never used again.
    pub fn close(&self, request: HandleRequest) -> Result<Fields, WireError> {
        self.jobs.cancel_for(request.handle);
        if !self.sessions.remove(request.handle) {
            return Err(WireError::unknown_handle(request.handle));
        }
        Ok(Fields::new())
    }

    /// The displayed size of a page and the box of user space it shows.
    pub fn page_info(&self, request: PageRequest) -> Result<Fields, WireError> {
        let (document, _) = self.snapshot(request.handle)?;
        let geometry = document.page_geometry(request.page)?;
        let bounds = geometry.bounds;
        let mut fields = field("width_pt", geometry.width);
        fields.insert("height_pt".to_owned(), json!(geometry.height));
        fields.insert("rotate".to_owned(), json!(geometry.rotate));
        fields.insert(
            "crop_box".to_owned(),
            json!([bounds.llx, bounds.lly, bounds.urx, bounds.ury]),
        );
        Ok(fields)
    }

    /// A page's text in reading order. `lines` carry the box of each glyph
    /// and whether the line ended by a wrap (`wrapped`, see
    /// `TextBlock::wrapped_lines`); `blocks` say whether each can be edited
    /// and, when not, why. The `revision` is the one a block index is good
    /// at.
    pub fn text(&self, request: TextRequest) -> Result<Fields, WireError> {
        let running = self.jobs.begin(request.job, Some(request.handle))?;
        let (document, revision, page) = self.text_page(request.handle, request.page, &running)?;
        let mut lines = Vec::new();
        let mut blocks = Vec::new();
        for (index, block) in page.blocks.iter().enumerate() {
            for (line, wrapped) in block.lines.iter().zip(block.wrapped_lines()) {
                lines.push(json!({
                    "block": index,
                    "text": line.text(),
                    "wrapped": wrapped,
                    "rect": wire::rect(&line.rect),
                    "glyphs": line.chars.iter().map(|c| wire::rect(&c.rect)).collect::<Vec<_>>(),
                }));
            }
            let mut entry = json!({ "id": index, "rect": wire::rect(&block.rect) });
            match document.text_block_editability(request.page, block, &running)? {
                Editability::Editable => entry["editable"] = Value::Bool(true),
                Editability::Blocked(reason) => {
                    entry["editable"] = Value::Bool(false);
                    entry["reason"] = Value::String(variant_name(&reason));
                }
            }
            blocks.push(entry);
        }
        let mut fields = field("revision", revision);
        fields.insert("lines".to_owned(), Value::Array(lines));
        fields.insert("blocks".to_owned(), Value::Array(blocks));
        fields.insert("tagged".to_owned(), Value::Bool(page.tagged));
        Ok(fields)
    }

    /// The fonts of a page's text, each with the size and colour its blocks
    /// are set in and the blocks that use it: the choices for the text that
    /// `addText` puts on the page, which follows the block it is given or the
    /// nearest. A block that new text cannot follow (it cannot be edited, is
    /// written vertically or is invisible) is in none. Most used first.
    /// `revision` is the one the block ids are good at.
    pub fn page_fonts(&self, request: PageFontsRequest) -> Result<Fields, WireError> {
        let running = self.jobs.begin(request.job, Some(request.handle))?;
        let (document, revision, page) = self.text_page(request.handle, request.page, &running)?;
        let mut groups: Vec<StyleGroup> = Vec::new();
        let mut styles: Vec<Option<BlockStyle>> = Vec::with_capacity(page.blocks.len());
        for (index, block) in page.blocks.iter().enumerate() {
            let style = block_style(&document, request.page, block, &running)?;
            if let Some(style) = &style {
                let chars: usize = block.lines.iter().map(|line| line.chars.len()).sum();
                match groups
                    .iter_mut()
                    .find(|group| same_style(&group.style, style))
                {
                    Some(group) => {
                        group.blocks.push(index);
                        group.chars += chars;
                    }
                    None => groups.push(StyleGroup {
                        style: style.clone(),
                        blocks: vec![index],
                        chars,
                    }),
                }
            }
            styles.push(style);
        }
        groups.sort_by(|a, b| b.chars.cmp(&a.chars).then(a.blocks[0].cmp(&b.blocks[0])));
        let nearest = match request.near {
            Some(near) => blocks_by_distance(&document, request.page, &page, near)?
                .into_iter()
                .find(|index| styles[*index].is_some()),
            None => None,
        };
        let fonts: Vec<Value> = groups
            .iter()
            .map(|group| {
                json!({
                    "font": group.style.font,
                    "embedded": group.style.embedded,
                    "size": group.style.size,
                    "colour": group.style.colour.map(wire::colour),
                    "blocks": group.blocks,
                    "chars": group.chars,
                    "nearest": nearest.is_some_and(|index| group.blocks.contains(&index)),
                })
            })
            .collect();
        let mut fields = field("revision", revision);
        fields.insert("fonts".to_owned(), Value::Array(fonts));
        fields.insert("nearest".to_owned(), json!(nearest));
        Ok(fields)
    }

    /// The matches of a query on one page. Kotlin loops over the pages, so
    /// it can stop between them.
    pub fn search(&self, request: SearchRequest) -> Result<Fields, WireError> {
        let running = self.jobs.begin(request.job, Some(request.handle))?;
        let (_, _, page) = self.text_page(request.handle, request.page, &running)?;
        let options = SearchOptions {
            case_sensitive: request.case_sensitive,
            whole_word: request.whole_word,
        };
        let matches: Vec<Value> = page
            .search(&request.query, &options)
            .iter()
            .map(|found| json!({ "rects": found.rects.iter().map(wire::rect).collect::<Vec<_>>() }))
            .collect();
        Ok(field("matches", Value::Array(matches)))
    }

    /// A page's links: where each is, and an address (`uri`) or a page of
    /// this document (`page`, with the point and zoom the link gives).
    pub fn links(&self, request: PageRequest) -> Result<Fields, WireError> {
        let (document, _) = self.snapshot(request.handle)?;
        let mut links = Vec::new();
        for link in document.links(request.page)? {
            let mut entry =
                json!({ "rects": link.rects.iter().map(wire::rect).collect::<Vec<_>>() });
            if let Some(uri) = link.uri() {
                entry["uri"] = json!(uri);
            } else if let Some(destination) = link.target() {
                if let Some(located) = document.locate(destination)? {
                    entry["page"] = json!(located.page);
                    entry["x"] = json!(located.x);
                    entry["y"] = json!(located.y);
                    entry["zoom"] = json!(located.zoom);
                }
            }
            links.push(entry);
        }
        Ok(field("links", Value::Array(links)))
    }

    /// A page's annotations. An annotation with no `id` was written
    /// directly in the page's `Annots`: no operation can name it.
    pub fn annotations(&self, request: PageRequest) -> Result<Fields, WireError> {
        let (document, _) = self.snapshot(request.handle)?;
        let annotations: Vec<Value> = document
            .annotations(request.page)?
            .iter()
            .map(|annotation| {
                let (kind, subtype) = match &annotation.kind {
                    AnnotationKind::Other(name) => ("Other".to_owned(), Some(name.clone())),
                    kind => (variant_name(kind), None),
                };
                json!({
                    "id": annotation.id.map(wire::object),
                    "kind": kind,
                    "subtype": subtype,
                    "drawn_by_library": annotation.kind.is_drawn_by_library(),
                    "rect": wire::rect(&annotation.rect),
                    "quads": annotation.quads.iter().map(wire::rect).collect::<Vec<_>>(),
                    "strokes": annotation.strokes,
                    "contents": annotation.contents,
                    "author": annotation.author,
                    "colour": annotation.colour.map(wire::colour),
                    "opacity": annotation.opacity,
                    "line_width": annotation.line_width,
                    "flags": annotation.flags,
                })
            })
            .collect();
        Ok(field("annotations", Value::Array(annotations)))
    }
}

/// Whether two blocks' styles are one entry of the page's fonts.
fn same_style(a: &BlockStyle, b: &BlockStyle) -> bool {
    a.font == b.font
        && a.embedded == b.embedded
        && (a.size / SIZE_STEP).round() == (b.size / SIZE_STEP).round()
        && a.colour == b.colour
}
