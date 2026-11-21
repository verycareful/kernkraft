// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The outline of a document that loses pages: ISO 32000-1:2008, 12.3.3
//! (Tables 152 and 153).
//!
//! An item that leads to a page that is gone leaves the outline. An item
//! that leads nowhere itself (a heading that only groups) stays while an
//! item below it does. The items below an item that leaves move up to the
//! nearest item that stays, so that nothing that still leads to a page is
//! lost with the item above it. The links of the items that stay
//! (`Parent`, `First`, `Last`, `Prev`, `Next`) and the counts are written
//! again from the list.

use std::collections::HashSet;

use super::rewrite::{integer, reference_object, Rewriter};
use crate::destination::DestinationPage;
use crate::syntax::{Dictionary, Name, ObjectId};
use crate::PdfError;

const OUTLINES_KEY: &[u8] = b"Outlines";
const PARENT_KEY: &[u8] = b"Parent";
const FIRST_KEY: &[u8] = b"First";
const LAST_KEY: &[u8] = b"Last";
const PREV_KEY: &[u8] = b"Prev";
const NEXT_KEY: &[u8] = b"Next";
const COUNT_KEY: &[u8] = b"Count";

/// An item of an outline to be linked: its object, whether it shows its
/// children, and the place in the list of the item it hangs from.
pub(crate) struct Node {
    pub(crate) id: ObjectId,
    pub(crate) open: bool,
    /// The place in the list of an earlier node.
    pub(crate) parent: Option<usize>,
}

/// Writes the links and counts of the outline whose root dictionary is
/// object `root` so that its items are `nodes`, in reading order.
pub(crate) fn relink(
    rewriter: &mut Rewriter<'_>,
    root: ObjectId,
    nodes: &[Node],
) -> Result<(), PdfError> {
    let mut children: Vec<Vec<usize>> = vec![Vec::new(); nodes.len()];
    let mut top: Vec<usize> = Vec::new();
    for (place, node) in nodes.iter().enumerate() {
        match node.parent {
            Some(parent) => children[parent].push(place),
            None => top.push(place),
        }
    }
    // Children come after their parents in the list, so the last item is
    // finished first.
    let mut visible = vec![0i64; nodes.len()];
    for place in (0..nodes.len()).rev() {
        visible[place] = children[place]
            .iter()
            .map(|&child| 1 + if nodes[child].open { visible[child] } else { 0 })
            .sum();
    }
    for (place, node) in nodes.iter().enumerate() {
        let parent = node.parent.map_or(root, |parent| nodes[parent].id);
        let siblings = match node.parent {
            Some(parent) => &children[parent],
            None => &top,
        };
        let at = siblings.iter().position(|&s| s == place).unwrap_or(0);
        let prev = at.checked_sub(1).map(|at| nodes[siblings[at]].id);
        let next = siblings.get(at + 1).map(|&next| nodes[next].id);
        let first = children[place].first().map(|&c| nodes[c].id);
        let last = children[place].last().map(|&c| nodes[c].id);
        let count = if visible[place] == 0 {
            None
        } else if node.open {
            Some(visible[place])
        } else {
            Some(-visible[place])
        };
        rewriter.edit(node.id, |_, item| {
            set_link(item, PARENT_KEY, Some(parent));
            set_link(item, PREV_KEY, prev);
            set_link(item, NEXT_KEY, next);
            set_link(item, FIRST_KEY, first);
            set_link(item, LAST_KEY, last);
            match count {
                Some(count) => {
                    item.insert(Name::new(COUNT_KEY), integer(count));
                }
                None => {
                    item.remove(COUNT_KEY);
                }
            }
            Ok(())
        })?;
    }
    let total: i64 = top
        .iter()
        .map(|&place| 1 + if nodes[place].open { visible[place] } else { 0 })
        .sum();
    let first = top.first().map(|&place| nodes[place].id);
    let last = top.last().map(|&place| nodes[place].id);
    rewriter.edit(root, |_, outlines| {
        set_link(outlines, FIRST_KEY, first);
        set_link(outlines, LAST_KEY, last);
        outlines.insert(Name::new(COUNT_KEY), integer(total));
        Ok(())
    })?;
    Ok(())
}

fn set_link(dictionary: &mut Dictionary, key: &'static [u8], target: Option<ObjectId>) {
    match target {
        Some(id) => {
            dictionary.insert(Name::new(key), reference_object(id));
        }
        None => {
            dictionary.remove(key);
        }
    }
}

/// Takes out of the outline of the document every item that leads to a
/// page whose number is in `gone`, as the module's header describes.
pub(crate) fn prune(
    rewriter: &mut Rewriter<'_>,
    catalog: ObjectId,
    gone: &HashSet<u32>,
) -> Result<(), PdfError> {
    let document = rewriter.document;
    let items = document.outline()?;
    if items.is_empty() {
        return Ok(());
    }
    let leads_nowhere = |place: usize| {
        let item = &items[place];
        item.destination.is_none() && item.actions.is_empty()
    };
    let points_gone = |place: usize| {
        matches!(
            items[place].destination.map(|destination| destination.page),
            Some(DestinationPage::Page(page)) if gone.contains(&page)
        )
    };
    // An item stays if it leads to a page that stays, or leads nowhere and
    // has an item below it that stays. Children follow their parents in the
    // list, so going backwards decides the children first.
    let mut stays = vec![false; items.len()];
    let mut child_stays = vec![false; items.len()];
    for place in (0..items.len()).rev() {
        stays[place] = if leads_nowhere(place) {
            child_stays[place]
        } else {
            !points_gone(place)
        };
        if let Some(parent) = items[place].parent {
            child_stays[parent] |= stays[place] || child_stays[place];
        }
    }
    if stays.iter().all(|&stays| stays) {
        return Ok(());
    }
    let Some(root) = outline_root(rewriter, catalog)? else {
        return Ok(());
    };
    let mut new_place: Vec<Option<usize>> = vec![None; items.len()];
    let mut nodes = Vec::new();
    for (place, item) in items.iter().enumerate() {
        if !stays[place] {
            continue;
        }
        let mut parent = item.parent;
        while let Some(above) = parent {
            if stays[above] {
                break;
            }
            parent = items[above].parent;
        }
        new_place[place] = Some(nodes.len());
        nodes.push(Node {
            id: item.id,
            open: item.open,
            parent: parent.and_then(|above| new_place[above]),
        });
    }
    if nodes.is_empty() {
        rewriter.edit(catalog, |_, catalog| {
            catalog.remove(OUTLINES_KEY);
            Ok(())
        })?;
        return Ok(());
    }
    relink(rewriter, root, &nodes)
}

/// The object of the outline's root dictionary.
fn outline_root(rewriter: &Rewriter<'_>, catalog: ObjectId) -> Result<Option<ObjectId>, PdfError> {
    Ok(rewriter.read_dictionary(catalog)?.and_then(|catalog| {
        catalog
            .get(OUTLINES_KEY)
            .and_then(super::rewrite::reference)
    }))
}
