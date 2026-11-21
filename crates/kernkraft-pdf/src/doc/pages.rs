// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The page tree: ISO 32000-1:2008, 7.7.3 (Tables 29 and 30), and
//! rectangles (7.9.5).
//!
//! The tree is walked once per snapshot, in document order. A node with
//! `Kids` is a page tree node and one without is a page, whatever its
//! `Type` says; a `Type` that says otherwise is reported. A page lacking
//! one of the four inheritable attributes takes it from its nearest
//! ancestor that has it (7.7.3.4).
//!
//! A tree in which a node is reached twice is no tree, and is refused, as
//! is a kid that is not a reference to a dictionary: every operation on
//! pages numbers them as this walk does, and a malformed tree has no
//! single numbering. Each node is visited once, so the walk ends within
//! the file's cross-reference entries.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

use super::{Document, Origin, Resolved};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::file::Location;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::PdfError;

const PAGES_KEY: &[u8] = b"Pages";
const KIDS_KEY: &[u8] = b"Kids";
const TYPE_KEY: &[u8] = b"Type";
const PAGE_TYPE: &[u8] = b"Page";
const PAGES_TYPE: &[u8] = b"Pages";

/// The inheritable attributes (Table 30), in the order of [`Inherited`].
const INHERITABLE: [&[u8]; 4] = [b"Resources", b"MediaBox", b"CropBox", b"Rotate"];
const RESOURCES: usize = 0;
const MEDIA_BOX: usize = 1;
const CROP_BOX: usize = 2;
const ROTATE: usize = 3;

/// The page tree node each inheritable attribute is taken from, for a page
/// that lacks it.
type Inherited = [Option<ObjectId>; 4];

/// The pages of one snapshot, in document order.
#[derive(Debug)]
pub(crate) struct PageList {
    pub(crate) pages: Vec<(ObjectId, Inherited)>,
    /// The nodes of the tree that have kids, the root included.
    pub(crate) nodes: HashSet<ObjectId>,
    /// The number of each page, counted from 1, built when first asked.
    numbers: OnceLock<HashMap<ObjectId, u32>>,
}

impl PageList {
    pub(crate) fn read(
        document: &Document,
        diagnostics: &mut Diagnostics,
    ) -> Result<Self, PdfError> {
        let catalog = document.catalog()?;
        let root = match catalog
            .dictionary()
            .and_then(|c| c.get(PAGES_KEY))
            .map(Object::value)
        {
            Some(Value::Reference(root)) => *root,
            _ => {
                return Err(PdfError::InvalidEntry {
                    object: Some(catalog.id),
                    key: "Pages",
                })
            }
        };
        let mut pages = Vec::new();
        let mut nodes = HashSet::new();
        let mut seen = HashSet::new();
        let mut stack: Vec<(ObjectId, Inherited)> = vec![(root, [None; 4])];
        while let Some((id, mut inherited)) = stack.pop() {
            if !seen.insert(id) {
                return Err(PdfError::PageTree { object: id });
            }
            let node = match document.object(id)? {
                Some(node) if node.object().is_some() => node,
                _ => return Err(PdfError::PageTree { object: id }),
            };
            let Some(dictionary) = node.dictionary() else {
                return Err(PdfError::PageTree { object: id });
            };
            let kids = document.get(dictionary, KIDS_KEY)?;
            let is_node = !matches!(kids, Resolved::Null);
            let offset = match node.origin {
                Origin::File {
                    location: Location::File(span),
                    ..
                } => span.start,
                _ => 0,
            };
            check_type(dictionary, id, offset, is_node, diagnostics);
            if !is_node {
                pages.push((id, inherited));
                continue;
            }
            let Some(kids) = kids.array() else {
                return Err(PdfError::PageTree { object: id });
            };
            nodes.insert(id);
            for (slot, key) in inherited.iter_mut().zip(INHERITABLE) {
                if dictionary.get(key).is_some() {
                    *slot = Some(id);
                }
            }
            for kid in kids.iter().rev() {
                let Value::Reference(kid) = kid.value() else {
                    return Err(PdfError::PageTree { object: id });
                };
                stack.push((*kid, inherited));
            }
        }
        Ok(Self {
            pages,
            nodes,
            numbers: OnceLock::new(),
        })
    }

    /// The number of the page `id`, counted from 1.
    pub(crate) fn number(&self, id: ObjectId) -> Option<u32> {
        self.numbers
            .get_or_init(|| {
                (1..)
                    .zip(&self.pages)
                    .map(|(number, (id, _))| (*id, number))
                    .collect()
            })
            .get(&id)
            .copied()
    }
}

/// Reports a node whose `Type` is not the one its place in the tree
/// gives it.
fn check_type(
    dictionary: &Dictionary,
    id: ObjectId,
    offset: u64,
    is_node: bool,
    diagnostics: &mut Diagnostics,
) {
    let expected = if is_node { PAGES_TYPE } else { PAGE_TYPE };
    let matches = matches!(
        dictionary.get(TYPE_KEY).map(Object::value),
        Some(Value::Name(name)) if name.as_bytes() == expected
    );
    if !matches {
        diagnostics.set_object(Some(id));
        diagnostics.push(
            DiagnosticKind::PageTreeType {
                expected: Name::new(expected),
            },
            offset,
        );
        diagnostics.set_object(None);
    }
}

/// A rectangle (7.9.5), its corners put in order: `llx <= urx` and
/// `lly <= ury`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rectangle {
    pub llx: f64,
    pub lly: f64,
    pub urx: f64,
    pub ury: f64,
}

impl Rectangle {
    /// The rectangle an array of four numbers gives, the coordinates of
    /// two opposite corners in either order. `None` for anything else.
    pub fn from_array(items: &[Object]) -> Option<Self> {
        let [a, b, c, d] = items else {
            return None;
        };
        let number = |object: &Object| match object.value() {
            // An integer is used as a real (7.3.3).
            Value::Integer(integer) => Some(*integer as f64),
            Value::Real(real) => Some(real.get()),
            _ => None,
        };
        let (x1, y1, x2, y2) = (number(a)?, number(b)?, number(c)?, number(d)?);
        Some(Self {
            llx: x1.min(x2),
            lly: y1.min(y2),
            urx: x1.max(x2),
            ury: y1.max(y2),
        })
    }

    pub fn width(&self) -> f64 {
        self.urx - self.llx
    }

    pub fn height(&self) -> f64 {
        self.ury - self.lly
    }
}

/// One page, with what it inherits (Table 30).
#[derive(Debug, Clone)]
pub struct Page {
    pub id: ObjectId,
    /// The page object's own dictionary.
    pub dictionary: Dictionary,
    /// `Resources`, its own or inherited, as written: maybe a reference.
    pub resources: Option<Object>,
    pub media_box: Option<Rectangle>,
    pub crop_box: Option<Rectangle>,
    /// `Rotate`, its own or inherited, in degrees clockwise: 0, 90, 180
    /// or 270.
    pub rotate: u16,
}

/// The number of degrees in a full turn, and the step `Rotate` takes.
const FULL_TURN: i64 = 360;
const QUARTER_TURN: i64 = 90;

impl Document {
    /// The number of pages.
    pub fn page_count(&self) -> Result<u32, PdfError> {
        let count = self.page_list()?.pages.len();
        u32::try_from(count).map_err(|_| PdfError::Invalid(format!("{count} pages")))
    }

    /// The object of page `number`, counted from 1.
    pub fn page_id(&self, number: u32) -> Result<ObjectId, PdfError> {
        Ok(self.page_entry(number)?.0)
    }

    /// The number, counted from 1, of the page whose object is `id`, or
    /// `None` when `id` is not a page of the document.
    pub fn page_number(&self, id: ObjectId) -> Result<Option<u32>, PdfError> {
        Ok(self.page_list()?.number(id))
    }

    /// Page `number`, counted from 1, with its inherited attributes.
    ///
    /// A `MediaBox` or `CropBox` that is not a rectangle, or a `Rotate`
    /// that is not a whole multiple of 90, is [`PdfError::InvalidEntry`].
    pub fn page(&self, number: u32) -> Result<Page, PdfError> {
        let (id, inherited) = self.page_entry(number)?;
        let page = self.required(id)?;
        let Some(dictionary) = page.dictionary() else {
            return Err(PdfError::PageTree { object: id });
        };
        let mut attributes: [Option<Object>; 4] = Default::default();
        for (index, key) in INHERITABLE.iter().enumerate() {
            attributes[index] = match dictionary.get(key) {
                Some(own) => Some(own.clone()),
                None => match inherited[index] {
                    Some(node) => self
                        .required(node)?
                        .dictionary()
                        .and_then(|node| node.get(key))
                        .cloned(),
                    None => None,
                },
            };
        }
        let rectangle = |index: usize, key: &'static str| -> Result<Option<Rectangle>, PdfError> {
            let Some(object) = &attributes[index] else {
                return Ok(None);
            };
            let resolved = self.resolve(object)?;
            let items = resolved.array().unwrap_or_default();
            let mut direct = Vec::with_capacity(items.len());
            for item in items {
                direct.push(self.resolve(item)?.object().clone());
            }
            Rectangle::from_array(&direct)
                .map(Some)
                .ok_or(PdfError::InvalidEntry {
                    object: Some(id),
                    key,
                })
        };
        let media_box = rectangle(MEDIA_BOX, "MediaBox")?;
        let crop_box = rectangle(CROP_BOX, "CropBox")?;
        let rotate = match &attributes[ROTATE] {
            None => 0,
            Some(object) => {
                let degrees = match self.resolve(object)?.value() {
                    Value::Integer(degrees) => Some(*degrees),
                    Value::Real(real) if real.get().fract() == 0.0 => Some(real.get() as i64),
                    _ => None,
                };
                match degrees.filter(|degrees| degrees % QUARTER_TURN == 0) {
                    // The remainder is 0 to 359, which fits in a u16.
                    Some(degrees) => degrees.rem_euclid(FULL_TURN) as u16,
                    None => {
                        return Err(PdfError::InvalidEntry {
                            object: Some(id),
                            key: "Rotate",
                        })
                    }
                }
            }
        };
        Ok(Page {
            id,
            dictionary: dictionary.clone(),
            resources: attributes[RESOURCES].take(),
            media_box,
            crop_box,
            rotate,
        })
    }

    /// The inheritable entries (Table 30) that page `number` takes from
    /// its ancestors because it has none of its own, as the ancestors
    /// write them: what a page needs in its own dictionary to stay the
    /// same when it is hung from another node.
    pub(crate) fn inherited_entries(&self, number: u32) -> Result<Vec<(Name, Object)>, PdfError> {
        let (id, inherited) = self.page_entry(number)?;
        let page = self.required(id)?;
        let Some(dictionary) = page.dictionary() else {
            return Err(PdfError::PageTree { object: id });
        };
        let mut entries = Vec::new();
        for (key, node) in INHERITABLE.iter().zip(inherited) {
            let Some(node) = node else {
                continue;
            };
            if dictionary.get(key).is_some() {
                continue;
            }
            if let Some(value) = self
                .required(node)?
                .dictionary()
                .and_then(|node| node.get(key))
            {
                entries.push((Name::new(*key), value.clone()));
            }
        }
        Ok(entries)
    }

    fn page_entry(&self, number: u32) -> Result<(ObjectId, Inherited), PdfError> {
        let list = self.page_list()?;
        let count = u32::try_from(list.pages.len()).unwrap_or(u32::MAX);
        number
            .checked_sub(1)
            .and_then(|index| list.pages.get(usize::try_from(index).ok()?))
            .copied()
            .ok_or(PdfError::PageOutOfRange {
                page: number,
                count,
            })
    }
}
