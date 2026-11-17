// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Test fixtures: font programs written byte by byte, so the reader is not
//! tested only against what the crate itself produces.
#![allow(dead_code)]

/// A Type 1 charstring under construction.
#[derive(Default, Clone)]
pub struct Cs(pub Vec<u8>);

impl Cs {
    pub fn new() -> Cs {
        Cs::default()
    }

    /// A number, in the encoding of the charstring chapter: one byte for
    /// -107..=107, two for 108..=1131 and -1131..=-108, five otherwise.
    pub fn n(mut self, v: i32) -> Cs {
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
                self.0.push(255);
                self.0.extend_from_slice(&v.to_be_bytes());
            }
        }
        self
    }

    pub fn nums(mut self, v: &[i32]) -> Cs {
        for &x in v {
            self = self.n(x);
        }
        self
    }

    /// A command by name.
    pub fn op(mut self, name: &str) -> Cs {
        match name {
            "hstem" => self.0.push(1),
            "vstem" => self.0.push(3),
            "vmoveto" => self.0.push(4),
            "rlineto" => self.0.push(5),
            "hlineto" => self.0.push(6),
            "vlineto" => self.0.push(7),
            "rrcurveto" => self.0.push(8),
            "closepath" => self.0.push(9),
            "callsubr" => self.0.push(10),
            "return" => self.0.push(11),
            "hsbw" => self.0.push(13),
            "endchar" => self.0.push(14),
            "rmoveto" => self.0.push(21),
            "hmoveto" => self.0.push(22),
            "vhcurveto" => self.0.push(30),
            "hvcurveto" => self.0.push(31),
            "dotsection" => self.0.extend([12, 0]),
            "vstem3" => self.0.extend([12, 1]),
            "hstem3" => self.0.extend([12, 2]),
            "seac" => self.0.extend([12, 6]),
            "sbw" => self.0.extend([12, 7]),
            "div" => self.0.extend([12, 12]),
            "callothersubr" => self.0.extend([12, 16]),
            "pop" => self.0.extend([12, 17]),
            "setcurrentpoint" => self.0.extend([12, 33]),
            other => panic!("unknown operator {other}"),
        }
        self
    }

    /// `x y` then the command.
    pub fn with(self, args: &[i32], name: &str) -> Cs {
        self.nums(args).op(name)
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.0.clone()
    }
}

/// The Type 1 cipher, written here again from the specification's
/// recurrence so the tests do not lean on the crate's own copy.
pub fn encrypt(plain: &[u8], key: u16, pad: usize) -> Vec<u8> {
    let mut r = key as u32;
    let mut out = Vec::new();
    for &p in std::iter::repeat_n(&0u8, pad).chain(plain) {
        let c = p ^ (r >> 8) as u8;
        r = ((c as u32 + r) * 52845 + 22719) & 0xFFFF;
        out.push(c);
    }
    out
}

pub struct Type1Spec {
    pub font_name: String,
    pub encoding: Option<Vec<(u8, String)>>,
    pub font_matrix: Option<String>,
    pub len_iv: i32,
    pub subrs: Vec<Vec<u8>>,
    pub glyphs: Vec<(String, Vec<u8>)>,
    pub rd: &'static str,
}

impl Type1Spec {
    pub fn new(glyphs: Vec<(&str, Cs)>) -> Type1Spec {
        Type1Spec {
            font_name: "TestFont".into(),
            encoding: None,
            font_matrix: None,
            len_iv: 4,
            subrs: Vec::new(),
            glyphs: glyphs
                .into_iter()
                .map(|(n, c)| (n.to_string(), c.bytes()))
                .collect(),
            rd: "RD",
        }
    }

    pub fn subrs(mut self, subrs: Vec<Cs>) -> Type1Spec {
        self.subrs = subrs.into_iter().map(|c| c.bytes()).collect();
        self
    }

    pub fn cleartext(&self) -> Vec<u8> {
        let mut t = String::new();
        t.push_str("%!PS-AdobeFont-1.0: TestFont 1.0\n11 dict begin\n");
        t.push_str("/FontInfo 5 dict dup begin\n/FullName (Test Font) readonly def\n");
        t.push_str("/FamilyName (Test) readonly def\n/ItalicAngle -12 def\n/isFixedPitch true def\nend readonly def\n");
        t.push_str(&format!("/FontName /{} def\n", self.font_name));
        match &self.encoding {
            None => t.push_str("/Encoding StandardEncoding def\n"),
            Some(entries) => {
                t.push_str("/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n");
                for (code, name) in entries {
                    t.push_str(&format!("dup {code} /{name} put\n"));
                }
                t.push_str("readonly def\n");
            }
        }
        match &self.font_matrix {
            None => t.push_str("/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n"),
            Some(m) => t.push_str(&format!("/FontMatrix {m} readonly def\n")),
        }
        t.push_str(
            "/FontBBox {-10 -20 1000 900} readonly def\ncurrentdict end\ncurrentfile eexec\n",
        );
        t.into_bytes()
    }

    fn seal(&self, code: &[u8]) -> Vec<u8> {
        if self.len_iv < 0 {
            code.to_vec()
        } else {
            encrypt(code, 4330, self.len_iv as usize)
        }
    }

    /// The private dictionary as plain text and bytes, before `eexec`
    /// encryption.
    pub fn private(&self) -> Vec<u8> {
        let rd = self.rd;
        let mut p: Vec<u8> = Vec::new();
        let mut put = |s: &str| p.extend_from_slice(s.as_bytes());
        put("dup /Private 8 dict dup begin\n");
        put(&format!(
            "/{rd} {{string currentfile exch readstring pop}} executeonly def\n"
        ));
        put("/ND {noaccess def} executeonly def\n/NP {noaccess put} executeonly def\n");
        put(&format!("/lenIV {} def\n", self.len_iv));
        put("/OtherSubrs [ {} {} {} { dup /x (str)  } ] def\n");
        let mut tail: Vec<u8> = Vec::new();
        tail.extend_from_slice(format!("/Subrs {} array\n", self.subrs.len()).as_bytes());
        for (i, s) in self.subrs.iter().enumerate() {
            let sealed = self.seal(s);
            tail.extend_from_slice(format!("dup {i} {} {rd} ", sealed.len()).as_bytes());
            tail.extend_from_slice(&sealed);
            tail.extend_from_slice(b" NP\n");
        }
        tail.extend_from_slice(b"ND\n");
        tail.extend_from_slice(
            format!(
                "2 index /CharStrings {} dict dup begin\n",
                self.glyphs.len()
            )
            .as_bytes(),
        );
        for (name, code) in &self.glyphs {
            let sealed = self.seal(code);
            tail.extend_from_slice(format!("/{name} {} {rd} ", sealed.len()).as_bytes());
            tail.extend_from_slice(&sealed);
            tail.extend_from_slice(b" ND\n");
        }
        tail.extend_from_slice(b"end end readonly put noaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n");
        let mut all = p;
        all.extend_from_slice(&tail);
        all
    }

    /// A font program as the `FontFile` of a PDF holds it: cleartext, then
    /// the binary encrypted portion, then the zeros and `cleartomark`.
    pub fn flat(&self) -> Vec<u8> {
        let mut f = self.cleartext();
        f.extend_from_slice(&encrypt(&self.private(), 55665, 4));
        f.extend_from_slice(&trailer());
        f
    }

    /// The same with the encrypted portion in hexadecimal digits.
    pub fn hex(&self) -> Vec<u8> {
        let mut f = self.cleartext();
        let enc = encrypt(&self.private(), 55665, 4);
        for (i, b) in enc.iter().enumerate() {
            f.extend_from_slice(format!("{b:02x}").as_bytes());
            if i % 32 == 31 {
                f.push(b'\n');
            }
        }
        f.push(b'\n');
        f.extend_from_slice(&trailer());
        f
    }

    /// The same as a PFB file.
    pub fn pfb(&self) -> Vec<u8> {
        let seg = |kind: u8, body: &[u8]| {
            let mut s = vec![0x80, kind];
            s.extend_from_slice(&(body.len() as u32).to_le_bytes());
            s.extend_from_slice(body);
            s
        };
        let mut f = seg(1, &self.cleartext());
        f.extend(seg(2, &encrypt(&self.private(), 55665, 4)));
        f.extend(seg(1, &trailer()));
        f.extend_from_slice(&[0x80, 3]);
        f
    }
}

fn trailer() -> Vec<u8> {
    let mut t = Vec::new();
    for _ in 0..8 {
        t.extend_from_slice(&[b'0'; 64]);
        t.push(b'\n');
    }
    t.extend_from_slice(b"cleartomark\n");
    t
}

pub mod cff;
pub mod sfnt;
