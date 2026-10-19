// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! MIME type lookup from the Shared MIME-info Database.
//!
//! Specification: Shared MIME-info Database 0.21.
//! - Section 2.4 (`globs2`, `subclasses`, `aliases` file formats): a `globs2`
//!   line is `weight:type:pattern[:flags]` where the flags are
//!   comma-separated and `cs` marks a case-sensitive pattern; `subclasses`
//!   and `aliases` lines are two whitespace-separated types.
//! - Section 2.11 (recommended checking order): a name equal to a literal
//!   pattern wins, then the glob with the highest weight, then the longest
//!   pattern; patterns match case-insensitively unless flagged `cs`.
//! - Section 2.5 (subclassing): every `text/*` type is a subclass of
//!   `text/plain`.
//!
//! Content sniffing is limited to text detection; the `magic` file is not
//! read.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::AppsError;

/// How many leading bytes [`MimeDb::guess`] reads to decide between text and
/// binary. 8192 is the window shared-mime-info's own text check uses.
pub const SNIFF_LEN: usize = 8192;

const TEXT_PLAIN: &str = "text/plain";
const OCTET_STREAM: &str = "application/octet-stream";
/// Marker lines in `globs2` that carry no pattern (section 2.4).
const NO_EXTENSIONS: &str = "__NOEXTENSIONS__";

/// A glob or literal from a `globs2` line.
#[derive(Debug)]
struct Pattern {
    mime: String,
    weight: u32,
    /// The pattern as chars, lowercased unless `case_sensitive`.
    chars: Vec<char>,
    case_sensitive: bool,
    literal: bool,
    /// Position of the data directory in the load order; lower wins ties.
    dir_rank: usize,
}

/// The MIME database merged from every data directory given to
/// [`MimeDb::load`].
#[derive(Debug, Default)]
pub struct MimeDb {
    patterns: Vec<Pattern>,
    /// alias -> canonical type.
    aliases: HashMap<String, String>,
    /// type -> direct parents.
    parents: HashMap<String, Vec<String>>,
}

impl MimeDb {
    /// Load `<dir>/mime/{globs2,subclasses,aliases}` for each directory in
    /// `data_dirs`, most important first (the user data directory, then the
    /// system ones).
    ///
    /// A directory without `mime/`, or a `mime/` without one of the three
    /// files, contributes nothing and is not an error. A line that does not
    /// have the documented shape fails the whole load with
    /// [`AppsError::MalformedMimeDb`]; nothing is skipped silently.
    ///
    /// Precedence between directories: patterns from all directories are
    /// pooled and ranked by section 2.11 (weight, then length), and a tie
    /// goes to the earlier directory. For an alias defined twice the earlier
    /// directory wins.
    pub fn load(data_dirs: &[PathBuf]) -> Result<MimeDb, AppsError> {
        let mut db = MimeDb::default();
        for (rank, dir) in data_dirs.iter().enumerate() {
            let mime_dir = dir.join("mime");
            if let Some((path, text)) = read_optional(&mime_dir.join("globs2"))? {
                db.add_globs(&path, &text, rank)?;
            }
            if let Some((path, text)) = read_optional(&mime_dir.join("subclasses"))? {
                db.add_subclasses(&path, &text)?;
            }
            if let Some((path, text)) = read_optional(&mime_dir.join("aliases"))? {
                db.add_aliases(&path, &text)?;
            }
        }
        Ok(db)
    }

    fn add_globs(&mut self, path: &Path, text: &str, dir_rank: usize) -> Result<(), AppsError> {
        for (index, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') || line.contains(NO_EXTENSIONS) {
                continue;
            }
            let bad = || AppsError::MalformedMimeDb {
                path: path.to_path_buf(),
                line: index + 1,
            };
            let mut fields = line.splitn(4, ':');
            let weight = fields
                .next()
                .and_then(|w| w.parse::<u32>().ok())
                .ok_or_else(bad)?;
            let mime = fields.next().filter(|m| m.contains('/')).ok_or_else(bad)?;
            let pattern = fields.next().filter(|p| !p.is_empty()).ok_or_else(bad)?;
            let case_sensitive = fields
                .next()
                .is_some_and(|flags| flags.split(',').any(|f| f == "cs"));
            let chars: Vec<char> = if case_sensitive {
                pattern.chars().collect()
            } else {
                pattern.to_lowercase().chars().collect()
            };
            self.patterns.push(Pattern {
                mime: mime.to_string(),
                weight,
                literal: !chars.iter().any(|c| matches!(c, '*' | '?' | '[')),
                chars,
                case_sensitive,
                dir_rank,
            });
        }
        Ok(())
    }

    fn add_subclasses(&mut self, path: &Path, text: &str) -> Result<(), AppsError> {
        for (child, parent) in type_pairs(path, text)? {
            let list = self.parents.entry(child).or_default();
            if !list.contains(&parent) {
                list.push(parent);
            }
        }
        Ok(())
    }

    fn add_aliases(&mut self, path: &Path, text: &str) -> Result<(), AppsError> {
        for (alias, canonical) in type_pairs(path, text)? {
            self.aliases.entry(alias).or_insert(canonical);
        }
        Ok(())
    }

    /// The type the file name alone indicates, or `None` when no pattern
    /// matches. A name equal to a literal pattern beats any glob; among
    /// globs the highest weight wins, then the longest pattern, then the
    /// earlier data directory.
    pub fn by_name(&self, file_name: &str) -> Option<&str> {
        let name: Vec<char> = file_name.chars().collect();
        let lower: Vec<char> = file_name.to_lowercase().chars().collect();
        let pick = |literal: bool| {
            self.patterns
                .iter()
                .filter(|p| p.literal == literal)
                .filter(|p| {
                    let subject = if p.case_sensitive { &name } else { &lower };
                    if literal {
                        *subject == p.chars
                    } else {
                        glob_match(&p.chars, subject)
                    }
                })
                .max_by(|a, b| {
                    a.weight
                        .cmp(&b.weight)
                        .then(a.chars.len().cmp(&b.chars.len()))
                        .then(b.dir_rank.cmp(&a.dir_rank))
                })
                .map(|p| p.mime.as_str())
        };
        pick(true).or_else(|| pick(false))
    }

    /// The type of the file at `path`.
    ///
    /// A symlink is guessed by its target (a dangling or looping link, whose
    /// target cannot be resolved, by its own name). A directory is
    /// `inode/directory`; a FIFO, socket or device node is the matching
    /// `inode/*` type and is never opened. A regular file is typed by its
    /// name first; when no pattern matches, its first [`SNIFF_LEN`] bytes
    /// decide: `text/plain` when they are valid UTF-8 without a NUL byte (a
    /// multi-byte sequence cut off by the end of the window is allowed, and
    /// an empty file counts as text), else `application/octet-stream`.
    ///
    /// The return type carries no error. A file that cannot be inspected
    /// (permission denied, removed meanwhile) is `application/octet-stream`;
    /// the caller learns the real reason when it opens the file.
    pub fn guess(&self, path: &Path) -> String {
        let resolved = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        let Ok(meta) = fs::metadata(&resolved) else {
            return self.name_or(&resolved, OCTET_STREAM);
        };
        let kind = meta.file_type();
        if kind.is_dir() {
            return "inode/directory".to_string();
        }
        {
            use std::os::unix::fs::FileTypeExt;
            if kind.is_fifo() {
                return "inode/fifo".to_string();
            }
            if kind.is_socket() {
                return "inode/socket".to_string();
            }
            if kind.is_char_device() {
                return "inode/chardevice".to_string();
            }
            if kind.is_block_device() {
                return "inode/blockdevice".to_string();
            }
        }
        if let Some(mime) = resolved
            .file_name()
            .and_then(|n| self.by_name(&n.to_string_lossy()))
        {
            return mime.to_string();
        }
        match sniff_is_text(&resolved) {
            Some(true) => TEXT_PLAIN.to_string(),
            _ => OCTET_STREAM.to_string(),
        }
    }

    fn name_or(&self, path: &Path, fallback: &str) -> String {
        path.file_name()
            .and_then(|n| self.by_name(&n.to_string_lossy()))
            .unwrap_or(fallback)
            .to_string()
    }

    /// Whether `mime` is `parent` or a subclass of it, through `subclasses`
    /// transitively, after resolving aliases on both sides. Every `text/*`
    /// type is a subclass of `text/plain` (section 2.5). Cycles in a
    /// malformed database terminate.
    pub fn is_a(&self, mime: &str, parent: &str) -> bool {
        let start = self.canonical(mime);
        let goal = self.canonical(parent);
        let mut seen: HashSet<&str> = HashSet::new();
        let mut stack = vec![start];
        while let Some(current) = stack.pop() {
            if current == goal || (goal == TEXT_PLAIN && current.starts_with("text/")) {
                return true;
            }
            if !seen.insert(current) {
                continue;
            }
            if let Some(direct) = self.parents.get(current) {
                stack.extend(direct.iter().map(|p| self.canonical(p)));
            }
        }
        false
    }

    /// Whether files of this type are text: `mime` starts with `text/`, or it
    /// is a subclass of `text/plain` (see [`MimeDb::is_a`]).
    pub fn is_text(&self, mime: &str) -> bool {
        mime.starts_with("text/") || self.is_a(mime, TEXT_PLAIN)
    }

    fn canonical<'a>(&'a self, mime: &'a str) -> &'a str {
        self.aliases.get(mime).map_or(mime, String::as_str)
    }
}

/// Read a file that may be absent.
fn read_optional(path: &Path) -> Result<Option<(PathBuf, String)>, AppsError> {
    match fs::read_to_string(path) {
        Ok(text) => Ok(Some((path.to_path_buf(), text))),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(AppsError::io(path, e)),
    }
}

/// The `(first, second)` type pairs of a `subclasses` or `aliases` file.
fn type_pairs(path: &Path, text: &str) -> Result<Vec<(String, String)>, AppsError> {
    let mut out = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let mut fields = line.split_whitespace();
        match (fields.next(), fields.next(), fields.next()) {
            (Some(a), Some(b), None) if a.contains('/') && b.contains('/') => {
                out.push((a.to_string(), b.to_string()));
            }
            _ => {
                return Err(AppsError::MalformedMimeDb {
                    path: path.to_path_buf(),
                    line: index + 1,
                })
            }
        }
    }
    Ok(out)
}

/// Whether the first [`SNIFF_LEN`] bytes of the file are text; `None` when
/// the file cannot be read.
fn sniff_is_text(path: &Path) -> Option<bool> {
    let file = fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(SNIFF_LEN);
    file.take(SNIFF_LEN as u64).read_to_end(&mut buf).ok()?;
    if buf.contains(&0) {
        return Some(false);
    }
    Some(match std::str::from_utf8(&buf) {
        Ok(_) => true,
        // `error_len() == None` is a sequence the window cut off. That is
        // only benign when the window is full; a shorter file really ends
        // mid-sequence.
        Err(e) => e.error_len().is_none() && buf.len() == SNIFF_LEN,
    })
}

/// fnmatch-style match of `pattern` against `text`: `*` any run, `?` one
/// char, `[...]` a set with ranges and `!` or `^` negation.
fn glob_match(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    // Where to resume after the last `*`: (pattern index after it, text index).
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        let advanced = match pattern.get(p) {
            Some('*') => {
                star = Some((p + 1, t));
                p += 1;
                continue;
            }
            Some('?') => Some(p + 1),
            Some('[') => match match_set(pattern, p, text[t]) {
                Some((true, next)) => Some(next),
                _ => None,
            },
            Some(&c) if c == text[t] => Some(p + 1),
            _ => None,
        };
        match advanced {
            Some(next) => {
                p = next;
                t += 1;
            }
            None => match star {
                Some((sp, st)) => {
                    p = sp;
                    t = st + 1;
                    star = Some((sp, st + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

/// Match `c` against the set starting at `pattern[open]` (a `[`). Returns
/// whether it matched and the index after the closing `]`, or `None` when the
/// set is not closed (then `[` is an ordinary char that matches nothing here).
fn match_set(pattern: &[char], open: usize, c: char) -> Option<(bool, usize)> {
    let mut i = open + 1;
    let negate = matches!(pattern.get(i), Some('!' | '^'));
    if negate {
        i += 1;
    }
    let first = i;
    let mut matched = false;
    loop {
        let lo = *pattern.get(i)?;
        if lo == ']' && i > first {
            return Some((matched != negate, i + 1));
        }
        if pattern.get(i + 1) == Some(&'-') && pattern.get(i + 2).is_some_and(|&h| h != ']') {
            let hi = pattern[i + 2];
            matched |= lo <= c && c <= hi;
            i += 3;
        } else {
            matched |= lo == c;
            i += 1;
        }
    }
}
