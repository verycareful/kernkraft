// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Link annotations and where destinations lead: ISO 32000-1:2008, 12.5.2
//! (Table 164: `Annots`, Table 165: annotation flags, Table 166: `Border`
//! and `BS`), 12.5.6.5 (Table 173: link annotations) and 12.3.2
//! (Table 151: destinations).
//!
//! A link is a region of a page and what following it does: a destination,
//! the actions of 12.6, or both (12.5.6.5 says an `A` entry makes `Dest`
//! absent, but a file may carry both and both are given). Nothing is
//! performed: the application decides what a URI or a launch action may
//! do. A link the file marks hidden or not viewable is not given, since
//! such an annotation neither shows nor takes input (Table 165).

use crate::action::{Action, ActionKind};
use crate::destination::{Destination, DestinationFile, DestinationPage, View};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::display::{PageView, Rect};
use crate::doc::{Document, Rectangle};
use crate::syntax::{Name, Object, ObjectId, Value};
use crate::PdfError;

const ANNOTS_KEY: &[u8] = b"Annots";
const SUBTYPE_KEY: &[u8] = b"Subtype";
const RECT_KEY: &[u8] = b"Rect";
const QUAD_POINTS_KEY: &[u8] = b"QuadPoints";
const FLAGS_KEY: &[u8] = b"F";
const DEST_KEY: &[u8] = b"Dest";
const A_KEY: &[u8] = b"A";
const BORDER_STYLE_KEY: &[u8] = b"BS";
const BORDER_KEY: &[u8] = b"Border";
const WIDTH_KEY: &[u8] = b"W";

/// Table 165: bit 2 (`Hidden`) and bit 6 (`NoView`).
const FLAG_HIDDEN: i64 = 1 << 1;
const FLAG_NO_VIEW: i64 = 1 << 5;

/// The width of a link's border when the file gives none (Table 166:
/// `Border` defaults to `[0 0 1]`), in points.
const DEFAULT_BORDER_WIDTH: f64 = 1.0;

/// The numbers of `Border` (Table 166): the corner radii and the width.
const BORDER_ENTRIES: usize = 3;

/// A quadrilateral in `QuadPoints` is eight numbers (Table 179).
const QUAD_NUMBERS: usize = 8;

/// A link on a page.
#[derive(Debug, Clone, PartialEq)]
pub struct Link {
    /// The annotation's object, when it is an indirect one.
    pub annotation: Option<ObjectId>,
    /// The regions that follow the link: one for each quadrilateral of
    /// `QuadPoints` (a link across lines of text has several), else the
    /// annotation's `Rect`.
    pub rects: Vec<Rect>,
    /// `Dest`.
    pub destination: Option<Destination>,
    /// `A` and the actions its `Next` chains.
    pub actions: Vec<Action>,
    /// The width of the border the link is drawn with, in points; 0 for
    /// none.
    pub border_width: f64,
}

impl Link {
    /// The destination following the link leads to: `Dest`, or else the
    /// destination of the first go-to action that has one.
    pub fn target(&self) -> Option<&Destination> {
        self.destination.as_ref().or_else(|| {
            self.actions
                .iter()
                .find(|action| action.kind == ActionKind::GoTo)
                .and_then(|action| action.destination.as_ref())
        })
    }

    /// The address of the first URI action.
    pub fn uri(&self) -> Option<&str> {
        self.actions
            .iter()
            .find(|action| action.kind == ActionKind::Uri)
            .and_then(|action| action.uri.as_ref())
            .map(|uri| uri.text.as_str())
    }

    /// Whether the point, given as fractions of the page, is in one of the
    /// link's regions.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        self.rects.iter().any(|rect| rect.contains(x, y))
    }
}

/// Where a destination shows, on the displayed page of the page it
/// names.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Located {
    /// The page, counted from 1.
    pub page: u32,
    /// The point the view puts at the top left of the window, as
    /// fractions of the displayed page, for each axis the view fixes. A
    /// view that keeps the viewer's position, or fits the page, fixes
    /// none.
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// The zoom factor of an `XYZ` view.
    pub zoom: Option<f64>,
}

impl Document {
    /// The links of page `number`, counted from 1, in the order the
    /// page's `Annots` lists them. An annotation that is not a link, or
    /// that the file marks hidden or not viewable, is not given; a link
    /// with no `Rect` of four numbers is left out and reported.
    pub fn links(&self, number: u32) -> Result<Vec<Link>, PdfError> {
        let page = self.page(number)?;
        let mut diagnostics = Diagnostics::new();
        diagnostics.set_object(Some(page.id));
        let view = PageView::of(&page, &mut diagnostics);
        let annots = self.get(&page.dictionary, ANNOTS_KEY)?;
        let items = annots.array().map(<[Object]>::to_vec).unwrap_or_default();
        let mut links = Vec::new();
        for item in &items {
            let resolved = match self.resolve(item) {
                Ok(resolved) => resolved,
                // A reference to a stream is no annotation.
                Err(PdfError::WrongKind { .. }) => {
                    ignored(&mut diagnostics, ANNOTS_KEY);
                    continue;
                }
                Err(error) => return Err(error),
            };
            let Some(dictionary) = resolved.dictionary() else {
                continue;
            };
            let is_link = matches!(
                self.get(dictionary, SUBTYPE_KEY)?.value(),
                Value::Name(name) if name.as_bytes() == b"Link"
            );
            if !is_link {
                continue;
            }
            let flags = match self.get(dictionary, FLAGS_KEY)?.value() {
                Value::Integer(flags) => *flags,
                _ => 0,
            };
            if flags & (FLAG_HIDDEN | FLAG_NO_VIEW) != 0 {
                continue;
            }
            diagnostics.set_object(resolved.id().or(Some(page.id)));
            let offset = self.offset_of(resolved.id());
            let Some(rects) = self.link_rects(dictionary, &view)? else {
                diagnostics.push(
                    DiagnosticKind::IgnoredEntry {
                        key: Name::new(RECT_KEY),
                    },
                    offset,
                );
                continue;
            };
            let destination = match dictionary.get(DEST_KEY) {
                Some(dest) => {
                    self.destination(dest, DestinationFile::This, offset, &mut diagnostics)?
                }
                None => None,
            };
            let actions = match dictionary.get(A_KEY) {
                Some(action) => self.action_sequence(action, &mut diagnostics)?,
                None => Vec::new(),
            };
            let border_width = self.border_width(dictionary)?;
            links.push(Link {
                annotation: resolved.id(),
                rects,
                destination,
                actions,
                border_width,
            });
        }
        diagnostics.set_object(None);
        self.record_diagnostics(diagnostics);
        Ok(links)
    }

    /// The regions of a link annotation, or `None` when it has no `Rect`.
    fn link_rects(
        &self,
        dictionary: &crate::syntax::Dictionary,
        view: &PageView,
    ) -> Result<Option<Vec<Rect>>, PdfError> {
        let quads = self.numbers(dictionary, QUAD_POINTS_KEY)?;
        if let Some(numbers) =
            quads.filter(|numbers| !numbers.is_empty() && numbers.len() % QUAD_NUMBERS == 0)
        {
            let (quads, _) = numbers.as_chunks::<QUAD_NUMBERS>();
            let rects = quads
                .iter()
                .filter_map(|quad| {
                    let (points, _) = quad.as_chunks::<2>();
                    view.rect_of(points.iter().map(|[x, y]| view.matrix.apply(*x, *y)))
                })
                .collect();
            return Ok(Some(rects));
        }
        let Some(numbers) = self.numbers(dictionary, RECT_KEY)? else {
            return Ok(None);
        };
        if numbers.len() != 4 {
            return Ok(None);
        }
        let (x1, y1, x2, y2) = (numbers[0], numbers[1], numbers[2], numbers[3]);
        let rectangle = Rectangle {
            llx: x1.min(x2),
            lly: y1.min(y2),
            urx: x1.max(x2),
            ury: y1.max(y2),
        };
        Ok(Some(vec![view.user_rect(&rectangle)]))
    }

    /// The array `key` of `dictionary` as numbers, or `None` when it is
    /// not an array of numbers.
    fn numbers(
        &self,
        dictionary: &crate::syntax::Dictionary,
        key: &[u8],
    ) -> Result<Option<Vec<f64>>, PdfError> {
        let resolved = self.get(dictionary, key)?;
        let Some(items) = resolved.array() else {
            return Ok(None);
        };
        let mut numbers = Vec::with_capacity(items.len());
        for item in items {
            match self.resolve(item)?.value() {
                Value::Integer(integer) => numbers.push(*integer as f64),
                Value::Real(real) => numbers.push(real.get()),
                _ => return Ok(None),
            }
        }
        Ok(Some(numbers))
    }

    /// The width of a link's border (Table 166): `BS`'s `W`, else the
    /// third number of `Border`, else 1.
    pub(crate) fn border_width(
        &self,
        dictionary: &crate::syntax::Dictionary,
    ) -> Result<f64, PdfError> {
        let style = self.get(dictionary, BORDER_STYLE_KEY)?;
        if let Some(style) = style.dictionary() {
            if let Some(width) = number(self.get(style, WIDTH_KEY)?.value()) {
                return Ok(width.max(0.0));
            }
        }
        if let Some(border) = self.numbers(dictionary, BORDER_KEY)? {
            if border.len() >= BORDER_ENTRIES {
                return Ok(border[BORDER_ENTRIES - 1].max(0.0));
            }
        }
        Ok(DEFAULT_BORDER_WIDTH)
    }

    /// Where `destination` shows. `None` when its page is not a page of
    /// this document: a reference to an object that is not one, or an
    /// index past the last page.
    pub fn locate(&self, destination: &Destination) -> Result<Option<Located>, PdfError> {
        let page = match destination.page {
            DestinationPage::Page(number) => number,
            DestinationPage::Index(index) => match index.checked_add(1) {
                Some(number) => number,
                None => return Ok(None),
            },
            DestinationPage::Missing(_) => return Ok(None),
        };
        if page == 0 || page > self.page_count()? {
            return Ok(None);
        }
        let mut diagnostics = Diagnostics::new();
        let view = PageView::of(&self.page(page)?, &mut diagnostics);
        self.record_diagnostics(diagnostics);
        let (left, top, zoom) = match destination.view {
            View::Xyz { left, top, zoom } => (left, top, zoom),
            View::FitH { top } | View::FitBH { top } => (None, top, None),
            View::FitV { left } | View::FitBV { left } => (left, None, None),
            View::FitR { left, top, .. } => (Some(left), Some(top), None),
            View::Fit | View::FitB => (None, None, None),
        };
        let (x, y) = view.locate(left, top);
        Ok(Some(Located { page, x, y, zoom }))
    }
}

fn number(value: &Value) -> Option<f64> {
    match value {
        Value::Integer(integer) => Some(*integer as f64),
        Value::Real(real) => Some(real.get()),
        _ => None,
    }
}

fn ignored(diagnostics: &mut Diagnostics, key: &'static [u8]) {
    diagnostics.push(
        DiagnosticKind::IgnoredEntry {
            key: Name::new(key),
        },
        0,
    );
}
