// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! The syntax of PDF objects: ISO 32000-1:2008, 7.2 (lexical conventions)
//! and 7.3 (objects).
//!
//! A [`Lexer`] turns the bytes of a [`Source`](crate::Source) into tokens
//! and a [`Parser`] builds [`Object`]s from them, each remembering the byte
//! span it was read from. [`write_object`] writes an object in one canonical
//! form. What the reader accepts against the format is reported through
//! [`Diagnostics`](crate::Diagnostics); what passes a hard limit of
//! [`limits`] is refused.
//!
//! A stream is read here as far as its dictionary and the offset of its
//! data: where the data ends is given by the dictionary's `Length`, which
//! may be an indirect reference and so cannot be resolved by this layer.

pub(crate) mod chars;
mod cursor;
pub(crate) mod lexer;
pub mod limits;
mod object;
mod parser;
mod serialise;

pub use cursor::Cursor;
pub use lexer::{Lexer, Token, TokenKind};
pub use object::{Dictionary, Name, Object, ObjectId, PdfString, Real, Span, Value};
pub use parser::{IndirectObject, Parser};
pub use serialise::{write_indirect_object, write_object};
