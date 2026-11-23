// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Lines, blocks and reading order from placed glyphs. The format fixes
//! none of this: a page is a set of glyphs, and ISO 32000-1:2008, 14.8.2.3
//! (page content order) only says that a reader recovers a reading order
//! from the content or from the structure tree. The rules here are
//! geometric, each measured in ems of the glyphs it concerns so that it
//! holds at any font size and page scale.
//!
//! A line is glyphs that share a baseline and a direction, in order along
//! it. A block is lines stacked one below the next with no more room
//! between than a paragraph's leading. Blocks are read in the order of the
//! structure tree when the page is tagged (see [`super::structure`]), and
//! by recursive cuts at the widest gap otherwise.

use crate::display::PageView;
use crate::text::bidi;
use crate::text::collect::{RawGlyph, RawRule};
use crate::text::{TextBlock, TextChar, TextLine};

/// The sine of the angle two glyphs' directions may differ by and still
/// be of one line: 2 degrees, the slant of a scanned page's text lines.
const DIRECTION_TOLERANCE: f64 = 0.035;

/// How far apart, across the line, two baselines may be and still be one
/// line, in ems: a superscript or a subscript sits up to about a third of
/// an em from its line (9.3.7 puts `Trise` there), and the lines of any
/// text are at least an em apart. Glyphs of one size are chained by this
/// many ems of that size; a cluster of smaller glyphs is a script of a
/// larger one within this many ems of the larger size (see
/// [`attach_scripts`]).
const BASELINE_TOLERANCE: f64 = 0.5;

/// The gap along a line, in ems, past which a space is read between two
/// glyphs that no space glyph separates. A space glyph is a quarter of an
/// em or more in the Core 14 fonts and justification shrinks one by a third
/// at most, while kerning within a word stays well under a tenth.
const SPACE_GAP: f64 = 0.1;

/// The gap along a line, in ems, past which the glyphs are two lines: the
/// columns of a page and the cells of a table are further apart than the
/// stretch of a justified space.
const MAX_JOIN_GAP: f64 = 1.0;

/// How near, along the line and across it, in ems, two glyphs of the same
/// text are to be one glyph drawn twice (a bold made by overprinting). Across
/// it matters: the dots of a vertical ellipsis share a position along the
/// line and are a few points apart across it.
const SAME_POSITION: f64 = 0.1;

/// The most a bar may be thick, in ems of the smaller of the glyphs it
/// parts, to be a fraction bar and not a shape: a bar is a hairline (TeX
/// draws one at 0.4 points under text of 7 to 10 points), and a quarter of
/// an em is far past any line a writer draws as one.
const BAR_THICKNESS: f64 = 0.25;

/// The most room, in ems of the larger line, between the baselines of
/// two lines of one block: the leading of body text is 1.2 ems and a
/// paragraph break is one em more or less.
const MAX_LEADING: f64 = 1.7;

/// The least room between baselines for two lines to be one above the
/// other and not side by side on one baseline, in ems.
const MIN_LEADING: f64 = 0.5;

/// The most two lines' sizes may differ by, as a ratio, and be of one
/// block: a heading is not part of the paragraph that follows it.
const MAX_SIZE_RATIO: f64 = 1.25;

/// What came of laying a page out.
pub(super) struct Laid {
    pub blocks: Vec<TextBlock>,
    /// Whether the structure tree gave the order of any block.
    pub tagged: bool,
}

/// A line while the page is laid out.
struct Line {
    /// The glyphs, in order along the line, each with whether a space
    /// precedes it.
    glyphs: Vec<(usize, bool)>,
    group: usize,
    block_key: Option<usize>,
    /// Where the baseline lies across the line, along the line, and the
    /// size of the text.
    perp: f64,
    start: f64,
    end: f64,
    size: f64,
    order: Option<usize>,
    bounds: Bounds,
}

/// A box on the displayed page, in points.
#[derive(Debug, Clone, Copy)]
struct Bounds {
    left: f64,
    top: f64,
    right: f64,
    bottom: f64,
}

impl Bounds {
    fn of(points: impl Iterator<Item = (f64, f64)>) -> Option<Bounds> {
        let mut bounds: Option<Bounds> = None;
        for (x, y) in points {
            bounds = Some(match bounds {
                None => Bounds {
                    left: x,
                    top: y,
                    right: x,
                    bottom: y,
                },
                Some(b) => Bounds {
                    left: b.left.min(x),
                    top: b.top.min(y),
                    right: b.right.max(x),
                    bottom: b.bottom.max(y),
                },
            });
        }
        bounds
    }

    fn union(&self, other: &Bounds) -> Bounds {
        Bounds {
            left: self.left.min(other.left),
            top: self.top.min(other.top),
            right: self.right.max(other.right),
            bottom: self.bottom.max(other.bottom),
        }
    }
}

fn dot(a: (f64, f64), b: (f64, f64)) -> f64 {
    a.0 * b.0 + a.1 * b.1
}

/// The component of `a` across `direction`: it grows towards the top of
/// the page for text that flows left to right, so that the lines of a
/// block follow one another in decreasing order.
fn cross(a: (f64, f64), direction: (f64, f64)) -> f64 {
    a.0 * direction.1 - a.1 * direction.0
}

/// A rule parallel to a group's text, as its measures along and across the
/// group's direction.
struct Bar {
    perp: f64,
    from: f64,
    to: f64,
    thickness: f64,
}

/// The rules parallel to `direction`, ordered by place across it.
fn bars(rules: &[RawRule], direction: (f64, f64)) -> Vec<Bar> {
    let mut bars: Vec<Bar> = rules
        .iter()
        .filter_map(|rule| {
            let along = (rule.to.0 - rule.from.0, rule.to.1 - rule.from.1);
            let length = along.0.hypot(along.1);
            if cross(along, direction).abs() > DIRECTION_TOLERANCE * length {
                return None;
            }
            let (a, b) = (dot(rule.from, direction), dot(rule.to, direction));
            let middle = (
                (rule.from.0 + rule.to.0) / 2.0,
                (rule.from.1 + rule.to.1) / 2.0,
            );
            Some(Bar {
                perp: cross(middle, direction),
                from: a.min(b),
                to: a.max(b),
                thickness: rule.thickness,
            })
        })
        .collect();
    bars.sort_by(|a, b| a.perp.total_cmp(&b.perp));
    bars
}

/// Lays `glyphs` out.
pub(super) fn lay_out(glyphs: &[RawGlyph], rules: &[RawRule], view: &PageView) -> Laid {
    let lines = lines(glyphs, rules);
    let blocks = blocks(&lines);
    let order = reading_order(&lines, &blocks);
    let tagged = blocks
        .iter()
        .any(|block| block.iter().any(|&line| lines[line].order.is_some()));
    let blocks = order
        .into_iter()
        .map(|block| build_block(glyphs, &lines, &blocks[block], view))
        .collect();
    Laid { blocks, tagged }
}

/// The lines of the page, unordered.
fn lines(glyphs: &[RawGlyph], rules: &[RawRule]) -> Vec<Line> {
    // Glyphs flowing the same way share a group; the first glyph of a
    // group gives the direction the group's measures are taken in.
    let mut groups: Vec<(f64, f64)> = Vec::new();
    let mut group_of = Vec::with_capacity(glyphs.len());
    for glyph in glyphs {
        let found = groups.iter().position(|&direction| {
            dot(direction, glyph.dir) > 0.0
                && cross(glyph.dir, direction).abs() <= DIRECTION_TOLERANCE
        });
        group_of.push(match found {
            Some(group) => group,
            None => {
                groups.push(glyph.dir);
                groups.len() - 1
            }
        });
    }
    let perp: Vec<f64> = glyphs
        .iter()
        .zip(&group_of)
        .map(|(glyph, &group)| cross(glyph.origin, groups[group]))
        .collect();
    let start: Vec<f64> = glyphs
        .iter()
        .zip(&group_of)
        .map(|(glyph, &group)| dot(glyph.origin, groups[group]))
        .collect();

    // Within a group, glyphs of different block-level elements are never
    // of one line.
    let mut order: Vec<usize> = (0..glyphs.len()).collect();
    let key = |glyph: usize| glyphs[glyph].slot.map(|slot| slot.block);
    order.sort_by(|&a, &b| {
        group_of[a]
            .cmp(&group_of[b])
            .then(key(a).cmp(&key(b)))
            .then(perp[a].total_cmp(&perp[b]))
            .then(a.cmp(&b))
    });

    // Glyphs are chained by baseline within the em of the smaller of two,
    // so that a glyph larger than the text around it, set between two lines,
    // does not carry the two into one cluster.
    let mut clusters: Vec<Vec<usize>> = Vec::new();
    for &glyph in &order {
        let continues = clusters
            .last()
            .and_then(|cluster| cluster.last())
            .is_some_and(|&last| {
                group_of[last] == group_of[glyph]
                    && key(last) == key(glyph)
                    && perp[glyph] - perp[last]
                        <= BASELINE_TOLERANCE * glyphs[last].size.min(glyphs[glyph].size)
            });
        match clusters.last_mut() {
            Some(cluster) if continues => cluster.push(glyph),
            _ => clusters.push(vec![glyph]),
        }
    }
    let bars: Vec<Vec<Bar>> = groups
        .iter()
        .map(|&direction| bars(rules, direction))
        .collect();
    // Whether a bar lies between two clusters and reaches the first: the
    // fraction bar between a numerator and what is below it.
    let parted = |script: &[usize], other: &[usize]| {
        let (&a, &b) = (&script[script.len() / 2], &other[other.len() / 2]);
        let (low, high) = (perp[a].min(perp[b]), perp[a].max(perp[b]));
        let along = |cluster: &[usize]| {
            cluster
                .iter()
                .fold((f64::INFINITY, f64::NEG_INFINITY), |(from, to), &glyph| {
                    (
                        from.min(start[glyph]),
                        to.max(start[glyph] + glyphs[glyph].extent),
                    )
                })
        };
        let (from, to) = along(script);
        let size = glyphs[a].size.min(glyphs[b].size);
        let group = &bars[group_of[a]];
        let first = group.partition_point(|bar| bar.perp <= low);
        group[first..]
            .iter()
            .take_while(|bar| bar.perp < high)
            .any(|bar| bar.thickness <= BAR_THICKNESS * size && bar.from < to && bar.to > from)
    };
    let mut lines = Vec::new();
    for cluster in attach_scripts(clusters, glyphs, &group_of, &perp, &key, &parted) {
        split_cluster(&cluster, glyphs, &group_of, &perp, &start, &mut lines);
    }
    lines
}

/// Merges each cluster that is a script of another into it.
///
/// A cluster is a script of the cluster whose size is larger, that has no
/// fewer glyphs, and whose baseline is within [`BASELINE_TOLERANCE`] of the
/// larger size from its own (the nearest such, the lowest first on a tie).
/// A superscript or a subscript is smaller than its base and is a few glyphs
/// beside many, and is not parted from it by a bar: a numerator, set apart
/// from what is below it by the bar of its fraction, is a line of its own; a line is neither smaller nor shorter than the large glyph
/// (the delimiter of a displayed formula) set between it and the next line,
/// so the lines stay apart and the glyph is a line of its own or the script
/// of neither. A script of a script goes to the cluster at the end of the
/// chain.
///
/// `clusters` are in the order of group, block and baseline, so those of
/// one group and block are neighbours and their baselines, the middle
/// glyph's, increase.
fn attach_scripts(
    clusters: Vec<Vec<usize>>,
    glyphs: &[RawGlyph],
    group_of: &[usize],
    perp: &[f64],
    key: &dyn Fn(usize) -> Option<usize>,
    parted: &dyn Fn(&[usize], &[usize]) -> bool,
) -> Vec<Vec<usize>> {
    let middle = |cluster: &[usize]| cluster[cluster.len() / 2];
    let size = |index: usize| glyphs[middle(&clusters[index])].size;
    let level = |index: usize| perp[middle(&clusters[index])];
    let same = |a: usize, b: usize| {
        let (a, b) = (middle(&clusters[a]), middle(&clusters[b]));
        group_of[a] == group_of[b] && key(a) == key(b)
    };
    let largest = (0..clusters.len()).map(size).fold(0.0, f64::max);
    let reach = BASELINE_TOLERANCE * largest;

    let mut target: Vec<Option<usize>> = vec![None; clusters.len()];
    for script in 0..clusters.len() {
        let mut best: Option<(f64, usize)> = None;
        let below = (0..script)
            .rev()
            .take_while(|&other| same(script, other) && level(script) - level(other) <= reach);
        let above = (script + 1..clusters.len())
            .take_while(|&other| same(script, other) && level(other) - level(script) <= reach);
        for other in below.chain(above) {
            let apart = (level(other) - level(script)).abs();
            let holds = size(other) > size(script)
                && clusters[other].len() >= clusters[script].len()
                && apart <= BASELINE_TOLERANCE * size(other)
                && !parted(&clusters[script], &clusters[other]);
            if holds && best.is_none_or(|(nearest, _)| apart < nearest) {
                best = Some((apart, other));
            }
        }
        target[script] = best.map(|(_, other)| other);
    }

    let root = |mut index: usize| {
        while let Some(next) = target[index] {
            index = next;
        }
        index
    };
    let mut merged: Vec<Vec<usize>> = vec![Vec::new(); clusters.len()];
    for (index, cluster) in clusters.iter().enumerate() {
        merged[root(index)].extend_from_slice(cluster);
    }
    merged.retain(|cluster| !cluster.is_empty());
    merged
}

/// Cuts a cluster of glyphs on one baseline into lines at the gaps that
/// are too wide for a line, in order along the baseline.
fn split_cluster(
    cluster: &[usize],
    glyphs: &[RawGlyph],
    group_of: &[usize],
    perp: &[f64],
    start: &[f64],
    lines: &mut Vec<Line>,
) {
    let mut along: Vec<usize> = cluster.to_vec();
    along.sort_by(|&a, &b| start[a].total_cmp(&start[b]).then(a.cmp(&b)));
    let mut current: Vec<(usize, bool)> = Vec::new();
    let mut reach = f64::NEG_INFINITY;
    for &glyph in &along {
        if let Some(&(last, _)) = current.last() {
            let size = glyphs[last].size.max(glyphs[glyph].size);
            let gap = start[glyph] - reach;
            let drawn_twice = current
                .iter()
                .rev()
                .take_while(|&&(other, _)| {
                    start[glyph] - start[other]
                        <= SAME_POSITION * glyphs[other].size.max(glyphs[glyph].size)
                })
                .any(|&(other, _)| {
                    (perp[glyph] - perp[other]).abs()
                        <= SAME_POSITION * glyphs[other].size.max(glyphs[glyph].size)
                        && glyphs[glyph].text == glyphs[other].text
                });
            if drawn_twice {
                continue;
            }
            if gap > MAX_JOIN_GAP * size {
                finish_line(&mut current, glyphs, group_of, perp, start, lines);
                reach = f64::NEG_INFINITY;
            }
        }
        let spaced = match current.last() {
            Some(&(last, _)) => {
                let size = glyphs[last].size.max(glyphs[glyph].size);
                start[glyph] - reach > SPACE_GAP * size
                    && !ends_with_space(&glyphs[last].text)
                    && !starts_with_space(&glyphs[glyph].text)
            }
            None => false,
        };
        current.push((glyph, spaced));
        reach = reach.max(start[glyph] + glyphs[glyph].extent);
    }
    finish_line(&mut current, glyphs, group_of, perp, start, lines);
}

fn ends_with_space(text: &str) -> bool {
    text.chars().next_back().is_some_and(char::is_whitespace)
}

fn starts_with_space(text: &str) -> bool {
    text.chars().next().is_some_and(char::is_whitespace)
}

/// Makes a line of the glyphs gathered, unless they are all white space.
fn finish_line(
    current: &mut Vec<(usize, bool)>,
    glyphs: &[RawGlyph],
    group_of: &[usize],
    perp: &[f64],
    start: &[f64],
    lines: &mut Vec<Line>,
) {
    let members = std::mem::take(current);
    let visible = members
        .iter()
        .any(|&(glyph, _)| glyphs[glyph].text.chars().any(|ch| !ch.is_whitespace()));
    if !visible {
        return;
    }
    let mut across: Vec<f64> = members.iter().map(|&(glyph, _)| perp[glyph]).collect();
    across.sort_by(f64::total_cmp);
    let first = members[0].0;
    let bounds = Bounds::of(
        members
            .iter()
            .flat_map(|&(glyph, _)| glyphs[glyph].corners.into_iter()),
    );
    let Some(bounds) = bounds else {
        return;
    };
    lines.push(Line {
        group: group_of[first],
        block_key: glyphs[first].slot.map(|slot| slot.block),
        perp: across[across.len() / 2],
        start: start[first],
        end: members
            .iter()
            .map(|&(glyph, _)| start[glyph] + glyphs[glyph].extent)
            .fold(f64::NEG_INFINITY, f64::max),
        size: members
            .iter()
            .map(|&(glyph, _)| glyphs[glyph].size)
            .fold(0.0, f64::max),
        order: members
            .iter()
            .filter_map(|&(glyph, _)| glyphs[glyph].slot.map(|slot| slot.order))
            .min(),
        bounds,
        glyphs: members,
    });
}

/// The blocks: lines of one group and block-level element, each the
/// nearest line below the one above it, which it also has as its nearest
/// above. Each block lists its lines from the top down.
fn blocks(lines: &[Line]) -> Vec<Vec<usize>> {
    let mut sorted: Vec<usize> = (0..lines.len()).collect();
    sorted.sort_by(|&a, &b| {
        lines[a]
            .group
            .cmp(&lines[b].group)
            .then(lines[a].block_key.cmp(&lines[b].block_key))
            .then(lines[b].perp.total_cmp(&lines[a].perp))
            .then(a.cmp(&b))
    });
    let same_set = |a: usize, b: usize| {
        lines[a].group == lines[b].group && lines[a].block_key == lines[b].block_key
    };
    let stacks = |above: usize, below: usize| {
        let (a, b) = (&lines[above], &lines[below]);
        let size = a.size.max(b.size);
        let room = a.perp - b.perp;
        let overlap = a.end.min(b.end) - a.start.max(b.start);
        let ratio = a.size.max(b.size) / a.size.min(b.size).max(f64::MIN_POSITIVE);
        room >= MIN_LEADING * size
            && room <= MAX_LEADING * size
            && overlap > 0.0
            && ratio <= MAX_SIZE_RATIO
    };
    // The lines are in order from the top of the group down, so the room
    // between two grows with the distance in the list. Stacking needs
    // sizes within `MAX_SIZE_RATIO`, which bounds how much room a line
    // can have to another that stacks with it.
    let reach = MAX_LEADING * MAX_SIZE_RATIO;
    let mut below_of = vec![None; sorted.len()];
    let mut above_of = vec![None; sorted.len()];
    for i in 0..sorted.len() {
        for j in i + 1..sorted.len() {
            let (a, b) = (sorted[i], sorted[j]);
            if !same_set(a, b) || lines[a].perp - lines[b].perp > reach * lines[a].size {
                break;
            }
            if stacks(a, b) {
                below_of[i] = Some(j);
                break;
            }
        }
    }
    for j in 0..sorted.len() {
        for i in (0..j).rev() {
            let (a, b) = (sorted[i], sorted[j]);
            if !same_set(a, b) || lines[a].perp - lines[b].perp > reach * lines[b].size {
                break;
            }
            if stacks(a, b) {
                above_of[j] = Some(i);
                break;
            }
        }
    }
    // Chains of lines joined by a mutual choice; the first line of each
    // starts a block.
    let mut blocks: Vec<Vec<usize>> = Vec::new();
    let mut taken = vec![false; sorted.len()];
    for i in 0..sorted.len() {
        if taken[i] {
            continue;
        }
        let mut block = vec![sorted[i]];
        taken[i] = true;
        let mut at = i;
        while let Some(next) = below_of[at] {
            if above_of[next] != Some(at) || taken[next] {
                break;
            }
            taken[next] = true;
            block.push(sorted[next]);
            at = next;
        }
        blocks.push(block);
    }
    blocks
}

/// The bounds of a block.
fn block_bounds(lines: &[Line], block: &[usize]) -> Bounds {
    block
        .iter()
        .map(|&line| lines[line].bounds)
        .reduce(|a, b| a.union(&b))
        .unwrap_or(Bounds {
            left: 0.0,
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
        })
}

/// The order the blocks are read in, as indices into `blocks`.
fn reading_order(lines: &[Line], blocks: &[Vec<usize>]) -> Vec<usize> {
    let bounds: Vec<Bounds> = blocks
        .iter()
        .map(|block| block_bounds(lines, block))
        .collect();
    let tag_of = |block: usize| {
        blocks[block]
            .iter()
            .filter_map(|&line| lines[line].order)
            .min()
    };
    let (mut tagged, untagged): (Vec<usize>, Vec<usize>) =
        (0..blocks.len()).partition(|&block| tag_of(block).is_some());
    tagged.sort_by(|&a, &b| {
        tag_of(a)
            .cmp(&tag_of(b))
            .then(bounds[a].top.total_cmp(&bounds[b].top))
            .then(bounds[a].left.total_cmp(&bounds[b].left))
    });
    let rest = cut_order(&untagged, &bounds);
    tagged.extend(rest);
    tagged
}

/// Orders `items` by cutting the page at the widest empty band, across or
/// down, and ordering what lies on each side, the upper or left part
/// first. What no band cuts is ordered top to bottom, left to right.
fn cut_order(items: &[usize], bounds: &[Bounds]) -> Vec<usize> {
    let mut out = Vec::with_capacity(items.len());
    // The parts still to order, the next one last.
    let mut parts = vec![items.to_vec()];
    while let Some(part) = parts.pop() {
        if part.len() <= 1 {
            out.extend(part);
            continue;
        }
        let across = widest_gap(&part, |item| (bounds[item].top, bounds[item].bottom));
        let along = widest_gap(&part, |item| (bounds[item].left, bounds[item].right));
        let cut = match (across, along) {
            (Some(a), Some(b)) => Some(if b.1 > a.1 { (b, false) } else { (a, true) }),
            (Some(a), None) => Some((a, true)),
            (None, Some(b)) => Some((b, false)),
            (None, None) => None,
        };
        match cut {
            Some(((split, _), rows)) => {
                let key = |item: usize| {
                    if rows {
                        bounds[item].top
                    } else {
                        bounds[item].left
                    }
                };
                let mut sorted = part;
                sorted.sort_by(|&a, &b| key(a).total_cmp(&key(b)).then(a.cmp(&b)));
                let second = sorted.split_off(split);
                parts.push(second);
                parts.push(sorted);
            }
            None => {
                let mut sorted = part;
                sorted.sort_by(|&a, &b| {
                    bounds[a]
                        .top
                        .total_cmp(&bounds[b].top)
                        .then(bounds[a].left.total_cmp(&bounds[b].left))
                        .then(a.cmp(&b))
                });
                out.extend(sorted);
            }
        }
    }
    out
}

/// The widest band, across one axis, that no item of `part` covers: the
/// number of items before it and its width. `span` gives an item's least
/// and greatest coordinate on the axis.
fn widest_gap(part: &[usize], span: impl Fn(usize) -> (f64, f64)) -> Option<(usize, f64)> {
    let mut spans: Vec<(f64, f64, usize)> = part
        .iter()
        .map(|&item| {
            let (least, greatest) = span(item);
            (least, greatest, item)
        })
        .collect();
    spans.sort_by(|a, b| a.0.total_cmp(&b.0).then(a.2.cmp(&b.2)));
    let mut best: Option<(usize, f64)> = None;
    let mut reach = spans[0].1;
    for (index, &(least, greatest, _)) in spans.iter().enumerate().skip(1) {
        let gap = least - reach;
        if gap > 0.0 && best.is_none_or(|(_, widest)| gap > widest) {
            best = Some((index, gap));
        }
        reach = reach.max(greatest);
    }
    best
}

/// A block of the result.
fn build_block(glyphs: &[RawGlyph], lines: &[Line], block: &[usize], view: &PageView) -> TextBlock {
    let mut out = Vec::with_capacity(block.len());
    for &line in block {
        out.push(build_line(glyphs, &lines[line], view));
    }
    bidi::in_logical_order(&mut out);
    let rect = out
        .iter()
        .map(|line| line.rect)
        .reduce(|a, b| a.union(&b))
        .unwrap_or_default();
    TextBlock { rect, lines: out }
}

fn build_line(glyphs: &[RawGlyph], line: &Line, view: &PageView) -> TextLine {
    let mut chars = Vec::with_capacity(line.glyphs.len());
    let mut previous: Option<&RawGlyph> = None;
    for &(glyph, spaced) in &line.glyphs {
        let glyph = &glyphs[glyph];
        if let (true, Some(before)) = (spaced, previous) {
            let gap = [
                before.corners[1],
                glyph.corners[0],
                glyph.corners[3],
                before.corners[2],
            ];
            chars.push(TextChar {
                text: " ".to_owned(),
                rect: view.rect_of(gap).unwrap_or_default(),
                quad: gap.map(|(x, y)| view.fraction(x, y)),
                synthetic: true,
                source: None,
            });
        }
        chars.push(TextChar {
            text: glyph.text.clone(),
            rect: view.rect_of(glyph.corners).unwrap_or_default(),
            quad: glyph.corners.map(|(x, y)| view.fraction(x, y)),
            synthetic: false,
            source: Some(glyph.source),
        });
        previous = Some(glyph);
    }
    let rect = view
        .rect_of([
            (line.bounds.left, line.bounds.top),
            (line.bounds.right, line.bounds.bottom),
        ])
        .unwrap_or_default();
    TextLine::new(rect, chars)
}
