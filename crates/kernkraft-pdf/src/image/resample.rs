// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Resampling of pixel planes: the nearest sample, to bring a mask and the
//! picture it masks to one size (8.9.6.3 lets them differ), and the mean of
//! a block, to bring a picture down to about the size it is painted at.

/// `src`, `width` by `height` pixels of `channels` bytes, as a plane of
/// `new_width` by `new_height`, each pixel the one whose area holds its
/// centre.
pub(super) fn nearest(
    src: &[u8],
    width: u32,
    height: u32,
    channels: usize,
    new_width: u32,
    new_height: u32,
) -> Vec<u8> {
    if (width, height) == (new_width, new_height) {
        return src.to_vec();
    }
    let mut out = Vec::with_capacity(new_width as usize * new_height as usize * channels);
    // The source column whose area holds the centre of destination column
    // `x`: floor((x + 1/2) * width / new_width), in integers.
    let column = |x: u64| ((2 * x + 1) * u64::from(width) / (2 * u64::from(new_width))) as usize;
    let row = |y: u64| ((2 * y + 1) * u64::from(height) / (2 * u64::from(new_height))) as usize;
    for y in 0..u64::from(new_height) {
        let source_row = row(y).min(height as usize - 1);
        for x in 0..u64::from(new_width) {
            let source =
                (source_row * width as usize + column(x).min(width as usize - 1)) * channels;
            out.extend_from_slice(&src[source..source + channels]);
        }
    }
    out
}

/// Straight RGBA brought down by `factor_x` across and `factor_y` down:
/// each new pixel is the mean of the block it covers, weighted by alpha so
/// that transparent pixels do not darken the colour. The last block of a
/// row or column may be smaller.
pub(crate) fn box_down(
    rgba: &[u8],
    width: u32,
    height: u32,
    factor_x: u32,
    factor_y: u32,
) -> (Vec<u8>, u32, u32) {
    let (new_width, new_height) = (width.div_ceil(factor_x), height.div_ceil(factor_y));
    let mut out = Vec::with_capacity(new_width as usize * new_height as usize * 4);
    for block_y in 0..new_height {
        let (y0, y1) = (block_y * factor_y, ((block_y + 1) * factor_y).min(height));
        for block_x in 0..new_width {
            let (x0, x1) = (block_x * factor_x, ((block_x + 1) * factor_x).min(width));
            let (mut weighted, mut alpha_sum, mut count) = ([0u64; 3], 0u64, 0u64);
            for y in y0..y1 {
                for x in x0..x1 {
                    let at = (y as usize * width as usize + x as usize) * 4;
                    let alpha = u64::from(rgba[at + 3]);
                    for c in 0..3 {
                        weighted[c] += u64::from(rgba[at + c]) * alpha;
                    }
                    alpha_sum += alpha;
                    count += 1;
                }
            }
            // A block with no alpha has no colour: it is all zero.
            for sum in weighted {
                out.push((sum + alpha_sum / 2).checked_div(alpha_sum).unwrap_or(0) as u8);
            }
            out.push((alpha_sum + count / 2).checked_div(count).unwrap_or(0) as u8);
        }
    }
    (out, new_width, new_height)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]

    use super::*;

    #[test]
    fn nearest_doubles_each_sample() {
        // 2 by 1 to 4 by 2: every source pixel covers two columns and two rows.
        let out = nearest(&[10, 20], 2, 1, 1, 4, 2);
        assert_eq!(out, [10, 10, 20, 20, 10, 10, 20, 20]);
    }

    #[test]
    fn nearest_takes_the_centre_when_shrinking() {
        // 4 to 2: destination 0 has its centre at source 1 (area [1, 2)),
        // destination 1 at source 3.
        let out = nearest(&[1, 2, 3, 4], 4, 1, 1, 2, 1);
        assert_eq!(out, [2, 4]);
    }

    #[test]
    fn nearest_keeps_the_channels_of_a_pixel_together() {
        let out = nearest(&[1, 2, 3, 4, 5, 6], 2, 1, 3, 3, 1);
        assert_eq!(out, [1, 2, 3, 4, 5, 6, 4, 5, 6]);
    }

    #[test]
    fn the_same_size_is_a_copy() {
        assert_eq!(nearest(&[1, 2, 3, 4], 2, 2, 1, 2, 2), [1, 2, 3, 4]);
    }

    #[test]
    fn box_down_averages_opaque_blocks() {
        let rgba = [
            0, 0, 0, 255, 100, 100, 100, 255, 200, 200, 200, 255, 100, 100, 100, 255,
        ];
        let (out, w, h) = box_down(&rgba, 2, 2, 2, 2);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [100, 100, 100, 255]);
    }

    #[test]
    fn box_down_weights_colour_by_alpha() {
        // A white opaque pixel beside a black transparent one: the mean
        // colour is white, at half alpha.
        let rgba = [255, 255, 255, 255, 0, 0, 0, 0];
        let (out, w, h) = box_down(&rgba, 2, 1, 2, 1);
        assert_eq!((w, h), (1, 1));
        assert_eq!(out, [255, 255, 255, 128]);
    }

    #[test]
    fn box_down_handles_a_short_last_block() {
        let rgba: Vec<u8> = (0..3).flat_map(|v| [v * 10, v * 10, v * 10, 255]).collect();
        let (out, w, h) = box_down(&rgba, 3, 1, 2, 1);
        assert_eq!((w, h), (2, 1));
        assert_eq!(out, [5, 5, 5, 255, 20, 20, 20, 255]);
    }
}
