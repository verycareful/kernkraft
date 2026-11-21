// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Every fixture opened with each of its passwords, and read through.

use super::*;
use kernkraft_pdf::crypt::{Access, CryptMethod, Revision};
use kernkraft_pdf::file::XrefEntry;
use kernkraft_pdf::DiagnosticKind;

/// A fixture and the encryption its qpdf options give it.
struct Case {
    name: &'static str,
    user: &'static str,
    version: u8,
    revision: Revision,
    key_bits: usize,
    method: CryptMethod,
    encrypt_metadata: bool,
}

const fn case(
    name: &'static str,
    user: &'static str,
    version: u8,
    revision: Revision,
    key_bits: usize,
    method: CryptMethod,
    encrypt_metadata: bool,
) -> Case {
    Case {
        name,
        user,
        version,
        revision,
        key_bits,
        method,
        encrypt_metadata,
    }
}

use CryptMethod::{Aes128, Aes256, Rc4};
use Revision::{R2, R3, R4, R5, R6};

/// qpdf writes 40 bits as `V` 1 and revision 2, 128-bit RC4 as `V` 2 and
/// revision 3, `--force-V4` and AES-128 as `V` 4 with crypt filters, and
/// 256 bits as `V` 5 with revision 6, or 5 with `--force-R5`.
const CASES: [Case; 18] = [
    case("rc4-40", USER, 1, R2, 40, Rc4, true),
    case("rc4-40-restricted", USER, 1, R2, 40, Rc4, true),
    case("rc4-128", USER, 2, R3, 128, Rc4, true),
    case("rc4-128-restricted", USER, 2, R3, 128, Rc4, true),
    case("rc4-128-accented", ACCENTED, 2, R3, 128, Rc4, true),
    case("rc4-128-utf8-bytes", ACCENTED, 2, R3, 128, Rc4, true),
    case("rc4-128-object-streams", USER, 2, R3, 128, Rc4, true),
    case("rc4-128-v4", USER, 4, R4, 128, Rc4, true),
    case("aes-128", USER, 4, R4, 128, Aes128, true),
    case(
        "aes-128-cleartext-metadata",
        USER,
        4,
        R4,
        128,
        Aes128,
        false,
    ),
    case("aes-128-object-streams", USER, 4, R4, 128, Aes128, true),
    case("aes-256-r5", USER, 5, R5, 256, Aes256, true),
    case("aes-256", USER, 5, R6, 256, Aes256, true),
    case("aes-256-restricted", USER, 5, R6, 256, Aes256, true),
    case(
        "aes-256-cleartext-metadata",
        USER,
        5,
        R6,
        256,
        Aes256,
        false,
    ),
    case("aes-256-object-streams", USER, 5, R6, 256, Aes256, true),
    case("aes-256-no-user-password", "", 5, R6, 256, Aes256, true),
    case("aes-256-accented", ACCENTED, 5, R6, 256, Aes256, true),
];

#[test]
fn the_plain_file_is_not_encrypted() {
    let mut diagnostics = Diagnostics::new();
    let file = open_bytes(fixture("plain"), &mut diagnostics).unwrap();
    assert!(Encryption::read(&file, &mut diagnostics).unwrap().is_none());
}

#[test]
fn encryption_dictionaries_read_as_written() {
    for case in &CASES {
        let mut diagnostics = Diagnostics::new();
        let file = open_bytes(fixture(case.name), &mut diagnostics).unwrap();
        let encryption = Encryption::read(&file, &mut diagnostics).unwrap().unwrap();
        let read = (
            encryption.version(),
            encryption.revision(),
            encryption.key_bits(),
            encryption.string_method(),
            encryption.stream_method(),
            encryption.encrypt_metadata(),
        );
        let expected = (
            case.version,
            case.revision,
            case.key_bits,
            case.method,
            case.method,
            case.encrypt_metadata,
        );
        assert_eq!(read, expected, "{}", case.name);
        assert!(diagnostics.is_empty(), "{}: {diagnostics:?}", case.name);
    }
}

#[test]
fn the_user_password_opens_every_fixture() {
    for case in &CASES {
        let mut opened = unlock(case.name, case.user).unwrap();
        assert_eq!(opened.decryptor.access(), Access::User, "{}", case.name);
        assert_eq!(opened.facts(), plain_facts(), "{}", case.name);
    }
}

#[test]
fn the_owner_password_opens_every_fixture() {
    for case in &CASES {
        let mut opened = unlock(case.name, OWNER).unwrap();
        assert_eq!(opened.decryptor.access(), Access::Owner, "{}", case.name);
        assert_eq!(opened.facts(), plain_facts(), "{}", case.name);
    }
}

/// Only a fixture written with the password's UTF-8 bytes, where its
/// revision defines PDFDocEncoding, raises `PasswordEncoding`; the reads
/// raise nothing else.
#[test]
fn diagnostics_of_a_clean_read() {
    for case in &CASES {
        let mut opened = unlock(case.name, case.user).unwrap();
        opened.facts();
        let kinds: Vec<_> = opened
            .diagnostics
            .as_slice()
            .iter()
            .map(|d| d.kind.clone())
            .collect();
        let expected = if case.name == "rc4-128-utf8-bytes" {
            vec![DiagnosticKind::PasswordEncoding]
        } else {
            Vec::new()
        };
        assert_eq!(kinds, expected, "{}", case.name);
    }
}

#[test]
fn other_passwords_are_refused() {
    for case in &CASES {
        let mut wrong = vec!["wrong", "User", "owner ", "use"];
        if !case.user.is_empty() {
            wrong.push("");
        }
        for password in wrong {
            assert!(
                matches!(unlock(case.name, password), Err(PdfError::WrongPassword)),
                "{} with {password:?}",
                case.name
            );
        }
    }
}

/// The accented password in PDFDocEncoding is one byte for each of its
/// eight characters; typed as UTF-8 it is twelve. A revision 3 file
/// encrypted with either opens with the typed password.
#[test]
fn accented_passwords() {
    for name in ["rc4-128-accented", "rc4-128-utf8-bytes", "aes-256-accented"] {
        let opened = unlock(name, ACCENTED).unwrap();
        assert_eq!(opened.decryptor.access(), Access::User, "{name}");
        // Without the accents the password is another one.
        assert!(matches!(
            unlock(name, "passwrd\u{20ac}"),
            Err(PdfError::WrongPassword)
        ));
    }
}

/// The permissions the restricted fixtures were written with: qpdf's
/// `--print=n --annotate=n` for 40 bits (bits 3 and 6 clear); `--print=low
/// --modify=form --extract=n` for 128 bits (bits 4, 5, 6 and 12 clear);
/// `--print=none --modify=none --extract=n` for 256 bits (bits 3, 4, 5, 6,
/// 9, 11 and 12 clear). Every other fixture allows everything.
#[test]
fn permissions_as_written() {
    // print, print faithful, modify, copy, annotate, fill forms,
    // accessibility, assemble
    let expected: [(&str, [bool; 8]); 4] = [
        ("rc4-40", [true; 8]),
        (
            "rc4-40-restricted",
            [false, false, true, true, false, false, true, true],
        ),
        (
            "rc4-128-restricted",
            [true, false, false, false, false, true, true, true],
        ),
        (
            "aes-256-restricted",
            [false, false, false, false, false, false, true, false],
        ),
    ];
    for (name, expected) in expected {
        // The owner gets the same report: the core enforces nothing.
        for password in [USER, OWNER] {
            let opened = unlock(name, password).unwrap();
            let p = opened.decryptor.permissions();
            let read = [
                p.print(),
                p.print_faithful(),
                p.modify(),
                p.copy(),
                p.annotate(),
                p.fill_forms(),
                p.extract_for_accessibility(),
                p.assemble(),
            ];
            assert_eq!(read, expected, "{name} with {password}");
        }
    }
}

/// With `EncryptMetadata` false the metadata stream is stored as it is.
#[test]
fn cleartext_metadata_is_stored_plain() {
    for name in ["aes-128-cleartext-metadata", "aes-256-cleartext-metadata"] {
        let bytes = fixture(name);
        assert!(
            bytes.windows(METADATA.len()).any(|w| w == METADATA),
            "{name}"
        );
    }
    for name in ["aes-128", "aes-256"] {
        let bytes = fixture(name);
        assert!(
            !bytes.windows(METADATA.len()).any(|w| w == METADATA),
            "{name}"
        );
    }
}

/// Objects inside an encrypted object stream cannot be read until the
/// file has the decryptor.
#[test]
fn object_streams_need_the_decryptor() {
    for name in [
        "rc4-128-object-streams",
        "aes-128-object-streams",
        "aes-256-object-streams",
    ] {
        let mut diagnostics = Diagnostics::new();
        let file = open_bytes(fixture(name), &mut diagnostics).unwrap();
        let compressed: Vec<u32> = file.sections()[0]
            .entries()
            .filter_map(|(number, entry)| {
                matches!(entry, XrefEntry::Compressed { .. }).then_some(number)
            })
            .collect();
        assert!(!compressed.is_empty(), "{name}");
        let id = ObjectId {
            number: compressed[0],
            generation: 0,
        };
        assert!(
            matches!(
                file.load(id, &mut Diagnostics::new()),
                Err(PdfError::ObjectStream { .. })
            ),
            "{name}"
        );

        let mut opened = unlock(name, USER).unwrap();
        for number in compressed {
            let id = ObjectId {
                number,
                generation: 0,
            };
            opened
                .file
                .load(id, &mut opened.diagnostics)
                .unwrap()
                .unwrap();
        }
    }
}

/// A broken cross-reference is rebuilt; the objects of an encrypted object
/// stream are found only once the decryptor is given, which takes back the
/// diagnostics that said they were missing.
#[test]
fn a_rebuilt_file_finds_its_compressed_objects_with_the_decryptor() {
    for name in ["rc4-128-object-streams", "aes-256-object-streams"] {
        let mut bytes = fixture(name);
        let at = bytes
            .windows(b"startxref".len())
            .rposition(|w| w == b"startxref")
            .unwrap();
        bytes.truncate(at);

        let mut diagnostics = Diagnostics::new();
        let file = open_bytes(bytes, &mut diagnostics).unwrap();
        assert!(file.xref_rebuilt());
        let unreadable = |diagnostics: &Diagnostics| {
            diagnostics
                .as_slice()
                .iter()
                .filter(|d| matches!(d.kind, DiagnosticKind::ObjectStreamUnreadable { .. }))
                .count()
        };
        assert_eq!(unreadable(&diagnostics), 1, "{name}");

        let encryption = Encryption::read(&file, &mut diagnostics).unwrap().unwrap();
        let decryptor = Arc::new(encryption.authenticate(USER, &mut diagnostics).unwrap());
        let file = decrypt_file(file, decryptor.clone(), &mut diagnostics).unwrap();
        assert!(file.xref_rebuilt());
        assert_eq!(unreadable(&diagnostics), 0, "{name}");
        let mut opened = Opened {
            file,
            decryptor,
            diagnostics,
        };
        assert_eq!(opened.facts(), plain_facts(), "{name}");
    }
}

/// `Perms` that does not decrypt to what the dictionary states is
/// reported, and the stated values are used.
#[test]
fn tampered_perms_are_reported() {
    for name in ["aes-256", "aes-256-r5"] {
        let mut bytes = fixture(name);
        let at = bytes
            .windows(b"/Perms <".len())
            .position(|w| w == b"/Perms <")
            .unwrap()
            + b"/Perms <".len();
        // One hexadecimal digit for another: the offsets stay right.
        bytes[at] = if bytes[at] == b'0' { b'1' } else { b'0' };
        let mut opened = unlock_bytes(bytes, USER).unwrap();
        assert!(
            opened
                .diagnostics
                .as_slice()
                .iter()
                .any(|d| d.kind == DiagnosticKind::PermissionsUnverified),
            "{name}"
        );
        assert_eq!(opened.facts(), plain_facts(), "{name}");
    }
}
