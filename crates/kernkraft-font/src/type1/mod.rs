// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Adobe Type 1 font programs, read from the specification (Adobe Type 1
//! Font Format: the font program, font dictionary, private dictionary,
//! encryption and charstring chapters). PFA, PFB and the raw `FontFile`
//! stream of a PDF are all accepted.
//!
//! The binary-read operator of the private dictionary is recognised as `RD`
//! or `-|`, the two names every Type 1 font uses.

mod charstring;
mod container;
mod crypt;
mod scan;
mod standard;

use std::collections::HashMap;

use crate::error::FontError;
use crate::outline::Outline;
use crate::program::{GlyphId, Metrics, Names};
use scan::{Scanner, Token};

/// `FontMatrix` when the font gives none: 1000 units to the em (the font
/// dictionary chapter).
const DEFAULT_FONT_MATRIX: [f32; 6] = [0.001, 0.0, 0.0, 0.001, 0.0, 0.0];
/// `lenIV` when the private dictionary gives none (the encryption chapter).
const DEFAULT_LEN_IV: i64 = 4;
/// Glyphs a [`GlyphId`] can number.
const MAX_GLYPHS: usize = 65_535;

/// The private dictionary's subroutines and the glyph descriptions, already
/// decrypted.
pub(crate) struct Private {
    subrs: Vec<Option<Vec<u8>>>,
    glyphs: Vec<(String, Vec<u8>)>,
    by_name: HashMap<String, usize>,
}

impl Private {
    pub(crate) fn subr(&self, index: f32) -> Option<&[u8]> {
        if index < 0.0 || index.fract() != 0.0 {
            return None;
        }
        self.subrs.get(index as usize)?.as_deref()
    }

    pub(crate) fn charstring_by_name(&self, name: &str) -> Option<&[u8]> {
        let i = *self.by_name.get(name)?;
        Some(&self.glyphs[i].1)
    }
}

enum Encoding {
    Standard,
    Custom(Box<[Option<String>; 256]>),
}

/// A decoded Type 1 font program.
pub struct Type1Font {
    private: Private,
    encoding: Encoding,
    font_matrix: [f32; 6],
    bbox: Option<[f32; 4]>,
    italic_angle: Option<f32>,
    fixed_pitch: Option<bool>,
    names: Names,
}

impl Type1Font {
    pub(crate) fn parse(data: &[u8]) -> Result<Type1Font, FontError> {
        let parts = container::split(data)?;
        let clear = parse_clear(&parts.cleartext)?;
        let plain = crypt::decrypt(&parts.encrypted, crypt::EEXEC_KEY, 4);
        let private = parse_private(&plain)?;
        Ok(Type1Font {
            private,
            encoding: clear.encoding,
            font_matrix: clear.font_matrix.unwrap_or(DEFAULT_FONT_MATRIX),
            bbox: clear.bbox,
            italic_angle: clear.italic_angle,
            fixed_pitch: clear.fixed_pitch,
            names: clear.names,
        })
    }

    pub(crate) fn glyph_count(&self) -> u16 {
        // The count is held to MAX_GLYPHS when the font is read.
        self.private.glyphs.len() as u16
    }

    pub(crate) fn font_matrix(&self) -> [f32; 6] {
        self.font_matrix
    }

    pub(crate) fn glyph_by_name(&self, name: &str) -> Option<GlyphId> {
        self.private.by_name.get(name).map(|&i| GlyphId(i as u16))
    }

    pub(crate) fn glyph_name(&self, glyph: GlyphId) -> Option<&str> {
        self.private
            .glyphs
            .get(usize::from(glyph.0))
            .map(|(n, _)| n.as_str())
    }

    pub(crate) fn glyph_by_code(&self, code: u8) -> Option<GlyphId> {
        let name = match &self.encoding {
            Encoding::Standard => standard::standard_name(code)?,
            Encoding::Custom(table) => table[usize::from(code)].as_deref()?,
        };
        self.glyph_by_name(name)
    }

    pub(crate) fn advance(&self, glyph: GlyphId) -> Option<f32> {
        self.render(glyph).ok().map(|r| r.advance)
    }

    pub(crate) fn outline(&self, glyph: GlyphId) -> Result<Outline, FontError> {
        self.render(glyph).map(|r| r.outline)
    }

    fn render(&self, glyph: GlyphId) -> Result<charstring::Rendered, FontError> {
        let (_, code) = self
            .private
            .glyphs
            .get(usize::from(glyph.0))
            .ok_or(FontError::NoSuchGlyph(glyph))?;
        charstring::render(&self.private, code)
            .map_err(|reason| FontError::Outline { glyph, reason })
    }

    pub(crate) fn metrics(&self) -> Metrics {
        Metrics {
            italic_angle: self.italic_angle,
            bbox: self.bbox,
            is_monospaced: self.fixed_pitch,
            is_italic: self.italic_angle.map(|a| a != 0.0),
            ..Metrics::default()
        }
    }

    pub(crate) fn names(&self) -> Names {
        self.names.clone()
    }
}

/// What the cleartext portion says.
struct Clear {
    font_matrix: Option<[f32; 6]>,
    bbox: Option<[f32; 4]>,
    italic_angle: Option<f32>,
    fixed_pitch: Option<bool>,
    encoding: Encoding,
    names: Names,
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Reads `n` numbers inside `[ ]` or `{ }`.
fn numbers<const N: usize>(s: &mut Scanner<'_>) -> Option<[f32; N]> {
    let Some(Token::Bracket(b'[' | b'{')) = s.next_token() else {
        return None;
    };
    let mut out = [0.0; N];
    for slot in &mut out {
        let Some(Token::Number(n)) = s.next_token() else {
            return None;
        };
        *slot = n as f32;
    }
    let Some(Token::Bracket(b']' | b'}')) = s.next_token() else {
        return None;
    };
    Some(out)
}

fn parse_clear(data: &[u8]) -> Result<Clear, FontError> {
    let mut clear = Clear {
        font_matrix: None,
        bbox: None,
        italic_angle: None,
        fixed_pitch: None,
        encoding: Encoding::Standard,
        names: Names::default(),
    };
    let mut s = Scanner::new(data);
    while let Some(token) = s.next_token() {
        let Token::Name(key) = token else {
            continue;
        };
        match key {
            b"FontName" => {
                if let Some(Token::Name(n)) = s.next_token() {
                    clear.names.postscript = Some(text(n));
                }
            }
            b"FontMatrix" => {
                clear.font_matrix = Some(
                    numbers::<6>(&mut s)
                        .ok_or(FontError::Type1("the FontMatrix is not six numbers"))?,
                );
            }
            b"FontBBox" => {
                clear.bbox = Some(
                    numbers::<4>(&mut s)
                        .ok_or(FontError::Type1("the FontBBox is not four numbers"))?,
                );
            }
            b"ItalicAngle" => {
                if let Some(Token::Number(n)) = s.next_token() {
                    clear.italic_angle = Some(n as f32);
                }
            }
            b"isFixedPitch" => {
                if let Some(Token::Word(w)) = s.next_token() {
                    clear.fixed_pitch = Some(w == b"true");
                }
            }
            b"FullName" => {
                if let Some(Token::Text(t)) = s.next_token() {
                    clear.names.full = Some(text(t));
                }
            }
            b"FamilyName" => {
                if let Some(Token::Text(t)) = s.next_token() {
                    clear.names.family = Some(text(t));
                }
            }
            b"Encoding" => clear.encoding = parse_encoding(&mut s)?,
            _ => {}
        }
    }
    Ok(clear)
}

/// `StandardEncoding def`, or `256 array ... dup <code> /<name> put ... def`.
fn parse_encoding(s: &mut Scanner<'_>) -> Result<Encoding, FontError> {
    if let Some(Token::Word(b"StandardEncoding")) = s.next_token() {
        return Ok(Encoding::Standard);
    }
    let mut table: Box<[Option<String>; 256]> = Box::new(std::array::from_fn(|_| None));
    let mut depth = 0usize;
    // The tokens of `dup <code> /<name> put` seen so far.
    let mut code: Option<usize> = None;
    let mut name: Option<String> = None;
    let mut dup = false;
    while let Some(token) = s.next_token() {
        match token {
            Token::Bracket(b'{') => depth += 1,
            Token::Bracket(b'}') => depth = depth.saturating_sub(1),
            _ if depth > 0 => {}
            Token::Word(b"def") => return Ok(Encoding::Custom(table)),
            Token::Word(b"dup") => {
                dup = true;
                code = None;
                name = None;
            }
            Token::Number(n) if dup && code.is_none() => {
                if n < 0.0 || n > 255.0 || n.fract() != 0.0 {
                    return Err(FontError::Type1(
                        "an Encoding entry with a code outside 0 to 255",
                    ));
                }
                code = Some(n as usize);
            }
            Token::Name(n) if dup && code.is_some() && name.is_none() => name = Some(text(n)),
            Token::Word(b"put") if dup => {
                if let (Some(c), Some(n)) = (code.take(), name.take()) {
                    table[c] = Some(n);
                }
                dup = false;
            }
            _ => {}
        }
    }
    Err(FontError::Type1("the Encoding array is never closed"))
}

enum State {
    Other,
    Subrs,
    CharStrings,
}

fn parse_private(plain: &[u8]) -> Result<Private, FontError> {
    let mut s = Scanner::new(plain);
    let mut len_iv = DEFAULT_LEN_IV;
    let mut subrs: Vec<Option<Vec<u8>>> = Vec::new();
    let mut raw_glyphs: Vec<(String, Vec<u8>)> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();
    let mut state = State::Other;
    let mut chars_declared = false;
    while let Some(token) = s.next_token() {
        match (&state, token) {
            (_, Token::Name(b"lenIV")) => {
                if let Some(Token::Number(n)) = s.next_token() {
                    len_iv = n as i64;
                }
            }
            (State::Other, Token::Name(b"Subrs")) => {
                let Some(Token::Number(n)) = s.next_token() else {
                    return Err(FontError::Type1("Subrs has no count"));
                };
                // A subroutine takes at least one byte of the portion, so a
                // count past its length cannot be true.
                if n < 0.0 || n as usize > plain.len() {
                    return Err(FontError::Type1(
                        "the Subrs count is past the size of the font",
                    ));
                }
                subrs = vec![None; n as usize];
                state = State::Subrs;
            }
            (State::Subrs, Token::Word(b"dup")) => {
                let (Some(Token::Number(i)), Some(Token::Number(len))) =
                    (s.next_token(), s.next_token())
                else {
                    return Err(FontError::Type1(
                        "a Subrs entry is not `dup index length RD`",
                    ));
                };
                read_rd(&mut s)?;
                let bytes = read_bytes(&mut s, len)?;
                if i < 0.0 || i as usize >= subrs.len() {
                    return Err(FontError::Type1("a Subrs entry past the declared count"));
                }
                subrs[i as usize] = Some(bytes.to_vec());
            }
            (_, Token::Name(b"CharStrings")) => {
                let Some(Token::Number(n)) = s.next_token() else {
                    return Err(FontError::Type1("CharStrings has no count"));
                };
                if n < 0.0 || n as usize > plain.len() || n as usize > MAX_GLYPHS {
                    return Err(FontError::Type1(
                        "the CharStrings count is past what the font can hold",
                    ));
                }
                chars_declared = true;
                state = State::CharStrings;
            }
            (State::CharStrings, Token::Name(name)) => {
                let Some(Token::Number(len)) = s.next_token() else {
                    return Err(FontError::Type1(
                        "a CharStrings entry is not `/name length RD`",
                    ));
                };
                read_rd(&mut s)?;
                let bytes = read_bytes(&mut s, len)?.to_vec();
                let name = text(name);
                match by_name.get(&name) {
                    Some(&i) => raw_glyphs[i].1 = bytes,
                    None => {
                        if raw_glyphs.len() >= MAX_GLYPHS {
                            return Err(FontError::Type1(
                                "more glyphs than a glyph number can hold",
                            ));
                        }
                        by_name.insert(name.clone(), raw_glyphs.len());
                        raw_glyphs.push((name, bytes));
                    }
                }
            }
            (State::CharStrings, Token::Word(b"end")) => break,
            _ => {}
        }
    }
    if !chars_declared {
        return Err(FontError::Type1(
            "the private dictionary has no CharStrings",
        ));
    }
    let decrypt = |bytes: &[u8]| -> Vec<u8> {
        match usize::try_from(len_iv) {
            Ok(skip) => crypt::decrypt(bytes, crypt::CHARSTRING_KEY, skip),
            // A negative `lenIV` marks charstrings that are not encrypted.
            Err(_) => bytes.to_vec(),
        }
    };
    for sub in subrs.iter_mut().flatten() {
        *sub = decrypt(sub);
    }
    for (_, code) in &mut raw_glyphs {
        *code = decrypt(code);
    }
    Ok(Private {
        subrs,
        glyphs: raw_glyphs,
        by_name,
    })
}

fn read_rd(s: &mut Scanner<'_>) -> Result<(), FontError> {
    match s.next_token() {
        Some(Token::Word(b"RD" | b"-|")) => Ok(()),
        _ => Err(FontError::Type1(
            "a charstring is not introduced by RD or -|",
        )),
    }
}

fn read_bytes<'a>(s: &mut Scanner<'a>, len: f64) -> Result<&'a [u8], FontError> {
    if len < 0.0 || len.fract() != 0.0 {
        return Err(FontError::Type1("a charstring with a bad length"));
    }
    s.binary(len as usize).ok_or(FontError::Type1(
        "a charstring runs past the end of the private dictionary",
    ))
}
