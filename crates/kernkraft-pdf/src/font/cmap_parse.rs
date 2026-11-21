// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The syntax of an embedded CMap: ISO 32000-1:2008, 9.7.5.4 (the operators)
//! and 9.10.3 (a ToUnicode CMap), after Adobe Technical Note #5014.
//!
//! The data is a PostScript-like program of which only a few operators
//! matter: the `begin`/`end` pairs of codespace ranges and of CID, notdef
//! and bf (text) mappings, `usecmap`, and the `def` of `WMode`, `CMapName`
//! and the `CIDSystemInfo` entries. Everything else is skipped. Each
//! mapping entry is read as soon as its operands are complete, so a long
//! block costs no more than its entries.

use std::sync::Arc;

use super::cmap::{
    code_of, key, CMap, CidSystemInfo, CodeSpace, Mapping, TextRange, MAX_CODE_BYTES,
};
use super::range_map::RangeMap;
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::syntax::{Lexer, TokenKind};
use crate::{MemorySource, PdfError};

/// The most operands one entry may collect: a bf range's array of texts
/// is the longest. A longer run is no entry.
const MAX_PENDING: usize = 1 << 12;
/// 9.7.5.4 allows `WMode` 0 and 1.
const VERTICAL: i64 = 1;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Block {
    None,
    CodeSpace,
    CidRange,
    CidChar,
    NotdefRange,
    NotdefChar,
    BfRange,
    BfChar,
}

impl Block {
    fn begun_by(word: &[u8]) -> Option<Block> {
        Some(match word {
            b"begincodespacerange" => Block::CodeSpace,
            b"begincidrange" => Block::CidRange,
            b"begincidchar" => Block::CidChar,
            b"beginnotdefrange" => Block::NotdefRange,
            b"beginnotdefchar" => Block::NotdefChar,
            b"beginbfrange" => Block::BfRange,
            b"beginbfchar" => Block::BfChar,
            _ => return None,
        })
    }

    fn ended_by(word: &[u8]) -> bool {
        matches!(
            word,
            b"endcodespacerange"
                | b"endcidrange"
                | b"endcidchar"
                | b"endnotdefrange"
                | b"endnotdefchar"
                | b"endbfrange"
                | b"endbfchar"
        )
    }
}

struct Builder {
    name: Option<String>,
    vertical: bool,
    codespaces: Vec<CodeSpace>,
    cid: RangeMap<(u64, u32)>,
    notdef: RangeMap<(u64, u32)>,
    text: RangeMap<TextRange>,
    registry: Option<String>,
    ordering: Option<String>,
    supplement: Option<i64>,
    parent: Option<Arc<CMap>>,
    overlapped: bool,
}

/// A code of one to four bytes, as `(length, value)`.
fn code(token: &TokenKind) -> Option<(u8, u32)> {
    let TokenKind::String(string) = token else {
        return None;
    };
    let bytes = string.as_bytes();
    if (1..=MAX_CODE_BYTES).contains(&bytes.len()) {
        // At most four, so it fits a byte.
        Some((bytes.len() as u8, code_of(bytes)))
    } else {
        None
    }
}

fn cid(token: &TokenKind) -> Option<u32> {
    match token {
        TokenKind::Integer(value) => u32::try_from(*value).ok(),
        _ => None,
    }
}

/// The UTF-16 code units of a bf destination: whole units, at least one.
fn units(token: &TokenKind) -> Option<Vec<u16>> {
    let TokenKind::String(string) = token else {
        return None;
    };
    let bytes = string.as_bytes();
    if bytes.is_empty() || bytes.len() % 2 == 1 {
        return None;
    }
    Some(
        bytes
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u16::from_be_bytes(*pair))
            .collect(),
    )
}

impl Builder {
    fn range(
        target: &mut RangeMap<(u64, u32)>,
        low: (u8, u32),
        high: (u8, u32),
        first: u32,
        overlapped: &mut bool,
    ) -> bool {
        if low.0 != high.0 || low.1 > high.1 {
            return false;
        }
        let start = key(low.0, low.1);
        let end = key(high.0, high.1);
        // The CID of a key past `u32::MAX` is not a CID.
        if u64::from(first) + (end - start) > u64::from(u32::MAX) {
            return false;
        }
        *overlapped |= target.insert(start, end, (start, first));
        true
    }

    /// Reads the entries `pending` holds complete, leaving the rest.
    /// Returns false when what is held cannot be an entry.
    fn entry(&mut self, block: Block, pending: &mut Vec<TokenKind>) -> bool {
        let used = match block {
            Block::None => return true,
            Block::CodeSpace => match pending.as_slice() {
                [low, high, ..] => {
                    let (Some(low), Some(high)) = (code(low), code(high)) else {
                        return false;
                    };
                    if low.0 != high.0 {
                        return false;
                    }
                    self.codespaces.push(CodeSpace {
                        low: low.1,
                        high: high.1,
                        len: low.0,
                    });
                    2
                }
                _ => return true,
            },
            Block::CidRange | Block::NotdefRange => match pending.as_slice() {
                [low, high, first, ..] => {
                    let (Some(low), Some(high), Some(first)) = (code(low), code(high), cid(first))
                    else {
                        return false;
                    };
                    let target = if block == Block::CidRange {
                        &mut self.cid
                    } else {
                        &mut self.notdef
                    };
                    if !Self::range(target, low, high, first, &mut self.overlapped) {
                        return false;
                    }
                    3
                }
                _ => return true,
            },
            Block::CidChar | Block::NotdefChar => match pending.as_slice() {
                [at, first, ..] => {
                    let (Some(at), Some(first)) = (code(at), cid(first)) else {
                        return false;
                    };
                    let target = if block == Block::CidChar {
                        &mut self.cid
                    } else {
                        &mut self.notdef
                    };
                    if !Self::range(target, at, at, first, &mut self.overlapped) {
                        return false;
                    }
                    2
                }
                _ => return true,
            },
            Block::BfChar => match pending.as_slice() {
                [at, text, ..] => {
                    let (Some(at), Some(text)) = (code(at), units(text)) else {
                        return false;
                    };
                    let start = key(at.0, at.1);
                    self.overlapped |= self.text.insert(start, start, (start, text.into()));
                    2
                }
                _ => return true,
            },
            Block::BfRange => return self.bf_range(pending),
        };
        pending.drain(..used);
        true
    }

    /// `low high text` or `low high [text text ...]`.
    fn bf_range(&mut self, pending: &mut Vec<TokenKind>) -> bool {
        let [low, high, third, ..] = pending.as_slice() else {
            return true;
        };
        let (Some(low), Some(high)) = (code(low), code(high)) else {
            return false;
        };
        if low.0 != high.0 || low.1 > high.1 {
            return false;
        }
        let (start, end) = (key(low.0, low.1), key(high.0, high.1));
        match third {
            TokenKind::ArrayOpen => {
                let Some(close) = pending.iter().position(|t| *t == TokenKind::ArrayClose) else {
                    return true;
                };
                let items = &pending[3..close];
                for (at, item) in (start..).zip(items) {
                    let Some(text) = units(item) else {
                        return false;
                    };
                    if at > end {
                        break;
                    }
                    self.overlapped |= self.text.insert(at, at, (at, text.into()));
                }
                pending.drain(..=close);
            }
            other => {
                let Some(text) = units(other) else {
                    return false;
                };
                self.overlapped |= self.text.insert(start, end, (start, text.into()));
                pending.drain(..3);
            }
        }
        true
    }

    /// The operands of a `def`, `usecmap` or other operator outside a
    /// block: `/Key value` pairs anywhere among them.
    fn definitions(&mut self, operands: &[TokenKind]) {
        for pair in operands.windows(2) {
            let (TokenKind::Name(name), value) = (&pair[0], &pair[1]) else {
                continue;
            };
            match (name.as_bytes(), value) {
                (b"WMode", TokenKind::Integer(mode)) => self.vertical = *mode == VERTICAL,
                (b"CMapName", TokenKind::Name(value)) => {
                    self.name = Some(String::from_utf8_lossy(value.as_bytes()).into_owned());
                }
                (b"Registry", TokenKind::String(value)) => {
                    self.registry = Some(String::from_utf8_lossy(value.as_bytes()).into_owned());
                }
                (b"Ordering", TokenKind::String(value)) => {
                    self.ordering = Some(String::from_utf8_lossy(value.as_bytes()).into_owned());
                }
                (b"Supplement", TokenKind::Integer(value)) => self.supplement = Some(*value),
                _ => {}
            }
        }
    }
}

pub(super) fn parse(
    data: &[u8],
    parent: Option<Arc<CMap>>,
    resolve: &dyn Fn(&str) -> Option<Arc<CMap>>,
    diagnostics: &mut Diagnostics,
) -> Result<CMap, PdfError> {
    let source = MemorySource::new(data.to_vec());
    let mut lexer = Lexer::new(&source);
    let mut builder = Builder {
        name: None,
        vertical: false,
        codespaces: Vec::new(),
        cid: RangeMap::new(),
        notdef: RangeMap::new(),
        text: RangeMap::new(),
        registry: None,
        ordering: None,
        supplement: None,
        parent,
        overlapped: false,
    };
    let mut block = Block::None;
    let mut pending: Vec<TokenKind> = Vec::new();
    let mut entry_offset = 0;
    loop {
        let token = lexer.next_token(diagnostics)?;
        match token.kind {
            TokenKind::End => break,
            TokenKind::Keyword(word) => {
                if let Some(begun) = Block::begun_by(&word) {
                    block = begun;
                    pending.clear();
                } else if Block::ended_by(&word) {
                    if !pending.is_empty() {
                        diagnostics.push(DiagnosticKind::CMapEntryInvalid, entry_offset);
                    }
                    block = Block::None;
                    pending.clear();
                } else if block == Block::None {
                    if word == b"usecmap" {
                        if let Some(TokenKind::Name(name)) = pending.last() {
                            let name = String::from_utf8_lossy(name.as_bytes()).into_owned();
                            if builder.parent.is_none() {
                                builder.parent =
                                    Some(resolve(&name).ok_or(PdfError::UnknownCMap { name })?);
                            }
                        }
                    } else {
                        builder.definitions(&pending);
                    }
                    pending.clear();
                }
            }
            kind => {
                if pending.is_empty() {
                    entry_offset = token.span.start;
                }
                pending.push(kind);
                if block != Block::None {
                    if pending.len() > MAX_PENDING || !builder.entry(block, &mut pending) {
                        diagnostics.push(DiagnosticKind::CMapEntryInvalid, entry_offset);
                        pending.clear();
                    }
                } else if pending.len() > MAX_PENDING {
                    pending.clear();
                }
            }
        }
    }
    if block != Block::None {
        diagnostics.push(DiagnosticKind::CMapUnterminated, entry_offset);
    }
    if builder.overlapped {
        diagnostics.push(DiagnosticKind::CMapRangesOverlap, 0);
    }
    let mut codespaces = builder.codespaces;
    if let Some(parent) = &builder.parent {
        codespaces.extend(parent.codespaces.iter().copied());
    }
    let system_info = match (builder.registry, builder.ordering) {
        (Some(registry), Some(ordering)) => Some(CidSystemInfo {
            registry,
            ordering,
            supplement: builder.supplement.unwrap_or(0),
        }),
        _ => None,
    };
    Ok(CMap {
        name: builder.name,
        vertical: builder.vertical,
        identity: false,
        codespaces,
        cid: Mapping::Owned {
            ranges: builder.cid,
            counts: true,
        },
        notdef: Mapping::Owned {
            ranges: builder.notdef,
            counts: false,
        },
        text: builder.text,
        parent: builder.parent,
        system_info,
    })
}
