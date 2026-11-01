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
    Int(u64),
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

/// Non-semantic source content carried before a token (N39A). The
/// parser ignores it; the formatter needs it — comments are the only
/// trivia the language has (there are no `/* */` block comments).
#[derive(Debug, Clone, PartialEq)]
pub enum Trivia {
    /// A `//...` line comment — `text` is the comment's exact source
    /// (including `//`). `same_line` is true when the comment shares a
    /// line with the preceding token (a trailing comment).
    Line { text: String, same_line: bool },
    /// One or more fully-blank source lines before the token.
    BlankLines(u32),
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
    /// The token's exact source text — `0xFF`, `"a\x41"`, escapes and
    /// separators intact (N39A: the formatter respells nothing).
    pub text: String,
    /// Comments and blank lines appearing before this token.
    pub leading: Vec<Trivia>,
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
    let mut it = src.char_indices().peekable();
    let (mut line, mut col) = (1u32, 0u32);
    // Byte offset just past the last consumed char — token text is
    // `src[start..end]`.
    let mut end_idx: usize;
    // Trivia collected since the previous token, attached to the next.
    let mut leading: Vec<Trivia> = Vec::new();
    // Newlines seen since the last token or comment — blank lines
    // between content are `pending - 1`.
    let mut pending_lines: u32 = 0;
    macro_rules! pos {
        () => {
            Span { line, col }
        };
    }
    /// Consume one char, tracking the byte cursor and column.
    macro_rules! bump {
        () => {{
            let r = it.next();
            if let Some((bi, c)) = r {
                end_idx = bi + c.len_utf8();
                col += 1;
            }
            r.map(|(_, c)| c)
        }};
    }
    macro_rules! peek {
        () => {
            it.peek().map(|&(_, c)| c)
        };
    }
    /// Fold accumulated blank lines into `leading`.
    macro_rules! flush_blanks {
        () => {
            if pending_lines > 1 {
                leading.push(Trivia::BlankLines(pending_lines - 1));
            }
            pending_lines = 0;
        };
    }
    while let Some((bi, c)) = it.next() {
        end_idx = bi + c.len_utf8();
        col += 1;
        if c == '\n' {
            line += 1;
            col = 0;
            pending_lines += 1;
            continue;
        }
        if c.is_whitespace() {
            continue;
        }
        // Line comments — trivia, not tokens.
        if c == '/' && peek!() == Some('/') {
            let cstart = bi;
            while let Some(c2) = peek!() {
                if c2 == '\n' {
                    break;
                }
                bump!();
            }
            // A comment with no newline since the previous token is a
            // trailing comment — `code; // like this`.
            let same_line = pending_lines == 0 && !out.is_empty();
            flush_blanks!();
            leading.push(Trivia::Line {
                text: src[cstart..end_idx].to_string(),
                same_line,
            });
            continue;
        }
        flush_blanks!();
        let span = pos!();
        let start_idx = bi;
        let tok = match c {
            '(' => Tok::LParen,
            ')' => Tok::RParen,
            '{' => Tok::LBrace,
            '}' => Tok::RBrace,
            ',' => Tok::Comma,
            '.' => {
                if peek!() == Some('.') {
                    bump!();
                    Tok::DotDot
                } else {
                    Tok::Dot
                }
            }
            '[' => Tok::LBracket,
            ']' => Tok::RBracket,
            ';' => Tok::Semicolon,
            ':' => {
                if peek!() == Some(':') {
                    bump!();
                    Tok::ColonColon
                } else {
                    Tok::Colon
                }
            }
            '+' => Tok::Plus,
            '*' => Tok::Star,
            '&' => {
                if peek!() == Some('&') {
                    bump!();
                    Tok::AmpAmp
                } else {
                    Tok::Amp
                }
            }
            '|' => {
                if peek!() == Some('|') {
                    bump!();
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
                if peek!() == Some('=') {
                    bump!();
                    Tok::Ne
                } else {
                    Tok::Bang
                }
            }
            '=' => {
                if peek!() == Some('=') {
                    bump!();
                    Tok::Eq
                } else if peek!() == Some('>') {
                    bump!();
                    Tok::FatArrow
                } else {
                    Tok::Assign
                }
            }
            '<' => {
                if peek!() == Some('=') {
                    bump!();
                    Tok::Le
                } else {
                    Tok::Lt
                }
            }
            '>' => {
                if peek!() == Some('=') {
                    bump!();
                    Tok::Ge
                } else {
                    Tok::Gt
                }
            }
            '-' => {
                if peek!() == Some('>') {
                    bump!();
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
                    () => {
                        bump!()
                    };
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
                let mut n: u64 = 0;
                let mut any = false;
                if c == '0' {
                    if let Some(p) = peek!() {
                        match p {
                            'x' | 'X' => {
                                radix = 16;
                                bump!();
                            }
                            'b' | 'B' => {
                                radix = 2;
                                bump!();
                            }
                            _ => {}
                        }
                    }
                }
                if radix == 10 {
                    n = c.to_digit(10).unwrap() as u64;
                    any = true;
                }
                let mut overflow = false;
                let mut bad_sep = false;
                let mut last_sep = false;
                while let Some(d) = peek!() {
                    if d == '_' {
                        bump!();
                        if !any {
                            bad_sep = true;
                        }
                        last_sep = true;
                    } else if let Some(v) = d.to_digit(radix) {
                        bump!();
                        last_sep = false;
                        any = true;
                        match n
                            .checked_mul(radix as u64)
                            .and_then(|m| m.checked_add(v as u64))
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
                if let Some(d) = peek!() {
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
                while let Some(d) = peek!() {
                    if d.is_alphanumeric() || d == '_' {
                        name.push(d);
                        bump!();
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
        out.push(Token {
            tok,
            span,
            text: src[start_idx..end_idx].to_string(),
            leading: std::mem::take(&mut leading),
        });
    }
    out.push(Token {
        tok: Tok::Eof,
        span: pos!(),
        text: String::new(),
        leading,
    });
    if errs.is_empty() {
        Ok(out)
    } else {
        Err(errs)
    }
}

/// Canonical token-stream dump — the Stage-0 oracle surface for the
/// self-hosted lexer (BOOTSTRAP.md §4). One record per line, bytes
/// identical between the two implementations on identical input:
///
/// ```text
/// T <line> <col> <kind> <escaped-text> [<decoded>]   a token (EOF included; `-` = empty text)
/// C <0|1> <escaped-text>                             leading line comment (same-line flag)
/// B <n>                                              leading run of n blank lines
/// ! <line> <col> <escaped-msg>                       lex error (file lexes no tokens)
/// ```
///
/// Trivia lines precede the token they attach to. Kind is the variant
/// name lowercased (`fn`, `ident`, `int`, `str`, `lparen`, `eof`, …).
/// The text field preserves every source byte (`0xFF`, escapes,
/// separators); payload variants append a decoded field so the dump
/// also pins the *semantic* token value, not just its spelling —
/// `int` gets the parsed `i64`, `str` the decoded byte string. `col`
/// is the char column the lexer tracks — both sides must count
/// identically.
///
/// Escaping: `\\` → `\\\\`, `\n` → `\\n`, `\r` → `\\r`, `\t` → `\\t`,
/// and (in byte strings) non-printable/non-ASCII → `\xNN`; all other
/// bytes pass through raw. Source text can never contain a real
/// newline in a token or comment (comments end at newline;
/// unterminated strings error), but escape defensively anyway.
pub fn dump_tokens(src: &str) -> String {
    fn esc(s: &str) -> String {
        let mut o = String::with_capacity(s.len() + 2);
        for c in s.chars() {
            match c {
                '\\' => o.push_str("\\\\"),
                '\n' => o.push_str("\\n"),
                '\r' => o.push_str("\\r"),
                '\t' => o.push_str("\\t"),
                _ => o.push(c),
            }
        }
        o
    }
    /// Byte-string escape — the decoded `Str` payload can hold any
    /// byte, so non-printable/non-ASCII bytes become `\xNN`.
    fn esc_bytes(b: &[u8]) -> String {
        let mut o = String::with_capacity(b.len() + 2);
        for &x in b {
            match x {
                b'\\' => o.push_str("\\\\"),
                0x0a => o.push_str("\\n"),
                0x0d => o.push_str("\\r"),
                0x09 => o.push_str("\\t"),
                0x20..=0x7e => o.push(x as char),
                _ => o.push_str(&format!("\\x{x:02x}")),
            }
        }
        o
    }
    fn trivia(o: &mut String, leading: &[Trivia]) {
        for t in leading {
            match t {
                Trivia::Line { text, same_line } => {
                    o.push_str(&format!("C {} {}\n", *same_line as u8, esc(text)));
                }
                Trivia::BlankLines(n) => o.push_str(&format!("B {n}\n")),
            }
        }
    }
    let mut o = String::new();
    match lex(src) {
        Ok(toks) => {
            for t in &toks {
                trivia(&mut o, &t.leading);
                let dbg = format!("{:?}", t.tok);
                let kind = dbg.split('(').next().unwrap().to_lowercase();
                let decoded = match &t.tok {
                    Tok::Int(v) => format!(" {v}"),
                    Tok::Str(b) => format!(" {}", esc_bytes(b)),
                    _ => String::new(),
                };
                let text = if t.text.is_empty() { "-".into() } else { esc(&t.text) };
                o.push_str(&format!("T {} {} {kind} {text}{decoded}\n", t.span.line, t.span.col));
            }
        }
        Err(ds) => {
            for d in &ds {
                o.push_str(&format!("! {} {} {}\n", d.span.line, d.span.col, esc(&d.msg)));
            }
        }
    }
    o
}

#[cfg(test)]
mod tests {
    use super::dump_tokens;

    /// Pins the canonical dump format (BOOTSTRAP.md §4) — the
    /// self-hosted lexer must reproduce these bytes exactly.
    #[test]
    fn dump_canonical() {
        let src = "// hi\nfn main() -> i32 { return 0xFF; }  // trail\n\nlet s = \"a\\x41\";";
        let d = dump_tokens(src);
        let want = concat!(
            "C 0 // hi\n",
            "T 2 1 fn fn\n",
            "T 2 4 ident main\n",
            "T 2 8 lparen (\n",
            "T 2 9 rparen )\n",
            "T 2 11 arrow ->\n",
            "T 2 14 ident i32\n",
            "T 2 18 lbrace {\n",
            "T 2 20 return return\n",
            "T 2 27 int 0xFF 255\n",
            "T 2 31 semicolon ;\n",
            "T 2 33 rbrace }\n",
            "C 1 // trail\n",
            "B 1\n",
            "T 4 1 let let\n",
            "T 4 5 ident s\n",
            "T 4 7 assign =\n",
            "T 4 9 str \"a\\\\x41\" aA\n",
            "T 4 16 semicolon ;\n",
            "T 4 16 eof -\n",
        );
        assert_eq!(d, want);
    }

    #[test]
    fn dump_error() {
        let d = dump_tokens("@\n");
        assert_eq!(d, "! 1 1 unexpected character `@`\n");
    }
}
