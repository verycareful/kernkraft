// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! User access permissions: ISO 32000-1:2008, 7.6.3.2, Table 22.

use super::dictionary::Revision;

/// The mask of the bit at `position`, counted from 1 at the low-order end
/// as Table 22 counts.
const fn bit(position: u32) -> u32 {
    1 << (position - 1)
}

const PRINT: u32 = bit(3);
const MODIFY: u32 = bit(4);
const COPY: u32 = bit(5);
const ANNOTATE: u32 = bit(6);
const FILL_FORMS: u32 = bit(9);
const ACCESSIBILITY: u32 = bit(10);
const ASSEMBLE: u32 = bit(11);
const PRINT_FAITHFUL: u32 = bit(12);

/// What a file allows someone who opened it with the user password: the
/// `P` entry of its encryption dictionary, read as its revision defines.
///
/// These are the author's wishes as the file states them. This library
/// reports them and enforces none; someone who opened the file with the
/// owner password is not bound by them (7.6.3.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    bits: u32,
    /// Whether the bits that revision 3 added (9 to 12) are meaningful.
    /// Revision 2 covers what they control with bits 3 to 6.
    selective: bool,
}

impl Permissions {
    pub(crate) fn new(bits: u32, revision: Revision) -> Self {
        Self {
            bits,
            selective: revision != Revision::R2,
        }
    }

    /// `P` as an unsigned 32-bit number.
    pub fn bits(self) -> u32 {
        self.bits
    }

    fn has(self, mask: u32) -> bool {
        self.bits & mask != 0
    }

    /// A bit of revision 3 or greater, or under revision 2 the bit that
    /// covers the same operation.
    fn selective(self, mask: u32, revision_2_mask: u32) -> bool {
        self.has(if self.selective {
            mask
        } else {
            revision_2_mask
        })
    }

    /// Print the document (bit 3). Under revision 3 or greater the print
    /// may be of low quality unless [`Self::print_faithful`] is also set.
    pub fn print(self) -> bool {
        self.has(PRINT)
    }

    /// Print to a representation from which a faithful digital copy could
    /// be made (bits 3 and 12). Revision 2 has one print permission, which
    /// allows this too.
    pub fn print_faithful(self) -> bool {
        self.print() && self.selective(PRINT_FAITHFUL, PRINT)
    }

    /// Modify the contents by operations other than those of
    /// [`Self::annotate`], [`Self::fill_forms`] and [`Self::assemble`]
    /// (bit 4).
    pub fn modify(self) -> bool {
        self.has(MODIFY)
    }

    /// Copy or otherwise extract text and graphics (bit 5).
    pub fn copy(self) -> bool {
        self.has(COPY)
    }

    /// Add or modify text annotations and fill in form fields (bit 6).
    pub fn annotate(self) -> bool {
        self.has(ANNOTATE)
    }

    /// Fill in existing form fields, signature fields included (bit 9, or
    /// bit 6). Revision 2 allows it with [`Self::annotate`] alone.
    pub fn fill_forms(self) -> bool {
        self.has(ANNOTATE) || self.selective(FILL_FORMS, ANNOTATE)
    }

    /// Extract text and graphics in support of accessibility (bit 10).
    /// Revision 2 allows it with [`Self::copy`].
    pub fn extract_for_accessibility(self) -> bool {
        self.selective(ACCESSIBILITY, COPY)
    }

    /// Insert, rotate or delete pages and create bookmarks or thumbnail
    /// images (bit 11, or bit 4). Revision 2 allows it with
    /// [`Self::modify`] alone.
    pub fn assemble(self) -> bool {
        self.has(MODIFY) || self.selective(ASSEMBLE, MODIFY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// What each permission says, in the order of the accessors below.
    fn all(permissions: Permissions) -> [bool; 8] {
        [
            permissions.print(),
            permissions.print_faithful(),
            permissions.modify(),
            permissions.copy(),
            permissions.annotate(),
            permissions.fill_forms(),
            permissions.extract_for_accessibility(),
            permissions.assemble(),
        ]
    }

    #[test]
    fn nothing_and_everything() {
        for revision in [Revision::R2, Revision::R3, Revision::R6] {
            assert_eq!(all(Permissions::new(0, revision)), [false; 8]);
            assert_eq!(all(Permissions::new(u32::MAX, revision)), [true; 8]);
        }
    }

    /// The note under Table 22's introduction: for revision 2, -44
    /// permits printing and copying and disallows modifying the contents
    /// and annotations.
    #[test]
    fn the_example_of_7_6_3_2() {
        let permissions = Permissions::new(-44i32 as u32, Revision::R2);
        assert_eq!(permissions.bits(), 0xFFFF_FFD4);
        assert!(permissions.print() && permissions.print_faithful());
        assert!(permissions.copy() && permissions.extract_for_accessibility());
        assert!(!permissions.modify() && !permissions.assemble());
        assert!(!permissions.annotate() && !permissions.fill_forms());
    }

    /// Each bit alone, under revision 3: bit position -> the permissions
    /// it grants by itself.
    #[test]
    fn single_bits_of_revision_3() {
        let none = [false; 8];
        let cases: [(u32, [usize; 2]); 8] = [
            (3, [0, 0]),                    // print, not faithful without bit 12
            (4, [2, 7]),                    // modify, and with it assemble
            (5, [3, 3]),                    // copy
            (6, [4, 5]),                    // annotate, and with it fill forms
            (9, [5, 5]),                    // fill forms
            (10, [6, 6]),                   // accessibility
            (11, [7, 7]),                   // assemble
            (12, [usize::MAX, usize::MAX]), // faithful print needs bit 3 too
        ];
        for (position, granted) in cases {
            let mut expected = none;
            for index in granted {
                if let Some(slot) = expected.get_mut(index) {
                    *slot = true;
                }
            }
            let permissions = Permissions::new(1 << (position - 1), Revision::R3);
            assert_eq!(all(permissions), expected, "bit {position}");
        }
        let faithful = Permissions::new((1 << 2) | (1 << 11), Revision::R3);
        assert!(faithful.print() && faithful.print_faithful());
    }

    /// Bits 9 to 12 mean nothing under revision 2.
    #[test]
    fn revision_2_ignores_the_later_bits() {
        let later = (1 << 8) | (1 << 9) | (1 << 10) | (1 << 11);
        assert_eq!(all(Permissions::new(later, Revision::R2)), [false; 8]);
    }
}
