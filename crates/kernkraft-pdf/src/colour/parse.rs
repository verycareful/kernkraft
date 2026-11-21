// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Reading colour space objects: ISO 32000-1:2008, 8.6.3 (the forms a
//! space takes), 8.6.5 (CIE-based), 8.6.6 (special) and Table 66
//! (ICCBased streams).

use std::sync::Arc;

use super::cie::{CalGray, CalRgb, Lab};
use super::icc::{IccTransform, MAX_PROFILE_BYTES};
use super::{
    ColourSpace, DefaultSpaces, IccBased, Indexed, Tint, INDEXED_MAX_HIVAL, MAX_COLOUR_SPACE_DEPTH,
    MAX_DEVICEN_COMPONENTS,
};
use crate::diagnostic::{DiagnosticKind, Diagnostics};
use crate::doc::Document;
use crate::syntax::{Dictionary, Name, Object, ObjectId, Value};
use crate::{Limit, PdfError};

/// Table 63, `Gamma` of CalGray: the default is 1.0.
const DEFAULT_GAMMA: f64 = 1.0;
/// Table 64, `Matrix`: the default is the identity.
const IDENTITY_MATRIX: [f64; 9] = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0];
/// Tables 63 to 65, `BlackPoint`: the default is `[0.0 0.0 0.0]`.
const DEFAULT_BLACK_POINT: [f64; 3] = [0.0; 3];
/// Table 65, `Range` of Lab: the default is `[-100 100 -100 100]`.
const DEFAULT_LAB_RANGE: [f64; 4] = [-100.0, 100.0, -100.0, 100.0];
/// Table 66, `Range` of ICCBased: the default is 0 to 1 per component.
const DEFAULT_ICC_RANGE: [f64; 2] = [0.0, 1.0];

impl Document {
    /// Reads the colour space that `object` is or refers to.
    pub fn colour_space(&self, object: &Object) -> Result<ColourSpace, PdfError> {
        let mut diagnostics = Diagnostics::new();
        let result = self.read_colour_space(object, 0, &mut diagnostics);
        self.record_diagnostics(diagnostics);
        result
    }

    /// The default spaces (8.6.5.6) of `resources`, a resource dictionary.
    /// A default that cannot be read, or whose component count is not the
    /// device space's, is [`DiagnosticKind::IgnoredEntry`] and left out.
    pub fn default_colour_spaces(&self, resources: &Dictionary) -> Result<DefaultSpaces, PdfError> {
        let mut defaults = DefaultSpaces::default();
        let Some(spaces) = self.get(resources, b"ColorSpace")?.dictionary().cloned() else {
            return Ok(defaults);
        };
        let mut diagnostics = Diagnostics::new();
        for (key, device, slot) in [
            (&b"DefaultGray"[..], 1, &mut defaults.gray),
            (&b"DefaultRGB"[..], 3, &mut defaults.rgb),
            (&b"DefaultCMYK"[..], 4, &mut defaults.cmyk),
        ] {
            let Some(entry) = spaces.get(key) else {
                continue;
            };
            match self.read_colour_space(entry, 0, &mut diagnostics) {
                Ok(space) if space.components() == device => *slot = Some(space),
                Ok(_) | Err(PdfError::InvalidEntry { .. }) => {
                    diagnostics.push(
                        DiagnosticKind::IgnoredEntry {
                            key: Name::new(key.to_vec()),
                        },
                        0,
                    );
                }
                Err(error) => return Err(error),
            }
        }
        self.record_diagnostics(diagnostics);
        Ok(defaults)
    }

    pub(crate) fn read_colour_space(
        &self,
        object: &Object,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<ColourSpace, PdfError> {
        if depth >= MAX_COLOUR_SPACE_DEPTH {
            return Err(PdfError::LimitExceeded {
                offset: 0,
                limit: Limit::ColourSpaceNesting,
            });
        }
        let resolved = self.resolve(object)?;
        let id = resolved.id();
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        match resolved.value() {
            Value::Name(name) => device_name(name).ok_or_else(|| invalid("ColorSpace")),
            Value::Array(items) => {
                let family = match items.first() {
                    Some(first) => match self.resolve(first)?.value() {
                        Value::Name(name) => name.clone(),
                        _ => return Err(invalid("ColorSpace")),
                    },
                    None => return Err(invalid("ColorSpace")),
                };
                let items = items.to_vec();
                self.read_array_space(&family, &items, id, depth, diagnostics)
            }
            _ => Err(invalid("ColorSpace")),
        }
    }

    fn read_array_space(
        &self,
        family: &Name,
        items: &[Object],
        id: Option<ObjectId>,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<ColourSpace, PdfError> {
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        diagnostics.set_object(id);
        match family.as_bytes() {
            b"DeviceGray" | b"G" | b"DeviceRGB" | b"RGB" | b"DeviceCMYK" | b"CMYK" => {
                device_name(family).ok_or_else(|| invalid("ColorSpace"))
            }
            b"CalGray" => {
                let dictionary = self.space_dictionary(items, id)?;
                let white_point = self.white_point(&dictionary, id)?;
                let black_point = self.black_point(&dictionary, id)?;
                let gamma = match self.number_entry(&dictionary, b"Gamma")? {
                    Some(gamma) if gamma > 0.0 && gamma.is_finite() => gamma,
                    Some(_) => return Err(invalid("Gamma")),
                    None => DEFAULT_GAMMA,
                };
                Ok(ColourSpace::CalGray(CalGray {
                    white_point,
                    black_point,
                    gamma,
                }))
            }
            b"CalRGB" => {
                let dictionary = self.space_dictionary(items, id)?;
                let white_point = self.white_point(&dictionary, id)?;
                let black_point = self.black_point(&dictionary, id)?;
                let gamma = match self.colour_numbers(id, &dictionary, "Gamma")? {
                    Some(gamma)
                        if gamma.len() == 3 && gamma.iter().all(|g| *g > 0.0 && g.is_finite()) =>
                    {
                        [gamma[0], gamma[1], gamma[2]]
                    }
                    Some(_) => return Err(invalid("Gamma")),
                    None => [DEFAULT_GAMMA; 3],
                };
                let matrix = match self.colour_numbers(id, &dictionary, "Matrix")? {
                    Some(matrix) if matrix.len() == 9 && matrix.iter().all(|m| m.is_finite()) => {
                        let mut array = [0.0; 9];
                        array.copy_from_slice(&matrix);
                        array
                    }
                    Some(_) => return Err(invalid("Matrix")),
                    None => IDENTITY_MATRIX,
                };
                Ok(ColourSpace::CalRgb(CalRgb {
                    white_point,
                    black_point,
                    gamma,
                    matrix,
                }))
            }
            b"Lab" => {
                let dictionary = self.space_dictionary(items, id)?;
                let white_point = self.white_point(&dictionary, id)?;
                let black_point = self.black_point(&dictionary, id)?;
                let range = match self.colour_numbers(id, &dictionary, "Range")? {
                    Some(range)
                        if range.len() == 4
                            && range.iter().all(|r| r.is_finite())
                            && range[0] <= range[1]
                            && range[2] <= range[3] =>
                    {
                        [range[0], range[1], range[2], range[3]]
                    }
                    Some(_) => return Err(invalid("Range")),
                    None => DEFAULT_LAB_RANGE,
                };
                Ok(ColourSpace::Lab(Lab {
                    white_point,
                    black_point,
                    range,
                }))
            }
            b"ICCBased" => self.read_icc(items, id, depth, diagnostics),
            b"Indexed" | b"I" => self.read_indexed(items, id, depth, diagnostics),
            b"Separation" => {
                if items.len() != 4 {
                    return Err(invalid("ColorSpace"));
                }
                let name = match self.resolve(&items[1])?.value() {
                    Value::Name(name) => name.clone(),
                    _ => return Err(invalid("ColorSpace")),
                };
                self.read_tint(&[name], &items[2], &items[3], id, depth, diagnostics)
                    .map(ColourSpace::Separation)
            }
            b"DeviceN" => {
                if items.len() < 4 || items.len() > 5 {
                    return Err(invalid("ColorSpace"));
                }
                let names = self.colourant_names(&items[1], id)?;
                self.read_tint(&names, &items[2], &items[3], id, depth, diagnostics)
                    .map(ColourSpace::DeviceN)
            }
            b"Pattern" => match items.get(1) {
                None => Ok(ColourSpace::Pattern(None)),
                Some(under) => {
                    let under = self.read_colour_space(under, depth + 1, diagnostics)?;
                    if matches!(under, ColourSpace::Pattern(_)) {
                        return Err(invalid("ColorSpace"));
                    }
                    Ok(ColourSpace::Pattern(Some(Box::new(under))))
                }
            },
            _ => Err(invalid("ColorSpace")),
        }
    }

    /// The dictionary that is the second item of a CIE-based space.
    fn space_dictionary(
        &self,
        items: &[Object],
        id: Option<ObjectId>,
    ) -> Result<Dictionary, PdfError> {
        let invalid = || PdfError::InvalidEntry {
            object: id,
            key: "ColorSpace",
        };
        let item = items.get(1).ok_or_else(invalid)?;
        self.resolve(item)?
            .dictionary()
            .cloned()
            .ok_or_else(invalid)
    }

    fn number_entry(&self, dictionary: &Dictionary, key: &[u8]) -> Result<Option<f64>, PdfError> {
        Ok(match self.get(dictionary, key)?.value() {
            Value::Integer(integer) => Some(*integer as f64),
            Value::Real(real) => Some(real.get()),
            _ => None,
        })
    }

    /// `WhitePoint`, required: three numbers with `Xw` and `Zw` positive
    /// and `Yw` 1.0 (Table 63). A white point with a `Yw` other than 1.0
    /// is not rejected: its colours are scaled by it as written.
    fn white_point(
        &self,
        dictionary: &Dictionary,
        id: Option<ObjectId>,
    ) -> Result<[f64; 3], PdfError> {
        match self.colour_numbers(id, dictionary, "WhitePoint")? {
            Some(point)
                if point.len() == 3
                    && point.iter().all(|v| v.is_finite())
                    && point[0] > 0.0
                    && point[1] > 0.0
                    && point[2] > 0.0 =>
            {
                Ok([point[0], point[1], point[2]])
            }
            _ => Err(PdfError::InvalidEntry {
                object: id,
                key: "WhitePoint",
            }),
        }
    }

    fn black_point(
        &self,
        dictionary: &Dictionary,
        id: Option<ObjectId>,
    ) -> Result<[f64; 3], PdfError> {
        Ok(match self.colour_numbers(id, dictionary, "BlackPoint")? {
            Some(point) if point.len() == 3 && point.iter().all(|v| v.is_finite()) => {
                [point[0], point[1], point[2]]
            }
            Some(_) => {
                return Err(PdfError::InvalidEntry {
                    object: id,
                    key: "BlackPoint",
                })
            }
            None => DEFAULT_BLACK_POINT,
        })
    }

    fn read_icc(
        &self,
        items: &[Object],
        id: Option<ObjectId>,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<ColourSpace, PdfError> {
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        let Some(Value::Reference(stream_id)) = items.get(1).map(Object::value) else {
            return Err(invalid("ColorSpace"));
        };
        let stream_id = *stream_id;
        let object = self.required(stream_id)?;
        let dictionary = object
            .dictionary()
            .cloned()
            .ok_or(PdfError::WrongKind { object: stream_id })?;
        let stream_invalid = |key| PdfError::InvalidEntry {
            object: Some(stream_id),
            key,
        };
        let components = match self.get(&dictionary, b"N")?.value() {
            Value::Integer(n @ (1 | 3 | 4)) => *n as usize,
            _ => return Err(stream_invalid("N")),
        };
        diagnostics.set_object(Some(stream_id));
        let range = match self.colour_pairs(Some(stream_id), &dictionary, "Range")? {
            Some(range) if range.len() == components => range,
            Some(_) => {
                diagnostics.push(
                    DiagnosticKind::IgnoredEntry {
                        key: Name::new(b"Range".to_vec()),
                    },
                    0,
                );
                vec![DEFAULT_ICC_RANGE; components]
            }
            None => vec![DEFAULT_ICC_RANGE; components],
        };
        let alternate = match dictionary.get(b"Alternate") {
            Some(entry) => match self.read_colour_space(entry, depth + 1, diagnostics) {
                Ok(space)
                    if space.components() == components
                        && !matches!(space, ColourSpace::Pattern(_)) =>
                {
                    space
                }
                Ok(_) | Err(PdfError::InvalidEntry { .. }) => {
                    diagnostics.set_object(Some(stream_id));
                    diagnostics.push(
                        DiagnosticKind::IgnoredEntry {
                            key: Name::new(b"Alternate".to_vec()),
                        },
                        0,
                    );
                    device_space(components)
                }
                Err(error) => return Err(error),
            },
            None => device_space(components),
        };
        diagnostics.set_object(Some(stream_id));
        let transform = match self.icc_profile(stream_id, &object) {
            Ok(profile) => IccTransform::new(&profile, components, range.clone()),
            Err(PdfError::Fs(error)) => return Err(PdfError::Fs(error)),
            Err(_) => None,
        };
        if transform.is_none() {
            diagnostics.push(DiagnosticKind::IccProfileUnusable, 0);
        }
        Ok(ColourSpace::IccBased(IccBased {
            components,
            range,
            alternate: Box::new(alternate),
            transform,
        }))
    }

    fn icc_profile(
        &self,
        id: ObjectId,
        object: &crate::doc::DocObject,
    ) -> Result<Vec<u8>, PdfError> {
        let stream = object.stream().ok_or(PdfError::WrongKind { object: id })?;
        Ok(self
            .decode_stream(id, stream, MAX_PROFILE_BYTES)?
            .data
            .into_owned())
    }

    fn read_indexed(
        &self,
        items: &[Object],
        id: Option<ObjectId>,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<ColourSpace, PdfError> {
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        if items.len() != 4 {
            return Err(invalid("ColorSpace"));
        }
        let base = self.read_colour_space(&items[1], depth + 1, diagnostics)?;
        if matches!(base, ColourSpace::Pattern(_) | ColourSpace::Indexed(_)) {
            return Err(invalid("ColorSpace"));
        }
        let hival = match self.resolve(&items[2])?.value() {
            Value::Integer(hival) => usize::try_from(*hival)
                .ok()
                .filter(|hival| *hival <= INDEXED_MAX_HIVAL)
                .ok_or_else(|| invalid("ColorSpace"))?,
            _ => return Err(invalid("ColorSpace")),
        };
        diagnostics.set_object(id);
        let lookup = match items[3].value() {
            Value::Reference(stream_id) => {
                let object = self.required(*stream_id)?;
                match object.stream() {
                    Some(stream) => self
                        .decode_stream(*stream_id, stream, (hival + 1) * MAX_INDEXED_ENTRY)?
                        .data
                        .into_owned(),
                    None => match object.object().map(Object::value) {
                        Some(Value::String(string)) => string.as_bytes().to_vec(),
                        _ => return Err(invalid("ColorSpace")),
                    },
                }
            }
            Value::String(string) => string.as_bytes().to_vec(),
            _ => return Err(invalid("ColorSpace")),
        };
        let size = (hival + 1) * base.components();
        let mut table = lookup;
        if table.len() < size {
            diagnostics.push(DiagnosticKind::IndexedTableShort, 0);
        }
        table.resize(size, 0);
        Ok(ColourSpace::Indexed(Indexed {
            base: Box::new(base),
            hival,
            table: Arc::from(table),
        }))
    }

    fn colourant_names(
        &self,
        object: &Object,
        id: Option<ObjectId>,
    ) -> Result<Vec<Name>, PdfError> {
        let invalid = PdfError::InvalidEntry {
            object: id,
            key: "ColorSpace",
        };
        let resolved = self.resolve(object)?;
        let items = resolved.array().ok_or(invalid)?;
        if items.is_empty() || items.len() > MAX_DEVICEN_COMPONENTS {
            return Err(PdfError::InvalidEntry {
                object: id,
                key: "ColorSpace",
            });
        }
        let mut names = Vec::with_capacity(items.len());
        for item in items {
            match self.resolve(item)?.value() {
                Value::Name(name) => names.push(name.clone()),
                _ => {
                    return Err(PdfError::InvalidEntry {
                        object: id,
                        key: "ColorSpace",
                    })
                }
            }
        }
        Ok(names)
    }

    fn read_tint(
        &self,
        names: &[Name],
        alternate: &Object,
        transform: &Object,
        id: Option<ObjectId>,
        depth: usize,
        diagnostics: &mut Diagnostics,
    ) -> Result<Tint, PdfError> {
        let invalid = |key| PdfError::InvalidEntry { object: id, key };
        let alternate = self.read_colour_space(alternate, depth + 1, diagnostics)?;
        if matches!(alternate, ColourSpace::Pattern(_)) {
            return Err(invalid("ColorSpace"));
        }
        let function = self.read_function(transform, 0, diagnostics)?;
        diagnostics.set_object(id);
        if function.inputs() != names.len() || function.outputs() != alternate.components() {
            return Err(invalid("ColorSpace"));
        }
        Ok(Tint {
            names: names.to_vec(),
            alternate: Box::new(alternate),
            transform: Arc::new(function),
        })
    }
}

/// The device space of 1, 3 or 4 components.
fn device_space(components: usize) -> ColourSpace {
    match components {
        1 => ColourSpace::DeviceGray,
        3 => ColourSpace::DeviceRgb,
        _ => ColourSpace::DeviceCmyk,
    }
}

/// The device space (or the Pattern space) a name stands for; the short
/// names of Table 94 are the ones inline images use.
fn device_name(name: &Name) -> Option<ColourSpace> {
    Some(match name.as_bytes() {
        b"DeviceGray" | b"G" => ColourSpace::DeviceGray,
        b"DeviceRGB" | b"RGB" => ColourSpace::DeviceRgb,
        b"DeviceCMYK" | b"CMYK" => ColourSpace::DeviceCmyk,
        b"Pattern" => ColourSpace::Pattern(None),
        _ => return None,
    })
}

/// The bytes one Indexed table entry can take: four components of one
/// byte each, the most a base space but DeviceN has. A stream longer than
/// this for its `hival` is cut here.
const MAX_INDEXED_ENTRY: usize = MAX_DEVICEN_COMPONENTS;
