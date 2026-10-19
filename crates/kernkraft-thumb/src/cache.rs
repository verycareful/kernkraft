// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The freedesktop thumbnail cache: names, lookup and the PNG writer.
//!
//! Thumbnail Managing Standard 0.9.0:
//! - "Thumbnail Directory": `$XDG_CACHE_HOME/thumbnails/<size>/`, sizes
//!   `normal` 128, `large` 256, `x-large` 512, `xx-large` 1024 pixels; the
//!   directories are `0700` and the files `0600`.
//! - "Thumbnail Saving": the file name is the lowercase hex MD5 of the
//!   canonical file URI plus `.png`.
//! - "Thumbnail Attributes": `Thumb::URI` and `Thumb::MTime` are required
//!   `tEXt` keys; a thumbnail is current only while both match the source.
//!   `Thumb::Size` and `Software` are optional and written here.
//! - "Failures": `thumbnails/fail/<application>/` holds an empty PNG with the
//!   same attributes for a file that could not be thumbnailed.
//!
//! PNG 1.2 / ISO 15948 section 11.3.4.3 defines the `tEXt` chunk; it is
//! Latin-1, and every value written here is ASCII.

use std::fs::{self, DirBuilder, File, Permissions};
use std::io::{BufReader, ErrorKind};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

use kernkraft_fs::write_bytes_atomic;
use md5::{Digest, Md5};
use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::ThumbError;

/// Name of this application's directory under `thumbnails/fail/`. The version
/// is part of it so a newer thumbnailer retries what an older one gave up on.
pub const FAIL_DIR: &str = concat!("kern-", env!("CARGO_PKG_VERSION"));

/// Directory under the cache home that holds every thumbnail directory.
const THUMBNAILS: &str = "thumbnails";
const FAIL: &str = "fail";

/// Mode of directories this crate creates (standard, "Thumbnail Directory").
const DIR_MODE: u32 = 0o700;
/// Mode of thumbnail files (standard, "Thumbnail Directory").
const FILE_MODE: u32 = 0o600;

pub(crate) const KEY_URI: &str = "Thumb::URI";
pub(crate) const KEY_MTIME: &str = "Thumb::MTime";
pub(crate) const KEY_SIZE: &str = "Thumb::Size";
pub(crate) const KEY_SOFTWARE: &str = "Software";
pub(crate) const SOFTWARE: &str = "kernkraft";

/// RFC 3986 section 2.3 unreserved characters and the path separator stay as
/// they are; every other byte of the path becomes `%XX`.
const URI_KEEP: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// A thumbnail size class (standard, "Thumbnail Directory").
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ThumbSize {
    Normal,
    Large,
    XLarge,
    XxLarge,
}

impl ThumbSize {
    /// The longest side of a thumbnail of this class, in pixels.
    pub fn pixels(self) -> u32 {
        match self {
            ThumbSize::Normal => 128,
            ThumbSize::Large => 256,
            ThumbSize::XLarge => 512,
            ThumbSize::XxLarge => 1024,
        }
    }

    /// The directory under `thumbnails/` that holds this class.
    pub fn dir_name(self) -> &'static str {
        match self {
            ThumbSize::Normal => "normal",
            ThumbSize::Large => "large",
            ThumbSize::XLarge => "x-large",
            ThumbSize::XxLarge => "xx-large",
        }
    }
}

/// What the cache holds for a file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Lookup {
    /// A thumbnail whose recorded URI and modification time match the file.
    Fresh(PathBuf),
    /// A failure entry for this version of the file: thumbnailing it failed.
    Failed,
    /// Nothing usable; [`crate::generate`] can make one.
    Missing,
}

/// The `file://` URI of `path` (made absolute if it is not), with every byte
/// outside the unreserved set and `/` percent-encoded, so a name that is not
/// UTF-8 maps to a URI like any other.
pub fn file_uri(path: &Path) -> String {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    format!(
        "file://{}",
        percent_encode(absolute.as_os_str().as_bytes(), URI_KEEP)
    )
}

/// Where the thumbnail for `uri` lives: lowercase hex MD5 of the URI, plus
/// `.png`, in the size's directory.
pub fn thumb_path(cache_home: &Path, uri: &str, size: ThumbSize) -> PathBuf {
    cache_home
        .join(THUMBNAILS)
        .join(size.dir_name())
        .join(name_for(uri))
}

fn fail_path(cache_home: &Path, uri: &str) -> PathBuf {
    cache_home
        .join(THUMBNAILS)
        .join(FAIL)
        .join(FAIL_DIR)
        .join(name_for(uri))
}

fn name_for(uri: &str) -> String {
    let digest = Md5::digest(uri.as_bytes());
    let mut name = String::with_capacity(digest.len() * 2 + 4);
    for byte in digest {
        name.push_str(&format!("{byte:02x}"));
    }
    name.push_str(".png");
    name
}

/// Whether the cache holds a current thumbnail of `path` at `size`, a
/// failure entry for it, or nothing. A thumbnail another file manager wrote
/// counts when its `Thumb::URI` equals the URI of `path` and its
/// `Thumb::MTime` equals the file's modification time in whole seconds. A
/// cache file that is not a readable PNG counts as missing.
pub fn lookup(cache_home: &Path, path: &Path, size: ThumbSize) -> Result<Lookup, ThumbError> {
    let meta = fs::metadata(path).map_err(|e| ThumbError::io(path, e))?;
    let uri = file_uri(path);
    let mtime = meta.mtime().to_string();

    let thumb = thumb_path(cache_home, &uri, size);
    if attributes_match(&thumb, &uri, &mtime)? {
        return Ok(Lookup::Fresh(thumb));
    }
    if attributes_match(&fail_path(cache_home, &uri), &uri, &mtime)? {
        return Ok(Lookup::Failed);
    }
    Ok(Lookup::Missing)
}

/// Reads only the PNG chunks before the image data (where text attributes
/// are written) and compares the two required keys.
fn attributes_match(file: &Path, uri: &str, mtime: &str) -> Result<bool, ThumbError> {
    let reader = match File::open(file) {
        Ok(f) => BufReader::new(f),
        Err(e) if e.kind() == ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(ThumbError::io(file, e)),
    };
    let Ok(decoder) = png::Decoder::new(reader).read_info() else {
        return Ok(false);
    };
    let info = decoder.info();
    let mut found_uri = None;
    let mut found_mtime = None;
    let mut record = |key: &str, text: String| match key {
        KEY_URI => found_uri = Some(text),
        KEY_MTIME => found_mtime = Some(text),
        _ => {}
    };
    for chunk in &info.uncompressed_latin1_text {
        record(&chunk.keyword, chunk.text.clone());
    }
    for chunk in &info.utf8_text {
        if let Ok(text) = chunk.get_text() {
            record(&chunk.keyword, text);
        }
    }
    Ok(found_uri.as_deref() == Some(uri) && found_mtime.as_deref() == Some(mtime))
}

/// The attributes of the source file a thumbnail records.
pub(crate) struct Source {
    pub uri: String,
    pub mtime: i64,
    pub size: u64,
}

impl Source {
    pub fn of(path: &Path) -> Result<Source, ThumbError> {
        let meta = fs::metadata(path).map_err(|e| ThumbError::io(path, e))?;
        Ok(Source {
            uri: file_uri(path),
            mtime: meta.mtime(),
            size: meta.len(),
        })
    }
}

/// Encodes straight-alpha RGBA8 pixels as a PNG carrying the attributes.
fn encode_png(
    source: &Source,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<Vec<u8>, png::EncodingError> {
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.add_text_chunk(KEY_URI.to_string(), source.uri.clone())?;
    encoder.add_text_chunk(KEY_MTIME.to_string(), source.mtime.to_string())?;
    encoder.add_text_chunk(KEY_SIZE.to_string(), source.size.to_string())?;
    encoder.add_text_chunk(KEY_SOFTWARE.to_string(), SOFTWARE.to_string())?;
    let mut writer = encoder.write_header()?;
    writer.write_image_data(rgba)?;
    writer.finish()?;
    Ok(out)
}

/// Writes a thumbnail of `width` x `height` RGBA8 pixels for `source`.
pub(crate) fn store_thumbnail(
    cache_home: &Path,
    source: &Source,
    size: ThumbSize,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<PathBuf, ThumbError> {
    let target = thumb_path(cache_home, &source.uri, size);
    store(&target, source, width, height, rgba)?;
    Ok(target)
}

/// Writes the failure entry for `source`: a transparent 1x1 image.
pub(crate) fn store_failure(cache_home: &Path, source: &Source) -> Result<(), ThumbError> {
    store(
        &fail_path(cache_home, &source.uri),
        source,
        1,
        1,
        &[0, 0, 0, 0],
    )
}

fn store(
    target: &Path,
    source: &Source,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<(), ThumbError> {
    let bytes = encode_png(source, width, height, rgba).map_err(|e| {
        ThumbError::io(
            target,
            std::io::Error::new(ErrorKind::InvalidData, e.to_string()),
        )
    })?;
    if let Some(dir) = target.parent() {
        DirBuilder::new()
            .recursive(true)
            .mode(DIR_MODE)
            .create(dir)
            .map_err(|e| ThumbError::io(dir, e))?;
    }
    write_bytes_atomic(target, None, &bytes)?;
    // The atomic writer keeps an existing file's mode and gives a new file
    // 0666 less the umask, so the mode is set here either way.
    fs::set_permissions(target, Permissions::from_mode(FILE_MODE))
        .map_err(|e| ThumbError::io(target, e))
}
