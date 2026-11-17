// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! A Type 1 font program written byte by byte: a cleartext part with an
//! `Encoding` array, the `eexec`-encrypted private dictionary with
//! charstrings, and the trailer (Adobe's Type 1 Font Format, chapters 5
//! and 7).

const EEXEC_KEY: u16 = 55665;
const CHARSTRING_KEY: u16 = 4330;
const LEN_IV: usize = 4;

fn encrypt(plain: &[u8], key: u16, pad: usize) -> Vec<u8> {
    let mut r = u32::from(key);
    let mut out = Vec::new();
    for &p in std::iter::repeat_n(&0u8, pad).chain(plain) {
        let c = p ^ (r >> 8) as u8;
        r = ((u32::from(c) + r) * 52845 + 22719) & 0xFFFF;
        out.push(c);
    }
    out
}

/// A font of the glyphs named, each an empty outline of width 500 (`0 500
/// hsbw endchar`), whose built-in encoding gives `encoding`'s codes the
/// names.
pub fn type1(glyphs: &[&str], encoding: &[(u8, &str)]) -> Vec<u8> {
    let mut clear = String::from("%!PS-AdobeFont-1.0: Test 1.0\n11 dict begin\n");
    clear.push_str(
        "/FontName /Test def\n/Encoding 256 array\n0 1 255 {1 index exch /.notdef put} for\n",
    );
    for (code, name) in encoding {
        clear.push_str(&format!("dup {code} /{name} put\n"));
    }
    clear.push_str("readonly def\n/FontMatrix [0.001 0 0 0.001 0 0] readonly def\n");
    clear.push_str("/FontBBox {0 0 1000 1000} readonly def\ncurrentdict end\ncurrentfile eexec\n");

    // `0 500 hsbw endchar`: 0 is byte 139, 500 two bytes (500 - 108 = 392).
    let charstring = [139, 247 + (392 >> 8) as u8, (392 & 0xFF) as u8, 13, 14];
    let sealed = encrypt(&charstring, CHARSTRING_KEY, LEN_IV);
    let mut private = Vec::new();
    private.extend_from_slice(
        b"dup /Private 8 dict dup begin\n/RD {string currentfile exch readstring pop} executeonly def\n\
          /ND {noaccess def} executeonly def\n/NP {noaccess put} executeonly def\n\
          /lenIV 4 def\n/Subrs 0 array\nND\n",
    );
    private.extend_from_slice(
        format!("2 index /CharStrings {} dict dup begin\n", glyphs.len()).as_bytes(),
    );
    for name in glyphs {
        private.extend_from_slice(format!("/{name} {} RD ", sealed.len()).as_bytes());
        private.extend_from_slice(&sealed);
        private.extend_from_slice(b" ND\n");
    }
    private.extend_from_slice(
        b"end end readonly put noaccess put\ndup /FontName get exch definefont pop\nmark currentfile closefile\n",
    );

    let mut file = clear.into_bytes();
    file.extend_from_slice(&encrypt(&private, EEXEC_KEY, LEN_IV));
    for _ in 0..8 {
        file.extend_from_slice(&[b'0'; 64]);
        file.push(b'\n');
    }
    file.extend_from_slice(b"cleartomark\n");
    file
}
