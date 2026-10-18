// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The `name (n)` scheme for making a clashing name unique, shared by the
//! trash and by copy and move.

/// `name` with ` (n)` inserted before its extension: `a.txt` becomes
/// `a (2).txt`. With `keep_extension = false` (directories) or when the only
/// dot leads the name (`.bashrc`), the suffix goes at the very end.
pub(crate) fn numbered(name: &str, n: u32, keep_extension: bool) -> String {
    let split = keep_extension
        .then(|| name.rfind('.'))
        .flatten()
        .filter(|&dot| dot > 0);
    match split {
        Some(dot) => format!("{} ({n}){}", &name[..dot], &name[dot..]),
        None => format!("{name} ({n})"),
    }
}

#[cfg(test)]
mod tests {
    use super::numbered;

    #[test]
    fn suffix_goes_before_last_extension() {
        assert_eq!(numbered("a.txt", 2, true), "a (2).txt");
        assert_eq!(numbered("a.tar.gz", 3, true), "a.tar (3).gz");
    }

    #[test]
    fn suffix_goes_last_for_dotfiles_and_directories() {
        assert_eq!(numbered(".bashrc", 2, true), ".bashrc (2)");
        assert_eq!(numbered("v1.2", 2, false), "v1.2 (2)");
        assert_eq!(numbered("plain", 2, true), "plain (2)");
    }
}
