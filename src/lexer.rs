//! Hand-written lexer for MML.

use crate::ast::Span;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    // literals / identifiers
    Ident(String),
    Int(i64),
    // keywords
    Fn,
    Struct,
    Let,
    Return,
    If,
    Else,
    While,
    Region,
    New,
    Free,
    Promote,
    Handle,
    Mutex,
    Spawn,
    Join,
    Own,
    Ref,
    Mut,
    True,
    False,
    // punctuation
    LParen,
    RParen,
    LBrace,
    RBrace,
    Comma,
    Colon,
    Semi,
    Dot,
    Arrow, // ->
    Amp,   // &
    Star,  // *
    // operators
    Plus,
    Minus,
    Slash,
    EqEq,
    Ne,
    Lt,
    Gt,
    AndAnd,
    OrOr,
    Pipe,
    Caret,
    Shl,
    Shr,
    Assign, // =
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

pub struct Lexer<'a> {
    src: &'a [u8],
    pos: usize,
    line: u32,
    col: u32,
}

impl<'a> Lexer<'a> {
    pub fn new(src: &'a str) -> Self {
        Lexer {
            src: src.as_bytes(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    fn span(&self) -> Span {
        Span {
            line: self.line,
            col: self.col,
        }
    }

    fn peek(&self) -> u8 {
        *self.src.get(self.pos).unwrap_or(&0)
    }

    fn peek2(&self) -> u8 {
        *self.src.get(self.pos + 1).unwrap_or(&0)
    }

    fn bump(&mut self) -> u8 {
        let c = self.peek();
        self.pos += 1;
        if c == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        c
    }

    fn skip_trivia(&mut self) {
        loop {
            let c = self.peek();
            if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
                self.bump();
            } else if c == b'/' && self.peek2() == b'/' {
                // line comment
                while self.peek() != b'\n' && self.peek() != 0 {
                    self.bump();
                }
            } else {
                break;
            }
        }
    }

    pub fn tokenize(mut self) -> Result<Vec<Token>, String> {
        let mut out = Vec::new();
        loop {
            self.skip_trivia();
            let span = self.span();
            let c = self.peek();
            if c == 0 {
                out.push(Token { tok: Tok::Eof, span });
                break;
            }
            let tok = if c.is_ascii_alphabetic() || c == b'_' {
                self.lex_ident()
            } else if c.is_ascii_digit() {
                self.lex_number()?
            } else {
                self.lex_symbol()?
            };
            out.push(Token { tok, span });
        }
        Ok(out)
    }

    fn lex_ident(&mut self) -> Tok {
        let start = self.pos;
        while self.peek().is_ascii_alphanumeric() || self.peek() == b'_' {
            self.bump();
        }
        let word = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        match word {
            "fn" => Tok::Fn,
            "struct" => Tok::Struct,
            "let" => Tok::Let,
            "return" => Tok::Return,
            "if" => Tok::If,
            "else" => Tok::Else,
            "while" => Tok::While,
            "region" => Tok::Region,
            "new" => Tok::New,
            "free" => Tok::Free,
            "promote" => Tok::Promote,
            "handle" => Tok::Handle,
            "mutex" => Tok::Mutex,
            "spawn" => Tok::Spawn,
            "join" => Tok::Join,
            "own" => Tok::Own,
            "ref" => Tok::Ref,
            "mut" => Tok::Mut,
            "true" => Tok::True,
            "false" => Tok::False,
            _ => Tok::Ident(word.to_string()),
        }
    }

    fn lex_number(&mut self) -> Result<Tok, String> {
        let start = self.pos;
        while self.peek().is_ascii_digit() {
            self.bump();
        }
        let text = std::str::from_utf8(&self.src[start..self.pos]).unwrap();
        text.parse::<i64>()
            .map(Tok::Int)
            .map_err(|e| format!("invalid integer literal `{text}`: {e}"))
    }

    fn lex_symbol(&mut self) -> Result<Tok, String> {
        let span = self.span();
        let c = self.bump();
        let tok = match c {
            b'(' => Tok::LParen,
            b')' => Tok::RParen,
            b'{' => Tok::LBrace,
            b'}' => Tok::RBrace,
            b',' => Tok::Comma,
            b':' => Tok::Colon,
            b';' => Tok::Semi,
            b'.' => Tok::Dot,
            b'+' => Tok::Plus,
            b'*' => Tok::Star,
            b'/' => Tok::Slash,
            b'^' => Tok::Caret,
            b'-' => {
                if self.peek() == b'>' {
                    self.bump();
                    Tok::Arrow
                } else {
                    Tok::Minus
                }
            }
            b'=' => {
                if self.peek() == b'=' {
                    self.bump();
                    Tok::EqEq
                } else {
                    Tok::Assign
                }
            }
            b'!' => {
                if self.peek() == b'=' {
                    self.bump();
                    Tok::Ne
                } else {
                    return Err(format!("unexpected `!` at {span}"));
                }
            }
            b'<' => {
                if self.peek() == b'<' {
                    self.bump();
                    Tok::Shl
                } else {
                    Tok::Lt
                }
            }
            b'>' => {
                if self.peek() == b'>' {
                    self.bump();
                    Tok::Shr
                } else {
                    Tok::Gt
                }
            }
            b'&' => {
                if self.peek() == b'&' {
                    self.bump();
                    Tok::AndAnd
                } else {
                    Tok::Amp
                }
            }
            b'|' => {
                if self.peek() == b'|' {
                    self.bump();
                    Tok::OrOr
                } else {
                    Tok::Pipe
                }
            }
            other => {
                return Err(format!(
                    "unexpected character `{}` at {span}",
                    other as char
                ))
            }
        };
        Ok(tok)
    }
}

pub fn lex(src: &str) -> Result<Vec<Token>, String> {
    Lexer::new(src).tokenize()
}
