// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Expanding a desktop entry's `Exec` key into argument vectors.
//!
//! Specification: Desktop Entry Specification 1.5, "The Exec key".
//! - Arguments are separated by spaces; an argument containing reserved
//!   characters is enclosed in double quotes, inside which `\"`, `` \` ``,
//!   `\$` and `\\` stand for the character after the backslash. (The string
//!   escapes of "Possible value types" were already applied by
//!   [`crate::parse_desktop_entry`].)
//! - Field codes: `%f` one file, `%F` a list of files, `%u` one URL, `%U` a
//!   list of URLs, `%i` the `Icon` as two arguments `--icon <value>`, `%c`
//!   the translated `Name` (here the unlocalized one), `%k` the location of
//!   the desktop file, `%%` a literal percent sign. `%d %D %n %N %v %m` are
//!   deprecated and expand to nothing. Any other code makes the entry
//!   invalid.
//! - An argument that is only `%f`, `%F`, `%u` or `%U` and has no file to
//!   take is removed.
//!
//! A launch hands the application either local files or one URL
//! ([`Targets`]). File paths are `OsString` throughout: a path is arbitrary
//! bytes on Linux and must reach the program unchanged, so it is not forced
//! through UTF-8. A file given to `%u` or `%U` is `file://` followed by the
//! absolute path with every byte outside RFC 3986 unreserved characters and
//! `/` written as `%XX`; a URL is given as it is. The specification lets a
//! launcher download a URL for an application that takes only files; Kern
//! does not, and refuses such a launch.

use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::path::PathBuf;

use percent_encoding::{percent_encode, AsciiSet, NON_ALPHANUMERIC};

use crate::desktop_entry::DesktopEntry;
use crate::error::AppsError;

/// RFC 3986 unreserved characters and the path separator stay as they are
/// in a `file://` URL; every other byte is `%XX`.
const URI_KEEP: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~')
    .remove(b'/');

/// What a launch hands the application.
#[derive(Clone, Copy, Debug)]
pub enum Targets<'a> {
    /// Local files: `%f` and `%F` take their paths, `%u` and `%U` their
    /// `file://` URLs.
    Files(&'a [PathBuf]),
    /// One URL, for `%u` or `%U`, passed as it is.
    Url(&'a str),
}

/// One file or URL of a launch.
#[derive(Clone, Copy, Debug)]
enum Item<'a> {
    File(&'a PathBuf),
    Url(&'a str),
}

/// Field codes the specification lists as deprecated: removed, not an error.
const DEPRECATED: &[char] = &['d', 'D', 'n', 'N', 'v', 'm'];

/// One piece of an argument as written in `Exec`.
#[derive(Debug)]
enum Piece {
    Text(String),
    Code(char),
}

/// An argument as written: its pieces, and whether it was quoted (a quoted
/// empty argument is kept; an unquoted one made only of removed codes is not).
#[derive(Debug)]
struct Arg {
    pieces: Vec<Piece>,
    quoted: bool,
}

/// Expands `entry.exec` into one argument vector per launch for `targets`.
///
/// `%f` and `%u` start the program once per file; `%F` and `%U` start it
/// once with all files; with no file code and no files it starts once. With
/// no file code and files given the result is [`AppsError::NoFileArgument`]:
/// an application without a file code takes none, and files are never
/// appended. A URL needs `%u` or `%U`, else [`AppsError::NoUrlArgument`]. A
/// single-file code together with a list code is invalid, as is a list code
/// or `%i` inside a longer argument, and an `Exec` that leaves nothing to
/// run.
pub fn expand_exec(
    entry: &DesktopEntry,
    targets: Targets<'_>,
) -> Result<Vec<Vec<OsString>>, AppsError> {
    let invalid = |reason: &str| AppsError::InvalidExec {
        id: entry.id.clone(),
        reason: reason.to_string(),
    };
    let mut args = tokenize(&entry.exec).map_err(|r| invalid(&r))?;
    // Deprecated codes expand to nothing, so they go first; an argument they
    // leave empty (and that was not quoted) disappears with them.
    for arg in &mut args {
        arg.pieces
            .retain(|p| !matches!(p, Piece::Code(c) if DEPRECATED.contains(c)));
    }
    args.retain(|a| a.quoted || !a.pieces.is_empty());

    // The program is run as written; a live field code there would make a
    // file path or URL the program.
    match args.first() {
        Some(first) if first.pieces.iter().any(|p| matches!(p, Piece::Code(_))) => {
            return Err(invalid("a field code in the program position"));
        }
        _ => {}
    }

    let has = |codes: &[char]| {
        args.iter()
            .flat_map(|a| a.pieces.iter())
            .any(|p| matches!(p, Piece::Code(c) if codes.contains(c)))
    };
    let single = has(&['f', 'u']);
    let multi = has(&['F', 'U']);
    if single && multi {
        return Err(invalid("both a single-file and a file-list code"));
    }
    for arg in &args {
        let lone = arg.pieces.len() == 1 && !arg.quoted;
        for piece in &arg.pieces {
            if let Piece::Code(c @ ('F' | 'U' | 'i')) = piece {
                if !lone {
                    return Err(invalid(&format!("%{c} must be an argument of its own")));
                }
            }
        }
    }
    let items: Vec<Item<'_>> = match targets {
        Targets::Files(files) => {
            if !single && !multi && !files.is_empty() {
                return Err(AppsError::NoFileArgument {
                    id: entry.id.clone(),
                });
            }
            files.iter().map(Item::File).collect()
        }
        Targets::Url(url) => {
            if !has(&['u', 'U']) {
                return Err(AppsError::NoUrlArgument {
                    id: entry.id.clone(),
                });
            }
            vec![Item::Url(url)]
        }
    };

    let launches: Vec<&[Item<'_>]> = if single && !items.is_empty() {
        items.chunks(1).collect()
    } else {
        vec![&items]
    };
    let mut out = Vec::with_capacity(launches.len());
    for group in launches {
        let mut argv: Vec<OsString> = Vec::new();
        for arg in &args {
            expand_arg(entry, arg, group, &mut argv);
        }
        if argv.is_empty() {
            return Err(invalid("nothing to run"));
        }
        out.push(argv);
    }
    Ok(out)
}

/// Splits `Exec` into arguments, applying the quoting rules.
fn tokenize(exec: &str) -> Result<Vec<Arg>, String> {
    let mut args: Vec<Arg> = Vec::new();
    let mut pieces: Vec<Piece> = Vec::new();
    let mut text = String::new();
    let mut quoted = false;
    let mut in_quotes = false;
    let mut started = false;

    fn flush(text: &mut String, pieces: &mut Vec<Piece>) {
        if !text.is_empty() {
            pieces.push(Piece::Text(std::mem::take(text)));
        }
    }

    let mut chars = exec.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                in_quotes = !in_quotes;
                quoted = true;
                started = true;
            }
            '\\' if in_quotes => match chars.peek() {
                Some(&n @ ('"' | '`' | '$' | '\\')) => {
                    text.push(n);
                    chars.next();
                }
                _ => text.push('\\'),
            },
            '%' => {
                started = true;
                match chars.next() {
                    Some('%') => text.push('%'),
                    Some(code) if "fFuUick".contains(code) || DEPRECATED.contains(&code) => {
                        flush(&mut text, &mut pieces);
                        pieces.push(Piece::Code(code));
                    }
                    Some(code) => return Err(format!("unknown field code %{code}")),
                    None => return Err("field code cut off at the end".to_string()),
                }
            }
            ' ' | '\t' | '\n' if !in_quotes => {
                if started {
                    flush(&mut text, &mut pieces);
                    args.push(Arg {
                        pieces: std::mem::take(&mut pieces),
                        quoted,
                    });
                    quoted = false;
                    started = false;
                }
            }
            other => {
                text.push(other);
                started = true;
            }
        }
    }
    if in_quotes {
        return Err("unterminated double quote".to_string());
    }
    if started {
        flush(&mut text, &mut pieces);
        args.push(Arg { pieces, quoted });
    }
    Ok(args)
}

/// Appends the expansion of one argument for one launch to `argv`.
fn expand_arg(entry: &DesktopEntry, arg: &Arg, items: &[Item<'_>], argv: &mut Vec<OsString>) {
    // The list and icon codes were checked to stand alone.
    if let [Piece::Code(code)] = arg.pieces.as_slice() {
        if !arg.quoted {
            match code {
                'F' => {
                    argv.extend(items.iter().filter_map(path_of));
                    return;
                }
                'U' => {
                    argv.extend(items.iter().map(|i| OsString::from_vec(url_of(i))));
                    return;
                }
                'i' => {
                    if let Some(icon) = &entry.icon {
                        argv.push(OsString::from("--icon"));
                        argv.push(OsString::from(icon));
                    }
                    return;
                }
                _ => {}
            }
        }
    }
    let mut bytes: Vec<u8> = Vec::new();
    let mut produced = arg.quoted;
    for piece in &arg.pieces {
        match piece {
            Piece::Text(t) => {
                bytes.extend_from_slice(t.as_bytes());
                produced = true;
            }
            Piece::Code(code) => match code {
                'f' => {
                    if let Some(path) = items.first().and_then(path_of) {
                        bytes.extend_from_slice(path.as_bytes());
                        produced = true;
                    }
                }
                'u' => {
                    if let Some(item) = items.first() {
                        bytes.extend(url_of(item));
                        produced = true;
                    }
                }
                'c' => {
                    bytes.extend_from_slice(entry.name.as_bytes());
                    produced = true;
                }
                'k' => {
                    bytes.extend_from_slice(entry.source.as_os_str().as_bytes());
                    produced = true;
                }
                // Removed codes, and list codes that stood alone but are
                // quoted, expand to nothing.
                _ => {}
            },
        }
    }
    if produced {
        argv.push(OsString::from_vec(bytes));
    }
}

/// The path a file code takes. A URL never reaches a file code:
/// [`expand_exec`] refuses a URL launch without `%u` or `%U`, and a single
/// and a list code together.
fn path_of(item: &Item<'_>) -> Option<OsString> {
    match item {
        Item::File(path) => Some(path.as_os_str().to_owned()),
        Item::Url(_) => None,
    }
}

/// What a URL code takes: a URL as given, a file as its `file://` URL.
fn url_of(item: &Item<'_>) -> Vec<u8> {
    match item {
        Item::File(path) => file_url(path),
        Item::Url(url) => url.as_bytes().to_vec(),
    }
}

/// `file://` and the absolute form of `path`, percent-encoded byte by byte.
fn file_url(path: &std::path::Path) -> Vec<u8> {
    let absolute = std::path::absolute(path).unwrap_or_else(|_| path.to_path_buf());
    let mut url = b"file://".to_vec();
    url.extend(
        percent_encode(absolute.as_os_str().as_bytes(), URI_KEEP)
            .to_string()
            .into_bytes(),
    );
    url
}
