// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A bare CFF font program written byte by byte (Adobe Technical Note 5176,
//! The Compact Font Format Specification: header, INDEX, DICT, charset,
//! FDSelect; Technical Note 5177 for the Type 2 charstring operators).
#![allow(dead_code)]

/// A Type 2 charstring under construction.
#[derive(Default, Clone)]
pub struct Cs2(pub Vec<u8>);

impl Cs2 {
    pub fn new() -> Cs2 {
        Cs2::default()
    }

    pub fn n(mut self, v: i32) -> Cs2 {
        match v {
            -107..=107 => self.0.push((v + 139) as u8),
            108..=1131 => {
                let w = v - 108;
                self.0.push(247 + (w >> 8) as u8);
                self.0.push((w & 0xFF) as u8);
            }
            -1131..=-108 => {
                let w = -v - 108;
                self.0.push(251 + (w >> 8) as u8);
                self.0.push((w & 0xFF) as u8);
            }
            _ => {
                self.0.push(28);
                self.0.extend_from_slice(&(v as i16).to_be_bytes());
            }
        }
        self
    }

    pub fn nums(mut self, v: &[i32]) -> Cs2 {
        for &x in v {
            self = self.n(x);
        }
        self
    }

    pub fn op(mut self, name: &str) -> Cs2 {
        self.0.push(match name {
            "vmoveto" => 4,
            "rlineto" => 5,
            "callsubr" => 10,
            "hlineto" => 6,
            "vlineto" => 7,
            "rrcurveto" => 8,
            "endchar" => 14,
            "rmoveto" => 21,
            "hmoveto" => 22,
            other => panic!("unknown operator {other}"),
        });
        self
    }

    pub fn with(self, args: &[i32], name: &str) -> Cs2 {
        self.nums(args).op(name)
    }
}

pub struct CffGlyph {
    /// The SID of a name-keyed font's glyph, or the CID of a CID-keyed one.
    pub id: u16,
    pub code: Cs2,
}

pub struct CffSpec {
    pub glyphs: Vec<CffGlyph>,
    pub default_width: i32,
    pub nominal_width: i32,
    pub matrix: Option<&'static str>,
    pub cid_keyed: bool,
    /// A format 0 Encoding: the code of glyph 1, glyph 2, and so on. `None`
    /// is StandardEncoding.
    pub encoding: Option<Vec<u8>>,
}

fn index(items: &[Vec<u8>]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend((items.len() as u16).to_be_bytes());
    if items.is_empty() {
        return out;
    }
    out.push(2); // offSize
    let mut off = 1u16;
    out.extend(off.to_be_bytes());
    for i in items {
        off += i.len() as u16;
        out.extend(off.to_be_bytes());
    }
    for i in items {
        out.extend(i);
    }
    out
}

/// A DICT integer in the five-byte form, so a DICT's length does not depend
/// on the offsets it holds.
fn int(v: i32) -> Vec<u8> {
    let mut o = vec![29];
    o.extend(v.to_be_bytes());
    o
}

/// A DICT real: decimal digits as nibbles, `a` for the point, `e` for a
/// minus, `f` to end.
fn real(text: &str) -> Vec<u8> {
    let mut nibbles: Vec<u8> = text
        .bytes()
        .map(|c| match c {
            b'0'..=b'9' => c - b'0',
            b'.' => 0xA,
            b'-' => 0xE,
            _ => panic!("not a real"),
        })
        .collect();
    nibbles.push(0xF);
    if nibbles.len() % 2 == 1 {
        nibbles.push(0xF);
    }
    let mut o = vec![30];
    for pair in nibbles.chunks(2) {
        o.push(pair[0] << 4 | pair[1]);
    }
    o
}

fn dict(entries: &[(&[u8], Vec<Vec<u8>>)]) -> Vec<u8> {
    let mut d = Vec::new();
    for (op, operands) in entries {
        for o in operands {
            d.extend(o);
        }
        d.extend(*op);
    }
    d
}

pub fn build(spec: &CffSpec) -> Vec<u8> {
    let charstrings: Vec<Vec<u8>> = spec.glyphs.iter().map(|g| g.code.0.clone()).collect();
    // The charset lists the SID (or CID) of every glyph but glyph 0.
    let mut charset = vec![0u8];
    for g in spec.glyphs.iter().skip(1) {
        charset.extend(g.id.to_be_bytes());
    }
    let private = dict(&[
        (&[20], vec![int(spec.default_width)]),
        (&[21], vec![int(spec.nominal_width)]),
    ]);
    let strings: Vec<Vec<u8>> = if spec.cid_keyed {
        vec![b"Adobe".to_vec(), b"Identity".to_vec()]
    } else {
        vec![]
    };
    let header = vec![1u8, 0, 4, 2];
    let names = index(&[b"TestCFF".to_vec()]);
    let string_index = index(&strings);
    let gsubrs = index(&[]);
    let cs_index = index(&charstrings);

    // FDSelect format 3: one range covering every glyph with font DICT 0.
    let mut fdselect = vec![3u8];
    fdselect.extend(1u16.to_be_bytes());
    fdselect.extend(0u16.to_be_bytes());
    fdselect.push(0);
    fdselect.extend((spec.glyphs.len() as u16).to_be_bytes());

    let top = |charset_at: i32,
               cs_at: i32,
               private_at: i32,
               fdsel_at: i32,
               fdarr_at: i32,
               enc_at: i32| {
        let mut entries: Vec<(&[u8], Vec<Vec<u8>>)> = Vec::new();
        if let Some(m) = spec.matrix {
            entries.push((&[12, 7], m.split(' ').map(real).collect()));
        }
        entries.push((&[15], vec![int(charset_at)]));
        if spec.encoding.is_some() {
            entries.push((&[16], vec![int(enc_at)]));
        }
        entries.push((&[17], vec![int(cs_at)]));
        if spec.cid_keyed {
            // ROS: registry SID 391 (the first custom string), ordering SID
            // 392, supplement 0.
            entries.push((&[12, 30], vec![int(391), int(392), int(0)]));
            entries.push((&[12, 34], vec![int(spec.glyphs.len() as i32)]));
            entries.push((&[12, 36], vec![int(fdarr_at)]));
            entries.push((&[12, 37], vec![int(fdsel_at)]));
        } else {
            entries.push((&[18], vec![int(private.len() as i32), int(private_at)]));
        }
        dict(&entries)
    };
    let top_len = index(&[top(0, 0, 0, 0, 0, 0)]).len();
    let fixed = header.len() + names.len() + top_len + string_index.len() + gsubrs.len();
    let charstrings_at = fixed;
    let charset_at = charstrings_at + cs_index.len();
    let after_charset = charset_at + charset.len();
    let (fdsel_at, fdarr_at, private_at);
    let font_dict_index;
    if spec.cid_keyed {
        fdsel_at = after_charset;
        fdarr_at = fdsel_at + fdselect.len();
        // The Font DICT's own length is fixed too: two five-byte integers and
        // the Private operator.
        let fd_len = index(&[dict(&[(&[18], vec![int(0), int(0)])])]).len();
        private_at = fdarr_at + fd_len;
        font_dict_index = index(&[dict(&[(
            &[18],
            vec![int(private.len() as i32), int(private_at as i32)],
        )])]);
    } else {
        fdsel_at = 0;
        fdarr_at = 0;
        private_at = after_charset;
        font_dict_index = Vec::new();
    }
    // The Encoding follows the Private DICT at the end of the font.
    let enc_at = private_at + private.len();
    let top_dict = top(
        charset_at as i32,
        charstrings_at as i32,
        private_at as i32,
        fdsel_at as i32,
        fdarr_at as i32,
        enc_at as i32,
    );

    let mut out = header;
    out.extend(names);
    out.extend(index(&[top_dict]));
    out.extend(string_index);
    out.extend(gsubrs);
    out.extend(cs_index);
    out.extend(charset);
    if spec.cid_keyed {
        out.extend(fdselect);
        out.extend(font_dict_index);
    }
    out.extend(private);
    if let Some(codes) = &spec.encoding {
        out.push(0); // format 0
        out.push(codes.len() as u8);
        out.extend(codes);
    }
    out
}

/// A `CFF2` table (OpenType 1.9, CFF2): header, Top DICT with `CharStrings`
/// and `FDArray`, an empty global subroutine INDEX, the charstrings (no width,
/// no `endchar`) and one Font DICT whose Private DICT is empty.
pub fn build_cff2(charstrings: &[Cs2]) -> Vec<u8> {
    const HEADER: usize = 5;
    const TOP_DICT: usize = 13;
    const GLOBAL_SUBRS: usize = 4;
    const OP_CHARSTRINGS: u8 = 17;
    const OP_PRIVATE: u8 = 18;
    const OP_ESCAPE: u8 = 12;
    const OP_FDARRAY: u8 = 36;
    const INT32: u8 = 29;
    fn index(items: &[Vec<u8>]) -> Vec<u8> {
        let mut out = (items.len() as u32).to_be_bytes().to_vec();
        out.push(2);
        let mut at = 1u16;
        out.extend(at.to_be_bytes());
        for item in items {
            at += item.len() as u16;
            out.extend(at.to_be_bytes());
        }
        for item in items {
            out.extend(item);
        }
        out
    }
    let int32 = |v: usize| {
        let mut b = vec![INT32];
        b.extend((v as i32).to_be_bytes());
        b
    };
    let strings = index(&charstrings.iter().map(|c| c.0.clone()).collect::<Vec<_>>());
    let strings_at = HEADER + TOP_DICT + GLOBAL_SUBRS;
    let font_dicts_at = strings_at + strings.len();
    // The Font DICT: Private size 0 (the number 139 is 0), then its offset.
    let font_dict_len = 1 + 5 + 1;
    let font_dicts_len = 4 + 1 + 2 * 2 + font_dict_len;
    let private_at = font_dicts_at + font_dicts_len;
    let mut font_dict = vec![139];
    font_dict.extend(int32(private_at));
    font_dict.push(OP_PRIVATE);
    let mut top = int32(strings_at);
    top.push(OP_CHARSTRINGS);
    top.extend(int32(font_dicts_at));
    top.extend([OP_ESCAPE, OP_FDARRAY]);
    assert_eq!(top.len(), TOP_DICT);
    let mut out = vec![2, 0, HEADER as u8];
    out.extend((TOP_DICT as u16).to_be_bytes());
    out.extend(top);
    out.extend(0u32.to_be_bytes());
    out.extend(strings);
    let font_dicts = index(&[font_dict]);
    assert_eq!(font_dicts.len(), font_dicts_len);
    out.extend(font_dicts);
    out
}
