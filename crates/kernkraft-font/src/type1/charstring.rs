// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The Type 1 charstring interpreter (Adobe Type 1 Font Format, the
//! charstring chapter: number encoding, command encoding, the hints and
//! path operators, `seac`, `callothersubr` with its flex and hint
//! replacement uses). Hints are read and dropped: the renderer fills the
//! outline.
//!
//! A charstring is a sequence of numbers and one-byte commands. Every
//! command takes its operands from the top of the operand stack and leaves
//! it empty, except `callsubr`, `return`, `div`, `callothersubr` and `pop`.

use super::standard::standard_name;
use super::Private;
use crate::outline::{Outline, Segment};

/// The operand stack of a charstring holds at most 24 numbers (the
/// charstring chapter, the operand stack limit).
const MAX_STACK: usize = 24;
/// `callsubr` nests at most 10 deep (the same chapter).
const MAX_SUBR_DEPTH: usize = 10;
/// The flex of the charstring chapter is seven points: a reference point
/// and two curves of three.
const FLEX_POINTS: usize = 7;
/// Commands one glyph may execute, subroutine calls included. Subroutines
/// may call subroutines, so a font can ask for far more than its size
/// suggests; the largest glyphs of real Type 1 fonts run a few thousand.
const MAX_OPERATIONS: usize = 100_000;
/// `seac` names its two characters in StandardEncoding and they may not
/// themselves be composites.
const MAX_SEAC_DEPTH: usize = 1;

/// A glyph's outline with the numbers `hsbw` gave.
pub(crate) struct Rendered {
    pub outline: Outline,
    pub advance: f32,
}

type Fail = &'static str;

pub(crate) fn render(private: &Private, charstring: &[u8]) -> Result<Rendered, Fail> {
    let mut run = Run::new(private, 0);
    run.exec(charstring, 0)?;
    run.finish()
}

struct Run<'a> {
    private: &'a Private,
    stack: Vec<f32>,
    /// Values the PostScript side holds for `pop` to return.
    ps: Vec<f32>,
    x: f32,
    y: f32,
    sbx: f32,
    advance: Option<f32>,
    outline: Outline,
    open: bool,
    flex: Option<Vec<(f32, f32)>>,
    operations: usize,
    seac_depth: usize,
    ended: bool,
}

/// What the commands of a charstring do next.
enum Step {
    Continue,
    Return,
}

impl<'a> Run<'a> {
    fn new(private: &'a Private, seac_depth: usize) -> Run<'a> {
        Run {
            private,
            stack: Vec::new(),
            ps: Vec::new(),
            x: 0.0,
            y: 0.0,
            sbx: 0.0,
            advance: None,
            outline: Outline::default(),
            open: false,
            flex: None,
            operations: 0,
            seac_depth,
            ended: false,
        }
    }

    fn finish(self) -> Result<Rendered, Fail> {
        if !self.ended {
            return Err("the charstring has no endchar");
        }
        let advance = self.advance.ok_or("the charstring has no hsbw")?;
        Ok(Rendered {
            outline: self.outline,
            advance,
        })
    }

    fn push(&mut self, v: f32) -> Result<(), Fail> {
        if self.stack.len() >= MAX_STACK {
            return Err("the operand stack overflowed");
        }
        self.stack.push(v);
        Ok(())
    }

    /// The top `n` operands, left on the stack.
    fn args(&self, n: usize) -> Result<&[f32], Fail> {
        let len = self.stack.len();
        if len < n {
            return Err("a command has fewer operands than it takes");
        }
        Ok(&self.stack[len - n..])
    }

    fn exec(&mut self, code: &[u8], depth: usize) -> Result<Step, Fail> {
        let mut i = 0;
        while i < code.len() {
            let b = code[i];
            i += 1;
            match b {
                32..=246 => self.push(f32::from(b) - 139.0)?,
                247..=250 => {
                    let w = *code.get(i).ok_or("a number is cut short")?;
                    i += 1;
                    self.push((f32::from(b) - 247.0) * 256.0 + f32::from(w) + 108.0)?;
                }
                251..=254 => {
                    let w = *code.get(i).ok_or("a number is cut short")?;
                    i += 1;
                    self.push(-(f32::from(b) - 251.0) * 256.0 - f32::from(w) - 108.0)?;
                }
                255 => {
                    let n = code.get(i..i + 4).ok_or("a number is cut short")?;
                    i += 4;
                    self.push(i32::from_be_bytes([n[0], n[1], n[2], n[3]]) as f32)?;
                }
                _ => {
                    self.operations += 1;
                    if self.operations > MAX_OPERATIONS {
                        return Err("the glyph runs too many commands");
                    }
                    let command = if b == 12 {
                        let e = *code.get(i).ok_or("an escape is cut short")?;
                        i += 1;
                        0x100 | u16::from(e)
                    } else {
                        u16::from(b)
                    };
                    if let Step::Return = self.command(command, depth)? {
                        return Ok(Step::Return);
                    }
                    if self.ended {
                        return Ok(Step::Return);
                    }
                }
            }
        }
        Ok(Step::Continue)
    }

    fn command(&mut self, command: u16, depth: usize) -> Result<Step, Fail> {
        match command {
            // hstem, vstem, dotsection, vstem3, hstem3: hints only.
            1 | 3 | 0x100 | 0x101 | 0x102 => self.stack.clear(),
            4 => {
                let dy = self.args(1)?[0];
                self.stack.clear();
                self.move_by(0.0, dy)?;
            }
            21 => {
                let [dx, dy] = self.take::<2>()?;
                self.move_by(dx, dy)?;
            }
            22 => {
                let dx = self.args(1)?[0];
                self.stack.clear();
                self.move_by(dx, 0.0)?;
            }
            5 => {
                let [dx, dy] = self.take::<2>()?;
                self.line_by(dx, dy)?;
            }
            6 => {
                let dx = self.args(1)?[0];
                self.stack.clear();
                self.line_by(dx, 0.0)?;
            }
            7 => {
                let dy = self.args(1)?[0];
                self.stack.clear();
                self.line_by(0.0, dy)?;
            }
            8 => {
                let [a, b, c, d, e, f] = self.take::<6>()?;
                self.curve_by(a, b, c, d, e, f)?;
            }
            30 => {
                let [dy1, dx2, dy2, dx3] = self.take::<4>()?;
                self.curve_by(0.0, dy1, dx2, dy2, dx3, 0.0)?;
            }
            31 => {
                let [dx1, dx2, dy2, dy3] = self.take::<4>()?;
                self.curve_by(dx1, 0.0, dx2, dy2, 0.0, dy3)?;
            }
            9 => {
                self.stack.clear();
                if self.open {
                    self.outline.close();
                    self.open = false;
                }
            }
            10 => {
                let index = self.args(1)?[0];
                self.stack.pop();
                if depth >= MAX_SUBR_DEPTH {
                    return Err("subroutines nest too deep");
                }
                let subr = self
                    .private
                    .subr(index)
                    .ok_or("a subroutine that does not exist")?;
                if let Step::Return = self.exec(subr, depth + 1)? {
                    if self.ended {
                        return Ok(Step::Return);
                    }
                }
            }
            11 => return Ok(Step::Return),
            13 => {
                let [sbx, wx] = self.take::<2>()?;
                self.sbx = sbx;
                self.x = sbx;
                self.y = 0.0;
                self.advance = Some(wx);
            }
            0x107 => {
                let [sbx, sby, wx, _wy] = self.take::<4>()?;
                self.sbx = sbx;
                self.x = sbx;
                self.y = sby;
                self.advance = Some(wx);
            }
            14 => {
                self.stack.clear();
                if self.open {
                    self.outline.close();
                    self.open = false;
                }
                self.ended = true;
            }
            0x10C => {
                let [a, b] = self.take::<2>()?;
                if b == 0.0 {
                    return Err("a division by zero");
                }
                self.push(a / b)?;
            }
            0x110 => self.call_other_subr()?,
            0x111 => {
                let v = self
                    .ps
                    .pop()
                    .ok_or("pop with nothing on the PostScript stack")?;
                self.push(v)?;
            }
            0x121 => {
                let [x, y] = self.take::<2>()?;
                self.x = x;
                self.y = y;
            }
            0x106 => self.seac()?,
            _ => return Err("a charstring command that is not defined"),
        }
        Ok(Step::Continue)
    }

    /// The top `N` operands, removed (the rest is cleared: commands leave
    /// the stack empty).
    fn take<const N: usize>(&mut self) -> Result<[f32; N], Fail> {
        let mut out = [0.0; N];
        out.copy_from_slice(self.args(N)?);
        self.stack.clear();
        Ok(out)
    }

    fn move_by(&mut self, dx: f32, dy: f32) -> Result<(), Fail> {
        self.x += dx;
        self.y += dy;
        if self.flex.is_some() {
            // Inside a flex the moves only place points; `callothersubr` 2
            // records each one.
            return Ok(());
        }
        if self.open {
            // A moveto straight after a moveto replaces it: the first began
            // a contour that holds nothing. Any other open contour is filled
            // as if closed.
            if let Some(Segment::MoveTo(_)) = self.outline.segments.last() {
                self.outline.segments.pop();
            } else {
                self.outline.close();
            }
        }
        self.outline.move_to(self.x, self.y);
        self.open = true;
        Ok(())
    }

    fn line_by(&mut self, dx: f32, dy: f32) -> Result<(), Fail> {
        if !self.open {
            return Err("a line before any moveto");
        }
        self.x += dx;
        self.y += dy;
        self.outline.line_to(self.x, self.y);
        Ok(())
    }

    fn curve_by(
        &mut self,
        dx1: f32,
        dy1: f32,
        dx2: f32,
        dy2: f32,
        dx3: f32,
        dy3: f32,
    ) -> Result<(), Fail> {
        if !self.open {
            return Err("a curve before any moveto");
        }
        let (x1, y1) = (self.x + dx1, self.y + dy1);
        let (x2, y2) = (x1 + dx2, y1 + dy2);
        self.x = x2 + dx3;
        self.y = y2 + dy3;
        self.outline.cubic_to(x1, y1, x2, y2, self.x, self.y);
        Ok(())
    }

    /// `arg1 ... argn n othersubr# callothersubr`.
    fn call_other_subr(&mut self) -> Result<(), Fail> {
        let [count, number] = {
            let top = self.args(2)?;
            [top[0], top[1]]
        };
        self.stack.truncate(self.stack.len() - 2);
        if count < 0.0 || count.fract() != 0.0 || count as usize > self.stack.len() {
            return Err("callothersubr with a bad operand count");
        }
        let args = self.stack.split_off(self.stack.len() - count as usize);
        match number as i64 {
            // End of flex: flex height, then the end point.
            0 => {
                let [_, x, y] = <[f32; 3]>::try_from(args.as_slice())
                    .map_err(|_| "the end of a flex takes three operands")?;
                self.end_flex()?;
                self.ps = vec![y, x];
            }
            // Start of flex.
            1 => self.flex = Some(Vec::with_capacity(FLEX_POINTS)),
            // A point of the flex.
            2 => {
                let point = (self.x, self.y);
                let points = self.flex.as_mut().ok_or("a flex point outside a flex")?;
                if points.len() >= FLEX_POINTS {
                    return Err("a flex with more than seven points");
                }
                points.push(point);
            }
            // Hint replacement: the subroutine number comes back by `pop`.
            3 => self.ps = args.into_iter().rev().collect(),
            // Counter control: operands are consumed and nothing returns.
            12 | 13 => self.ps.clear(),
            // An othersubr this reader does not know hands its operands
            // back to `pop` in order.
            _ => self.ps = args.into_iter().rev().collect(),
        }
        Ok(())
    }

    fn end_flex(&mut self) -> Result<(), Fail> {
        let points = self
            .flex
            .take()
            .ok_or("the end of a flex that never began")?;
        if points.len() != FLEX_POINTS {
            return Err("a flex without seven points");
        }
        if !self.open {
            return Err("a flex before any moveto");
        }
        // points[0] is the reference point; it is not on the path.
        for curve in [&points[1..4], &points[4..7]] {
            self.outline.cubic_to(
                curve[0].0, curve[0].1, curve[1].0, curve[1].1, curve[2].0, curve[2].1,
            );
        }
        Ok(())
    }

    /// `asb adx ady bchar achar seac`: the base character, then the accent
    /// character with its origin moved so that its left side bearing point
    /// lies `adx` from this character's.
    fn seac(&mut self) -> Result<(), Fail> {
        let [asb, adx, ady, bchar, achar] = self.take::<5>()?;
        if self.seac_depth >= MAX_SEAC_DEPTH {
            return Err("a composite glyph inside a composite glyph");
        }
        let advance = self.advance.ok_or("seac before hsbw")?;
        let sbx = self.sbx;
        let base = self.component(bchar)?;
        let accent = self.component(achar)?;
        self.outline.segments.extend(base.outline.segments);
        let (dx, dy) = (sbx + adx - asb, ady);
        for s in accent.outline.segments {
            self.outline.segments.push(shift(s, dx, dy));
        }
        self.advance = Some(advance);
        self.open = false;
        self.ended = true;
        Ok(())
    }

    fn component(&self, code: f32) -> Result<Rendered, Fail> {
        if code < 0.0 || code > 255.0 || code.fract() != 0.0 {
            return Err("a seac character code outside 0 to 255");
        }
        let name = standard_name(code as u8)
            .ok_or("a seac character that StandardEncoding does not name")?;
        let charstring = self
            .private
            .charstring_by_name(name)
            .ok_or("a seac character the font does not hold")?;
        let mut run = Run::new(self.private, self.seac_depth + 1);
        run.exec(charstring, 0)?;
        run.finish()
    }
}

fn shift(s: Segment, dx: f32, dy: f32) -> Segment {
    use crate::outline::Point;
    let p = |q: Point| Point::new(q.x + dx, q.y + dy);
    match s {
        Segment::MoveTo(a) => Segment::MoveTo(p(a)),
        Segment::LineTo(a) => Segment::LineTo(p(a)),
        Segment::QuadTo(a, b) => Segment::QuadTo(p(a), p(b)),
        Segment::CubicTo(a, b, c) => Segment::CubicTo(p(a), p(b), p(c)),
        Segment::Close => Segment::Close,
    }
}
