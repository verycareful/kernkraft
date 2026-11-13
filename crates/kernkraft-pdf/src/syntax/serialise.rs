// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Objects as bytes, in one canonical form: ISO 32000-1:2008, 7.3.2 to
//! 7.3.7, 7.3.9 and 7.3.10.
//!
//! The same value always gives the same bytes, and those bytes read back as
//! the same value. Nothing here looks at an object's span: copying the
//! original bytes of an object that was not changed is for a writer that
//! holds the source.

use std::io::Write;

use crate::error::Limit;
use crate::syntax::chars::is_regular;
use crate::syntax::limits::MAX_NESTING_DEPTH;
use crate::syntax::{Dictionary, Name, Object, ObjectId, PdfString, Real, Value};
use crate::PdfError;

/// The nesting depth of an object that no array or dictionary contains.
const TOP_LEVEL_DEPTH: usize = 1;

/// The first and last of the printable ASCII characters, the range 7.3.5
/// recommends writing a name's regular characters from as themselves.
const NAME_PLAIN_FIRST: u8 = b'!';
const NAME_PLAIN_LAST: u8 = b'~';

const HEX_DIGITS: &[u8; 16] = b"0123456789ABCDEF";

/// Appends `object` to `out`.
///
/// Fails only when arrays and dictionaries nest deeper than
/// [`MAX_NESTING_DEPTH`]: [`PdfError::LimitExceeded`], whose offset is the
/// length of `out` when the limit was passed. What was written up to then
/// stays in `out`.
pub fn write_object(object: &Object, out: &mut Vec<u8>) -> Result<(), PdfError> {
    write_value(object.value(), out, TOP_LEVEL_DEPTH)
}

/// Appends the definition of an indirect object (7.3.10): `N G obj`, the
/// object and `endobj`, each on its own line. Fails as [`write_object`]
/// does.
pub fn write_indirect_object(
    id: ObjectId,
    object: &Object,
    out: &mut Vec<u8>,
) -> Result<(), PdfError> {
    append(out, format_args!("{} {} obj\n", id.number, id.generation));
    write_object(object, out)?;
    out.extend_from_slice(b"\nendobj\n");
    Ok(())
}

fn write_value(value: &Value, out: &mut Vec<u8>, depth: usize) -> Result<(), PdfError> {
    match value {
        Value::Null => out.extend_from_slice(b"null"),
        Value::Boolean(true) => out.extend_from_slice(b"true"),
        Value::Boolean(false) => out.extend_from_slice(b"false"),
        Value::Integer(integer) => append(out, format_args!("{integer}")),
        Value::Real(real) => write_real(*real, out),
        Value::String(string) => write_string(string, out),
        Value::Name(name) => write_name(name, out),
        Value::Reference(id) => append(out, format_args!("{} {} R", id.number, id.generation)),
        Value::Array(items) => {
            check_depth(depth, out)?;
            out.push(b'[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(b' ');
                }
                write_value(item.value(), out, depth + 1)?;
            }
            out.push(b']');
        }
        Value::Dictionary(dictionary) => write_dictionary(dictionary, out, depth)?,
    }
    Ok(())
}

fn write_dictionary(
    dictionary: &Dictionary,
    out: &mut Vec<u8>,
    depth: usize,
) -> Result<(), PdfError> {
    check_depth(depth, out)?;
    out.extend_from_slice(b"<<");
    for (key, value) in dictionary.iter() {
        out.push(b' ');
        write_name(key, out);
        out.push(b' ');
        write_value(value.value(), out, depth + 1)?;
    }
    out.extend_from_slice(b" >>");
    Ok(())
}

fn check_depth(depth: usize, out: &[u8]) -> Result<(), PdfError> {
    if depth > MAX_NESTING_DEPTH {
        return Err(PdfError::LimitExceeded {
            // A slice is at most isize::MAX bytes, which fits in a u64.
            offset: out.len() as u64,
            limit: Limit::NestingDepth,
        });
    }
    Ok(())
}

/// Formatted text appended to `out`.
fn append(out: &mut Vec<u8>, text: std::fmt::Arguments<'_>) {
    // Writing to a Vec never fails: it only extends the vector.
    let _ = out.write_fmt(text);
}

/// 7.3.3: decimal digits with a period and no exponent. Rust's `Display`
/// for `f64` is exactly that, with the fewest digits that read back as the
/// same value. It leaves the period out of a whole number, which would read
/// back as an integer, so one is added.
fn write_real(real: Real, out: &mut Vec<u8>) {
    let start = out.len();
    append(out, format_args!("{}", real.get()));
    if !out[start..].contains(&b'.') {
        out.extend_from_slice(b".0");
    }
}

/// 7.3.4.2: a literal string. A backslash and both parentheses are escaped,
/// so that the parentheses need not balance. A carriage return is written
/// as `\r`, because a reader takes a bare one, or one with a line feed, for
/// a single line feed. Every other byte stands for itself.
fn write_string(string: &PdfString, out: &mut Vec<u8>) {
    out.push(b'(');
    for &byte in string.as_bytes() {
        match byte {
            b'\\' | b'(' | b')' => out.extend_from_slice(&[b'\\', byte]),
            b'\r' => out.extend_from_slice(b"\\r"),
            other => out.push(other),
        }
    }
    out.push(b')');
}

/// 7.3.5: a solidus, then each byte as itself when it is a printable
/// regular character other than the number sign, and as a number sign and
/// two hexadecimal digits otherwise.
fn write_name(name: &Name, out: &mut Vec<u8>) {
    out.push(b'/');
    for &byte in name.as_bytes() {
        let plain = (NAME_PLAIN_FIRST..=NAME_PLAIN_LAST).contains(&byte)
            && is_regular(byte)
            && byte != b'#';
        if plain {
            out.push(byte);
        } else {
            out.extend_from_slice(&[
                b'#',
                HEX_DIGITS[usize::from(byte >> 4)],
                HEX_DIGITS[usize::from(byte & 0x0F)],
            ]);
        }
    }
}
