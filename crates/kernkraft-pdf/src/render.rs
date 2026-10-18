// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Page rendering through MuPDF (feature `render`).
//!
//! The contract matches Kern Android's `renderPage(index, widthPx)`, with
//! 1-based pages: a page is rendered to a requested pixel width, its height
//! following the page's aspect ratio, as opaque RGBA8 on a white page. Page
//! sizes come from MuPDF's page bounds, which already account for the
//! page's `/Rotate`.

use std::path::Path;

use mupdf::{Colorspace, Document, Matrix};

use crate::PdfError;

/// Largest width or height rendered, in pixels: the 2D image side limit
/// common desktop GPUs report (Vulkan `maxImageDimension2D` = 16384), so a
/// rendered page can always be uploaded as one texture. It also bounds the
/// buffer: 16384 x 16384 x 4 bytes is 1 GiB.
pub const MAX_RENDER_DIMENSION: u32 = 16384;

const RGB_CHANNELS: usize = 3;
const RGBA_CHANNELS: usize = 4;
const OPAQUE: u8 = 255;

/// A rendered page: `width * height` pixels of straight (not premultiplied)
/// RGBA8, row by row from the top, every alpha 255.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RenderedPage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

/// An open document for rendering many pages without reopening the file.
/// MuPDF state is per thread, so a `Renderer` stays on the thread that
/// opened it.
pub struct Renderer {
    doc: Document,
    pages: u32,
}

impl Renderer {
    /// Opens the document at `path`. A missing file is
    /// `PdfError::Fs(NotFound)`; an encrypted one [`PdfError::Encrypted`];
    /// anything MuPDF cannot open [`PdfError::Render`].
    pub fn open(path: &Path) -> Result<Renderer, PdfError> {
        std::fs::metadata(path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                kernkraft_fs::FsError::NotFound {
                    path: path.to_path_buf(),
                }
            } else {
                kernkraft_fs::FsError::Io {
                    path: path.to_path_buf(),
                    source: e,
                }
            }
        })?;
        let doc = Document::open(path).map_err(render_error)?;
        if doc.needs_password().map_err(render_error)? {
            return Err(PdfError::Encrypted);
        }
        let count = doc.page_count().map_err(render_error)?;
        let pages = u32::try_from(count)
            .map_err(|_| PdfError::Render(format!("MuPDF reports {count} pages")))?;
        if pages == 0 {
            return Err(PdfError::NoPages);
        }
        Ok(Renderer { doc, pages })
    }

    pub fn page_count(&self) -> u32 {
        self.pages
    }

    /// Width and height of 1-based `page` in points, as displayed.
    pub fn page_size_pt(&self, page: u32) -> Result<(f32, f32), PdfError> {
        let bounds = self.load(page)?.bounds().map_err(render_error)?;
        Ok((bounds.x1 - bounds.x0, bounds.y1 - bounds.y0))
    }

    /// Renders 1-based `page` at `width_px` pixels wide.
    ///
    /// `width_px` of 0, or a width or resulting height above
    /// [`MAX_RENDER_DIMENSION`], is [`PdfError::Invalid`]; so is a page with
    /// no area.
    pub fn render_page(&self, page: u32, width_px: u32) -> Result<RenderedPage, PdfError> {
        if width_px == 0 || width_px > MAX_RENDER_DIMENSION {
            return Err(PdfError::Invalid(format!(
                "render width {width_px} is outside 1-{MAX_RENDER_DIMENSION}"
            )));
        }
        let loaded = self.load(page)?;
        let bounds = loaded.bounds().map_err(render_error)?;
        let (w_pt, h_pt) = (bounds.x1 - bounds.x0, bounds.y1 - bounds.y0);
        if !(w_pt > 0.0 && h_pt > 0.0) {
            return Err(PdfError::Invalid(format!("page {page} has no area")));
        }
        let scale = width_px as f32 / w_pt;
        let height_px = (h_pt * scale).round();
        if height_px > MAX_RENDER_DIMENSION as f32 {
            return Err(PdfError::Invalid(format!(
                "page {page} would render {height_px} pixels tall at width {width_px}"
            )));
        }
        let pixmap = loaded
            .to_pixmap(
                &Matrix::new_scale(scale, scale),
                &Colorspace::device_rgb(),
                false,
                true,
            )
            .map_err(render_error)?;
        let (width, height) = (pixmap.width(), pixmap.height());
        let stride = usize::try_from(pixmap.stride())
            .map_err(|_| PdfError::Render("negative pixmap stride".to_string()))?;
        if usize::from(pixmap.n()) != RGB_CHANNELS {
            return Err(PdfError::Render(format!(
                "pixmap has {} channels, expected {RGB_CHANNELS}",
                pixmap.n()
            )));
        }
        let samples = pixmap.samples();
        let row_len = width as usize * RGB_CHANNELS;
        let mut rgba = Vec::with_capacity(width as usize * height as usize * RGBA_CHANNELS);
        for row in samples.chunks(stride).take(height as usize) {
            for px in row[..row_len].as_chunks::<RGB_CHANNELS>().0 {
                rgba.extend_from_slice(px);
                rgba.push(OPAQUE);
            }
        }
        Ok(RenderedPage {
            width,
            height,
            rgba,
        })
    }

    fn load(&self, page: u32) -> Result<mupdf::Page, PdfError> {
        if page == 0 || page > self.pages {
            return Err(PdfError::PageOutOfRange {
                page,
                count: self.pages,
            });
        }
        // `page <= self.pages`, which came from an i32.
        let index = i32::try_from(page - 1).unwrap_or(i32::MAX);
        self.doc.load_page(index).map_err(render_error)
    }
}

/// Renders one page of the file at `path`. See [`Renderer::render_page`].
pub fn render_page(path: &Path, page: u32, width_px: u32) -> Result<RenderedPage, PdfError> {
    Renderer::open(path)?.render_page(page, width_px)
}

/// Size of one page of the file at `path`. See [`Renderer::page_size_pt`].
pub fn page_size_pt(path: &Path, page: u32) -> Result<(f32, f32), PdfError> {
    Renderer::open(path)?.page_size_pt(page)
}

fn render_error(e: mupdf::Error) -> PdfError {
    PdfError::Render(e.to_string())
}
