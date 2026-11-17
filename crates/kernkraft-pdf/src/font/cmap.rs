// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! CMaps: ISO 32000-1:2008, 9.7.5 (their syntax and the predefined ones of
//! Table 118), 9.7.6 (how a string is split into codes and a code gives a
//! CID) and 9.10.3 (ToUnicode).
//!
//! A [`CMap`] splits the bytes of a string into character codes by its
//! codespace ranges, maps a code to a CID, and, for a ToUnicode CMap, to
//! text. The predefined CMaps are held as the sorted tables of
//! [`data::cmaps`](super::data::cmaps), read in place; an embedded one is
//! read by [`CMap::parse`]. A CMap that names another with `usecmap` adds
//! its mappings to that one's: a code is looked up in the CMap itself first.

use std::cmp::Reverse;
use std::fmt;
use std::sync::Arc;

use super::data::cmaps::{COLLECTION_UNICODE, PREDEFINED};
use super::data::{Packed, PackedUnicode, PredefinedCMap};
use super::range_map::RangeMap;
use crate::diagnostic::Diagnostics;
use crate::PdfError;

/// The most bytes of one character code (9.7.6.2).
pub(crate) const MAX_CODE_BYTES: usize = 4;
/// The CID of a code no mapping covers (9.7.6.3).
const NOTDEF_CID: u32 = 0;
/// Both bytes of the identity CMaps' codes.
const IDENTITY_CODE_BYTES: u8 = 2;
const IDENTITY_NAMES: [&str; 2] = ["Identity-H", "Identity-V"];

/// One codespace range: the codes of `len` bytes each of whose bytes lies
/// between the corresponding bytes of `low` and `high`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct CodeSpace {
    pub low: u32,
    pub high: u32,
    pub len: u8,
}

impl CodeSpace {
    /// The byte `index` (counted from the first) of `code`, as a code of
    /// this length.
    fn byte(code: u32, len: u8, index: usize) -> u8 {
        code.to_be_bytes()[MAX_CODE_BYTES - usize::from(len) + index]
    }

    /// How many leading bytes of `bytes` lie within the range's bytes, up
    /// to the range's length.
    fn matched(&self, bytes: &[u8]) -> usize {
        let count = usize::from(self.len).min(bytes.len());
        bytes[..count]
            .iter()
            .enumerate()
            .take_while(|(index, byte)| {
                (Self::byte(self.low, self.len, *index)..=Self::byte(self.high, self.len, *index))
                    .contains(byte)
            })
            .count()
    }
}

/// The character collection a CIDFont uses (9.7.3, Table 116).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CidSystemInfo {
    pub registry: String,
    pub ordering: String,
    pub supplement: i64,
}

/// The CID mappings of a CMap: a predefined one's table, or an embedded
/// one's ranges. Each embedded range holds the key it starts at and the
/// CID of that key.
///
/// The codes of a `cidrange` map to consecutive CIDs from the range's
/// first; every code of a `notdefrange` maps to the one CID (9.7.5.4).
#[derive(Clone)]
pub(crate) enum Mapping {
    Static {
        table: &'static [Packed],
        counts: bool,
    },
    Owned {
        ranges: RangeMap<(u64, u32)>,
        counts: bool,
    },
}

impl Mapping {
    fn get(&self, len: u8, code: u32) -> Option<u32> {
        match self {
            Mapping::Static { table, counts } => {
                let at = table.partition_point(|r| (r.len, r.code) <= (len, code));
                let range = table.get(at.checked_sub(1)?)?;
                (range.len == len && code - range.code <= u32::from(range.span)).then(|| {
                    let offset = if *counts { code - range.code } else { 0 };
                    u32::from(range.cid) + offset
                })
            }
            Mapping::Owned { ranges, counts } => {
                let key = key(len, code);
                let (_, (origin, cid)) = ranges.get(key)?;
                // A key is at or past the origin of its range, and the
                // distance is at most the 32 bits of a code.
                let offset = if *counts { (key - origin) as u32 } else { 0 };
                Some(cid.saturating_add(offset))
            }
        }
    }
}

/// The text of a ToUnicode range: the UTF-16 code units of its first code,
/// and the key it starts at; the last unit counts up from code to code.
pub(crate) type TextRange = (u64, Arc<[u16]>);

/// A CMap.
#[derive(Clone)]
pub struct CMap {
    pub(crate) name: Option<String>,
    pub(crate) vertical: bool,
    pub(crate) identity: bool,
    pub(crate) codespaces: Vec<CodeSpace>,
    pub(crate) cid: Mapping,
    pub(crate) notdef: Mapping,
    pub(crate) text: RangeMap<TextRange>,
    pub(crate) parent: Option<Arc<CMap>>,
    pub(crate) system_info: Option<CidSystemInfo>,
}

impl fmt::Debug for CMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CMap")
            .field("name", &self.name)
            .field("vertical", &self.vertical)
            .field("identity", &self.identity)
            .field("codespaces", &self.codespaces.len())
            .field(
                "parent",
                &self.parent.as_ref().and_then(|p| p.name.as_deref()),
            )
            .finish_non_exhaustive()
    }
}

/// The key of a code of `len` bytes in a range map: codes of different
/// lengths do not meet.
pub(crate) fn key(len: u8, code: u32) -> u64 {
    (u64::from(len) << 32) | u64::from(code)
}

/// The code of `bytes`, first byte highest.
pub(crate) fn code_of(bytes: &[u8]) -> u32 {
    bytes
        .iter()
        .fold(0, |code, byte| (code << 8) | u32::from(*byte))
}

impl CMap {
    /// The identity CMap (`Identity-H`, or `Identity-V` when `vertical`):
    /// two-byte codes, each its own CID (9.7.5.2).
    pub fn identity(vertical: bool) -> CMap {
        CMap {
            name: Some(IDENTITY_NAMES[usize::from(vertical)].to_string()),
            vertical,
            identity: true,
            codespaces: vec![CodeSpace {
                low: 0,
                high: 0xFFFF,
                len: IDENTITY_CODE_BYTES,
            }],
            cid: Mapping::Static {
                table: &[],
                counts: true,
            },
            notdef: Mapping::Static {
                table: &[],
                counts: false,
            },
            text: RangeMap::new(),
            parent: None,
            system_info: None,
        }
    }

    /// The predefined CMap called `name` (Table 118, and the two identity
    /// CMaps), or `None` when there is none.
    pub fn predefined(name: &str) -> Option<Arc<CMap>> {
        if let Some(vertical) = IDENTITY_NAMES.iter().position(|n| *n == name) {
            return Some(Arc::new(Self::identity(vertical == 1)));
        }
        let index = PREDEFINED
            .binary_search_by(|cmap| cmap.name.cmp(name))
            .ok()?;
        Some(Arc::new(Self::from_table(&PREDEFINED[index])))
    }

    fn from_table(table: &'static PredefinedCMap) -> CMap {
        let parent = table.use_cmap.and_then(CMap::predefined);
        let mut codespaces: Vec<CodeSpace> = table
            .codespaces
            .iter()
            .map(|&(low, high, len)| CodeSpace { low, high, len })
            .collect();
        if let Some(parent) = &parent {
            codespaces.extend(parent.codespaces.iter().copied());
        }
        CMap {
            name: Some(table.name.to_string()),
            vertical: table.vertical,
            identity: false,
            codespaces,
            cid: Mapping::Static {
                table: table.cid,
                counts: true,
            },
            notdef: Mapping::Static {
                table: table.notdef,
                counts: false,
            },
            text: RangeMap::new(),
            parent,
            system_info: None,
        }
    }

    /// The CMap's name, when it has one.
    pub fn name(&self) -> Option<&str> {
        self.name.as_deref()
    }

    /// True when the CMap sets vertical writing (`WMode` 1).
    pub fn is_vertical(&self) -> bool {
        self.vertical
    }

    /// The character collection the CMap is for, when it says.
    pub fn system_info(&self) -> Option<&CidSystemInfo> {
        self.system_info.as_ref()
    }

    /// Reads the character code at the start of `bytes` (9.7.6.2): its
    /// value and how many bytes it takes. A string that no codespace range
    /// matches takes the bytes of the best partial match (9.7.6.3): the
    /// code is then undefined and has the notdef CID. Takes at least one
    /// byte when there is one, and never more than are left.
    pub fn read_code(&self, bytes: &[u8]) -> (u32, usize) {
        if bytes.is_empty() {
            return (0, 0);
        }
        let longest = bytes.len().min(MAX_CODE_BYTES);
        for len in 1..=longest {
            if self
                .codespaces
                .iter()
                .any(|space| usize::from(space.len) == len && space.matched(&bytes[..len]) == len)
            {
                return (code_of(&bytes[..len]), len);
            }
        }
        let best = self
            .codespaces
            .iter()
            .map(|space| (space.matched(bytes), Reverse(space.len)))
            .max();
        let taken = match best {
            Some((matched, Reverse(len))) if matched > 0 => usize::from(len),
            _ => self.partial_length(),
        };
        let taken = taken.min(bytes.len());
        (code_of(&bytes[..taken]), taken)
    }

    /// The length of the shortest codespace range: what is taken when no
    /// range matches the first byte. A CMap with no codespace range takes
    /// one byte.
    fn partial_length(&self) -> usize {
        self.codespaces
            .iter()
            .map(|space| usize::from(space.len))
            .min()
            .unwrap_or(1)
    }

    fn lookup(&self, select: fn(&CMap) -> &Mapping, len: u8, code: u32) -> Option<u32> {
        let mut cmap = self;
        loop {
            if let Some(cid) = select(cmap).get(len, code) {
                return Some(cid);
            }
            cmap = cmap.parent.as_deref()?;
        }
    }

    /// The CID of the code `code` of `len` bytes: the CMap's own mapping,
    /// else that of the CMap it uses, else its notdef mapping, else CID 0
    /// (9.7.6.2, 9.7.6.3).
    pub fn cid(&self, code: u32, len: usize) -> u32 {
        if self.identity {
            return code;
        }
        let Ok(len) = u8::try_from(len) else {
            return NOTDEF_CID;
        };
        self.lookup(|c| &c.cid, len, code)
            .or_else(|| self.lookup(|c| &c.notdef, len, code))
            .unwrap_or(NOTDEF_CID)
    }

    /// The text the ToUnicode mapping gives the code `code` of `len` bytes.
    pub fn to_unicode(&self, code: u32, len: usize) -> Option<String> {
        let len = u8::try_from(len).ok()?;
        let key = key(len, code);
        let mut cmap = self;
        loop {
            if let Some((_, (origin, units))) = cmap.text.get(key) {
                return Some(text_of(units, key - origin));
            }
            cmap = cmap.parent.as_deref()?;
        }
    }
}

/// The text of UTF-16 code units whose last unit is raised by `offset`.
/// A unit that would pass U+FFFF or lies alone as a surrogate is U+FFFD.
fn text_of(units: &[u16], offset: u64) -> String {
    let mut units = units.to_vec();
    if let Some(last) = units.last_mut() {
        let raised = u64::from(*last).saturating_add(offset);
        *last = u16::try_from(raised).unwrap_or(u16::MAX);
    }
    char::decode_utf16(units)
        .map(|unit| unit.unwrap_or(char::REPLACEMENT_CHARACTER))
        .collect()
}

/// The text of CID `cid` of the character collection `registry`-`ordering`
/// (9.10.2): the Unicode CMap `Adobe-Japan1-UCS2` and its kind.
pub(crate) fn collection_text(registry: &str, ordering: &str, cid: u32) -> Option<String> {
    let name = format!("{registry}-{ordering}-UCS2");
    let (_, table, pool) = COLLECTION_UNICODE.iter().find(|(n, _, _)| *n == name)?;
    let cid = u16::try_from(cid).ok()?;
    let at = table.partition_point(|r: &PackedUnicode| r.code <= cid);
    let range = table.get(at.checked_sub(1)?)?;
    let offset = cid - range.code;
    if offset > u16::from(range.span) {
        return None;
    }
    let start = usize::try_from(range.at).ok()?;
    let units = pool.get(start..start + usize::from(range.len))?;
    Some(text_of(units, u64::from(offset)))
}

/// Reads an embedded CMap (a ToUnicode CMap or an encoding CMap stream).
/// `parent` is the CMap the stream's `UseCMap` names; `resolve` finds the
/// CMap a `usecmap` operator in the data names.
pub(crate) fn parse(
    data: &[u8],
    parent: Option<Arc<CMap>>,
    resolve: &dyn Fn(&str) -> Option<Arc<CMap>>,
    diagnostics: &mut Diagnostics,
) -> Result<CMap, PdfError> {
    super::cmap_parse::parse(data, parent, resolve, diagnostics)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    fn predefined(name: &str) -> Arc<CMap> {
        CMap::predefined(name).expect("a predefined CMap")
    }

    #[test]
    fn the_predefined_names_are_sorted_for_binary_search() {
        assert!(PREDEFINED
            .windows(2)
            .all(|pair| pair[0].name < pair[1].name));
    }

    #[test]
    fn every_used_cmap_is_predefined() {
        for table in &PREDEFINED {
            if let Some(parent) = table.use_cmap {
                assert!(CMap::predefined(parent).is_some(), "{parent}");
            }
        }
    }

    #[test]
    fn identity_maps_two_byte_codes_to_themselves() {
        let cmap = CMap::identity(false);
        assert_eq!(cmap.read_code(&[0x12, 0x34, 0x56]), (0x1234, 2));
        assert_eq!(cmap.cid(0x1234, 2), 0x1234);
        assert!(!cmap.is_vertical());
        assert!(predefined("Identity-V").is_vertical());
    }

    #[test]
    fn a_string_ending_inside_a_code_takes_what_is_left() {
        let cmap = CMap::identity(false);
        assert_eq!(cmap.read_code(&[0x12]), (0x12, 1));
        assert_eq!(cmap.read_code(&[]), (0, 0));
    }

    /// 90ms-RKSJ-H: codespace <00>-<80>, <8140>-<9FFC>, <A0>-<DF>,
    /// <E040>-<FCFC>. A byte below 0x81 is one code and 0x81 starts a
    /// two-byte one.
    #[test]
    fn shift_jis_mixes_one_and_two_byte_codes() {
        let cmap = predefined("90ms-RKSJ-H");
        assert_eq!(cmap.read_code(&[0x41, 0x81]), (0x41, 1));
        assert_eq!(cmap.read_code(&[0x81, 0x40]), (0x8140, 2));
        assert_eq!(cmap.read_code(&[0xB1]), (0xB1, 1));
        // 0x20 to 0x7E are CIDs 231 on (the half-width Latin of Adobe-Japan1).
        assert_eq!(cmap.cid(0x20, 1), 231);
        assert_eq!(cmap.cid(0x8140, 2), 633);
    }

    /// 9.7.5.4: every code of a notdef range has the range's CID, where a
    /// CID range counts up. 90ms-RKSJ-H has `<00> <1f> 231` as a notdef
    /// range.
    #[test]
    fn a_notdef_range_gives_one_cid() {
        let cmap = predefined("90ms-RKSJ-H");
        assert_eq!(cmap.cid(0x00, 1), 231);
        assert_eq!(cmap.cid(0x05, 1), 231);
        assert_eq!(cmap.cid(0x1F, 1), 231);
    }

    /// 9.7.6.3 a): a first byte no codespace range starts with takes the
    /// shortest range's length.
    #[test]
    fn an_invalid_first_byte_takes_the_shortest_length() {
        let cmap = predefined("90ms-RKSJ-H");
        // 0x81 starts the two-byte range <8140>-<9FFC> but 0x30 is below its
        // second byte's lower bound: that range is the best partial match,
        // so two bytes are taken.
        assert_eq!(cmap.read_code(&[0x81, 0x30, 0x41]), (0x8130, 2));
        assert_eq!(cmap.cid(0x8130, 2), 0);
    }

    #[test]
    fn a_vertical_cmap_adds_to_its_horizontal_one() {
        let h = predefined("90ms-RKSJ-H");
        let v = predefined("90ms-RKSJ-V");
        assert!(v.is_vertical());
        // A code the V map does not redefine is the H map's.
        assert_eq!(v.cid(0x8140, 2), h.cid(0x8140, 2));
        assert_eq!(v.read_code(&[0x81, 0x40]), (0x8140, 2));
    }

    #[test]
    fn a_unicode_cmap_reads_two_byte_codes() {
        let cmap = predefined("UniJIS-UCS2-H");
        assert_eq!(cmap.read_code(&[0x00, 0x41]), (0x41, 2));
        assert!(cmap.cid(0x3042, 2) > 0);
    }

    #[test]
    fn the_utf16_cmap_has_four_byte_codes() {
        let cmap = predefined("UniJIS-UTF16-H");
        // A surrogate pair is one code of four bytes.
        let (code, len) = cmap.read_code(&[0xD8, 0x40, 0xDC, 0x0B]);
        assert_eq!((code, len), (0xD840_DC0B, 4));
    }

    #[test]
    fn collection_text_maps_a_cid_to_unicode() {
        // CID 1 of Adobe-Japan1 is the space.
        assert_eq!(collection_text("Adobe", "Japan1", 1).as_deref(), Some(" "));
        assert_eq!(collection_text("Adobe", "Nowhere", 1), None);
    }

    #[test]
    fn an_unknown_name_is_not_predefined() {
        assert!(CMap::predefined("Not-A-CMap").is_none());
    }
}
