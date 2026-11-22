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
//! `isFixedPitch`, `cmap`, and `fvar` for the named instances of a variable
//! font) and, for a collection, the TTC header.

use std::fs::File;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

use kernkraft_fs::Progress;
use read_fonts::tables::cmap::{Cmap, CmapIterLimits, PlatformId};
use read_fonts::tables::fvar::Fvar;
use read_fonts::tables::name::Name;
use read_fonts::{FontData, FontRead};
use skrifa::string::{LocalizedString, StringId};

use crate::coverage::Coverage;
use crate::error::FontError;
use crate::program::Program;
use crate::variation::{ascii_alphanumeric, named_instance_name, AxisTag, Variations};

/// The weight and width a font states when it states none: 400 is Regular
/// and 5 is Medium (normal) in the `OS/2` classes.
const REGULAR_WEIGHT: u16 = 400;
const BOLD_WEIGHT: u16 = 700;
const NORMAL_WIDTH: u8 = 5;
const MAX_WEIGHT: u16 = 1000;
const MIN_WIDTH: u16 = 1;
const MAX_WIDTH: u16 = 9;

/// The `wght` axis is the weight on the scale of `usWeightClass`; the `wdth`
/// axis is a percentage of normal width, which `usWidthClass` sets in classes
/// (OpenType 1.9, `OS/2`): ultra-condensed 50, extra-condensed 62.5,
/// condensed 75, semi-condensed 87.5, medium 100, semi-expanded 112.5,
/// expanded 125, extra-expanded 150 and ultra-expanded 200.
const AXIS_WIDTH: AxisTag = *b"wdth";
const AXIS_WEIGHT_TAG: AxisTag = crate::variation::AXIS_WEIGHT;
const AXIS_ITALIC_TAG: AxisTag = crate::variation::AXIS_ITALIC;
const AXIS_SLANT_TAG: AxisTag = crate::variation::AXIS_SLANT;
/// The least weight an instance's `wght` value is taken as.
const MIN_INSTANCE_WEIGHT: u16 = 1;
const WIDTH_CLASS_PERCENT: [f64; 9] = [50.0, 62.5, 75.0, 87.5, 100.0, 112.5, 125.0, 150.0, 200.0];
/// The `fvar` coordinates are 16.16 fixed point.
const FIXED_ONE: f64 = 65536.0;

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
    /// The named instance of a variable font this face is; `None` for a
    /// static font and for the default face of a variable one. The weight,
    /// width and slant above are those of the instance.
    pub instance: Option<Instance>,
}

/// A named instance of a variable font (`fvar`): a face a font lists by
/// name, with a value for each axis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Instance {
    /// Its place among the font's named instances, from 0.
    pub number: u32,
    /// Its subfamily name (`Bold`, `Semi Bold`).
    pub subfamily: String,
    /// The value of each axis, in the order of the `fvar` table, as the
    /// 16.16 fixed-point number the table holds.
    pub coordinates: Vec<(AxisTag, i32)>,
}

impl Instance {
    /// The instance as the variations to open the font at (see
    /// [`Program::with_variations`]).
    pub fn variations(&self) -> Variations {
        self.coordinates
            .iter()
            .fold(Variations::new(), |variations, &(tag, value)| {
                variations.with(tag, (f64::from(value) / FIXED_ONE) as f32)
            })
    }
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

/// The indexed fonts. Faces are in the order of path, then index, then
/// instance (a variable font's default face before its named instances), so
/// the same files give the same list in any order they are handed in.
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
        list.faces.sort_by(|a, b| {
            let number = |face: &FaceInfo| face.instance.as_ref().map(|i| i.number);
            (&a.path, a.index, number(a)).cmp(&(&b.path, b.index, number(b)))
        });
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
        [0x00, 0x01, 0x00, 0x00] | b"true" | b"OTTO" => index_sfnt(&file, file_len, path, 0, 0),
        b"ttcf" => {
            let header = read_at(&file, file_len, 0, TTC_HEADER_BYTES)?;
            let count = u64::from(be32(&header, 8));
            // Each offset is four bytes of the file, so the count is bounded
            // by the file before anything is allocated.
            let offsets = read_at(&file, file_len, TTC_HEADER_BYTES, count * 4)?;
            let mut faces = Vec::new();
            for i in 0..count as usize {
                let at = u64::from(be32(&offsets, i * 4));
                faces.extend(index_sfnt(&file, file_len, path, at, i as u32)?);
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
) -> Result<Vec<FaceInfo>, FontError> {
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
    let default = FaceInfo {
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
        instance: None,
    };
    let instances = match dir.read(file, file_len, b"fvar", u64::MAX)? {
        Some(data) => named_instances(&data, &default, &collect),
        None => Vec::new(),
    };
    let mut faces = vec![default];
    faces.extend(instances);
    Ok(faces)
}

/// The faces of the named instances an `fvar` table lists, each the default
/// face with the instance's name and its weight, width and slant. A table
/// that is damaged lists none: the font stays its default face.
fn named_instances(
    data: &[u8],
    default: &FaceInfo,
    strings: &dyn Fn(StringId) -> Vec<String>,
) -> Vec<FaceInfo> {
    let Ok(fvar) = Fvar::read(FontData::new(data)) else {
        return Vec::new();
    };
    let Ok(arrays) = fvar.axis_instance_arrays() else {
        return Vec::new();
    };
    let tags: Vec<AxisTag> = arrays
        .axes()
        .iter()
        .map(|axis| axis.axis_tag().to_be_bytes())
        .collect();
    let first = |id: StringId| strings(id).into_iter().next();
    // The prefix of an instance's PostScript name (Adobe TN 5902): `name`
    // ID 25, else the typographic family.
    let prefix = first(StringId::VARIATIONS_POSTSCRIPT_NAME_PREFIX)
        .or_else(|| default.families.first().cloned())
        .map(|name| ascii_alphanumeric(&name))
        .filter(|name| !name.is_empty());
    let mut faces = Vec::new();
    for (number, record) in arrays.instances().iter().enumerate() {
        let Ok(record) = record else { continue };
        let Some(subfamily) = first(record.subfamily_name_id) else {
            continue;
        };
        let coordinates: Vec<(AxisTag, i32)> = tags
            .iter()
            .zip(record.coordinates)
            .map(|(&tag, value)| (tag, value.get().to_bits()))
            .collect();
        let value = |tag: AxisTag| {
            coordinates
                .iter()
                .find(|(t, _)| *t == tag)
                .map(|&(_, v)| f64::from(v) / FIXED_ONE)
        };
        let weight = value(AXIS_WEIGHT_TAG)
            .map(|w| {
                w.round()
                    .clamp(f64::from(MIN_INSTANCE_WEIGHT), f64::from(MAX_WEIGHT))
                    as u16
            })
            .unwrap_or(default.weight);
        let width = value(AXIS_WIDTH).map(width_class).unwrap_or(default.width);
        let italic = match (value(AXIS_ITALIC_TAG), value(AXIS_SLANT_TAG)) {
            (Some(ital), _) if ital > 0.0 => true,
            (_, Some(slnt)) if slnt != 0.0 => true,
            (Some(_), _) | (_, Some(_)) => false,
            (None, None) => default.italic,
        };
        let family = default.families.first().cloned().unwrap_or_default();
        let postscript_name = record.post_script_name_id.and_then(first).or_else(|| {
            prefix
                .as_deref()
                .map(|p| named_instance_name(p, &subfamily))
        });
        faces.push(FaceInfo {
            full_name: Some(format!("{family} {subfamily}")),
            postscript_name,
            weight,
            width,
            italic,
            instance: Some(Instance {
                number: number as u32,
                subfamily,
                coordinates,
            }),
            ..default.clone()
        });
    }
    faces
}

/// The `usWidthClass` nearest to a `wdth` value in percent.
fn width_class(percent: f64) -> u8 {
    let nearest = WIDTH_CLASS_PERCENT
        .iter()
        .enumerate()
        .min_by(|a, b| (a.1 - percent).abs().total_cmp(&(b.1 - percent).abs()))
        .map_or(NORMAL_WIDTH as usize - 1, |(index, _)| index);
    nearest as u8 + 1
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
        instance: None,
    })
}
