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

use std::collections::HashMap;
use std::fs::{self, File};
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

/// Stack of the thread that parses and draws an SVG, 16 MiB. It is address
/// space reserved, not memory used until touched. A debug build, whose frames
/// are the largest, draws usvg's maximum nesting on it (see `MAX_SVG_DEPTH`),
/// about thirty times the depth the scan allows.
const RENDER_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Deepest effective element nesting accepted in an SVG, the root counted as
/// 1 and every `<use>` or other reference expanded in place. usvg and resvg
/// recurse once per level. Measured with nested `<g opacity="0.9">`:
/// - debug build, 2 MiB thread: depth 130 renders, 140 overflows the stack
///   and aborts;
/// - debug build, 16 MiB thread (`RENDER_STACK_BYTES`): usvg's own limit of
///   about 1020 renders without overflow;
/// - release build, 2 MiB thread: 1020 renders without overflow.
///
/// 32 is a quarter of the lowest limit measured (debug, 2 MiB) and sits above
/// the 10 to 15 levels graphics editors write.
const MAX_SVG_DEPTH: usize = 32;

/// Most elements the SVG scan keeps. The table holds a few dozen bytes per
/// element, so this bounds its memory at tens of megabytes; no drawing a
/// person browses has a million elements.
const MAX_ELEMENTS: usize = 1_000_000;

/// Most elements instantiated through references (`<use>` copies and the
/// like) in one SVG. usvg builds a node for every copy, so a chain of
/// references that doubles at each level costs memory and time exponential
/// in the file size. 100 000 copies is a sprite sheet of thousands of icons
/// and builds in well under a second; beyond it a document is a blow-up,
/// not a picture.
const MAX_INSTANCED_ELEMENTS: usize = 100_000;

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

/// `<image>` href resolvers: `data:` raster URLs through usvg's own decoder,
/// after a size check; a relative path only when it resolves, symlinks followed,
/// to a PNG, JPEG, GIF or WebP file inside `dir`; everything else (URLs of
/// any scheme, absolute paths elsewhere, `..` escapes, nested SVG, by path or `data:`) is
/// refused and the element is left out.
fn href_resolver(dir: PathBuf) -> ImageHrefResolver<'static> {
    let inner_data = ImageHrefResolver::default_data_resolver();
    // Only PNG, JPEG, GIF and WebP reach usvg, judged by the content and not
    // by the declared type. usvg would parse a nested SVG with no depth or
    // compression checks, and this scan covers only the outer document.
    let data = Arc::new(
        move |_declared: &str, bytes: Arc<Vec<u8>>, opts: &usvg::Options| {
            let mime = raster_mime(image::guess_format(&bytes).ok()?)?;
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
            for_files("", Arc::new(bytes), opts)
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
    // A FIFO or device named in an href would block the read.
    if !fs::metadata(&real).ok()?.is_file() {
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

/// Refuses an SVG whose effective nesting is deeper than [`MAX_SVG_DEPTH`] or
/// that instantiates more than [`MAX_INSTANCED_ELEMENTS`] elements through
/// references, before any XML parser or renderer recurses through it.
///
/// usvg expands a `<use>` into a copy of its target, and a pattern, mask,
/// clip path, filter or gradient is resolved the same way, so the depth the
/// renderer sees is the document depth plus the depth of everything
/// referenced along the way. The scan builds a flat element table (one pass
/// over the bytes, no recursion) with each element's `id` and the targets of
/// its `href`, `xlink:href` and `url(#id)` references, then walks the
/// expanded tree depth first. That walk is itself bounded by the depth cap,
/// so its recursion is at most [`MAX_SVG_DEPTH`] frames.
///
/// A reference cycle is refused. A reference to an id that does not exist,
/// or to something outside the document, is skipped: usvg skips it too, so
/// it adds no depth. A document the scan cannot follow to its end (an
/// unterminated tag or comment) is refused; the parser would reject it
/// anyway, and refusing keeps a confused scan from waving through a deep
/// document. An entity whose value holds markup could add depth the scan
/// cannot see, so it is refused.
fn check_depth(path: &Path, data: &[u8]) -> Result<(), ThumbError> {
    scan(data)
        .and_then(|doc| doc.check())
        .map_err(|reason| ThumbError::decode(path, reason))
}

/// One element: its children, its `id`, and the ids it refers to.
struct Element<'a> {
    children: Vec<usize>,
    refs: Vec<&'a [u8]>,
}

struct Doc<'a> {
    elements: Vec<Element<'a>>,
    roots: Vec<usize>,
    by_id: HashMap<&'a [u8], Vec<usize>>,
}

impl Doc<'_> {
    fn check(&self) -> Result<(), String> {
        let mut instanced = 0usize;
        let mut path = Vec::new();
        for &root in &self.roots {
            self.visit(root, 1, false, &mut path, &mut instanced)?;
        }
        Ok(())
    }

    /// Visits element `i` at effective depth `depth`; `path` holds the
    /// reference targets being expanded, innermost last.
    fn visit(
        &self,
        i: usize,
        depth: usize,
        instance: bool,
        path: &mut Vec<usize>,
        instanced: &mut usize,
    ) -> Result<(), String> {
        if depth > MAX_SVG_DEPTH {
            return Err(format!("SVG nested deeper than {MAX_SVG_DEPTH} elements"));
        }
        if instance {
            *instanced += 1;
            if *instanced > MAX_INSTANCED_ELEMENTS {
                return Err(format!(
                    "SVG references expand to more than {MAX_INSTANCED_ELEMENTS} elements"
                ));
            }
        }
        let element = &self.elements[i];
        for target in element.refs.iter().filter_map(|r| self.by_id.get(r)) {
            for &t in target {
                if path.contains(&t) || t == i {
                    return Err("SVG references form a cycle".to_string());
                }
                path.push(t);
                let result = self.visit(t, depth + 1, true, path, instanced);
                path.pop();
                result?;
            }
        }
        for &child in &element.children {
            self.visit(child, depth + 1, instance, path, instanced)?;
        }
        Ok(())
    }
}

fn scan(data: &[u8]) -> Result<Doc<'_>, String> {
    const UNFINISHED: &str = "SVG ends inside a tag, comment or declaration";
    let find = |from: usize, needle: &[u8]| {
        data.get(from..)
            .and_then(|rest| rest.windows(needle.len()).position(|w| w == needle))
            .map(|p| from + p + needle.len())
            .ok_or_else(|| UNFINISHED.to_string())
    };
    let mut doc = Doc {
        elements: Vec::new(),
        roots: Vec::new(),
        by_id: HashMap::new(),
    };
    let mut open: Vec<usize> = Vec::new();
    let mut i = 0;
    while let Some(offset) = data
        .get(i..)
        .and_then(|r| r.iter().position(|&b| b == b'<'))
    {
        // Text between tags can carry CSS with `url(#id)` references.
        if let Some(&current) = open.last() {
            collect_urls(&data[i..i + offset], &mut doc.elements[current].refs);
        }
        i += offset;
        let rest = &data[i..];
        if rest.starts_with(b"<!--") {
            i = find(i + 4, b"-->")?;
        } else if rest.starts_with(b"<![CDATA[") {
            i = find(i + 9, b"]]>")?;
        } else if rest.starts_with(b"<?") {
            i = find(i + 2, b"?>")?;
        } else if rest.starts_with(b"<!") {
            i = skip_declaration(data, i + 2)?;
        } else if rest.starts_with(b"</") {
            open.pop();
            i = find(i + 2, b">")?;
        } else {
            let tag = parse_tag(data, i + 1)?;
            if open.len() + 1 > MAX_SVG_DEPTH {
                return Err(format!("SVG nested deeper than {MAX_SVG_DEPTH} elements"));
            }
            if doc.elements.len() >= MAX_ELEMENTS {
                return Err(format!("SVG has more than {MAX_ELEMENTS} elements"));
            }
            let index = doc.elements.len();
            match open.last() {
                Some(&parent) => doc.elements[parent].children.push(index),
                None => doc.roots.push(index),
            }
            if let Some(id) = tag.id {
                doc.by_id.entry(id).or_default().push(index);
            }
            doc.elements.push(Element {
                children: Vec::new(),
                refs: tag.refs,
            });
            if !tag.self_closing {
                open.push(index);
            }
            i = tag.end;
        }
    }
    Ok(doc)
}

struct Tag<'a> {
    end: usize,
    self_closing: bool,
    id: Option<&'a [u8]>,
    refs: Vec<&'a [u8]>,
}

/// Parses a start tag whose name begins at `from`: its attributes, of which
/// `id`, `href` (any prefix) and anything holding `url(#id)` matter here.
/// A `>` inside a quoted value does not end the tag.
fn parse_tag(data: &[u8], from: usize) -> Result<Tag<'_>, String> {
    const MALFORMED: &str = "malformed SVG tag";
    let is_space = |b: u8| matches!(b, b' ' | b'\t' | b'\n' | b'\r');
    let mut j = from;
    while j < data.len() && !is_space(data[j]) && !matches!(data[j], b'>' | b'/') {
        j += 1;
    }
    let mut tag = Tag {
        end: 0,
        self_closing: false,
        id: None,
        refs: Vec::new(),
    };
    loop {
        while j < data.len() && is_space(data[j]) {
            j += 1;
        }
        match data.get(j) {
            None => return Err(MALFORMED.to_string()),
            Some(b'>') => {
                tag.end = j + 1;
                return Ok(tag);
            }
            Some(b'/') if data.get(j + 1) == Some(&b'>') => {
                tag.end = j + 2;
                tag.self_closing = true;
                return Ok(tag);
            }
            Some(b'/') => return Err(MALFORMED.to_string()),
            Some(_) => {}
        }
        let name_start = j;
        while j < data.len() && !is_space(data[j]) && !matches!(data[j], b'=' | b'>' | b'/') {
            j += 1;
        }
        let name = &data[name_start..j];
        while j < data.len() && is_space(data[j]) {
            j += 1;
        }
        if data.get(j) != Some(&b'=') {
            return Err(MALFORMED.to_string());
        }
        j += 1;
        while j < data.len() && is_space(data[j]) {
            j += 1;
        }
        let quote = match data.get(j) {
            Some(&q) if q == b'"' || q == b'\'' => q,
            _ => return Err(MALFORMED.to_string()),
        };
        let value_start = j + 1;
        let value_end = data[value_start..]
            .iter()
            .position(|&b| b == quote)
            .map(|p| value_start + p)
            .ok_or_else(|| MALFORMED.to_string())?;
        let value = &data[value_start..value_end];
        j = value_end + 1;

        let local = name.rsplit(|&b| b == b':').next().unwrap_or(name);
        if local == b"id" {
            tag.id = Some(value);
        } else if local == b"href" {
            if let Some(target) = value.trim_ascii().strip_prefix(b"#") {
                tag.refs.push(target);
            }
        }
        collect_urls(value, &mut tag.refs);
    }
}

/// Adds the id of every `url(#id)` in `text` to `refs`.
fn collect_urls<'a>(text: &'a [u8], refs: &mut Vec<&'a [u8]>) {
    let mut rest = text;
    while let Some(p) = rest.windows(4).position(|w| w == b"url(") {
        rest = &rest[p + 4..];
        let inner = rest.trim_ascii_start();
        let inner = inner
            .strip_prefix(b"\"")
            .or(inner.strip_prefix(b"'"))
            .unwrap_or(inner);
        if let Some(id) = inner.strip_prefix(b"#") {
            let len = id
                .iter()
                .position(|&b| matches!(b, b')' | b'"' | b'\'' | b' ' | b'\t' | b'\n' | b'\r'))
                .unwrap_or(id.len());
            refs.push(&id[..len]);
        }
    }
}

/// End of a `<!...>` declaration (DOCTYPE) starting after `<!`, honouring
/// quoted strings, comments, processing instructions and the bracketed
/// internal subset. `Err` when a quoted value holds `<`, which only an
/// entity with markup does.
fn skip_declaration(data: &[u8], from: usize) -> Result<usize, String> {
    let mut quote = None;
    let mut brackets = 0usize;
    let mut j = from;
    while j < data.len() {
        let b = data[j];
        match quote {
            Some(q) if b == q => quote = None,
            Some(_) if b == b'<' => return Err("SVG entity containing markup".to_string()),
            Some(_) => {}
            None => match b {
                b'"' | b'\'' => quote = Some(b),
                b'[' => brackets += 1,
                b']' => brackets = brackets.saturating_sub(1),
                b'>' if brackets == 0 => return Ok(j + 1),
                b'<' if data[j..].starts_with(b"<!--") => {
                    let p = data[j + 4..].windows(3).position(|w| w == b"-->");
                    j += 4 + p.ok_or("SVG ends inside a comment")? + 2;
                }
                b'<' if data[j..].starts_with(b"<?") => {
                    let p = data[j + 2..].windows(2).position(|w| w == b"?>");
                    j += 2 + p.ok_or("SVG ends inside a processing instruction")? + 1;
                }
                _ => {}
            },
        }
        j += 1;
    }
    Err("SVG ends inside a declaration".to_string())
}

fn render_svg(path: &Path, fit: u32) -> Result<Rendered, ThumbError> {
    let mut data = Vec::new();
    File::open(path)
        .and_then(|f| f.take(MAX_INPUT_BYTES + 1).read_to_end(&mut data))
        .map_err(|e| ThumbError::io(path, e))?;
    if data.len() as u64 > MAX_INPUT_BYTES {
        return Err(ThumbError::decode(path, "SVG larger than the size limit"));
    }
    // usvg inflates anything starting with the gzip magic without a cap on
    // the output, so compressed SVG is refused here whatever the features.
    if data.starts_with(&[0x1f, 0x8b]) {
        return Err(ThumbError::decode(path, "compressed SVG is not supported"));
    }
    check_depth(path, &data)?;
    // Parsing and drawing recurse once per nesting level, so they run on a
    // thread with a stack of known size and the caller's stack never matters.
    let owned_path = path.to_path_buf();
    let worker = std::thread::Builder::new()
        .name("kernkraft-svg".to_string())
        .stack_size(RENDER_STACK_BYTES)
        .spawn(move || draw_svg(&owned_path, &data, fit))
        .map_err(|e| ThumbError::io(path, e))?;
    worker
        .join()
        .unwrap_or_else(|_| Err(ThumbError::decode(path, "the SVG renderer panicked")))
}

fn draw_svg(path: &Path, data: &[u8], fit: u32) -> Result<Rendered, ThumbError> {
    let dir = path.parent().unwrap_or(Path::new(".")).to_path_buf();
    let options = usvg::Options {
        image_href_resolver: href_resolver(dir),
        fontdb: fonts(),
        ..usvg::Options::default()
    };
    let tree = usvg::Tree::from_data(data, &options).map_err(|e| ThumbError::decode(path, e))?;

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
