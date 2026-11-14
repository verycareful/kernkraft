// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Destinations: ISO 32000-1:2008, 12.3.2 (Table 151), named through the
//! catalog's `Dests` dictionary (PDF 1.1) or the `Dests` name tree of the
//! name dictionary (7.7.4); 12.6.4.3 for the page numbers of a destination
//! in another file.
//!
//! An explicit destination is an array: the page, a view name, and the
//! view's numbers. A named one is a name (looked up in the catalog's
//! `Dests`) or a string (looked up in the name tree); what either maps to
//! is the array, or a dictionary whose `D` is the array.

use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::{Document, NameTree};
use crate::syntax::{Object, ObjectId, Value};
use crate::PdfError;

const DESTS_KEY: &[u8] = b"Dests";
const NAMES_KEY: &[u8] = b"Names";
const D_KEY: &[u8] = b"D";

/// How a destination shows its page (Table 151). A coordinate or zoom of
/// `None` keeps what the viewer has; a zoom of 0 is `None` (Table 151).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum View {
    Xyz {
        left: Option<f64>,
        top: Option<f64>,
        zoom: Option<f64>,
    },
    Fit,
    FitH {
        top: Option<f64>,
    },
    FitV {
        left: Option<f64>,
    },
    FitR {
        left: f64,
        bottom: f64,
        right: f64,
        top: f64,
    },
    FitB,
    FitBH {
        top: Option<f64>,
    },
    FitBV {
        left: Option<f64>,
    },
}

/// The page a destination shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DestinationPage {
    /// A page of this document, counted from 1.
    Page(u32),
    /// A page given by its index, counted from 0: how a destination in
    /// another file names its page (12.6.4.3).
    Index(u32),
    /// A reference to an object that is not a page of this document.
    Missing(ObjectId),
}

/// An explicit destination: a page and a view of it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Destination {
    pub page: DestinationPage,
    pub view: View,
}

/// Where a destination is to be found.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DestinationFile {
    /// In this document: its page is a reference to a page object.
    This,
    /// In another file: its page is an index (12.6.4.3).
    Other,
}

impl Document {
    /// The destination `object` designates, explicit or named, in this
    /// document. `None`, with a diagnostic, when it is not one or names
    /// one the document does not define.
    pub(crate) fn destination(
        &self,
        object: &Object,
        file: DestinationFile,
        offset: u64,
        diagnostics: &mut Diagnostics,
    ) -> Result<Option<Destination>, PdfError> {
        let resolved = self.resolve(object)?;
        let target = match resolved.value() {
            Value::Name(name) if file == DestinationFile::This => {
                self.named_by_name(name.as_bytes())?
            }
            Value::String(string) if file == DestinationFile::This => {
                self.named_by_string(string.as_bytes())?
            }
            // A name in another file cannot be looked up here.
            Value::Name(_) | Value::String(_) => return Ok(None),
            Value::Array(_) => Some(resolved.object().clone()),
            _ => None,
        };
        let Some(target) = target else {
            diagnostics.push(DiagnosticKind::UnknownDestination, offset);
            return Ok(None);
        };
        let explicit = self.explicit(&target, file)?;
        if explicit.is_none() {
            diagnostics.push(DiagnosticKind::InvalidDestination, offset);
        }
        Ok(explicit)
    }

    /// The destination array the catalog's `Dests` gives `name`.
    fn named_by_name(&self, name: &[u8]) -> Result<Option<Object>, PdfError> {
        let catalog = self.catalog()?;
        let Some(dictionary) = catalog.dictionary() else {
            return Ok(None);
        };
        let dests = self.get(dictionary, DESTS_KEY)?;
        let Some(dests) = dests.dictionary() else {
            return Ok(None);
        };
        match dests.get(name) {
            Some(value) => self.destination_array(value),
            None => Ok(None),
        }
    }

    /// The destination array the `Dests` name tree gives `key`.
    fn named_by_string(&self, key: &[u8]) -> Result<Option<Object>, PdfError> {
        let catalog = self.catalog()?;
        let Some(dictionary) = catalog.dictionary() else {
            return Ok(None);
        };
        let names = self.get(dictionary, NAMES_KEY)?;
        let Some(root) = names.dictionary().and_then(|names| names.get(DESTS_KEY)) else {
            return Ok(None);
        };
        match NameTree::new(self, root.clone()).get(key)? {
            Some(value) => self.destination_array(&value),
            None => Ok(None),
        }
    }

    /// A named destination's value: the array, or a dictionary's `D`.
    fn destination_array(&self, value: &Object) -> Result<Option<Object>, PdfError> {
        let resolved = self.resolve(value)?;
        if resolved.array().is_some() {
            return Ok(Some(resolved.object().clone()));
        }
        let Some(dictionary) = resolved.dictionary() else {
            return Ok(None);
        };
        let d = self.get(dictionary, D_KEY)?;
        Ok(d.array().map(|_| d.object().clone()))
    }

    /// The explicit destination `array` is, or `None` when it is not one
    /// that Table 151 defines.
    fn explicit(
        &self,
        array: &Object,
        file: DestinationFile,
    ) -> Result<Option<Destination>, PdfError> {
        let Some(items) = self.resolve(array)?.array().map(<[Object]>::to_vec) else {
            return Ok(None);
        };
        let Some((page, rest)) = items.split_first() else {
            return Ok(None);
        };
        let page = match (page.value(), file) {
            (Value::Reference(id), DestinationFile::This) => match self.page_number(*id)? {
                Some(number) => DestinationPage::Page(number),
                None => DestinationPage::Missing(*id),
            },
            (Value::Integer(index), DestinationFile::Other) => match u32::try_from(*index) {
                Ok(index) => DestinationPage::Index(index),
                Err(_) => return Ok(None),
            },
            _ => return Ok(None),
        };
        let Some((view, numbers)) = rest.split_first() else {
            return Ok(None);
        };
        let Value::Name(view) = self.resolve(view)?.value().clone() else {
            return Ok(None);
        };
        let mut values = Vec::with_capacity(numbers.len());
        for number in numbers {
            values.push(match self.resolve(number)?.value() {
                Value::Null => None,
                Value::Integer(integer) => Some(*integer as f64),
                Value::Real(real) => Some(real.get()),
                _ => return Ok(None),
            });
        }
        Ok(view_of(view.as_bytes(), &values).map(|view| Destination { page, view }))
    }
}

/// The view `name` with its numbers `values`, when they are as many as
/// Table 151 gives it. `FitR` needs all four.
fn view_of(name: &[u8], values: &[Option<f64>]) -> Option<View> {
    Some(match (name, values) {
        (b"XYZ", [left, top, zoom]) => View::Xyz {
            left: *left,
            top: *top,
            zoom: zoom.filter(|zoom| *zoom != 0.0),
        },
        (b"Fit", []) => View::Fit,
        (b"FitH", [top]) => View::FitH { top: *top },
        (b"FitV", [left]) => View::FitV { left: *left },
        (b"FitR", [Some(left), Some(bottom), Some(right), Some(top)]) => View::FitR {
            left: *left,
            bottom: *bottom,
            right: *right,
            top: *top,
        },
        (b"FitB", []) => View::FitB,
        (b"FitBH", [top]) => View::FitBH { top: *top },
        (b"FitBV", [left]) => View::FitBV { left: *left },
        _ => return None,
    })
}
