// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! How geometry, colours, object numbers and dates cross the boundary.
//!
//! Geometry is fractions of the displayed page: `[left, top, right, bottom]`,
//! `y` growing downwards, `(0, 0)` the top left corner. A colour is
//! `[red, green, blue]`, each 0 to 1. An object is `{"object":n,
//! "generation":g}`. A date is the PDF text string of 7.9.4,
//! `D:YYYYMMDDHHmmSSOHH'mm'`, which Kotlin writes from its own clock: the
//! library reads none.

use kernkraft_pdf::colour::Rgb;
use kernkraft_pdf::doc::Date;
use kernkraft_pdf::syntax::ObjectId;
use kernkraft_pdf::Rect;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::error::WireError;

pub fn rect(rect: &Rect) -> Value {
    json!([rect.left, rect.top, rect.right, rect.bottom])
}

pub fn rect_from(parts: [f64; 4]) -> Rect {
    Rect {
        left: parts[0],
        top: parts[1],
        right: parts[2],
        bottom: parts[3],
    }
}

pub fn colour(colour: Rgb) -> Value {
    json!([colour.r, colour.g, colour.b])
}

pub fn colour_from(parts: [f64; 3]) -> Rgb {
    Rgb {
        r: parts[0],
        g: parts[1],
        b: parts[2],
    }
}

pub fn object(id: ObjectId) -> Value {
    json!({ "object": id.number, "generation": id.generation })
}

/// An object as a request names it.
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WireId {
    pub object: u32,
    pub generation: u16,
}

impl From<WireId> for ObjectId {
    fn from(id: WireId) -> Self {
        ObjectId {
            number: id.object,
            generation: id.generation,
        }
    }
}

/// A date as the PDF text string `D:...`.
pub fn date(text: &str) -> Result<Date, WireError> {
    Date::parse(text).ok_or_else(|| {
        WireError::bad_request(format!(
            "`{text}` is not a PDF date (D:YYYYMMDDHHmmSSOHH'mm')"
        ))
    })
}
