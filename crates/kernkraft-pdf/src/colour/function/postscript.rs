// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Type 4 functions: ISO 32000-1:2008, 7.10.5, Table 42. The program is
//! read once into a tree of [`Op`]s (a conditional holds its branches)
//! and run on an operand stack of 100 entries, the least the format
//! requires an implementation to provide (7.10.5.1). Integers are 32 bits
//! (Annex C, Table C.1); an integer operation whose result does not fit
//! gives a real, as in PostScript.

use std::sync::Arc;

use super::{fault, FunctionFault, Kind, Loaded};
use crate::doc::Document;
use crate::syntax::limits::MAX_NESTING_DEPTH;
use crate::PdfError;

/// 7.10.5.1: the stack of every implementation holds at least this many.
const STACK_LIMIT: usize = 100;
/// The angle of a full turn: `sin`, `cos` and `atan` work in degrees.
const FULL_TURN: f64 = 360.0;

/// A calculator program, read and checked for syntax.
#[derive(Debug, PartialEq)]
pub struct PostScript {
    code: Vec<Op>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Value {
    Int(i32),
    Real(f64),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Operator {
    Abs,
    Add,
    Atan,
    Ceiling,
    Cos,
    Cvi,
    Cvr,
    Div,
    Exp,
    Floor,
    Idiv,
    Ln,
    Log,
    Mod,
    Mul,
    Neg,
    Round,
    Sin,
    Sqrt,
    Sub,
    Truncate,
    And,
    Bitshift,
    Eq,
    False,
    Ge,
    Gt,
    Le,
    Lt,
    Ne,
    Not,
    Or,
    True,
    Xor,
    Copy,
    Dup,
    Exch,
    Index,
    Pop,
    Roll,
}

#[derive(Debug, Clone, PartialEq)]
enum Op {
    Push(Value),
    Run(Operator),
    If(Vec<Op>),
    IfElse(Vec<Op>, Vec<Op>),
}

impl Operator {
    fn named(word: &[u8]) -> Option<Self> {
        Some(match word {
            b"abs" => Self::Abs,
            b"add" => Self::Add,
            b"atan" => Self::Atan,
            b"ceiling" => Self::Ceiling,
            b"cos" => Self::Cos,
            b"cvi" => Self::Cvi,
            b"cvr" => Self::Cvr,
            b"div" => Self::Div,
            b"exp" => Self::Exp,
            b"floor" => Self::Floor,
            b"idiv" => Self::Idiv,
            b"ln" => Self::Ln,
            b"log" => Self::Log,
            b"mod" => Self::Mod,
            b"mul" => Self::Mul,
            b"neg" => Self::Neg,
            b"round" => Self::Round,
            b"sin" => Self::Sin,
            b"sqrt" => Self::Sqrt,
            b"sub" => Self::Sub,
            b"truncate" => Self::Truncate,
            b"and" => Self::And,
            b"bitshift" => Self::Bitshift,
            b"eq" => Self::Eq,
            b"false" => Self::False,
            b"ge" => Self::Ge,
            b"gt" => Self::Gt,
            b"le" => Self::Le,
            b"lt" => Self::Lt,
            b"ne" => Self::Ne,
            b"not" => Self::Not,
            b"or" => Self::Or,
            b"true" => Self::True,
            b"xor" => Self::Xor,
            b"copy" => Self::Copy,
            b"dup" => Self::Dup,
            b"exch" => Self::Exch,
            b"index" => Self::Index,
            b"pop" => Self::Pop,
            b"roll" => Self::Roll,
            _ => return None,
        })
    }
}

pub(super) fn read(
    doc: &Document,
    loaded: &Loaded,
    range: Option<&[[f64; 2]]>,
) -> Result<(Kind, usize), PdfError> {
    let invalid = |key| PdfError::InvalidEntry {
        object: loaded.id,
        key,
    };
    let range = range
        .filter(|range| !range.is_empty())
        .ok_or_else(|| invalid("Range"))?;
    let data = doc.function_data(loaded)?;
    let program = PostScript::parse(&data).ok_or_else(|| invalid("Function"))?;
    Ok((Kind::PostScript(Arc::new(program)), range.len()))
}

impl PostScript {
    /// Reads calculator code: a block in braces whose items are numbers,
    /// the operators of Table 42 and nested blocks followed by `if` or
    /// `ifelse`. `None` for anything else.
    pub fn parse(source: &[u8]) -> Option<Self> {
        let mut tokens = Tokens {
            source,
            position: 0,
        };
        if tokens.next()? != Token::Open {
            return None;
        }
        let code = parse_block(&mut tokens, 0)?;
        // Nothing but white space may follow the closing brace.
        tokens.next().is_none().then_some(Self { code })
    }

    /// Runs the program on `input` (the initial stack, first input at the
    /// bottom) and appends the `outputs` values it leaves.
    pub(super) fn eval(
        &self,
        input: &[f64],
        outputs: usize,
        output: &mut Vec<f64>,
    ) -> Result<(), PdfError> {
        let mut stack: Vec<Value> = Vec::with_capacity(STACK_LIMIT);
        for x in input {
            push(&mut stack, Value::Real(*x))?;
        }
        run(&self.code, &mut stack)?;
        if stack.len() != outputs {
            return Err(fault(FunctionFault::OutputCount {
                expected: outputs,
                found: stack.len(),
            }));
        }
        for value in stack {
            output.push(match value {
                Value::Int(integer) => f64::from(integer),
                Value::Real(real) => real,
                Value::Bool(_) => return Err(fault(FunctionFault::Type)),
            });
        }
        Ok(())
    }
}

// --- reading ---

#[derive(Debug, PartialEq)]
enum Token<'a> {
    Open,
    Close,
    Word(&'a [u8]),
}

struct Tokens<'a> {
    source: &'a [u8],
    position: usize,
}

impl<'a> Tokens<'a> {
    /// The next token, skipping white space and `%` comments.
    fn next(&mut self) -> Option<Token<'a>> {
        loop {
            let byte = *self.source.get(self.position)?;
            match byte {
                b'%' => {
                    while self
                        .source
                        .get(self.position)
                        .is_some_and(|b| !matches!(b, b'\n' | b'\r'))
                    {
                        self.position += 1;
                    }
                }
                b'{' => {
                    self.position += 1;
                    return Some(Token::Open);
                }
                b'}' => {
                    self.position += 1;
                    return Some(Token::Close);
                }
                b if is_space(b) => self.position += 1,
                _ => {
                    let start = self.position;
                    while self
                        .source
                        .get(self.position)
                        .is_some_and(|b| !is_space(*b) && !matches!(b, b'{' | b'}' | b'%'))
                    {
                        self.position += 1;
                    }
                    return Some(Token::Word(&self.source[start..self.position]));
                }
            }
        }
    }
}

/// White space of 7.2.2, Table 1 (and a zero byte).
fn is_space(byte: u8) -> bool {
    matches!(byte, 0 | b'\t' | b'\n' | 0x0c | b'\r' | b' ')
}

/// The items up to the `}` that closes the block just opened.
fn parse_block(tokens: &mut Tokens<'_>, depth: usize) -> Option<Vec<Op>> {
    if depth > MAX_NESTING_DEPTH {
        return None;
    }
    let mut code = Vec::new();
    loop {
        match tokens.next()? {
            Token::Close => return Some(code),
            Token::Open => {
                let first = parse_block(tokens, depth + 1)?;
                match tokens.next()? {
                    Token::Word(b"if") => code.push(Op::If(first)),
                    Token::Open => {
                        let second = parse_block(tokens, depth + 1)?;
                        if tokens.next()? != Token::Word(b"ifelse") {
                            return None;
                        }
                        code.push(Op::IfElse(first, second));
                    }
                    _ => return None,
                }
            }
            Token::Word(word) => {
                if let Some(operator) = Operator::named(word) {
                    code.push(Op::Run(operator));
                } else {
                    code.push(Op::Push(number(word)?));
                }
            }
        }
    }
}

/// An integer (7.3.3) or a real; `if` and `ifelse` are only valid after
/// their blocks and are refused here.
fn number(word: &[u8]) -> Option<Value> {
    let text = std::str::from_utf8(word).ok()?;
    let digits = text.strip_prefix(['+', '-']).unwrap_or(text);
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit() || b == b'.') {
        return None;
    }
    if digits.bytes().filter(|b| *b == b'.').count() > 1 || digits == "." {
        return None;
    }
    if !digits.contains('.') {
        if let Ok(integer) = text.parse::<i32>() {
            return Some(Value::Int(integer));
        }
    }
    text.parse::<f64>()
        .ok()
        .filter(|r| r.is_finite())
        .map(Value::Real)
}

// --- running ---

fn push(stack: &mut Vec<Value>, value: Value) -> Result<(), PdfError> {
    if stack.len() >= STACK_LIMIT {
        return Err(fault(FunctionFault::StackOverflow));
    }
    stack.push(value);
    Ok(())
}

fn pop(stack: &mut Vec<Value>) -> Result<Value, PdfError> {
    stack
        .pop()
        .ok_or_else(|| fault(FunctionFault::StackUnderflow))
}

fn run(code: &[Op], stack: &mut Vec<Value>) -> Result<(), PdfError> {
    for op in code {
        match op {
            Op::Push(value) => push(stack, *value)?,
            Op::Run(operator) => execute(*operator, stack)?,
            Op::If(branch) => {
                if pop_bool(stack)? {
                    run(branch, stack)?;
                }
            }
            Op::IfElse(then, otherwise) => {
                let branch = if pop_bool(stack)? { then } else { otherwise };
                run(branch, stack)?;
            }
        }
    }
    Ok(())
}

fn pop_bool(stack: &mut Vec<Value>) -> Result<bool, PdfError> {
    match pop(stack)? {
        Value::Bool(boolean) => Ok(boolean),
        _ => Err(fault(FunctionFault::Type)),
    }
}

fn pop_int(stack: &mut Vec<Value>) -> Result<i32, PdfError> {
    match pop(stack)? {
        Value::Int(integer) => Ok(integer),
        _ => Err(fault(FunctionFault::Type)),
    }
}

/// A number operand as a real.
fn pop_real(stack: &mut Vec<Value>) -> Result<f64, PdfError> {
    real_of(pop(stack)?)
}

fn real_of(value: Value) -> Result<f64, PdfError> {
    match value {
        Value::Int(integer) => Ok(f64::from(integer)),
        Value::Real(real) => Ok(real),
        Value::Bool(_) => Err(fault(FunctionFault::Type)),
    }
}

/// A real result as a value; infinity and NaN are not values.
fn real(result: f64) -> Result<Value, PdfError> {
    if result.is_finite() {
        Ok(Value::Real(result))
    } else {
        Err(fault(FunctionFault::Undefined))
    }
}

/// The integer a real is, when it is whole and fits 32 bits.
fn fits_int(value: f64) -> Option<i32> {
    (value >= f64::from(i32::MIN) && value <= f64::from(i32::MAX)).then_some(value as i32)
}

fn execute(operator: Operator, stack: &mut Vec<Value>) -> Result<(), PdfError> {
    use Operator::*;
    let result = match operator {
        Abs => match pop(stack)? {
            Value::Int(integer) => integer
                .checked_abs()
                .map(Value::Int)
                .unwrap_or(Value::Real(-f64::from(integer))),
            other => Value::Real(real_of(other)?.abs()),
        },
        Neg => match pop(stack)? {
            Value::Int(integer) => integer
                .checked_neg()
                .map(Value::Int)
                .unwrap_or(Value::Real(-f64::from(integer))),
            other => Value::Real(-real_of(other)?),
        },
        Add | Sub | Mul => {
            let (b, a) = (pop(stack)?, pop(stack)?);
            arithmetic(operator, a, b)?
        }
        Div => {
            let (b, a) = (pop_real(stack)?, pop_real(stack)?);
            if b == 0.0 {
                return Err(fault(FunctionFault::Undefined));
            }
            real(a / b)?
        }
        Idiv | Mod => {
            let (b, a) = (pop_int(stack)?, pop_int(stack)?);
            if b == 0 {
                return Err(fault(FunctionFault::Undefined));
            }
            // i32::MIN / -1 does not fit: the result is a real, as for
            // the other integer operations.
            if operator == Idiv {
                a.checked_div(b)
                    .map(Value::Int)
                    .unwrap_or(Value::Real(-f64::from(a)))
            } else {
                Value::Int(a.checked_rem(b).unwrap_or(0))
            }
        }
        Ceiling | Floor | Round | Truncate => match pop(stack)? {
            Value::Int(integer) => Value::Int(integer),
            other => {
                let x = real_of(other)?;
                let rounded = match operator {
                    Ceiling => x.ceil(),
                    Floor => x.floor(),
                    // PostScript: round half up.
                    Round => (x + 0.5).floor(),
                    _ => x.trunc(),
                };
                Value::Real(rounded)
            }
        },
        Cvi => match pop(stack)? {
            Value::Int(integer) => Value::Int(integer),
            other => {
                let truncated = real_of(other)?.trunc();
                Value::Int(fits_int(truncated).ok_or_else(|| fault(FunctionFault::Range))?)
            }
        },
        Cvr => Value::Real(pop_real(stack)?),
        Sqrt => {
            let x = pop_real(stack)?;
            if x < 0.0 {
                return Err(fault(FunctionFault::Range));
            }
            Value::Real(x.sqrt())
        }
        Ln | Log => {
            let x = pop_real(stack)?;
            if x <= 0.0 {
                return Err(fault(FunctionFault::Range));
            }
            Value::Real(if operator == Ln { x.ln() } else { x.log10() })
        }
        Exp => {
            let (exponent, base) = (pop_real(stack)?, pop_real(stack)?);
            if base == 0.0 && exponent < 0.0 {
                return Err(fault(FunctionFault::Undefined));
            }
            if base < 0.0 && exponent.fract() != 0.0 {
                return Err(fault(FunctionFault::Range));
            }
            real(base.powf(exponent))?
        }
        Sin => Value::Real(pop_real(stack)?.to_radians().sin()),
        Cos => Value::Real(pop_real(stack)?.to_radians().cos()),
        Atan => {
            let (den, num) = (pop_real(stack)?, pop_real(stack)?);
            if num == 0.0 && den == 0.0 {
                return Err(fault(FunctionFault::Undefined));
            }
            let degrees = num.atan2(den).to_degrees();
            Value::Real(if degrees < 0.0 {
                degrees + FULL_TURN
            } else {
                degrees
            })
        }
        And | Or | Xor => {
            let (b, a) = (pop(stack)?, pop(stack)?);
            match (a, b) {
                (Value::Bool(a), Value::Bool(b)) => Value::Bool(match operator {
                    And => a && b,
                    Or => a || b,
                    _ => a != b,
                }),
                (Value::Int(a), Value::Int(b)) => Value::Int(match operator {
                    And => a & b,
                    Or => a | b,
                    _ => a ^ b,
                }),
                _ => return Err(fault(FunctionFault::Type)),
            }
        }
        Not => match pop(stack)? {
            Value::Bool(boolean) => Value::Bool(!boolean),
            Value::Int(integer) => Value::Int(!integer),
            Value::Real(_) => return Err(fault(FunctionFault::Type)),
        },
        Bitshift => {
            let (shift, value) = (pop_int(stack)?, pop_int(stack)?);
            // Bits shifted in are zero in both directions; a shift of the
            // word's width or more leaves nothing.
            let bits = value as u32;
            let shifted = if shift >= 0 {
                bits.checked_shl(shift as u32).unwrap_or(0)
            } else {
                bits.checked_shr(shift.unsigned_abs()).unwrap_or(0)
            };
            Value::Int(shifted as i32)
        }
        True => Value::Bool(true),
        False => Value::Bool(false),
        Eq | Ne => {
            let (b, a) = (pop(stack)?, pop(stack)?);
            let equal = match (a, b) {
                (Value::Bool(a), Value::Bool(b)) => a == b,
                (Value::Bool(_), _) | (_, Value::Bool(_)) => {
                    return Err(fault(FunctionFault::Type))
                }
                (a, b) => real_of(a)? == real_of(b)?,
            };
            Value::Bool(equal == (operator == Eq))
        }
        Ge | Gt | Le | Lt => {
            let (b, a) = (pop_real(stack)?, pop_real(stack)?);
            Value::Bool(match operator {
                Ge => a >= b,
                Gt => a > b,
                Le => a <= b,
                _ => a < b,
            })
        }
        Dup => {
            let top = *stack
                .last()
                .ok_or_else(|| fault(FunctionFault::StackUnderflow))?;
            push(stack, top)?;
            return Ok(());
        }
        Pop => {
            pop(stack)?;
            return Ok(());
        }
        Exch => {
            let (b, a) = (pop(stack)?, pop(stack)?);
            push(stack, b)?;
            push(stack, a)?;
            return Ok(());
        }
        Copy => {
            let count = pop_count(stack)?;
            if count > stack.len() {
                return Err(fault(FunctionFault::StackUnderflow));
            }
            if stack.len() + count > STACK_LIMIT {
                return Err(fault(FunctionFault::StackOverflow));
            }
            let start = stack.len() - count;
            stack.extend_from_within(start..);
            return Ok(());
        }
        Index => {
            let depth = pop_count(stack)?;
            let position = stack
                .len()
                .checked_sub(depth + 1)
                .ok_or_else(|| fault(FunctionFault::StackUnderflow))?;
            let value = stack[position];
            push(stack, value)?;
            return Ok(());
        }
        Roll => {
            let shift = pop_int(stack)?;
            let count = pop_count(stack)?;
            if count > stack.len() {
                return Err(fault(FunctionFault::StackUnderflow));
            }
            if count > 0 {
                let start = stack.len() - count;
                // A positive shift moves the top items toward the bottom
                // of the group, as `roll` does in PostScript.
                let by = shift.rem_euclid(count as i32) as usize;
                stack[start..].rotate_right(by);
            }
            return Ok(());
        }
    };
    push(stack, result)
}

/// A count operand of `copy`, `index` or `roll`: never negative.
fn pop_count(stack: &mut Vec<Value>) -> Result<usize, PdfError> {
    usize::try_from(pop_int(stack)?).map_err(|_| fault(FunctionFault::Range))
}

/// `add`, `sub` and `mul`: an integer result while both operands are
/// integers and it fits, a real otherwise.
fn arithmetic(operator: Operator, a: Value, b: Value) -> Result<Value, PdfError> {
    if let (Value::Int(a), Value::Int(b)) = (a, b) {
        let exact = match operator {
            Operator::Add => a.checked_add(b),
            Operator::Sub => a.checked_sub(b),
            _ => a.checked_mul(b),
        };
        if let Some(integer) = exact {
            return Ok(Value::Int(integer));
        }
    }
    let (a, b) = (real_of(a)?, real_of(b)?);
    real(match operator {
        Operator::Add => a + b,
        Operator::Sub => a - b,
        _ => a * b,
    })
}
