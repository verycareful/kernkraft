// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Damaged encrypted files give a typed error or a result, never a panic.

use super::*;

/// Reads `bytes` as far as it goes with `password`. Every step may fail;
/// none may panic.
fn read_through(bytes: Vec<u8>, password: &str) {
    let mut diagnostics = Diagnostics::new();
    let Ok(file) = open_bytes(bytes, &mut diagnostics) else {
        return;
    };
    let Ok(Some(encryption)) = Encryption::read(&file, &mut diagnostics) else {
        return;
    };
    let Ok(decryptor) = encryption.authenticate(password, &mut diagnostics) else {
        return;
    };
    let decryptor = Arc::new(decryptor);
    let Ok(file) = decrypt_file(file, decryptor.clone(), &mut diagnostics) else {
        return;
    };
    let numbers: Vec<u32> = file.sections()[0].entries().map(|(n, _)| n).collect();
    for number in numbers {
        for generation in [0, 1] {
            let id = ObjectId { number, generation };
            let Ok(Some(mut object)) = file.load(id, &mut diagnostics) else {
                continue;
            };
            let _ = decryptor.decrypt_object(&mut object, &mut diagnostics);
            if let Body::Stream(stream) = &object.body {
                if let Ok(data) = file.stream_data(stream) {
                    let _ = decryptor.decrypt_stream(
                        id,
                        &stream.dictionary,
                        &data,
                        stream.data.start,
                        &mut diagnostics,
                    );
                }
            }
        }
    }
}

/// A deterministic mutation of `bytes`: the byte at `at` flipped.
fn flipped(bytes: &[u8], at: usize) -> Vec<u8> {
    let mut out = bytes.to_vec();
    out[at] ^= 0xFF;
    out
}

/// Every seventh byte of the RC4 and AES-128 fixtures flipped, and every
/// file cut short at every 61st byte. Revision 6 takes many hash rounds
/// for each password checked, so its fixtures are mutated more sparsely.
#[test]
fn flipped_and_truncated_fixtures() {
    let fixtures = [
        ("rc4-40", 7),
        ("rc4-128-v4", 7),
        ("aes-128", 7),
        ("aes-128-object-streams", 7),
        ("aes-256", 101),
        ("aes-256-object-streams", 101),
    ];
    for (name, step) in fixtures {
        let bytes = fixture(name);
        for at in (0..bytes.len()).step_by(step) {
            read_through(flipped(&bytes, at), USER);
        }
        for length in (0..bytes.len()).step_by(61) {
            read_through(bytes[..length].to_vec(), USER);
        }
    }
}
