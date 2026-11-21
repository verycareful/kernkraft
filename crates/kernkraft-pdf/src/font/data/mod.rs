// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The tables of the `font` module, generated from the specification and
//! Adobe's resource files, and not edited by hand.

pub(crate) mod cmaps;
pub(crate) mod encodings;
pub(crate) mod glyphlist;
pub(crate) mod standard14;

/// A range of a predefined CMap: the `span + 1` codes of `len` bytes from
/// `code` map to the CIDs from `cid` on.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Packed {
    pub code: u32,
    pub len: u8,
    pub span: u8,
    pub cid: u16,
}

pub(crate) const fn p(code: u32, len: u8, span: u8, cid: u16) -> Packed {
    Packed {
        code,
        len,
        span,
        cid,
    }
}

/// A range of a CID-to-Unicode map: the `span + 1` CIDs from `code` map to
/// the text of `len` UTF-16 code units at `at` in the table's pool, the last
/// unit counting up from one CID to the next.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PackedUnicode {
    pub code: u16,
    pub span: u8,
    pub len: u8,
    pub at: u32,
}

pub(crate) const fn u(code: u16, span: u8, len: u8, at: u32) -> PackedUnicode {
    PackedUnicode {
        code,
        span,
        len,
        at,
    }
}

/// One predefined CMap (ISO 32000-1:2008, Table 118).
#[derive(Debug)]
pub(crate) struct PredefinedCMap {
    pub name: &'static str,
    pub vertical: bool,
    /// The CMap whose mappings this one adds to (`usecmap`).
    pub use_cmap: Option<&'static str>,
    /// `(low, high, length)` of each codespace range.
    pub codespaces: &'static [(u32, u32, u8)],
    pub cid: &'static [Packed],
    pub notdef: &'static [Packed],
}
