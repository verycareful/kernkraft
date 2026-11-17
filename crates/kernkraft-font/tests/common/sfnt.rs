// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A TrueType font written table by table (OpenType 1.9: head, hhea, maxp,
//! hmtx, loca, glyf, cmap, name, OS/2, post).
#![allow(dead_code)]

pub struct Glyph {
    pub name: &'static str,
    pub advance: u16,
    /// Contours of `(x, y, on_curve)`; empty for a glyph with no marks.
    pub contours: Vec<Vec<(i16, i16, bool)>>,
}

pub struct SfntSpec {
    pub glyphs: Vec<Glyph>,
    pub units_per_em: u16,
    pub ascender: i16,
    pub descender: i16,
    /// `(character, glyph index)` for the Windows Unicode BMP subtable.
    pub unicode: Vec<(u16, u16)>,
    /// `(code, glyph index)` for the Macintosh Roman subtable (format 0).
    pub mac: Vec<(u8, u16)>,
    pub family: &'static str,
    pub subfamily: &'static str,
    pub full: &'static str,
    pub postscript: &'static str,
    pub weight: u16,
    pub italic: bool,
    pub fixed_pitch: bool,
    pub cap_height: i16,
    pub x_height: i16,
    pub italic_angle_tenths: i16,
}

impl SfntSpec {
    pub fn new(glyphs: Vec<Glyph>) -> SfntSpec {
        SfntSpec {
            glyphs,
            units_per_em: 1000,
            ascender: 800,
            descender: -200,
            unicode: Vec::new(),
            mac: Vec::new(),
            family: "Test Sans",
            subfamily: "Regular",
            full: "Test Sans Regular",
            postscript: "TestSans-Regular",
            weight: 400,
            italic: false,
            fixed_pitch: false,
            cap_height: 700,
            x_height: 500,
            italic_angle_tenths: 0,
        }
    }
}

fn u16b(v: u16) -> [u8; 2] {
    v.to_be_bytes()
}
fn i16b(v: i16) -> [u8; 2] {
    v.to_be_bytes()
}
fn u32b(v: u32) -> [u8; 4] {
    v.to_be_bytes()
}

fn glyf_entry(contours: &[Vec<(i16, i16, bool)>]) -> Vec<u8> {
    if contours.is_empty() {
        return Vec::new();
    }
    let pts: Vec<(i16, i16, bool)> = contours.iter().flatten().copied().collect();
    let (xmin, xmax) = (
        pts.iter().map(|p| p.0).min().unwrap_or(0),
        pts.iter().map(|p| p.0).max().unwrap_or(0),
    );
    let (ymin, ymax) = (
        pts.iter().map(|p| p.1).min().unwrap_or(0),
        pts.iter().map(|p| p.1).max().unwrap_or(0),
    );
    let mut g = Vec::new();
    g.extend(i16b(contours.len() as i16));
    for v in [xmin, ymin, xmax, ymax] {
        g.extend(i16b(v));
    }
    let mut end = 0u16;
    for c in contours {
        end += c.len() as u16;
        g.extend(u16b(end - 1));
    }
    g.extend(u16b(0)); // instruction length
                       // One flag byte per point: bit 0 on-curve; x and y as full 16-bit deltas
                       // (neither short-vector bit nor same-bit set).
    for p in &pts {
        g.push(u8::from(p.2));
    }
    let mut prev = 0i16;
    for p in &pts {
        g.extend(i16b(p.0 - prev));
        prev = p.0;
    }
    prev = 0;
    for p in &pts {
        g.extend(i16b(p.1 - prev));
        prev = p.1;
    }
    while g.len() % 4 != 0 {
        g.push(0);
    }
    g
}

fn cmap_format4(map: &[(u16, u16)]) -> Vec<u8> {
    let mut sorted = map.to_vec();
    sorted.sort();
    // One segment per character, then the 0xFFFF terminator.
    let seg = sorted.len() + 1;
    let mut t = Vec::new();
    t.extend(u16b(4));
    t.extend(u16b((16 + seg * 8) as u16));
    t.extend(u16b(0));
    t.extend(u16b((seg * 2) as u16));
    let mut pow = 1usize;
    let mut log = 0u16;
    while pow * 2 <= seg {
        pow *= 2;
        log += 1;
    }
    t.extend(u16b((pow * 2) as u16));
    t.extend(u16b(log));
    t.extend(u16b((seg * 2 - pow * 2) as u16));
    for &(c, _) in &sorted {
        t.extend(u16b(c));
    }
    t.extend(u16b(0xFFFF));
    t.extend(u16b(0));
    for &(c, _) in &sorted {
        t.extend(u16b(c));
    }
    t.extend(u16b(0xFFFF));
    for &(c, g) in &sorted {
        t.extend(u16b(g.wrapping_sub(c)));
    }
    t.extend(u16b(1));
    for _ in 0..seg {
        t.extend(u16b(0));
    }
    t
}

fn cmap_format0(map: &[(u8, u16)]) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(u16b(0));
    t.extend(u16b(262));
    t.extend(u16b(0));
    let mut arr = [0u8; 256];
    for &(c, g) in map {
        arr[c as usize] = g as u8;
    }
    t.extend(arr);
    t
}

fn name_table(spec: &SfntSpec) -> Vec<u8> {
    let records = [
        (1u16, spec.family),
        (2, spec.subfamily),
        (4, spec.full),
        (6, spec.postscript),
    ];
    let mut strings = Vec::new();
    let mut recs = Vec::new();
    for (id, s) in records {
        let utf16: Vec<u8> = s.encode_utf16().flat_map(|u| u.to_be_bytes()).collect();
        recs.extend(u16b(3));
        recs.extend(u16b(1));
        recs.extend(u16b(0x409));
        recs.extend(u16b(id));
        recs.extend(u16b(utf16.len() as u16));
        recs.extend(u16b(strings.len() as u16));
        strings.extend(utf16);
    }
    let mut t = Vec::new();
    t.extend(u16b(0));
    t.extend(u16b(records.len() as u16));
    t.extend(u16b((6 + recs.len()) as u16));
    t.extend(recs);
    t.extend(strings);
    t
}

fn post_table(spec: &SfntSpec) -> Vec<u8> {
    // Format 2.0: every glyph name is a custom one (index 258 and up).
    let mut t = Vec::new();
    t.extend(u32b(0x0002_0000));
    // italicAngle is Fixed 16.16.
    t.extend(((i32::from(spec.italic_angle_tenths) * 65536) / 10).to_be_bytes());
    t.extend(i16b(-100));
    t.extend(i16b(50));
    t.extend(u32b(u32::from(spec.fixed_pitch)));
    t.extend([0u8; 16]);
    t.extend(u16b(spec.glyphs.len() as u16));
    for i in 0..spec.glyphs.len() {
        t.extend(u16b(258 + i as u16));
    }
    for g in &spec.glyphs {
        t.push(g.name.len() as u8);
        t.extend(g.name.as_bytes());
    }
    t
}

fn os2_table(spec: &SfntSpec) -> Vec<u8> {
    let mut t = Vec::new();
    t.extend(u16b(2)); // version
    t.extend(i16b(500)); // xAvgCharWidth
    t.extend(u16b(spec.weight));
    t.extend(u16b(5)); // usWidthClass: medium
    t.extend(u16b(0)); // fsType
    for _ in 0..10 {
        t.extend(i16b(0)); // subscript, superscript and strikeout sizes
    }
    t.extend(i16b(0)); // sFamilyClass
    t.extend([0u8; 10]); // panose
    t.extend([0u8; 16]); // ulUnicodeRange
    t.extend(*b"TEST");
    // fsSelection: bit 0 italic, bit 6 regular (bit 5 bold is left to the weight).
    let sel: u16 = if spec.italic { 1 } else { 1 << 6 };
    t.extend(u16b(sel));
    t.extend(u16b(0x20));
    t.extend(u16b(0x7E));
    t.extend(i16b(spec.ascender));
    t.extend(i16b(spec.descender));
    t.extend(i16b(0));
    t.extend(u16b(spec.ascender as u16));
    t.extend(u16b((-spec.descender) as u16));
    t.extend(u32b(0)); // ulCodePageRange
    t.extend(u32b(0));
    t.extend(i16b(spec.x_height));
    t.extend(i16b(spec.cap_height));
    t.extend(u16b(0));
    t.extend(u16b(0x20));
    t.extend(u16b(0));
    t
}

pub fn build(spec: &SfntSpec) -> Vec<u8> {
    let n = spec.glyphs.len();
    let mut glyf = Vec::new();
    let mut loca = Vec::new();
    for g in &spec.glyphs {
        loca.extend(u16b((glyf.len() / 2) as u16));
        glyf.extend(glyf_entry(&g.contours));
    }
    loca.extend(u16b((glyf.len() / 2) as u16));

    let mut head = Vec::new();
    head.extend(u32b(0x0001_0000));
    head.extend(u32b(0x0001_0000));
    head.extend(u32b(0));
    head.extend(u32b(0x5F0F_3CF5));
    head.extend(u16b(0));
    head.extend(u16b(spec.units_per_em));
    head.extend([0u8; 16]);
    for v in [0i16, -200, 1000, 900] {
        head.extend(i16b(v));
    }
    head.extend(u16b(0));
    head.extend(u16b(8));
    head.extend(i16b(2));
    head.extend(i16b(0)); // short loca
    head.extend(i16b(0));

    let mut hhea = Vec::new();
    hhea.extend(u32b(0x0001_0000));
    hhea.extend(i16b(spec.ascender));
    hhea.extend(i16b(spec.descender));
    hhea.extend(i16b(0));
    hhea.extend(u16b(1000));
    hhea.extend([0u8; 6]);
    hhea.extend(i16b(1));
    hhea.extend(i16b(0));
    hhea.extend(i16b(0));
    hhea.extend([0u8; 8]);
    hhea.extend(i16b(0));
    hhea.extend(u16b(n as u16));

    let mut maxp = Vec::new();
    maxp.extend(u32b(0x0000_5000));
    maxp.extend(u16b(n as u16));

    let mut hmtx = Vec::new();
    for g in &spec.glyphs {
        hmtx.extend(u16b(g.advance));
        hmtx.extend(i16b(0));
    }

    let mut cmap = Vec::new();
    let uni = cmap_format4(&spec.unicode);
    let mac = cmap_format0(&spec.mac);
    cmap.extend(u16b(0));
    cmap.extend(u16b(2));
    // Macintosh Roman (1, 0) first, then Windows Unicode BMP (3, 1): records
    // are in ascending platform order.
    let base = 4 + 2 * 8;
    cmap.extend(u16b(1));
    cmap.extend(u16b(0));
    cmap.extend(u32b(base as u32));
    cmap.extend(u16b(3));
    cmap.extend(u16b(1));
    cmap.extend(u32b((base + mac.len()) as u32));
    cmap.extend(mac);
    cmap.extend(uni);

    let mut tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"OS/2", os2_table(spec)),
        (b"cmap", cmap),
        (b"glyf", glyf),
        (b"head", head),
        (b"hhea", hhea),
        (b"hmtx", hmtx),
        (b"loca", loca),
        (b"maxp", maxp),
        (b"name", name_table(spec)),
        (b"post", post_table(spec)),
    ];
    tables.sort_by_key(|t| *t.0);

    let count = tables.len();
    let mut out = Vec::new();
    out.extend(u32b(0x0001_0000));
    out.extend(u16b(count as u16));
    let mut pow = 1usize;
    let mut log = 0u16;
    while pow * 2 <= count {
        pow *= 2;
        log += 1;
    }
    out.extend(u16b((pow * 16) as u16));
    out.extend(u16b(log));
    out.extend(u16b((count * 16 - pow * 16) as u16));
    let mut offset = 12 + count * 16;
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend(*tag);
        out.extend(u32b(0));
        out.extend(u32b(offset as u32));
        out.extend(u32b(data.len() as u32));
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
        offset = 12 + count * 16 + body.len();
    }
    out.extend(body);
    out
}

/// A triangle: (0,0) (100,0) (50,100), all on the curve.
pub fn triangle() -> Vec<Vec<(i16, i16, bool)>> {
    vec![vec![(0, 0, true), (100, 0, true), (50, 100, true)]]
}

/// One quadratic arc: on (0,0), off (50,100), on (100,0).
pub fn arc() -> Vec<Vec<(i16, i16, bool)>> {
    vec![vec![(0, 0, true), (50, 100, false), (100, 0, true)]]
}

/// Where a table's bytes sit in a built font: `(offset, length)`.
pub fn table(font: &[u8], tag: &[u8; 4]) -> Option<(usize, usize)> {
    let count = u16::from_be_bytes([font[4], font[5]]) as usize;
    for i in 0..count {
        let r = 12 + i * 16;
        if &font[r..r + 4] == tag {
            let off =
                u32::from_be_bytes([font[r + 8], font[r + 9], font[r + 10], font[r + 11]]) as usize;
            let len = u32::from_be_bytes([font[r + 12], font[r + 13], font[r + 14], font[r + 15]])
                as usize;
            return Some((off, len));
        }
    }
    None
}

/// A TrueType collection of the fonts, each placed after the header with its
/// table offsets moved to match.
pub fn collection(fonts: &[Vec<u8>]) -> Vec<u8> {
    let header = 12 + 4 * fonts.len();
    let mut out = Vec::new();
    out.extend(*b"ttcf");
    out.extend(0x0001_0000u32.to_be_bytes());
    out.extend((fonts.len() as u32).to_be_bytes());
    let mut at = header;
    let mut body = Vec::new();
    for f in fonts {
        out.extend((at as u32).to_be_bytes());
        let mut moved = f.clone();
        let count = u16::from_be_bytes([moved[4], moved[5]]) as usize;
        for i in 0..count {
            let r = 12 + i * 16 + 8;
            let off = u32::from_be_bytes([moved[r], moved[r + 1], moved[r + 2], moved[r + 3]])
                + at as u32;
            moved[r..r + 4].copy_from_slice(&off.to_be_bytes());
        }
        at += moved.len();
        body.extend(moved);
    }
    out.extend(body);
    out
}
