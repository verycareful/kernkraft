// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The font list an application hands in once: font files, indexed by what
//! matching needs (family names, weight, width, slant, pitch, and the
//! characters each covers). Only the sfnt directory and the `name`, `OS/2`,
//! `head`, `post` and `cmap` tables are read, by offset, so indexing a list
//! of large CJK fonts reads a small part of them; the glyph tables stay on
//! disk until a font is opened.
//!
//! Formats: OpenType 1.9 (the table directory, `name`, `OS/2` `usWeightClass`,
//! `usWidthClass` and `fsSelection`, `head` `macStyle`, `post`
//! `isFixedPitch`, `cmap`) and, for a collection, the TTC header.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use kernkraft_fs::Progress;
use read_fonts::tables::cmap::{Cmap, CmapIterLimits, PlatformId};
use read_fonts::tables::name::Name;
use read_fonts::{FontData, FontRead};
use skrifa::string::{LocalizedString, StringId};

use crate::coverage::Coverage;
use crate::error::FontError;
use crate::program::Program;

/// The weight and width a font states when it states none: 400 is Regular
/// and 5 is Medium (normal) in the `OS/2` classes.
const REGULAR_WEIGHT: u16 = 400;
const BOLD_WEIGHT: u16 = 700;
const NORMAL_WIDTH: u8 = 5;
const MAX_WEIGHT: u16 = 1000;
const MIN_WIDTH: u16 = 1;
const MAX_WIDTH: u16 = 9;

/// Offsets inside the tables read (OpenType 1.9).
const SFNT_HEADER_BYTES: u64 = 12;
const TABLE_RECORD_BYTES: usize = 16;
const TTC_HEADER_BYTES: u64 = 12;
const OS2_USWEIGHTCLASS: usize = 4;
const OS2_USWIDTHCLASS: usize = 6;
const OS2_FSSELECTION: usize = 62;
/// Bytes of `OS/2` needed to reach `fsSelection`.
const OS2_PREFIX_BYTES: usize = OS2_FSSELECTION + 2;
const FS_SELECTION_ITALIC: u16 = 1 << 0;
const FS_SELECTION_OBLIQUE: u16 = 1 << 9;
const HEAD_MACSTYLE: usize = 44;
const HEAD_PREFIX_BYTES: usize = HEAD_MACSTYLE + 2;
const MAC_STYLE_BOLD: u16 = 1 << 0;
const MAC_STYLE_ITALIC: u16 = 1 << 1;
const POST_ISFIXEDPITCH: usize = 12;
const POST_PREFIX_BYTES: usize = POST_ISFIXEDPITCH + 4;
const MAXP_NUMGLYPHS: usize = 4;
const MAXP_PREFIX_BYTES: usize = MAXP_NUMGLYPHS + 2;
/// Windows `cmap` encoding ids of Unicode subtables: BMP only, and the full
/// repertoire.
const WINDOWS_UNICODE_BMP: u16 = 1;
const WINDOWS_UNICODE_FULL: u16 = 10;
const MAX_CODE_POINT: u32 = 0x10FFFF;

/// One face of a font file, as indexed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FaceInfo {
    pub path: PathBuf,
    /// The font's number in a collection; 0 for a single font.
    pub index: u32,
    /// Typographic family names first, then family names, every language,
    /// without repeats.
    pub families: Vec<String>,
    pub full_name: Option<String>,
    pub postscript_name: Option<String>,
    /// 1 to 1000; 400 is Regular, 700 Bold.
    pub weight: u16,
    /// 1 (ultra-condensed) to 9 (ultra-expanded); 5 is normal.
    pub width: u8,
    /// Italic or oblique.
    pub italic: bool,
    pub monospaced: bool,
    /// The characters the font maps. `None` for a font whose characters are
    /// not known from a Unicode `cmap` (a symbol font, a Type 1 program);
    /// such a face never matches by coverage.
    pub coverage: Option<Coverage>,
}

impl FaceInfo {
    /// Whether the font is known to have a glyph for `c`.
    pub fn covers(&self, c: char) -> bool {
        self.coverage.as_ref().is_some_and(|cov| cov.contains(c))
    }
}

/// A file that could not be indexed, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    pub path: PathBuf,
    pub error: FontError,
}

/// The indexed fonts. Faces are in the order of path, then index, so the
/// same files give the same list in any order they are handed in.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FontList {
    pub(crate) faces: Vec<FaceInfo>,
    skipped: Vec<Skipped>,
}

impl FontList {
    /// Indexes `paths`. A file that cannot be read or is not a font is not
    /// an error of the whole: it is listed in [`FontList::skipped`] with its
    /// reason. Only a cancel through `progress` fails the call.
    pub fn index(paths: &[PathBuf], progress: &dyn Progress) -> Result<FontList, FontError> {
        let total = paths.len() as u64;
        let mut list = FontList::default();
        for (done, path) in paths.iter().enumerate() {
            if progress.is_cancelled() {
                return Err(FontError::Cancelled);
            }
            progress.report(done as u64, total, "indexing fonts");
            match index_file(path) {
                Ok(faces) => list.faces.extend(faces),
                Err(error) => list.skipped.push(Skipped {
                    path: path.clone(),
                    error,
                }),
            }
        }
        progress.report(total, total, "indexing fonts");
        list.faces
            .sort_by(|a, b| (&a.path, a.index).cmp(&(&b.path, b.index)));
        list.skipped.sort_by(|a, b| a.path.cmp(&b.path));
        Ok(list)
    }

    pub fn faces(&self) -> &[FaceInfo] {
        &self.faces
    }

    pub fn skipped(&self) -> &[Skipped] {
        &self.skipped
    }
}

fn io(e: std::io::Error) -> FontError {
    FontError::Io { kind: e.kind() }
}

/// Reads `len` bytes at `offset`, refusing a range that is not inside the
/// file, so the allocation is never larger than the file.
fn read_at(file: &File, file_len: u64, offset: u64, len: u64) -> Result<Vec<u8>, FontError> {
    let end = offset
        .checked_add(len)
        .ok_or(FontError::Sfnt("a table runs past the end of the file"))?;
    if end > file_len {
        return Err(FontError::Sfnt("a table runs past the end of the file"));
    }
    let mut buf = vec![0u8; len as usize];
    file.read_exact_at(&mut buf, offset).map_err(io)?;
    Ok(buf)
}

fn be16(b: &[u8], at: usize) -> u16 {
    u16::from_be_bytes([b[at], b[at + 1]])
}

fn be32(b: &[u8], at: usize) -> u32 {
    u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

fn index_file(path: &Path) -> Result<Vec<FaceInfo>, FontError> {
    let file = File::open(path).map_err(io)?;
    let meta = file.metadata().map_err(io)?;
    if !meta.is_file() {
        return Err(FontError::UnknownFormat);
    }
    let file_len = meta.len();
    let head = read_at(&file, file_len, 0, file_len.min(4))?;
    match head.as_slice() {
        [0x00, 0x01, 0x00, 0x00] | b"true" | b"OTTO" => {
            Ok(vec![index_sfnt(&file, file_len, path, 0, 0)?])
        }
        b"ttcf" => {
            let header = read_at(&file, file_len, 0, TTC_HEADER_BYTES)?;
            let count = u64::from(be32(&header, 8));
            // Each offset is four bytes of the file, so the count is bounded
            // by the file before anything is allocated.
            let offsets = read_at(&file, file_len, TTC_HEADER_BYTES, count * 4)?;
            let mut faces = Vec::new();
            for i in 0..count as usize {
                let at = u64::from(be32(&offsets, i * 4));
                faces.push(index_sfnt(&file, file_len, path, at, i as u32)?);
            }
            Ok(faces)
        }
        [b'%', b'!', ..] | [0x80, 0x01, ..] => {
            let data = read_at(&file, file_len, 0, file_len)?;
            Ok(vec![index_type1(path, &data)?])
        }
        _ => Err(FontError::UnknownFormat),
    }
}

struct Directory {
    tables: Vec<([u8; 4], u64, u64)>,
}

impl Directory {
    fn find(&self, tag: &[u8; 4]) -> Option<(u64, u64)> {
        self.tables.iter().find(|t| &t.0 == tag).map(|t| (t.1, t.2))
    }

    /// The first `max` bytes of a table, or all of it when it is shorter;
    /// `None` when the font has no such table.
    fn read(
        &self,
        file: &File,
        file_len: u64,
        tag: &[u8; 4],
        max: u64,
    ) -> Result<Option<Vec<u8>>, FontError> {
        match self.find(tag) {
            None => Ok(None),
            Some((offset, len)) => read_at(file, file_len, offset, len.min(max)).map(Some),
        }
    }
}

fn directory(file: &File, file_len: u64, at: u64) -> Result<Directory, FontError> {
    let header = read_at(file, file_len, at, SFNT_HEADER_BYTES)?;
    match &header[..4] {
        [0x00, 0x01, 0x00, 0x00] | b"true" | b"OTTO" => {}
        _ => return Err(FontError::UnknownFormat),
    }
    let count = u64::from(be16(&header, 4));
    let records = read_at(
        file,
        file_len,
        at + SFNT_HEADER_BYTES,
        count * TABLE_RECORD_BYTES as u64,
    )?;
    let mut tables = Vec::with_capacity(count as usize);
    for r in records.as_chunks::<TABLE_RECORD_BYTES>().0 {
        tables.push((
            [r[0], r[1], r[2], r[3]],
            u64::from(be32(r, 8)),
            u64::from(be32(r, 12)),
        ));
    }
    Ok(Directory { tables })
}

fn index_sfnt(
    file: &File,
    file_len: u64,
    path: &Path,
    at: u64,
    index: u32,
) -> Result<FaceInfo, FontError> {
    let dir = directory(file, file_len, at)?;
    let name_data = dir
        .read(file, file_len, b"name", u64::MAX)?
        .ok_or(FontError::Sfnt("the font has no name table"))?;
    let names = Name::read(FontData::new(&name_data))
        .map_err(|_| FontError::Sfnt("the name table is damaged"))?;
    let collect = |id: StringId| -> Vec<String> {
        names
            .name_record()
            .iter()
            .filter(|r| r.name_id() == id)
            .map(|r| LocalizedString::new(&names, r).chars().collect::<String>())
            .filter(|s| !s.is_empty())
            .collect()
    };
    let mut families: Vec<String> = Vec::new();
    for name in collect(StringId::TYPOGRAPHIC_FAMILY_NAME)
        .into_iter()
        .chain(collect(StringId::FAMILY_NAME))
    {
        if !families.contains(&name) {
            families.push(name);
        }
    }
    if families.is_empty() {
        return Err(FontError::Sfnt(
            "the font has no family name this crate decodes",
        ));
    }
    let first = |id: StringId| collect(id).into_iter().next();
    let maxp = dir
        .read(file, file_len, b"maxp", MAXP_PREFIX_BYTES as u64)?
        .filter(|t| t.len() >= MAXP_PREFIX_BYTES)
        .ok_or(FontError::Sfnt("the font has no maxp table"))?;
    let glyph_count = u32::from(be16(&maxp, MAXP_NUMGLYPHS));

    let os2 = dir
        .read(file, file_len, b"OS/2", OS2_PREFIX_BYTES as u64)?
        .filter(|t| t.len() >= OS2_PREFIX_BYTES);
    let head = dir
        .read(file, file_len, b"head", HEAD_PREFIX_BYTES as u64)?
        .filter(|t| t.len() >= HEAD_PREFIX_BYTES);
    let mac_style = head.as_ref().map_or(0, |h| be16(h, HEAD_MACSTYLE));
    let weight = match os2.as_ref().map(|t| be16(t, OS2_USWEIGHTCLASS)) {
        Some(w) if (1..=MAX_WEIGHT).contains(&w) => w,
        _ if mac_style & MAC_STYLE_BOLD != 0 => BOLD_WEIGHT,
        _ => REGULAR_WEIGHT,
    };
    let width = match os2.as_ref().map(|t| be16(t, OS2_USWIDTHCLASS)) {
        Some(w) if (MIN_WIDTH..=MAX_WIDTH).contains(&w) => w as u8,
        _ => NORMAL_WIDTH,
    };
    let italic = match &os2 {
        Some(t) => be16(t, OS2_FSSELECTION) & (FS_SELECTION_ITALIC | FS_SELECTION_OBLIQUE) != 0,
        None => mac_style & MAC_STYLE_ITALIC != 0,
    };
    let monospaced = dir
        .read(file, file_len, b"post", POST_PREFIX_BYTES as u64)?
        .filter(|t| t.len() >= POST_PREFIX_BYTES)
        .is_some_and(|t| be32(&t, POST_ISFIXEDPITCH) != 0);

    let coverage = match dir.read(file, file_len, b"cmap", u64::MAX)? {
        None => None,
        Some(data) => unicode_coverage(&data, glyph_count),
    };
    Ok(FaceInfo {
        path: path.to_path_buf(),
        index,
        families,
        full_name: first(StringId::FULL_NAME),
        postscript_name: first(StringId::POSTSCRIPT_NAME),
        weight,
        width,
        italic,
        monospaced,
        coverage,
    })
}

/// The union of every Unicode subtable of a `cmap`; `None` when it has none.
/// A subtable is read within the largest character Unicode has and the
/// glyph count of the font, so a damaged range cannot name billions of
/// pairs.
fn unicode_coverage(data: &[u8], glyph_count: u32) -> Option<Coverage> {
    let cmap = Cmap::read(FontData::new(data)).ok()?;
    let limits = CmapIterLimits {
        max_char: MAX_CODE_POINT,
        glyph_count,
    };
    let mut points = Vec::new();
    let mut any = false;
    for record in cmap.encoding_records() {
        let unicode = matches!(
            (record.platform_id(), record.encoding_id()),
            (PlatformId::Unicode, _)
                | (
                    PlatformId::Windows,
                    WINDOWS_UNICODE_BMP | WINDOWS_UNICODE_FULL
                )
        );
        if !unicode {
            continue;
        }
        let Ok(sub) = record.subtable(cmap.offset_data()) else {
            continue;
        };
        any = true;
        // A character that maps to glyph 0 (`.notdef`) is not covered; the
        // closing 0xFFFF segment of a format 4 subtable maps there.
        points.extend(
            sub.iter_with_limits(limits)
                .filter(|(_, g)| g.to_u32() != 0)
                .map(|(c, _)| c),
        );
    }
    any.then(|| Coverage::from_code_points(points))
}

fn index_type1(path: &Path, data: &[u8]) -> Result<FaceInfo, FontError> {
    let program = Program::parse(data, 0)?;
    let names = program.names();
    let metrics = program.metrics();
    let family = names
        .family
        .clone()
        .or_else(|| names.postscript.clone())
        .ok_or(FontError::Type1("the font names no family"))?;
    Ok(FaceInfo {
        path: path.to_path_buf(),
        index: 0,
        families: vec![family],
        full_name: names.full,
        postscript_name: names.postscript,
        weight: REGULAR_WEIGHT,
        width: NORMAL_WIDTH,
        italic: metrics.is_italic.unwrap_or(false),
        monospaced: metrics.is_monospaced.unwrap_or(false),
        coverage: None,
    })
}
