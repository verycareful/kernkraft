// SPDX-License-Identifier: AGPL-3.0-or-later
// Copyright (c) 2026 Sricharan Suresh (github.com/verycareful)

//! Lexer tests. Inputs and expected values are the examples of ISO 32000
//! 7.2 and 7.3 unless a test builds its own input.

use kernkraft_pdf::syntax::limits::{
    MAX_NAME_BYTES, MAX_STRING_BYTES, MAX_TOKEN_BYTES, PORTABLE_NAME_BYTES,
};
use kernkraft_pdf::syntax::{Lexer, Name, PdfString, Real, Span, Token, TokenKind};
use kernkraft_pdf::{DiagnosticKind, Diagnostics, Limit, PdfError, PortableLimit};

use crate::{
    assert_limit, assert_unexpected_end, assert_unexpected_token, kinds_and_offsets, memory,
};

type Lexed = (Vec<Token>, Vec<(DiagnosticKind, u64)>);

/// Every token up to and including `End`, with the diagnostics raised.
fn lex(input: &[u8]) -> Result<Lexed, PdfError> {
    let source = memory(input);
    let mut lexer = Lexer::new(&source);
    let mut diagnostics = Diagnostics::new();
    let mut tokens = Vec::new();
    loop {
        let token = lexer.next_token(&mut diagnostics)?;
        let done = token.kind == TokenKind::End;
        tokens.push(token);
        if done {
            return Ok((tokens, kinds_and_offsets(&diagnostics)));
        }
    }
}

/// The token kinds before `End`, for input that raises no diagnostic.
fn kinds(input: &[u8]) -> Vec<TokenKind> {
    let (tokens, diagnostics) = lex(input).unwrap();
    assert!(
        diagnostics.is_empty(),
        "{diagnostics:?} for {:?}",
        String::from_utf8_lossy(input)
    );
    let mut kinds: Vec<TokenKind> = tokens.into_iter().map(|token| token.kind).collect();
    assert_eq!(kinds.pop(), Some(TokenKind::End));
    kinds
}

fn integer(value: i64) -> TokenKind {
    TokenKind::Integer(value)
}

fn real(value: f64) -> TokenKind {
    TokenKind::Real(Real::new(value).unwrap())
}

fn name(bytes: &[u8]) -> TokenKind {
    TokenKind::Name(Name::new(bytes))
}

fn string(bytes: &[u8]) -> TokenKind {
    TokenKind::String(PdfString::new(bytes))
}

fn keyword(bytes: &[u8]) -> TokenKind {
    TokenKind::Keyword(bytes.to_vec())
}

#[test]
fn integers_of_example_1() {
    assert_eq!(
        kinds(b"123 43445 +17 -98 0"),
        [
            integer(123),
            integer(43445),
            integer(17),
            integer(-98),
            integer(0)
        ]
    );
}

#[test]
fn reals_of_example_2() {
    assert_eq!(
        kinds(b"34.5 -3.62 +123.6 4. -.002 0.0"),
        [
            real(34.5),
            real(-3.62),
            real(123.6),
            real(4.0),
            real(-0.002),
            real(0.0)
        ]
    );
}

#[test]
fn non_numbers_are_keywords() {
    assert_eq!(
        kinds(b"--5 1e5 0.0.5 + . 16#FFFE"),
        [
            keyword(b"--5"),
            keyword(b"1e5"),
            keyword(b"0.0.5"),
            keyword(b"+"),
            keyword(b"."),
            keyword(b"16#FFFE")
        ]
    );
}

#[test]
fn keywords_and_brackets() {
    assert_eq!(
        kinds(b"true false null obj endobj R [ ] << >> { }"),
        [
            keyword(b"true"),
            keyword(b"false"),
            keyword(b"null"),
            keyword(b"obj"),
            keyword(b"endobj"),
            keyword(b"R"),
            TokenKind::ArrayOpen,
            TokenKind::ArrayClose,
            TokenKind::DictOpen,
            TokenKind::DictClose,
            keyword(b"{"),
            keyword(b"}")
        ]
    );
}

#[test]
fn delimiters_end_a_token_without_space() {
    assert_eq!(
        kinds(b"1/A(b)<41>[2]<</K 3>>"),
        [
            integer(1),
            name(b"A"),
            string(b"b"),
            string(b"A"),
            TokenKind::ArrayOpen,
            integer(2),
            TokenKind::ArrayClose,
            TokenKind::DictOpen,
            name(b"K"),
            integer(3),
            TokenKind::DictClose
        ]
    );
    assert_eq!(
        kinds(b"a{b}c"),
        [
            keyword(b"a"),
            keyword(b"{"),
            keyword(b"b"),
            keyword(b"}"),
            keyword(b"c")
        ]
    );
}

#[test]
fn comment_separates_tokens() {
    assert_eq!(
        kinds(b"abc% comment ( /% ) blah blah blah\n123"),
        [keyword(b"abc"), integer(123)]
    );
}

#[test]
fn comment_ends_at_cr_too() {
    assert_eq!(kinds(b"1%x\r2"), [integer(1), integer(2)]);
    assert_eq!(kinds(b"1%x"), [integer(1)]);
}

#[test]
fn all_six_whitespace_bytes_separate() {
    assert_eq!(
        kinds(b"1\x002\t3\n4\x0c5\r6 7"),
        (1..=7).map(integer).collect::<Vec<_>>()
    );
}

#[test]
fn names_of_table_4() {
    let rows: [(&[u8], &[u8]); 11] = [
        (b"/Name1", b"Name1"),
        (b"/ASomewhatLongerName", b"ASomewhatLongerName"),
        (
            b"/A;Name_With-Various***Characters?",
            b"A;Name_With-Various***Characters?",
        ),
        (b"/1.2", b"1.2"),
        (b"/$$", b"$$"),
        (b"/@pattern", b"@pattern"),
        (b"/.notdef", b".notdef"),
        (b"/lime#20Green", b"lime Green"),
        (b"/paired#28#29parentheses", b"paired()parentheses"),
        (b"/The_Key_of_F#23_Minor", b"The_Key_of_F#_Minor"),
        (b"/A#42", b"AB"),
    ];
    for (syntax, resulting) in rows {
        assert_eq!(kinds(syntax), [name(resulting)]);
    }
}

#[test]
fn solidus_alone_is_the_empty_name() {
    assert_eq!(kinds(b"/ /A"), [name(b""), name(b"A")]);
    assert_eq!(kinds(b"/"), [name(b"")]);
}

#[test]
fn bad_name_escape_is_kept_and_reported() {
    let input = b"/A#G1 /B#4";
    let (tokens, diagnostics) = lex(input).unwrap();
    assert_eq!(tokens[0].kind, name(b"A#G1"));
    assert_eq!(tokens[1].kind, name(b"B#4"));
    let first = input.iter().position(|&b| b == b'#').unwrap() as u64;
    let second = input.iter().rposition(|&b| b == b'#').unwrap() as u64;
    assert_eq!(
        diagnostics,
        [
            (DiagnosticKind::InvalidNameEscape, first),
            (DiagnosticKind::InvalidNameEscape, second)
        ]
    );
}

#[test]
fn nul_in_name_is_kept_and_reported() {
    let (tokens, diagnostics) = lex(b"/A#00B").unwrap();
    assert_eq!(tokens[0].kind, name(b"A\0B"));
    assert_eq!(diagnostics, [(DiagnosticKind::NulInName, 2)]);
}

#[test]
fn name_of_128_bytes_is_reported() {
    let mut portable = b"/".to_vec();
    portable.extend(std::iter::repeat_n(b'a', PORTABLE_NAME_BYTES));
    assert_eq!(kinds(&portable), [name(&portable[1..])]);

    let mut longer = portable.clone();
    longer.push(b'a');
    let (tokens, diagnostics) = lex(&longer).unwrap();
    assert_eq!(tokens[0].kind, name(&longer[1..]));
    assert_eq!(
        diagnostics,
        [(
            DiagnosticKind::BeyondPortableLimit(PortableLimit::NameLength),
            0
        )]
    );
}

#[test]
fn literal_strings_of_example_1() {
    assert_eq!(kinds(b"()"), [string(b"")]);
    assert_eq!(kinds(b"(a (b) c)"), [string(b"a (b) c")]);
    assert_eq!(kinds(b"( * ! & } ^ % )"), [string(b" * ! & } ^ % ")]);
}

#[test]
fn escapes_of_table_3() {
    assert_eq!(
        kinds(br"(\n\r\t\b\f\(\)\\)"),
        [string(&[0x0A, 0x0D, 0x09, 0x08, 0x0C, 0x28, 0x29, 0x5C])]
    );
}

#[test]
fn octal_escapes_of_example_5() {
    assert_eq!(kinds(br"(\0053)"), [string(&[0o005, b'3'])]);
    assert_eq!(kinds(br"(\053)"), [string(b"+")]);
    assert_eq!(kinds(br"(\53)"), [string(b"+")]);
    assert_eq!(kinds(br"(\245)"), [string(&[0o245])]);
    assert_eq!(kinds(br"(\777)"), [string(&[(0o777 & 0xFF) as u8])]);
    assert_eq!(kinds(br"(\5)"), [string(&[5])]);
    assert_eq!(kinds(br"(\58)"), [string(&[5, b'8'])]);
}

#[test]
fn unknown_escape_drops_the_backslash() {
    assert_eq!(kinds(br"(a\qb)"), [string(b"aqb")]);
}

#[test]
fn line_continuation_of_example_2() {
    for end_of_line in [&b"\n"[..], b"\r", b"\r\n"] {
        let mut input = b"(These \\".to_vec();
        input.extend_from_slice(end_of_line);
        input.extend_from_slice(b"two)");
        assert_eq!(kinds(&input), [string(b"These two")]);
    }
}

#[test]
fn bare_end_of_line_is_one_lf() {
    for end_of_line in [&b"\n"[..], b"\r", b"\r\n"] {
        let mut input = b"(a".to_vec();
        input.extend_from_slice(end_of_line);
        input.extend_from_slice(b"b)");
        assert_eq!(kinds(&input), [string(b"a\nb")]);
    }
}

#[test]
fn hex_strings_of_7_3_4_3() {
    assert_eq!(
        kinds(b"<4E6F762073686D6F7A206B6120706F702E>"),
        [string(b"Nov shmoz ka pop.")]
    );
    assert_eq!(kinds(b"<901FA3>"), [string(&[0x90, 0x1F, 0xA3])]);
    assert_eq!(kinds(b"<901FA>"), [string(&[0x90, 0x1F, 0xA0])]);
    assert_eq!(kinds(b"< 90 1f\nA3 >"), [string(&[0x90, 0x1F, 0xA3])]);
    assert_eq!(kinds(b"<>"), [string(b"")]);
}

#[test]
fn bad_hex_digit_is_skipped_and_reported() {
    let (tokens, diagnostics) = lex(b"<4X1>").unwrap();
    assert_eq!(tokens[0].kind, string(&[0x41]));
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::InvalidHexDigit { byte: b'X' }, 2)]
    );
}

#[test]
fn integer_range() {
    assert_eq!(
        kinds(format!("{} {}", i32::MAX, i32::MIN).as_bytes()),
        [integer(i64::from(i32::MAX)), integer(i64::from(i32::MIN))]
    );

    let above = i64::from(i32::MAX) + 1;
    let (tokens, diagnostics) = lex(above.to_string().as_bytes()).unwrap();
    assert_eq!(tokens[0].kind, integer(above));
    assert_eq!(
        diagnostics,
        [(
            DiagnosticKind::BeyondPortableLimit(PortableLimit::Integer),
            0
        )]
    );

    let below = i64::from(i32::MIN) - 1;
    let (tokens, diagnostics) = lex(below.to_string().as_bytes()).unwrap();
    assert_eq!(tokens[0].kind, integer(below));
    assert_eq!(
        diagnostics,
        [(
            DiagnosticKind::BeyondPortableLimit(PortableLimit::Integer),
            0
        )]
    );

    let (tokens, _) = lex(i64::MAX.to_string().as_bytes()).unwrap();
    assert_eq!(tokens[0].kind, integer(i64::MAX));

    // One above i64::MAX, written out because no integer type here holds it.
    let (tokens, diagnostics) = lex(b"9223372036854775808").unwrap();
    assert_eq!(tokens[0].kind, real(9_223_372_036_854_775_808.0));
    assert_eq!(diagnostics, [(DiagnosticKind::IntegerOverflow, 0)]);
}

#[test]
fn real_range() {
    let above_portable = format!("1{}.0", "0".repeat(39));
    let (tokens, diagnostics) = lex(above_portable.as_bytes()).unwrap();
    assert_eq!(tokens[0].kind, real(1e39));
    assert_eq!(
        diagnostics,
        [(DiagnosticKind::BeyondPortableLimit(PortableLimit::Real), 0)]
    );

    // f64::MAX is below 1e309, so 1e400 has no finite f64.
    for (sign, clamped) in [("", f64::MAX), ("-", -f64::MAX)] {
        let not_finite = format!("{sign}1{}.0", "0".repeat(400));
        let (tokens, diagnostics) = lex(not_finite.as_bytes()).unwrap();
        assert_eq!(tokens[0].kind, real(clamped));
        assert_eq!(diagnostics, [(DiagnosticKind::RealOverflow, 0)]);
    }
}

#[test]
fn spans_cover_each_token() {
    let input = b"  /Ab (c) <41> 12 >>";
    let (tokens, _) = lex(input).unwrap();
    let spans: Vec<(u64, u64)> = tokens.iter().map(|t| (t.span.start, t.span.end)).collect();
    let end = input.len() as u64;
    assert_eq!(
        spans,
        [(2, 5), (6, 9), (10, 14), (15, 17), (18, 20), (end, end)]
    );
    for token in &tokens[..5] {
        let text = &input[token.span.start as usize..token.span.end as usize];
        assert!(!text.contains(&b' '), "{text:?}");
    }
}

#[test]
fn unterminated_constructs() {
    for input in [&b"(abc"[..], b"(a(b)", b"<41", b"(a\\"] {
        assert_unexpected_end(lex(input), 0);
    }
    assert_unexpected_end(lex(b"  (x"), 2);
}

#[test]
fn stray_closers() {
    assert_unexpected_token(lex(b")"), 0, ")");
    assert_unexpected_token(lex(b"1 >"), 2, ">");
    assert_unexpected_token(lex(b"1 > >"), 2, ">");
}

#[test]
fn name_and_keyword_caps_at_their_limits() {
    let mut name_at_cap = b"/".to_vec();
    name_at_cap.extend(std::iter::repeat_n(b'n', MAX_NAME_BYTES));
    let (tokens, _) = lex(&name_at_cap).unwrap();
    assert_eq!(tokens[0].kind, name(&name_at_cap[1..]));
    name_at_cap.push(b'n');
    assert_limit(lex(&name_at_cap), 0, Limit::NameBytes);

    let mut keyword_at_cap = vec![b'k'; MAX_TOKEN_BYTES];
    assert_eq!(kinds(&keyword_at_cap), [keyword(&keyword_at_cap)]);
    keyword_at_cap.push(b'k');
    assert_limit(lex(&keyword_at_cap), 0, Limit::TokenBytes);

    let mut digits_at_cap = vec![b'1'; MAX_TOKEN_BYTES + 1];
    digits_at_cap[0] = b' ';
    assert!(lex(&digits_at_cap).is_ok());
    digits_at_cap.push(b'1');
    assert_limit(lex(&digits_at_cap), 1, Limit::TokenBytes);
}

/// The first token of `input` is a string of `expected_len` bytes.
fn assert_string_of(input: &[u8], expected_len: usize) {
    let source = memory(input);
    let mut lexer = Lexer::new(&source);
    let token = lexer.next_token(&mut Diagnostics::new()).unwrap();
    match token.kind {
        TokenKind::String(string) => assert_eq!(string.as_bytes().len(), expected_len),
        other => panic!("expected a string, got {other:?}"),
    }
    assert_eq!(
        token.span,
        Span {
            start: 0,
            end: input.len() as u64
        }
    );
}

#[test]
fn string_caps_at_their_limits() {
    let mut literal = Vec::with_capacity(MAX_STRING_BYTES + 3);
    literal.push(b'(');
    literal.resize(MAX_STRING_BYTES + 1, b's');
    literal.push(b')');
    assert_string_of(&literal, MAX_STRING_BYTES);
    literal.insert(1, b's');
    assert_limit(lex(&literal), 0, Limit::StringBytes);
    drop(literal);

    let mut hex = Vec::with_capacity(2 * MAX_STRING_BYTES + 4);
    hex.push(b'<');
    hex.resize(2 * MAX_STRING_BYTES + 1, b'4');
    hex.push(b'>');
    assert_string_of(&hex, MAX_STRING_BYTES);
    // One more digit is an odd final digit: one more byte.
    hex.insert(1, b'4');
    assert_limit(lex(&hex), 0, Limit::StringBytes);
}

#[test]
fn seek_then_lex() {
    let source = memory(b"1 2 3");
    let mut lexer = Lexer::new(&source);
    let mut diagnostics = Diagnostics::new();
    for _ in 0..3 {
        lexer.next_token(&mut diagnostics).unwrap();
    }
    lexer.seek(2).unwrap();
    assert_eq!(lexer.position(), 2);
    assert_eq!(
        lexer.next_token(&mut diagnostics).unwrap(),
        Token {
            kind: integer(2),
            span: Span { start: 2, end: 3 }
        }
    );
    assert_eq!(lexer.position(), 3);
}

#[test]
fn the_cursor_reads_raw_bytes_after_a_token() {
    let source = memory(b"ID \x00\xFF(raw");
    let mut lexer = Lexer::new(&source);
    assert_eq!(
        lexer.next_token(&mut Diagnostics::new()).unwrap().kind,
        keyword(b"ID")
    );
    let cursor = lexer.cursor();
    assert_eq!(cursor.next().unwrap(), Some(b' '));
    assert_eq!(&cursor.read_exact(2).unwrap()[..], [0x00, 0xFF]);
    assert_eq!(lexer.position(), 5);
}
