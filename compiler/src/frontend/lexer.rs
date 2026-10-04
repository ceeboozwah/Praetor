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
    /// `"..."` — a string literal, decoded to bytes.
    Str(Vec<u8>),
    Eof,
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
            '&' => Tok::Amp,
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
                let mut n = c.to_digit(10).unwrap() as i64;
                while let Some(&d) = it.peek() {
                    if let Some(v) = d.to_digit(10) {
                        n = n * 10 + v as i64;
                        it.next();
                        col += 1;
                    } else {
                        break;
                    }
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
