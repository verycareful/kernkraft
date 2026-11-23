// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Sample data and its conversion to colour: ISO 32000-1:2008, 8.9.3
//! (sample representation), 8.9.5.2 (Decode arrays, Table 90) and 8.9.6.4
//! (colour key masking).
//!
//! Samples are packed `bits` to a component, the components of a pixel
//! together, the pixels of a row together, and each row padded to a whole
//! byte (8.9.3). A sample `s` of `bits` bits becomes the value
//! `Dmin + s * (Dmax - Dmin) / (2^bits - 1)` (8.9.5.2, Decode arrays), a
//! colour component in the space's own range.

use crate::colour::ColourSpace;
use crate::PdfError;

const BITS_PER_BYTE: usize = 8;
/// The pixels converted in one pass, which bounds the memory a conversion
/// through a profile or a function holds at once.
const CHUNK_PIXELS: usize = 1 << 16;
/// Samples wider than a byte are looked up by their high byte alone when
/// the space has one component: a result is 8 bits whatever the sample.
const LOOKUP_BITS: u32 = 8;
const OPAQUE: u8 = u8::MAX;
const TRANSPARENT: u8 = 0;

/// The samples of an image.
#[derive(Debug, Clone)]
pub(super) struct Packed {
    pub width: u32,
    pub height: u32,
    pub components: usize,
    pub bits: u32,
    /// `height` rows of `row_bytes()` bytes.
    pub data: Vec<u8>,
}

impl Packed {
    /// The bytes of one padded row.
    pub fn row_bytes(width: u32, components: usize, bits: u32) -> usize {
        (width as usize * components * bits as usize).div_ceil(BITS_PER_BYTE)
    }

    /// A sample set of the stated size with `data`, cut or padded with
    /// zeros to the bytes the size calls for. The flag says data was
    /// short.
    pub fn new(
        width: u32,
        height: u32,
        components: usize,
        bits: u32,
        mut data: Vec<u8>,
    ) -> (Self, bool) {
        let wanted = Self::row_bytes(width, components, bits) * height as usize;
        let short = data.len() < wanted;
        data.resize(wanted, 0);
        (
            Self {
                width,
                height,
                components,
                bits,
                data,
            },
            short,
        )
    }

    pub fn rows(&self) -> impl Iterator<Item = &[u8]> {
        let row_bytes = Self::row_bytes(self.width, self.components, self.bits).max(1);
        self.data.chunks_exact(row_bytes)
    }

    /// The largest sample.
    pub fn max_sample(&self) -> u32 {
        // `bits` is at most 16.
        (1u32 << self.bits) - 1
    }
}

/// Sample number `index` of a padded row of `bits`-bit samples.
pub(super) fn sample(row: &[u8], index: usize, bits: u32) -> u32 {
    match bits {
        8 => u32::from(row[index]),
        16 => u32::from(u16::from_be_bytes([row[index * 2], row[index * 2 + 1]])),
        _ => {
            let bit = index * bits as usize;
            let shift = BITS_PER_BYTE - bits as usize - bit % BITS_PER_BYTE;
            u32::from(row[bit / BITS_PER_BYTE] >> shift) & ((1 << bits) - 1)
        }
    }
}

/// The `Decode` array as `[Dmin, Dmax]` pairs: the one the dictionary gave
/// if it has a pair for each component, else the default of the space
/// (Table 90). The flag is true when a given array was unusable.
pub(super) fn decode_pairs(
    given: Option<&[f64]>,
    space: &ColourSpace,
    bits: u32,
) -> (Vec<[f64; 2]>, bool) {
    let components = space.components();
    if let Some(given) = given {
        if given.len() == components * 2 {
            return (given.as_chunks::<2>().0.to_vec(), false);
        }
    }
    (space.default_decode(bits), given.is_some())
}

/// One sample as the colour component it stands for.
fn component(sample: u32, max: f64, pair: [f64; 2]) -> f64 {
    pair[0] + f64::from(sample) * (pair[1] - pair[0]) / max
}

/// The colour key (8.9.6.4): a pixel all of whose samples lie in the
/// `[min, max]` ranges is transparent.
pub(super) type ColourKey = Vec<[u32; 2]>;

/// The pixels as straight RGBA, 8 bits to a channel. Opaque unless the
/// colour key matches.
pub(super) fn to_rgba(
    packed: &Packed,
    space: &ColourSpace,
    pairs: &[[f64; 2]],
    key: Option<&ColourKey>,
) -> Result<Vec<u8>, PdfError> {
    let pixels = packed.width as usize * packed.height as usize;
    let mut rgba = vec![OPAQUE; pixels * 4];
    let components = packed.components;
    let max = f64::from(packed.max_sample());

    if components == 1 {
        let lookup = lookup_table(space, packed.bits, pairs[0])?;
        let shift = packed.bits.saturating_sub(LOOKUP_BITS);
        let mut written = 0;
        for row in packed.rows() {
            for x in 0..packed.width as usize {
                let sample = sample(row, x, packed.bits);
                let pixel = &mut rgba[written * 4..written * 4 + 4];
                pixel[..3].copy_from_slice(&lookup[(sample >> shift) as usize]);
                pixel[3] = alpha_of(key, &[sample]);
                written += 1;
            }
        }
        return Ok(rgba);
    }

    let straight_rgb = matches!(space, ColourSpace::DeviceRgb)
        && packed.bits == 8
        && pairs.iter().all(|pair| *pair == [0.0, 1.0]);
    let mut written = 0;
    let mut colours: Vec<f64> = Vec::new();
    let mut raw: Vec<u32> = Vec::new();
    let mut converted = Vec::new();
    // Converts the pixels gathered so far and writes them from pixel
    // `written` on.
    let mut flush = |colours: &mut Vec<f64>,
                     raw: &mut Vec<u32>,
                     rgba: &mut [u8],
                     written: &mut usize|
     -> Result<(), PdfError> {
        converted.clear();
        space.to_rgb_many(colours, &mut converted)?;
        for (rgb, samples) in converted.iter().zip(raw.chunks_exact(components)) {
            let pixel = &mut rgba[*written * 4..*written * 4 + 4];
            pixel[..3].copy_from_slice(&rgb.to_bytes());
            pixel[3] = alpha_of(key, samples);
            *written += 1;
        }
        colours.clear();
        raw.clear();
        Ok(())
    };
    for row in packed.rows() {
        if straight_rgb {
            for x in 0..packed.width as usize {
                let samples = [
                    u32::from(row[x * 3]),
                    u32::from(row[x * 3 + 1]),
                    u32::from(row[x * 3 + 2]),
                ];
                let pixel = &mut rgba[written * 4..written * 4 + 4];
                pixel[..3].copy_from_slice(&row[x * 3..x * 3 + 3]);
                pixel[3] = alpha_of(key, &samples);
                written += 1;
            }
            continue;
        }
        for x in 0..packed.width as usize {
            for (c, pair) in pairs.iter().enumerate().take(components) {
                let s = sample(row, x * components + c, packed.bits);
                raw.push(s);
                colours.push(component(s, max, *pair));
            }
            if raw.len() >= CHUNK_PIXELS * components {
                flush(&mut colours, &mut raw, &mut rgba, &mut written)?;
            }
        }
    }
    if !raw.is_empty() {
        flush(&mut colours, &mut raw, &mut rgba, &mut written)?;
    }
    Ok(rgba)
}

/// The sRGB of each value a one-component sample can take, indexed by the
/// sample (by its high byte for samples of more than 8 bits).
fn lookup_table(space: &ColourSpace, bits: u32, pair: [f64; 2]) -> Result<Vec<[u8; 3]>, PdfError> {
    let bits = bits.min(LOOKUP_BITS);
    let steps = 1usize << bits;
    let top = (steps - 1) as f64;
    (0..steps)
        .map(|sample| {
            let value = pair[0] + sample as f64 * (pair[1] - pair[0]) / top;
            space.to_rgb(&[value]).map(|rgb| rgb.to_bytes())
        })
        .collect()
}

fn alpha_of(key: Option<&ColourKey>, samples: &[u32]) -> u8 {
    match key {
        Some(key)
            if key.len() == samples.len()
                && samples
                    .iter()
                    .zip(key)
                    .all(|(sample, [low, high])| (low..=high).contains(&sample)) =>
        {
            TRANSPARENT
        }
        _ => OPAQUE,
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn samples_unpack_from_the_high_bits_down() {
        let row = [0b1011_0010, 0b0100_0000];
        let one: Vec<u32> = (0..10).map(|i| sample(&row, i, 1)).collect();
        assert_eq!(one, [1, 0, 1, 1, 0, 0, 1, 0, 0, 1]);
        let two: Vec<u32> = (0..8).map(|i| sample(&row, i, 2)).collect();
        assert_eq!(two, [2, 3, 0, 2, 1, 0, 0, 0]);
        let four: Vec<u32> = (0..4).map(|i| sample(&row, i, 4)).collect();
        assert_eq!(four, [0xB, 0x2, 0x4, 0x0]);
        assert_eq!(sample(&row, 1, 8), 0x40);
        assert_eq!(sample(&row, 0, 16), 0xB240);
    }

    #[test]
    fn rows_are_padded_to_whole_bytes() {
        assert_eq!(Packed::row_bytes(10, 1, 1), 2);
        assert_eq!(Packed::row_bytes(3, 3, 4), 5);
        assert_eq!(Packed::row_bytes(3, 3, 8), 9);
        assert_eq!(Packed::row_bytes(5, 1, 16), 10);
    }

    #[test]
    fn short_data_is_padded_with_zeros_and_said_to_be_short() {
        let (packed, short) = Packed::new(4, 2, 1, 8, vec![1, 2, 3]);
        assert!(short);
        assert_eq!(packed.data, [1, 2, 3, 0, 0, 0, 0, 0]);
        let (packed, short) = Packed::new(2, 1, 1, 8, vec![1, 2, 3]);
        assert!(!short);
        assert_eq!(packed.data, [1, 2]);
    }

    #[test]
    fn gray_samples_map_through_the_decode_array() {
        let (packed, _) = Packed::new(2, 1, 1, 8, vec![0, 255]);
        let space = ColourSpace::DeviceGray;
        let normal = to_rgba(&packed, &space, &[[0.0, 1.0]], None).unwrap();
        assert_eq!(normal, [0, 0, 0, 255, 255, 255, 255, 255]);
        let inverted = to_rgba(&packed, &space, &[[1.0, 0.0]], None).unwrap();
        assert_eq!(inverted, [255, 255, 255, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn one_bit_gray_has_two_values() {
        let (packed, _) = Packed::new(3, 1, 1, 1, vec![0b0100_0000]);
        let rgba = to_rgba(&packed, &ColourSpace::DeviceGray, &[[0.0, 1.0]], None).unwrap();
        assert_eq!(rgba, [0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn rgb_passes_through_and_the_colour_key_makes_pixels_transparent() {
        let data = vec![10, 20, 30, 200, 210, 220];
        let (packed, _) = Packed::new(2, 1, 3, 8, data);
        let key: ColourKey = vec![[200, 255], [200, 255], [0, 255]];
        let rgba = to_rgba(
            &packed,
            &ColourSpace::DeviceRgb,
            &[[0.0, 1.0]; 3],
            Some(&key),
        )
        .unwrap();
        assert_eq!(rgba, [10, 20, 30, 255, 200, 210, 220, 0]);
    }

    #[test]
    fn cmyk_goes_through_the_space() {
        // No ink is the paper, white, and full black ink is dark.
        let (packed, _) = Packed::new(2, 1, 4, 8, vec![0, 0, 0, 0, 0, 0, 0, 255]);
        let space = ColourSpace::DeviceCmyk(crate::colour::CmykProfile::bundled().unwrap());
        let rgba = to_rgba(&packed, &space, &[[0.0, 1.0]; 4], None).unwrap();
        assert!(rgba[..3].iter().all(|level| *level >= 250), "{rgba:?}");
        assert!(rgba[4..7].iter().all(|level| *level <= 64), "{rgba:?}");
        assert_eq!([rgba[3], rgba[7]], [255, 255]);
    }

    #[test]
    fn a_decode_array_of_the_wrong_length_is_ignored_and_said_so() {
        let space = ColourSpace::DeviceRgb;
        let (pairs, ignored) = decode_pairs(Some(&[1.0, 0.0]), &space, 8);
        assert!(ignored);
        assert_eq!(pairs, vec![[0.0, 1.0]; 3]);
        let (pairs, ignored) = decode_pairs(Some(&[1.0, 0.0, 1.0, 0.0, 1.0, 0.0]), &space, 8);
        assert!(!ignored);
        assert_eq!(pairs, vec![[1.0, 0.0]; 3]);
        let (_, ignored) = decode_pairs(None, &space, 8);
        assert!(!ignored);
    }
}
