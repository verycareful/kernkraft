// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Opening and closing a document, and reading a page: its size, text,
//! search matches, links and annotations.

use kernkraft_fs::NoProgress;
use kernkraft_pdf::text::SearchOptions;
use kernkraft_pdf::{AnnotationKind, EditSession, Editability};
use serde::Deserialize;
use serde_json::{json, Value};

use crate::engine::{Engine, HandleRequest, PageRequest, Session};
use crate::error::{variant_name, WireError};
use crate::fields::{field, Fields};
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

    /// A page's text in reading order. `lines` carry the box of each glyph;
    /// `blocks` say whether each can be edited and, when not, why. The
    /// `revision` is the one a block index is good at.
    pub fn text(&self, request: PageRequest) -> Result<Fields, WireError> {
        let (document, revision, page) = self.text_page(request.handle, request.page)?;
        let mut lines = Vec::new();
        let mut blocks = Vec::new();
        for (index, block) in page.blocks.iter().enumerate() {
            for line in &block.lines {
                lines.push(json!({
                    "block": index,
                    "text": line.text(),
                    "rect": wire::rect(&line.rect),
                    "glyphs": line.chars.iter().map(|c| wire::rect(&c.rect)).collect::<Vec<_>>(),
                }));
            }
            let mut entry = json!({ "id": index, "rect": wire::rect(&block.rect) });
            match document.text_block_editability(request.page, block, &NoProgress)? {
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

    /// The matches of a query on one page. Kotlin loops over the pages, so
    /// it can stop between them.
    pub fn search(&self, request: SearchRequest) -> Result<Fields, WireError> {
        let (_, _, page) = self.text_page(request.handle, request.page)?;
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
