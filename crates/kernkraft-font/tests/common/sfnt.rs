// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A TrueType font written table by table (OpenType 1.9: head, hhea, maxp,
//! hmtx, loca, glyf, cmap, name, OS/2, post), and with one variation axis
//! (fvar, gvar).
#![allow(dead_code)]

pub struct Glyph {
    pub name: &'static str,
    pub advance: u16,
    /// Contours of `(x, y, on_curve)`; empty for a glyph with no marks.
    pub contours: Vec<Vec<(i16, i16, bool)>>,
}

/// How one glyph moves at the maximum of the axis, in design units: a delta
/// for each point of its contours in order, and the change of its advance.
/// Between the default and the maximum the deltas scale linearly.
#[derive(Clone)]
pub struct Move {
    pub dx: Vec<i16>,
    pub dy: Vec<i16>,
    pub advance: i16,
}

/// A named instance of the axis: its subfamily name (`name` ID `name_id`, from
/// 256 up), the axis value, and optionally the ID of its PostScript name.
#[derive(Clone)]
pub struct Instance {
    pub name_id: u16,
    pub value: i32,
    pub postscript_name_id: Option<u16>,
}

/// One variation axis: `min <= default < max`, and a [`Move`] for each glyph
/// that moves (`None` for one that does not). Below the default nothing moves.
#[derive(Clone)]
pub struct Variable {
    pub tag: [u8; 4],
    pub min: i32,
    pub default: i32,
    pub max: i32,
    pub moves: Vec<Option<Move>>,
    pub instances: Vec<Instance>,
}

pub struct SfntSpec {
    pub glyphs: Vec<Glyph>,
    pub variable: Option<Variable>,
    /// More `name` records, Windows English: `(name ID, string)`.
    pub extra_names: Vec<(u16, &'static str)>,
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
            variable: None,
            extra_names: Vec::new(),
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
    let mut records = vec![
        (1u16, spec.family),
        (2, spec.subfamily),
        (4, spec.full),
        (6, spec.postscript),
    ];
    records.extend(spec.extra_names.iter().copied());
    records.sort_by_key(|r| r.0);
    let mut strings = Vec::new();
    let mut recs = Vec::new();
    for &(id, s) in &records {
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

fn fixed(v: i32) -> [u8; 4] {
    (v << 16).to_be_bytes()
}

/// The `fvar` table (OpenType 1.9): one axis and its named instances; every
/// instance record carries a PostScript name ID when any does (0xFFFF for
/// none).
fn fvar_table(v: &Variable) -> Vec<u8> {
    const AXIS_ARRAY_OFFSET: u16 = 16;
    const AXIS_SIZE: u16 = 20;
    const NAME_ID: u16 = 256;
    const NO_NAME: u16 = 0xFFFF;
    let with_postscript = v.instances.iter().any(|i| i.postscript_name_id.is_some());
    let instance_size: u16 = 4 + 4 + if with_postscript { 2 } else { 0 };
    let mut t = Vec::new();
    t.extend(u16b(1));
    t.extend(u16b(0));
    t.extend(u16b(AXIS_ARRAY_OFFSET));
    t.extend(u16b(2));
    t.extend(u16b(1));
    t.extend(u16b(AXIS_SIZE));
    t.extend(u16b(v.instances.len() as u16));
    t.extend(u16b(instance_size));
    t.extend(v.tag);
    t.extend(fixed(v.min));
    t.extend(fixed(v.default));
    t.extend(fixed(v.max));
    t.extend(u16b(0));
    t.extend(u16b(NAME_ID));
    for i in &v.instances {
        t.extend(u16b(i.name_id));
        t.extend(u16b(0));
        t.extend(fixed(i.value));
        if with_postscript {
            t.extend(u16b(i.postscript_name_id.unwrap_or(NO_NAME)));
        }
    }
    t
}

/// Packed point deltas: runs of at most 64 words.
fn packed_deltas(deltas: &[i16]) -> Vec<u8> {
    const WORDS: u8 = 0x40;
    const RUN: usize = 64;
    let mut out = Vec::new();
    for run in deltas.chunks(RUN) {
        out.push(WORDS | (run.len() as u8 - 1));
        for d in run {
            out.extend(i16b(*d));
        }
    }
    out
}

/// The `gvar` table (OpenType 1.9): one tuple a moving glyph, peaking at the
/// axis maximum (1.0), over all points, the four phantom points last. The
/// advance changes through the right phantom point.
fn gvar_table(spec: &SfntSpec, v: &Variable) -> Vec<u8> {
    const HEADER: usize = 20;
    const EMBEDDED_PEAK: u16 = 0x8000;
    const PRIVATE_POINTS: u16 = 0x2000;
    const PEAK_ONE: u16 = 0x4000;
    const ALL_POINTS: u8 = 0;
    let n = spec.glyphs.len();
    let mut data = Vec::new();
    let mut offsets = vec![0u16];
    for (i, g) in spec.glyphs.iter().enumerate() {
        if let Some(Some(m)) = v.moves.get(i) {
            let points: usize = g.contours.iter().map(Vec::len).sum();
            assert_eq!((m.dx.len(), m.dy.len()), (points, points));
            let mut dx = m.dx.clone();
            dx.extend([0, m.advance, 0, 0]);
            let mut dy = m.dy.clone();
            dy.extend([0; 4]);
            let mut body = vec![ALL_POINTS];
            body.extend(packed_deltas(&dx));
            body.extend(packed_deltas(&dy));
            let mut entry = Vec::new();
            entry.extend(u16b(1));
            entry.extend(u16b(4 + 6));
            entry.extend(u16b(body.len() as u16));
            entry.extend(u16b(EMBEDDED_PEAK | PRIVATE_POINTS));
            entry.extend(u16b(PEAK_ONE));
            entry.extend(body);
            if entry.len() % 2 != 0 {
                entry.push(0);
            }
            data.extend(entry);
        }
        offsets.push((data.len() / 2) as u16);
    }
    let array = HEADER + (n + 1) * 2;
    let mut t = Vec::new();
    t.extend(u16b(1));
    t.extend(u16b(0));
    t.extend(u16b(1));
    t.extend(u16b(0));
    t.extend(u32b(array as u32));
    t.extend(u16b(n as u16));
    t.extend(u16b(0));
    t.extend(u32b(array as u32));
    for o in offsets {
        t.extend(u16b(o));
    }
    t.extend(data);
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
    if let Some(v) = &spec.variable {
        tables.push((b"fvar", fvar_table(v)));
        tables.push((b"gvar", gvar_table(spec, v)));
    }
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

/// The font with the tables in `drop` removed and `add` put in, under the
/// sfnt version `version` (`0x0001_0000` for glyf outlines, `OTTO` for CFF).
pub fn retable(
    font: &[u8],
    version: [u8; 4],
    drop: &[&[u8; 4]],
    add: Vec<([u8; 4], Vec<u8>)>,
) -> Vec<u8> {
    let count = u16::from_be_bytes([font[4], font[5]]) as usize;
    let mut tables: Vec<([u8; 4], Vec<u8>)> = Vec::new();
    for i in 0..count {
        let tag: [u8; 4] = font[12 + i * 16..16 + i * 16].try_into().unwrap();
        if drop.contains(&&tag) {
            continue;
        }
        let (off, len) = table(font, &tag).unwrap();
        tables.push((tag, font[off..off + len].to_vec()));
    }
    tables.extend(add);
    tables.sort_by_key(|t| t.0);
    let n = tables.len();
    let mut out = Vec::new();
    out.extend(version);
    out.extend((n as u16).to_be_bytes());
    let mut pow = 1usize;
    let mut log = 0u16;
    while pow * 2 <= n {
        pow *= 2;
        log += 1;
    }
    out.extend(((pow * 16) as u16).to_be_bytes());
    out.extend(log.to_be_bytes());
    out.extend(((n * 16 - pow * 16) as u16).to_be_bytes());
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend(tag);
        out.extend(0u32.to_be_bytes());
        out.extend(((12 + n * 16 + body.len()) as u32).to_be_bytes());
        out.extend((data.len() as u32).to_be_bytes());
        body.extend(data);
        while body.len() % 4 != 0 {
            body.push(0);
        }
    }
    out.extend(body);
    out
}

/// An OpenType font with `cff` as its outlines, one glyph for each advance
/// (the CFF must hold as many glyphs), reached from `unicode`: `(character,
/// glyph index)`.
pub fn open_type_with_cff(cff: Vec<u8>, advances: &[u16], unicode: &[(u16, u16)]) -> Vec<u8> {
    let mut spec = SfntSpec::new(
        advances
            .iter()
            .map(|&advance| Glyph {
                name: "g",
                advance,
                contours: vec![],
            })
            .collect(),
    );
    spec.unicode = unicode.to_vec();
    let base = build(&spec);
    retable(&base, *b"OTTO", &[b"glyf", b"loca"], vec![(*b"CFF ", cff)])
}
