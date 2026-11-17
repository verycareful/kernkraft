// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! ICCBased colour spaces: ISO 32000-1:2008, 8.6.5.5, with the profile
//! format of ICC.1:2010. The profile is read and its transform to sRGB is
//! built by `moxcms`; this module checks that the profile fits the `N`
//! the stream dictionary states and hands it components normalised to the
//! `Range`.

use std::fmt;
use std::sync::Arc;

use moxcms::{ColorProfile, DataColorSpace, Layout, RenderingIntent, TransformOptions};

/// The most bytes of a decoded profile. `moxcms` refuses a larger one
/// (its limit is 10 MiB, which fits the large CMYK profiles of print
/// standards).
pub(crate) const MAX_PROFILE_BYTES: usize = 10 << 20;

/// An ICC profile's transform from its colour space to sRGB.
#[derive(Clone)]
pub struct IccTransform {
    components: usize,
    /// `[min, max]` of each component, which the colour is normalised from.
    range: Vec<[f64; 2]>,
    executor: Arc<moxcms::TransformF32Executor>,
}

impl fmt::Debug for IccTransform {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("IccTransform")
            .field("components", &self.components)
            .finish_non_exhaustive()
    }
}

impl IccTransform {
    /// The transform of `profile` for a stream stating `components`
    /// components in `range`. `None` when the profile cannot be read, its
    /// data colour space does not have that many components, or no
    /// transform to sRGB can be built.
    pub(crate) fn new(profile: &[u8], components: usize, range: Vec<[f64; 2]>) -> Option<Self> {
        let source = ColorProfile::new_from_slice(profile).ok()?;
        let layout = match (components, source.color_space) {
            (1, DataColorSpace::Gray) => Layout::Gray,
            (3, DataColorSpace::Rgb) => Layout::Rgb,
            // Four-component CMYK data uses the layout of four-channel RGBA.
            (4, DataColorSpace::Cmyk) => Layout::Rgba,
            _ => return None,
        };
        // 8.6.5.8: the default rendering intent is relative colorimetric.
        let options = TransformOptions {
            rendering_intent: RenderingIntent::RelativeColorimetric,
            ..TransformOptions::default()
        };
        let executor = source
            .create_transform_f32(layout, &ColorProfile::new_srgb(), Layout::Rgb, options)
            .ok()?;
        Some(Self {
            components,
            range,
            executor,
        })
    }

    /// The sRGB of each colour in `colours` (components one after the
    /// other, `components` to a colour), appended to `out`. `None` when
    /// the transform fails, which it does not for whole colours.
    pub(crate) fn convert(&self, colours: &[f64], out: &mut Vec<[f64; 3]>) -> Option<()> {
        if !colours.len().is_multiple_of(self.components) {
            return None;
        }
        let count = colours.len() / self.components;
        let mut source = Vec::with_capacity(colours.len());
        for colour in colours.chunks_exact(self.components) {
            for (value, [low, high]) in colour.iter().zip(&self.range) {
                let width = high - low;
                let unit = if width == 0.0 {
                    0.0
                } else {
                    (value - low) / width
                };
                source.push(unit.clamp(0.0, 1.0) as f32);
            }
        }
        let mut destination = vec![0.0f32; count * 3];
        self.executor.transform(&source, &mut destination).ok()?;
        out.extend(
            destination
                .as_chunks::<3>()
                .0
                .iter()
                .map(|rgb| rgb.map(|channel| f64::from(channel).clamp(0.0, 1.0))),
        );
        Some(())
    }
}
