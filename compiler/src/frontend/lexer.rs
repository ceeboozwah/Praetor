//! The `.lang` lexer. Deliberately tiny: the first language surface is
//! integers, variables, functions, return, arithmetic, if, while.

use crate::{Diag, Span};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Fn,
    Let,
    Return,
    If,
    Else,
    While,
    True,
    False,
    Ident(String),
    Int(i64),
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Semicolon,
    Colon,
    ColonColon,
    Arrow,
    Dot,
    DotDot,
    LBracket,
    RBracket,
    Struct,
    Assign,
    Eq,
    /// `=>` — match-arm separator (N22).
    FatArrow,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    Plus,
    Minus,
    Star,
    Amp,
    Region,
    Mut,
    Slash,
    Percent,
    Bang,
    Question,
    // N25A/N25C — bitwise + logical.
    Pipe,
    PipePipe,
    Caret,
    Tilde,
    AmpAmp,
    /// `"..."` — a string literal, decoded to bytes.
    Str(Vec<u8>),
    Eof,
}

impl Tok {
    /// The user-facing spelling for `expected`/`found` diagnostics —
    /// `expected `;`, found `}``, never Debug names like `Semicolon`.
    pub fn describe(&self) -> String {
        match self {
            Tok::Fn | Tok::Let | Tok::Return | Tok::If | Tok::Else
            | Tok::While | Tok::True | Tok::False | Tok::Struct | Tok::Mut => {
                format!("`{}`", format!("{self:?}").to_lowercase())
            }
            Tok::Ident(n) => format!("identifier `{n}`"),
            Tok::Int(_) => "integer literal".to_string(),
            Tok::Str(_) => "string literal".to_string(),
            Tok::LParen => "`(`".into(),
            Tok::RParen => "`)`".into(),
            Tok::LBrace => "`{`".into(),
            Tok::RBrace => "`}`".into(),
            Tok::Comma => "`,`".into(),
            Tok::Semicolon => "`;`".into(),
            Tok::Colon => "`:`".into(),
            Tok::ColonColon => "`::`".into(),
            Tok::Arrow => "`->`".into(),
            Tok::Dot => "`.`".into(),
            Tok::DotDot => "`..`".into(),
            Tok::LBracket => "`[`".into(),
            Tok::RBracket => "`]`".into(),
            Tok::Assign => "`=`".into(),
            Tok::Eq => "`==`".into(),
            Tok::FatArrow => "`=>`".into(),
            Tok::Ne => "`!=`".into(),
            Tok::Lt => "`<`".into(),
            Tok::Gt => "`>`".into(),
            Tok::Le => "`<=`".into(),
            Tok::Ge => "`>=`".into(),
            Tok::Plus => "`+`".into(),
            Tok::Minus => "`-`".into(),
            Tok::Star => "`*`".into(),
            Tok::Amp => "`&`".into(),
            Tok::Region => "`'`".into(),
            Tok::Slash => "`/`".into(),
            Tok::Percent => "`%`".into(),
            Tok::Bang => "`!`".into(),
            Tok::Question => "`?`".into(),
            Tok::Pipe => "`|`".into(),
            Tok::PipePipe => "`||`".into(),
            Tok::Caret => "`^`".into(),
            Tok::Tilde => "`~`".into(),
            Tok::AmpAmp => "`&&`".into(),
            Tok::Eof => "end of file".to_string(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

fn keyword(s: &str) -> Option<Tok> {
    Some(match s {
        "fn" => Tok::Fn,
        "let" => Tok::Let,
        "return" => Tok::Return,
        "if" => Tok::If,
        "else" => Tok::Else,
        "while" => Tok::While,
        "struct" => Tok::Struct,
        "region" => Tok::Region,
        "mut" => Tok::Mut,
        "true" => Tok::True,
        "false" => Tok::False,
        _ => return None,
    })
}

pub fn lex(src: &str) -> Result<Vec<Token>, Vec<Diag>> {
    let mut out = Vec::new();
    let mut errs = Vec::new();
    let mut it = src.chars().peekable();
    let (mut line, mut col) = (1u32, 0u32);
    macro_rules! pos {
        () => {
            Span { line, col }
        };
    }
    while let Some(c) = it.next() {
        col += 1;
        if c == '\n' {
            line += 1;
            col = 0;
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        // Line comments.
        if c == '/' && it.peek() == Some(&'/') {
            while let Some(&c2) = it.peek() {
                if c2 == '\n' {
                    break;
                }
                it.next();
                col += 1;
            }
            continue;
        }
        let span = pos!();
        let tok = match c {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            ',' => Tok::Comma,
            '.' => {
                if it.peek() == Some(&'.') {
                    it.next();
                    col += 1;
                    Tok::DotDot
                } else {
                    Tok::Dot
                }
            }
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            ';' => Tok::Semicolon,
            ':' => {
                if it.peek() == Some(&':') {
                    it.next();
                    col += 1;
                    Tok::ColonColon
                } else {
                    Tok::Colon
                }
            }
            '+' => Tok::Plus,
            '*' => Tok::Star,
            '&' => {
                if it.peek() == Some(&'&') {
                    it.next();
                    col += 1;
                    Tok::AmpAmp
                } else {
                    Tok::Amp
                }
            }
            '|' => {
                if it.peek() == Some(&'|') {
                    it.next();
                    col += 1;
                    Tok::PipePipe
                } else {
                    Tok::Pipe
                }
            }
            '^' => Tok::Caret,
            '~' => Tok::Tilde,
            '/' => Tok::Slash,
            '%' => Tok::Percent,
            '?' => Tok::Question,
            '!' => {
                if it.peek() == Some(&'=') {
                    it.next();
                    col += 1;
                    Tok::Ne
                } else {
                    Tok::Bang
                }
            }
            '=' => {
                if it.peek() == Some(&'=') {
                    it.next();
                    col += 1;
                    Tok::Eq
                } else if it.peek() == Some(&'>') {
                    it.next();
                    col += 1;
                    Tok::FatArrow
                } else {
                    Tok::Assign
                }
            }
            '<' => {
                if it.peek() == Some(&'=') {
                    it.next();
                    col += 1;
                    Tok::Le
                } else {
                    Tok::Lt
                }
            }
            '>' => {
                if it.peek() == Some(&'=') {
                    it.next();
                    col += 1;
                    Tok::Ge
                } else {
                    Tok::Gt
                }
            }
            '-' => {
                if it.peek() == Some(&'>') {
                    it.next();
                    col += 1;
                    Tok::Arrow
                } else {
                    Tok::Minus
                }
            }
            '"' => {
                // C-style escapes; the literal carries decoded bytes.
                let mut bytes = Vec::new();
                let mut bad = None;
                macro_rules! next {
                    () => {{
                        let c = it.next();
                        if c.is_some() {
                            col += 1;
                        }
                        c
                    }};
                }
                loop {
                    match next!() {
                        None => {
                            bad = Some("unterminated string literal".to_string());
                            break;
                        }
                        Some('"') => break,
                        Some('\n') => {
                            bad = Some("newline in string literal".to_string());
                            break;
                        }
                        Some('\\') => {
                            match next!() {
                                Some('n') => bytes.push(b'\n'),
                                Some('t') => bytes.push(b'\t'),
                                Some('r') => bytes.push(b'\r'),
                                Some('0') => bytes.push(0),
                                Some('\\') => bytes.push(b'\\'),
                                Some('"') => bytes.push(b'"'),
                                Some('x') => match (next!(), next!()) {
                                    (Some(a), Some(b))
                                        if a.is_ascii_hexdigit()
                                            && b.is_ascii_hexdigit() =>
                                    {
                                        bytes.push(
                                            u8::from_str_radix(&format!("{a}{b}"), 16)
                                                .unwrap_or(0),
                                        );
                                    }
                                    _ => {
                                        bad = Some(
                                            "bad \\x escape (want \\xNN)".to_string(),
                                        );
                                        break;
                                    }
                                },
                                other => {
                                    bad = Some(format!(
                                        "unknown escape `\\{}`",
                                        other.map(|c| c.to_string()).unwrap_or_default()
                                    ));
                                    break;
                                }
                            }
                        }
                        Some(c2) => {
                            let mut buf = [0u8; 4];
                            bytes.extend_from_slice(c2.encode_utf8(&mut buf).as_bytes());
                        }
                    }
                }
                match bad {
                    Some(m) => {
                        errs.push(Diag::new(span, m));
                        continue;
                    }
                    None => Tok::Str(bytes),
                }
            }
            _ if c.is_ascii_digit() => {
                // N25B — `0x`/`0X` hex, `0b`/`0B` binary, plain
                // decimal otherwise. `_` separators between digits.
                let mut radix = 10u32;
                let mut n: i64 = 0;
                let mut any = false;
                if c == '0' {
                    if let Some(&p) = it.peek() {
                        match p {
                            'x' | 'X' => {
                                radix = 16;
                                it.next();
                                col += 1;
                            }
                            'b' | 'B' => {
                                radix = 2;
                                it.next();
                                col += 1;
                            }
                            _ => {}
                        }
                    }
                }
                if radix == 10 {
                    n = c.to_digit(10).unwrap() as i64;
                    any = true;
                }
                let mut overflow = false;
                let mut bad_sep = false;
                let mut last_sep = false;
                while let Some(&d) = it.peek() {
                    if d == '_' {
                        it.next();
                        col += 1;
                        if !any {
                            bad_sep = true;
                        }
                        last_sep = true;
                    } else if let Some(v) = d.to_digit(radix) {
                        it.next();
                        col += 1;
                        last_sep = false;
                        any = true;
                        match n
                            .checked_mul(radix as i64)
                            .and_then(|m| m.checked_add(v as i64))
                        {
                            Some(m) => n = m,
                            None => overflow = true,
                        }
                    } else {
                        break;
                    }
                }
                if last_sep {
                    bad_sep = true;
                }
                let name = if radix == 16 {
                    "hex"
                } else if radix == 2 {
                    "binary"
                } else {
                    "decimal"
                };
                if !any {
                    errs.push(Diag::new(
                        span,
                        format!("expected {name} digits after literal prefix"),
                    ));
                    continue;
                }
                if let Some(&d) = it.peek() {
                    if d.is_ascii_alphanumeric() {
                        errs.push(Diag::new(
                            span,
                            format!("invalid digit `{d}` in {name} literal"),
                        ));
                        continue;
                    }
                }
                if bad_sep {
                    errs.push(Diag::new(
                        span,
                        "misplaced `_` in integer literal".to_string(),
                    ));
                    continue;
                }
                if overflow {
                    errs.push(Diag::new(span, "integer literal too large".to_string()));
                    continue;
                }
                Tok::Int(n)
            }
            _ if c.is_alphabetic() || c == '_' => {
                let mut name = String::from(c);
                while let Some(&d) = it.peek() {
                    if d.is_alphanumeric() || d == '_' {
                        name.push(d);
                        it.next();
                        col += 1;
                    } else {
                        break;
                    }
                }
                keyword(&name).unwrap_or(Tok::Ident(name))
            }
            _ => {
                errs.push(Diag::new(span, format!("unexpected character `{c}`")));
                continue;
            }
        };
        out.push(Token { tok, span });
    }
    out.push(Token {
        tok: Tok::Eof,
        span: pos!(),
    });
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}
