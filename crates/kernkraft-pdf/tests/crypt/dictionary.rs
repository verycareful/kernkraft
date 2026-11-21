// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading encryption dictionaries that a test writes: what is refused and
//! what is reported.

use super::*;
use kernkraft_pdf::crypt::{CryptMethod, Revision};
use kernkraft_pdf::DiagnosticKind;

/// 32 bytes as a hexadecimal string, for `O` and `U` of revisions 2 to 4.
fn hex_32() -> String {
    format!("<{}>", "ab".repeat(32))
}

fn hex_bytes(count: usize) -> String {
    format!("<{}>", "cd".repeat(count))
}

/// A revision 3 dictionary with `entries` in place of the defaults that
/// share their key.
fn r3(entries: &[(&str, &str)]) -> String {
    let o = hex_32();
    let u = hex_32();
    let mut base: Vec<(&str, String)> = vec![
        ("Filter", "/Standard".into()),
        ("V", "2".into()),
        ("R", "3".into()),
        ("Length", "128".into()),
        ("O", o),
        ("U", u),
        ("P", "-4".into()),
    ];
    for (key, value) in entries {
        match base.iter_mut().find(|(k, _)| k == key) {
            Some(slot) => slot.1 = (*value).into(),
            None => base.push((key, (*value).into())),
        }
    }
    let body: Vec<String> = base
        .into_iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(key, value)| format!("/{key} {value}"))
        .collect();
    format!("<<{}>>", body.join(" "))
}

const ID: &str = "/ID [<0123456789abcdef0123456789abcdef> <00>]";

fn read(dictionary: &str, trailer: &str) -> (Result<Option<Encryption>, PdfError>, Diagnostics) {
    let bytes = file_with_trailer(&[dictionary], &format!("/Encrypt 2 0 R {trailer}"));
    let mut diagnostics = Diagnostics::new();
    let file = open_bytes(bytes, &mut diagnostics).unwrap();
    (Encryption::read(&file, &mut diagnostics), diagnostics)
}

fn read_error(dictionary: &str) -> PdfError {
    match read(dictionary, ID).0 {
        Err(error) => error,
        Ok(found) => panic!("expected an error for {dictionary}, read {found:?}"),
    }
}

#[test]
fn a_minimal_revision_3_dictionary() {
    let (encryption, diagnostics) = read(&r3(&[]), ID);
    let encryption = encryption.unwrap().unwrap();
    assert_eq!(encryption.revision(), Revision::R3);
    assert_eq!(encryption.key_bits(), 128);
    assert_eq!(encryption.string_method(), CryptMethod::Rc4);
    assert!(diagnostics.is_empty());
}

#[test]
fn a_direct_dictionary_in_the_trailer() {
    let bytes = file_with_trailer(&[], &format!("/Encrypt {} {ID}", r3(&[])));
    let mut diagnostics = Diagnostics::new();
    let file = open_bytes(bytes, &mut diagnostics).unwrap();
    let encryption = Encryption::read(&file, &mut diagnostics).unwrap().unwrap();
    assert_eq!(encryption.revision(), Revision::R3);
}

#[test]
fn handlers_other_than_standard_are_refused() {
    for handler in ["Adobe.PPKLite", "Adobe.PubSec", "Entrust.PPKEF"] {
        match read_error(&r3(&[("Filter", &format!("/{handler}"))])) {
            PdfError::UnsupportedSecurityHandler { name } => assert_eq!(name, handler),
            other => panic!("{handler}: {other:?}"),
        }
    }
    assert!(matches!(
        read_error(&r3(&[("Filter", "")])),
        PdfError::EncryptionDictionary { key: "Filter" }
    ));
}

/// Table 21 pairs: revision 2 or 3 with `V` 1 or 2, 4 with 4, 5 or 6 with
/// 5. `V` 0 (absent), 3 and every other pair are refused.
#[test]
fn algorithm_and_revision_pairs() {
    let refused = [
        ("", "3"),
        ("0", "2"),
        ("3", "3"),
        ("4", "3"),
        ("2", "4"),
        ("5", "4"),
        ("6", "6"),
        ("1", "1"),
    ];
    for (v, r) in refused {
        match read_error(&r3(&[("V", v), ("R", r)])) {
            PdfError::UnsupportedEncryption { revision, .. } => {
                assert_eq!(revision.to_string(), r)
            }
            other => panic!("V {v} R {r}: {other:?}"),
        }
    }
    for (v, r, revision) in [
        ("1", "2", Revision::R2),
        ("1", "3", Revision::R3),
        ("2", "2", Revision::R2),
    ] {
        let (encryption, _) = read(&r3(&[("V", v), ("R", r)]), ID);
        let encryption = encryption.unwrap().unwrap();
        assert_eq!(encryption.revision(), revision, "V {v} R {r}");
    }
}

/// Revision 2 and `V` 1 always use 40 bits (Table 20, Algorithm 2 step i).
#[test]
fn key_lengths() {
    let cases = [
        (("1", "3", "128"), 40),
        (("2", "2", "128"), 40),
        (("2", "3", "40"), 40),
        (("2", "3", "96"), 96),
        (("2", "3", ""), 40),
    ];
    for ((v, r, length), bits) in cases {
        let (encryption, _) = read(&r3(&[("V", v), ("R", r), ("Length", length)]), ID);
        assert_eq!(
            encryption.unwrap().unwrap().key_bits(),
            bits,
            "V {v} R {r} Length {length}"
        );
    }
    for length in ["32", "136", "100", "-128", "/Big"] {
        assert!(
            matches!(
                read_error(&r3(&[("Length", length)])),
                PdfError::EncryptionDictionary { key: "Length" }
            ),
            "Length {length}"
        );
    }
}

#[test]
fn required_entries() {
    let cases = [
        ("R", ""),
        ("O", ""),
        ("U", ""),
        ("P", ""),
        ("O", "/Name"),
        ("U", "(short)"),
        ("P", "1.5"),
        ("P", "4294967296"),
        ("P", "-2147483649"),
    ];
    for (key, value) in cases {
        match read_error(&r3(&[(key, value)])) {
            PdfError::EncryptionDictionary { key: found } => {
                assert_eq!(found, key, "{key} {value}")
            }
            other => panic!("{key} {value}: {other:?}"),
        }
    }
    // P as an unsigned 32-bit number is the same as its signed form.
    let (signed, _) = read(&r3(&[("P", "-4")]), ID);
    let (unsigned, _) = read(&r3(&[("P", "4294967292")]), ID);
    assert!(signed.unwrap().is_some() && unsigned.unwrap().is_some());
}

#[test]
fn the_encrypt_entry_itself() {
    let bytes = file_with_trailer(&[], "/Encrypt 7");
    let mut diagnostics = Diagnostics::new();
    let file = open_bytes(bytes, &mut diagnostics).unwrap();
    assert!(matches!(
        Encryption::read(&file, &mut diagnostics),
        Err(PdfError::EncryptionDictionary { key: "Encrypt" })
    ));
    // A reference to an object the file does not have.
    let bytes = file_with_trailer(&[], "/Encrypt 9 0 R");
    let file = open_bytes(bytes, &mut diagnostics).unwrap();
    assert!(matches!(
        Encryption::read(&file, &mut diagnostics),
        Err(PdfError::EncryptionDictionary { key: "Encrypt" })
    ));
    // A null `Encrypt` is no encryption (7.3.7).
    let bytes = file_with_trailer(&[], "/Encrypt null");
    let file = open_bytes(bytes, &mut diagnostics).unwrap();
    assert!(Encryption::read(&file, &mut diagnostics).unwrap().is_none());
}

#[test]
fn a_missing_file_identifier_is_reported() {
    let (encryption, diagnostics) = read(&r3(&[]), "");
    assert!(encryption.unwrap().is_some());
    let kinds: Vec<_> = diagnostics
        .as_slice()
        .iter()
        .map(|d| d.kind.clone())
        .collect();
    assert_eq!(kinds, [DiagnosticKind::MissingFileId]);
}

/// A revision 6 dictionary with `CF`, `StmF` and `StrF` as given.
fn r6(filters: &str, stmf: &str, strf: &str) -> String {
    format!(
        "<</Filter /Standard /V 5 /R 6 /Length 256 /O {o} /U {u} /OE {oe} /UE {ue} \
         /Perms {perms} /P -4 /CF {filters} /StmF {stmf} /StrF {strf}>>",
        o = hex_bytes(48),
        u = hex_bytes(48),
        oe = hex_bytes(32),
        ue = hex_bytes(32),
        perms = hex_bytes(16),
    )
}

#[test]
fn crypt_filter_methods() {
    let aesv3 = "<</StdCF <</CFM /AESV3 /AuthEvent /DocOpen /Length 32>>>>";
    let (encryption, _) = read(&r6(aesv3, "/StdCF", "/Identity"), "");
    let encryption = encryption.unwrap().unwrap();
    assert_eq!(encryption.stream_method(), CryptMethod::Aes256);
    assert_eq!(encryption.string_method(), CryptMethod::Identity);

    // Methods `V` 5 does not take, and `CFM` None, which hands decryption
    // to a handler of the application's own.
    for filter in ["/V2", "/AESV2", "/None", "/Bogus"] {
        let filters = format!("<</StdCF <</CFM {filter}>>>>");
        match read(&r6(&filters, "/StdCF", "/StdCF"), "").0 {
            Err(PdfError::UnsupportedCryptFilter { name }) => assert_eq!(name, "StdCF"),
            other => panic!("{filter}: {other:?}"),
        }
    }
    // A name `CF` does not define.
    assert!(matches!(
        read(&r6(aesv3, "/Other", "/StdCF"), "").0,
        Err(PdfError::UnsupportedCryptFilter { .. })
    ));
}

#[test]
fn revision_6_strings_too_short() {
    let short = format!(
        "<</Filter /Standard /V 5 /R 6 /O {} /U {} /OE {} /UE {} /P -4>>",
        hex_bytes(48),
        hex_bytes(32),
        hex_bytes(32),
        hex_bytes(32),
    );
    assert!(matches!(
        read_error(&short),
        PdfError::EncryptionDictionary { key: "U" }
    ));
    let no_ue = format!(
        "<</Filter /Standard /V 5 /R 6 /O {} /U {} /OE {} /P -4>>",
        hex_bytes(48),
        hex_bytes(48),
        hex_bytes(32),
    );
    assert!(matches!(
        read_error(&no_ue),
        PdfError::EncryptionDictionary { key: "UE" }
    ));
}

/// `V` 4 with no `Length` takes the key length of the filter `StmF`
/// names, which writers give in bytes or in bits.
#[test]
fn version_4_key_length_from_the_crypt_filter() {
    let dictionary = |filter_length: &str| {
        format!(
            "<</Filter /Standard /V 4 /R 4 /O {} /U {} /P -4 \
             /CF <</StdCF <</CFM /V2 {filter_length}>>>> /StmF /StdCF /StrF /StdCF>>",
            hex_32(),
            hex_32()
        )
    };
    for (length, bits) in [
        ("/Length 16", 128),
        ("/Length 128", 128),
        ("/Length 5", 40),
        ("", 128),
    ] {
        let (encryption, _) = read(&dictionary(length), ID);
        assert_eq!(encryption.unwrap().unwrap().key_bits(), bits, "{length}");
    }
}
