// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The current colour as a screen colour: ISO 32000-1:2008, 8.6 (colour
//! spaces), 8.6.5.6 (default colour spaces) and 8.6.6.4 (Separation with
//! the colourant `None`, which makes no mark).

use crate::colour::{ColourSpace, DefaultSpaces};
use crate::content::{ColorSpace, Paint};
use crate::doc::Document;
use crate::syntax::{Dictionary, Object};
use crate::PdfError;

/// What a paint comes to on the screen.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Resolved {
    /// An sRGB colour.
    Rgb([u8; 3]),
    /// A colour that leaves no mark (a Separation or DeviceN of `None`).
    Nothing,
    /// A pattern (8.7): the pattern object, and the colour an uncoloured
    /// tiling pattern is painted in.
    Pattern {
        object: Object,
        under: Option<[u8; 3]>,
    },
}

/// Turns paints into colours, reading each colour space once.
pub(super) struct Colours<'d> {
    doc: &'d Document,
    defaults: DefaultSpaces,
    /// The spaces read so far, by the object that defined them.
    spaces: Vec<(Object, ColourSpace)>,
}

impl<'d> Colours<'d> {
    /// `resources` are the page's: their `DefaultGray`, `DefaultRGB` and
    /// `DefaultCMYK` stand in for the device spaces for the whole page.
    pub fn new(doc: &'d Document, resources: &Dictionary) -> Result<Self, PdfError> {
        Ok(Self {
            doc,
            defaults: doc.default_colour_spaces(resources)?,
            spaces: Vec::new(),
        })
    }

    /// The page's default colour spaces, for the images it paints.
    pub fn defaults(&self) -> &DefaultSpaces {
        &self.defaults
    }

    pub fn resolve(&mut self, paint: &Paint) -> Result<Resolved, PdfError> {
        if let Some((_, object)) = &paint.pattern {
            let under = if paint.components.is_empty() {
                None
            } else {
                self.underlying(paint)?
            };
            return Ok(Resolved::Pattern {
                object: object.clone(),
                under,
            });
        }
        let device = match &paint.space {
            ColorSpace::DeviceGray => Some(ColourSpace::DeviceGray),
            ColorSpace::DeviceRGB => Some(ColourSpace::DeviceRgb),
            ColorSpace::DeviceCMYK => Some(ColourSpace::DeviceCmyk),
            _ => None,
        };
        if let Some(device) = device {
            let space = self.defaults.apply(&device);
            return convert(space, &paint.components);
        }
        let ColorSpace::Resource { object, .. } = &paint.space else {
            // The Pattern space with no pattern chosen paints nothing.
            return Ok(Resolved::Nothing);
        };
        let at = self.space_at(object)?;
        convert(&self.spaces[at].1, &paint.components)
    }

    /// The colour of an uncoloured pattern: the components in the space the
    /// Pattern space was given (`[/Pattern base]`, 8.6.6.2).
    fn underlying(&mut self, paint: &Paint) -> Result<Option<[u8; 3]>, PdfError> {
        let ColorSpace::Resource { object, .. } = &paint.space else {
            return Ok(None);
        };
        let at = self.space_at(object)?;
        match &self.spaces[at].1 {
            ColourSpace::Pattern(Some(base)) => {
                Ok(Some(base.to_rgb(&paint.components)?.to_bytes()))
            }
            _ => Ok(None),
        }
    }

    fn space_at(&mut self, object: &Object) -> Result<usize, PdfError> {
        Ok(
            match self.spaces.iter().position(|(known, _)| known == object) {
                Some(at) => at,
                None => {
                    let space = self.doc.colour_space(object)?;
                    self.spaces.push((object.clone(), space));
                    self.spaces.len() - 1
                }
            },
        )
    }
}

fn convert(space: &ColourSpace, components: &[f64]) -> Result<Resolved, PdfError> {
    let paints_nothing = match space {
        ColourSpace::Separation(tint) | ColourSpace::DeviceN(tint) => tint.paints_nothing(),
        _ => false,
    };
    if paints_nothing {
        return Ok(Resolved::Nothing);
    }
    if matches!(space, ColourSpace::Pattern(_)) {
        return Ok(Resolved::Nothing);
    }
    Ok(Resolved::Rgb(space.to_rgb(components)?.to_bytes()))
}
