// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Decoding and scaling: raster images through `image`, SVG through `resvg`.
//!
//! Thumbnail Managing Standard 0.9.0, "Thumbnail Saving": a thumbnail fits
//! its size class and keeps the source's aspect ratio. A source smaller than
//! the class is not enlarged. An image carrying an EXIF orientation is
//! rotated to display upright, since the thumbnail has no orientation tag.
//!
//! SVG 1.1 (W3C Recommendation) `<image>` elements may reference other files
//! and URLs. A thumbnailer opens files a user merely browsed past, so nothing
//! outside the SVG's own directory is ever read and no URL is ever followed;
//! `data:` URLs are decoded in place.

use std::fs::File;
use std::io::{BufReader, Cursor, Read};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use image::{ImageDecoder, ImageFormat, ImageReader, Limits};
use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb, ImageHrefResolver};

use crate::cache::{store_failure, store_thumbnail, Source};
use crate::{ThumbError, ThumbSize};

/// Widest or tallest raster accepted. WebP cannot exceed 16383 (RFC 9649
/// section 2.2) and no thumbnail source above 16384 is a photograph a person
/// browses; larger headers are the shape of a decompression bomb.
const MAX_SIDE: u32 = 16384;

/// Allocation cap for one decode, 512 MiB: room for a 100 megapixel RGBA8
/// image (400 MB) and nothing like the gigabytes a forged header can ask for.
const MAX_ALLOC: u64 = 512 * 1024 * 1024;

/// Largest SVG or embedded file read, in bytes. A hand-made or exported SVG
/// is far below this; the cap bounds memory and parse time.
const MAX_INPUT_BYTES: u64 = 32 * 1024 * 1024;

/// Whether [`generate`] handles `mime`.
pub fn can_thumbnail(mime: &str) -> bool {
    matches!(
        mime,
        "image/png" | "image/jpeg" | "image/webp" | "image/gif" | "image/svg+xml"
    )
}

/// Makes the thumbnail of `path` (of MIME type `mime`) at `size`, stores it
/// in the cache under `cache_home`, and returns its path.
///
/// The image is scaled down to fit `size.pixels()` on its longer side,
/// keeping its aspect ratio, and never scaled up. The PNG carries
/// `Thumb::URI`, `Thumb::MTime`, `Thumb::Size` and `Software`. If the file
/// cannot be decoded, a failure entry is stored so [`crate::lookup`] reports
/// [`crate::Lookup::Failed`], and the result is [`ThumbError::Decode`].
pub fn generate(
    cache_home: &Path,
    path: &Path,
    mime: &str,
    size: ThumbSize,
) -> Result<PathBuf, ThumbError> {
    if !can_thumbnail(mime) {
        return Err(ThumbError::Unsupported {
            mime: mime.to_string(),
        });
    }
    // Taken before decoding: if the file changes meanwhile, the recorded
    // time is the older one and the thumbnail reads as stale.
    let source = Source::of(path)?;
    let rendered = if mime == "image/svg+xml" {
        render_svg(path, size.pixels())
    } else {
        render_raster(path, size.pixels())
    };
    match rendered {
        Ok(img) => store_thumbnail(cache_home, &source, size, img.width, img.height, &img.rgba),
        Err(ThumbError::Decode { path, reason }) => {
            let reason = match store_failure(cache_home, &source) {
                Ok(()) => reason,
                Err(e) => format!("{reason} (failure entry not stored: {e})"),
            };
            Err(ThumbError::Decode { path, reason })
        }
        Err(other) => Err(other),
    }
}

/// Straight-alpha RGBA8 pixels.
struct Rendered {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
}

fn limits() -> Limits {
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(MAX_ALLOC);
    limits
}

fn is_supported_raster(format: ImageFormat) -> bool {
    matches!(
        format,
        ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::WebP | ImageFormat::Gif
    )
}

fn render_raster(path: &Path, fit: u32) -> Result<Rendered, ThumbError> {
    let file = File::open(path).map_err(|e| ThumbError::io(path, e))?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(|e| ThumbError::io(path, e))?;
    match reader.format() {
        Some(format) if is_supported_raster(format) => {}
        Some(format) => {
            return Err(ThumbError::decode(path, format!("unsupported {format:?}")));
        }
        None => return Err(ThumbError::decode(path, "unrecognised image data")),
    }
    let mut decoder = reader
        .into_decoder()
        .map_err(|e| ThumbError::decode(path, e))?;
    decoder
        .set_limits(limits())
        .map_err(|e| ThumbError::decode(path, e))?;
    // Orientation metadata that cannot be read leaves the image as stored.
    let orientation = decoder.orientation().ok();
    let mut img =
        image::DynamicImage::from_decoder(decoder).map_err(|e| ThumbError::decode(path, e))?;
    if img.width() > fit || img.height() > fit {
        img = img.thumbnail(fit, fit);
    }
    if let Some(orientation) = orientation {
        img.apply_orientation(orientation);
    }
    let rgba = img.to_rgba8();
    Ok(Rendered {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// System fonts, loaded once: reading the font directories is the slow part
/// of the first SVG with text and is not repeated.
fn fonts() -> Arc<fontdb::Database> {
    static FONTS: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    FONTS
        .get_or_init(|| {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            Arc::new(db)
        })
        .clone()
}

/// Mime type usvg's data resolver expects for a raster format.
fn raster_mime(format: ImageFormat) -> Option<&'static str> {
    match format {
        ImageFormat::Png => Some("image/png"),
        ImageFormat::Jpeg => Some("image/jpeg"),
        ImageFormat::Gif => Some("image/gif"),
        ImageFormat::WebP => Some("image/webp"),
        _ => None,
    }
}

/// Whether the embedded raster `data` has a header within the decode limits.
/// Data that is not a raster at all is not this check's concern.
fn raster_within_limits(data: &[u8]) -> bool {
    let Ok(reader) = ImageReader::new(Cursor::new(data)).with_guessed_format() else {
        return true;
    };
    if !reader.format().is_some_and(is_supported_raster) {
        return true;
    }
    match reader.into_dimensions() {
        Ok((w, h)) => {
            w <= MAX_SIDE && h <= MAX_SIDE && u64::from(w) * u64::from(h) * 4 <= MAX_ALLOC
        }
        Err(_) => false,
    }
}

/// `<image>` href resolvers: `data:` URLs through usvg's own decoder, after a
/// size check; a relative path only when it resolves, symlinks followed,
/// to a PNG, JPEG, GIF or WebP file inside `dir`; everything else (URLs of
/// any scheme, absolute paths elsewhere, `..` escapes, nested SVG files) is
/// refused and the element is left out.
fn href_resolver(dir: PathBuf) -> ImageHrefResolver<'static> {
    let inner_data = ImageHrefResolver::default_data_resolver();
    let data = Arc::new(
        move |mime: &str, bytes: Arc<Vec<u8>>, opts: &usvg::Options| {
            if raster_within_limits(&bytes) {
                inner_data(mime, bytes, opts)
            } else {
                None
            }
        },
    );
    let for_files = Arc::clone(&data);
    ImageHrefResolver {
        resolve_data: Box::new(move |mime, bytes, opts| data(mime, bytes, opts)),
        resolve_string: Box::new(move |href, opts| {
            let bytes = read_inside(&dir, href)?;
            let mime = raster_mime(image::guess_format(&bytes).ok()?)?;
            for_files(mime, Arc::new(bytes), opts)
        }),
    }
}

/// The bytes of `href` if it is a plain relative or absolute path whose real
/// location is inside `dir`, else `None`.
fn read_inside(dir: &Path, href: &str) -> Option<Vec<u8>> {
    // A scheme (`http:`, `file:`, `ftp:`) before the first `/` marks a URL.
    let head = href.split('/').next().unwrap_or("");
    if head.contains(':') {
        return None;
    }
    let real_dir = dir.canonicalize().ok()?;
    let real = dir.join(href).canonicalize().ok()?;
    if !real.starts_with(&real_dir) {
        return None;
    }
    let mut bytes = Vec::new();
    File::open(&real)
        .ok()?
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .ok()?;
    (bytes.len() as u64 <= MAX_INPUT_BYTES).then_some(bytes)
}

fn render_svg(path: &Path, fit: u32) -> Result<Rendered, ThumbError> {
    let mut data = Vec::new();
    File::open(path)
        .and_then(|f| f.take(MAX_INPUT_BYTES + 1).read_to_end(&mut data))
        .map_err(|e| ThumbError::io(path, e))?;
    if data.len() as u64 > MAX_INPUT_BYTES {
        return Err(ThumbError::decode(path, "SVG larger than the size limit"));
    }
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let options = usvg::Options {
        image_href_resolver: href_resolver(dir),
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(&data, &options).map_err(|e| ThumbError::decode(path, e))?;

    let size = tree.size();
    let (w, h) = (size.width(), size.height());
    let scale = (fit as f32 / w).min(fit as f32 / h).min(1.0);
    let out_w = ((w * scale).round() as u32).clamp(1, fit);
    let out_h = ((h * scale).round() as u32).clamp(1, fit);
    let mut pixmap =
        Pixmap::new(out_w, out_h).ok_or_else(|| ThumbError::decode(path, "empty render target"))?;
    resvg::render(
        &tree,
        Transform::from_scale(out_w as f32 / w, out_h as f32 / h),
        &mut pixmap.as_mut(),
    );
    // tiny-skia stores premultiplied alpha; PNG stores straight alpha.
    let mut rgba = Vec::with_capacity(pixmap.pixels().len() * 4);
    for pixel in pixmap.pixels() {
        let c = pixel.demultiply();
        rgba.extend_from_slice(&[c.red(), c.green(), c.blue(), c.alpha()]);
    }
    Ok(Rendered {
        width: out_w,
        height: out_h,
        rgba,
    })
}
