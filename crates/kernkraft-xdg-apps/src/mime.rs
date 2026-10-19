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

use std::borrow::Cow;
use std::cell::OnceCell;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::BuildHasherDefault;
use std::io::Read;
use std::path::{Path, PathBuf};

use crate::error::AppsError;

/// How many leading bytes [`MimeDb::guess`] reads to decide between text and
/// binary. 8192 is the window shared-mime-info's own text check uses.
pub const SNIFF_LEN: usize = 8192;

const TEXT_PLAIN: &str = "text/plain";
const OCTET_STREAM: &str = "application/octet-stream";
/// In a `globs2` line `weight:type:__NOGLOBS__` (written by
/// `update-mime-database`'s `glob-deleteall`) takes the place of the pattern
/// and drops every glob for `type` from data directories of lower priority.
const NO_GLOBS: &str = "__NOGLOBS__";

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
    /// Position among all loaded lines; the earlier line wins a full tie.
    order: usize,
}

/// FNV-1a. The keys are file-name fragments from a trusted local database,
/// so the default hasher's flood resistance buys nothing, and a lookup runs
/// once per directory entry.
#[derive(Default)]
struct Fnv(u64);

impl std::hash::Hasher for Fnv {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        let mut h = if self.0 == 0 {
            0xcbf2_9ce4_8422_2325
        } else {
            self.0
        };
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        self.0 = h;
    }
}

type FastMap = HashMap<String, Vec<usize>, BuildHasherDefault<Fnv>>;

/// Where each pattern in `MimeDb::patterns` is looked up from. Built once by
/// `MimeDb::load`, so a name costs a few hash probes instead of a scan of
/// every pattern. Both maps are keyed by the lowercased text; a
/// case-sensitive entry is confirmed against the exact name after the probe.
#[derive(Debug, Default)]
struct Index {
    /// Literals, keyed by the whole name.
    literal: FastMap,
    /// `*.ext` patterns, keyed by the text after the `*` (`.ext`).
    suffix: FastMap,
    /// Every other glob starting with a plain char (`readme*`), keyed by that
    /// char; a name matches only if it starts with it.
    complex_first: HashMap<char, Vec<usize>, BuildHasherDefault<Fnv>>,
    /// Every other glob ending in a plain char (`*~`), keyed by that char.
    complex_last: HashMap<char, Vec<usize>, BuildHasherDefault<Fnv>>,
    /// The rest (`*.so.[0-9]*`), each with the first run of plain chars it
    /// requires (`.so.`), which a name must contain to match, and, for a
    /// glob ending in a set (`*.[1-9]`), where that set opens: the name's
    /// last char must be in it.
    complex_run: Vec<(usize, String, Option<usize>)>,
}

/// The MIME database merged from every data directory given to
/// [`MimeDb::load`].
#[derive(Debug, Default)]
pub struct MimeDb {
    patterns: Vec<Pattern>,
    index: Index,
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
    /// directory wins. A `weight:type:__NOGLOBS__` line removes every glob
    /// for `type` from the directories after the one it appears in.
    pub fn load(data_dirs: &[PathBuf]) -> Result<MimeDb, AppsError> {
        let mut db = MimeDb::default();
        let mut no_globs: Vec<(String, usize)> = Vec::new();
        for (rank, dir) in data_dirs.iter().enumerate() {
            let mime_dir = dir.join("mime");
            if let Some((path, text)) = read_optional(&mime_dir.join("globs2"))? {
                db.add_globs(&path, &text, rank, &mut no_globs)?;
            }
            if let Some((path, text)) = read_optional(&mime_dir.join("subclasses"))? {
                db.add_subclasses(&path, &text)?;
            }
            if let Some((path, text)) = read_optional(&mime_dir.join("aliases"))? {
                db.add_aliases(&path, &text)?;
            }
        }
        db.patterns.retain(|p| {
            !no_globs
                .iter()
                .any(|(mime, rank)| *mime == p.mime && *rank < p.dir_rank)
        });
        db.index = Index::build(&db.patterns);
        Ok(db)
    }

    fn add_globs(
        &mut self,
        path: &Path,
        text: &str,
        dir_rank: usize,
        no_globs: &mut Vec<(String, usize)>,
    ) -> Result<(), AppsError> {
        for (index, line) in text.lines().enumerate() {
            let line = line.trim_end_matches('\r');
            if line.is_empty() || line.starts_with('#') {
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
            if pattern == NO_GLOBS {
                no_globs.push((mime.to_string(), dir_rank));
                continue;
            }
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
                order: self.patterns.len(),
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
    /// globs the highest weight wins, then the longest pattern, then a
    /// case-sensitive pattern over a case-insensitive one (`x.C` is C++, not
    /// C), then the earlier data directory, then the line listed first.
    ///
    /// Candidates come from the index built by [`MimeDb::load`]: exact-name
    /// maps for literals, a map keyed by `.ext` for `*.ext` globs (tried for
    /// every `.` suffix of the name, so `.tar.gz` is found as well as
    /// `.gz`), and a scan of the few remaining globs.
    pub fn by_name<'a>(&'a self, file_name: &str) -> Option<&'a str> {
        let patterns: &'a [Pattern] = &self.patterns;
        let lower: Cow<str> = if file_name.chars().any(char::is_uppercase) {
            Cow::Owned(file_name.to_lowercase())
        } else {
            Cow::Borrowed(file_name)
        };
        let ix = &self.index;
        // Strongest candidate so far: weight, then pattern length, then the
        // earlier data directory.
        let better = |new: &Pattern, old: &Pattern| {
            (
                new.weight,
                new.chars.len(),
                new.case_sensitive,
                old.dir_rank,
                old.order,
            ) > (
                old.weight,
                old.chars.len(),
                old.case_sensitive,
                new.dir_rank,
                new.order,
            )
        };
        // A case-sensitive pattern found through the lowercased key must
        // still equal the name exactly (literal) or end the name (suffix).
        let confirmed = |p: &Pattern, literal: bool| {
            if !p.case_sensitive {
                return true;
            }
            let text: String = p.chars.iter().collect();
            if literal {
                text == file_name
            } else {
                file_name.ends_with(&text[1..])
            }
        };
        let mut best: Option<&Pattern> = None;
        for &i in ids(&ix.literal, &lower) {
            let p = &self.patterns[i];
            if confirmed(p, true) && best.is_none_or(|b| better(p, b)) {
                best = Some(p);
            }
        }
        if let Some(p) = best {
            return Some(p.mime.as_str());
        }
        for (at, _) in lower.match_indices('.') {
            for &i in ids(&ix.suffix, &lower[at..]) {
                let p = &self.patterns[i];
                if confirmed(p, false) && best.is_none_or(|b| better(p, b)) {
                    best = Some(p);
                }
            }
        }
        // Globs are gated by a char or run every match must contain, so most
        // names never reach `glob_match`. The gate is probed with the exact
        // and the lowercased name; `glob_match` then decides on the subject
        // the pattern's own case rule picks.
        let exact_chars: OnceCell<Vec<char>> = OnceCell::new();
        let lower_chars: OnceCell<Vec<char>> = OnceCell::new();
        let try_glob = |i: usize, best: &mut Option<&'a Pattern>| {
            let p: &'a Pattern = &patterns[i];
            if best.is_none_or(|b| better(p, b)) {
                let text = if p.case_sensitive {
                    exact_chars.get_or_init(|| file_name.chars().collect())
                } else {
                    lower_chars.get_or_init(|| lower.chars().collect())
                };
                if glob_match(&p.chars, text) {
                    *best = Some(p);
                }
            }
        };
        let (first, last) = (lower.chars().next(), lower.chars().next_back());
        let (first_exact, last_exact) = (file_name.chars().next(), file_name.chars().next_back());
        for (key, exact, map) in [
            (first, first_exact, &ix.complex_first),
            (last, last_exact, &ix.complex_last),
        ] {
            for key in key.into_iter().chain(exact.filter(|e| Some(*e) != key)) {
                for &i in map.get(&key).map_or(&[][..], Vec::as_slice) {
                    try_glob(i, &mut best);
                }
            }
        }
        for (i, needle, tail_set) in &ix.complex_run {
            let subject: &str = if patterns[*i].case_sensitive {
                file_name
            } else {
                &lower
            };
            let tail_ok = tail_set.is_none_or(|open| {
                subject.chars().next_back().is_some_and(|c| {
                    matches!(match_set(&patterns[*i].chars, open, c), Some((true, _)))
                })
            });
            if tail_ok && subject.contains(needle.as_str()) {
                try_glob(*i, &mut best);
            }
        }
        best.map(|p| p.mime.as_str())
    }

    /// The type of the file at `path`.
    ///
    /// A symlink is guessed by its target; a link whose target cannot be
    /// resolved (dangling, or a loop) is `inode/symlink`, since it is not a
    /// file anyone can open. A directory is `inode/directory`; a FIFO,
    /// socket or device node is the matching `inode/*` type and is never
    /// opened. A regular file is typed by its name first; when no pattern
    /// matches, its first [`SNIFF_LEN`] bytes decide: `text/plain` when they
    /// are valid UTF-8 without a NUL byte (a multi-byte sequence cut off by
    /// the end of the window is allowed, and an empty file counts as text),
    /// else `application/octet-stream`.
    ///
    /// The return type carries no error. A file that cannot be inspected
    /// (permission denied, removed meanwhile) is `application/octet-stream`,
    /// except that a path which does not exist is typed by its name alone
    /// when a pattern matches it. The caller learns the real reason when it
    /// opens the file.
    pub fn guess(&self, path: &Path) -> String {
        let Ok(link_meta) = fs::symlink_metadata(path) else {
            return self.name_or(path, OCTET_STREAM);
        };
        let (resolved, meta) = if link_meta.file_type().is_symlink() {
            let target = fs::canonicalize(path).ok();
            match target.and_then(|t| fs::metadata(&t).ok().map(|m| (t, m))) {
                Some(pair) => pair,
                None => return "inode/symlink".to_string(),
            }
        } else {
            (path.to_path_buf(), link_meta)
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

    /// Every name `mime` goes by: `mime` itself first, then its canonical
    /// type when `mime` is an alias, then the other aliases of that type.
    /// Association files written by older tools may use any of them, so a
    /// lookup tries all.
    pub fn names(&self, mime: &str) -> Vec<String> {
        let canonical = self.canonical(mime);
        let mut out = vec![mime.to_string()];
        if canonical != mime {
            out.push(canonical.to_string());
        }
        let mut others: Vec<&str> = self
            .aliases
            .iter()
            .filter(|(alias, target)| target.as_str() == canonical && alias.as_str() != mime)
            .map(|(alias, _)| alias.as_str())
            .collect();
        others.sort_unstable();
        out.extend(others.into_iter().map(str::to_string));
        out
    }

    /// The types `mime` is a subclass of, nearest first (breadth first
    /// through `subclasses`), as canonical names, without `mime` itself and
    /// without repeats. Every `text/*` type has `text/plain` among them
    /// (section 2.5). `application/octet-stream`, which section 2.5 makes
    /// the root of every non-directory type, is not listed: it carries no
    /// information for choosing an application. Cycles terminate.
    pub fn ancestors(&self, mime: &str) -> Vec<String> {
        let start = self.canonical(mime);
        let mut seen: HashSet<&str> = HashSet::from([start]);
        let mut out: Vec<String> = Vec::new();
        let mut queue: std::collections::VecDeque<&str> = std::collections::VecDeque::from([start]);
        while let Some(current) = queue.pop_front() {
            let mut next: Vec<&str> = self
                .parents
                .get(current)
                .map(|direct| direct.iter().map(|p| self.canonical(p)).collect())
                .unwrap_or_default();
            if current.starts_with("text/") && current != TEXT_PLAIN {
                next.push(TEXT_PLAIN);
            }
            for parent in next {
                if seen.insert(parent) {
                    out.push(parent.to_string());
                    queue.push_back(parent);
                }
            }
        }
        out
    }

    fn canonical<'a>(&'a self, mime: &'a str) -> &'a str {
        self.aliases.get(mime).map_or(mime, String::as_str)
    }
}

impl Index {
    fn build(patterns: &[Pattern]) -> Index {
        let mut ix = Index::default();
        for (i, p) in patterns.iter().enumerate() {
            let text: String = p.chars.iter().collect();
            if p.literal {
                ix.literal.entry(text.to_lowercase()).or_default().push(i);
            } else if let Some(ext) = text
                .strip_prefix('*')
                .filter(|rest| rest.starts_with('.') && !rest.contains(['*', '?', '[']))
            {
                ix.suffix.entry(ext.to_lowercase()).or_default().push(i);
            } else {
                let plain = |c: &&char| !matches!(c, '*' | '?' | '[' | ']');
                if let Some(&c) = p.chars.first().filter(plain) {
                    ix.complex_first.entry(c).or_default().push(i);
                } else if let Some(&c) = p.chars.last().filter(plain) {
                    ix.complex_last.entry(c).or_default().push(i);
                } else {
                    let tail_set = (p.chars.last() == Some(&']'))
                        .then(|| p.chars.iter().rposition(|&c| c == '['))
                        .flatten();
                    ix.complex_run.push((i, required_run(&p.chars), tail_set));
                }
            }
        }
        ix
    }
}

/// The first maximal run of plain chars in a glob (outside `[...]` sets),
/// which every name matching the glob contains; empty when there is none.
fn required_run(pattern: &[char]) -> String {
    let mut run = String::new();
    let mut in_set = false;
    for &c in pattern {
        match c {
            '[' => in_set = true,
            ']' if in_set => in_set = false,
            '*' | '?' => {}
            _ if in_set => continue,
            _ => {
                run.push(c);
                continue;
            }
        }
        if !run.is_empty() {
            break;
        }
    }
    run
}

/// The pattern ids stored under `key`, or none.
fn ids<'a>(map: &'a FastMap, key: &str) -> &'a [usize] {
    map.get(key).map_or(&[], Vec::as_slice)
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
