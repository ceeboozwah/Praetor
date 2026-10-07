//! `our fmt` — the token-stream formatter (N39C). Works on the
//! lexer's trivia-carrying token stream (N39A/N39B): comments and
//! blank lines are data, so the output is *lossless modulo
//! whitespace normalization* — every token's exact `text` is
//! respelled verbatim (hex literals stay `0xFF`, string escapes
//! stay `\x41`), every comment survives exactly once.
//!
//! Deliberately boring style:
//!   4-space indent · one statement per line · spaces around binary
//!   ops · no space before call parens · `{` after `)`/`else`/`=>`/
//!   block positions newline-indents; `Name { f: v }` literals stay
//!   inline · at most one blank line preserved · `} else {` stays
//!   together · trailing comments keep their line.
//!
//! The `{` lit-vs-block rule mirrors the parser: `{` followed by
//! `Ident :` is a struct/enum literal; anything else is a block.
//! `<`/`>` generics-vs-comparison uses a balanced type-token scan —
//! a comparison like `a < b, c > d` can be misread as targs (the
//! tokens are identical; only spacing differs).

use crate::frontend::lexer::{self, Tok, Token, Trivia};
use crate::Diag;


/// Format a source string. Refuses on lex errors — a file that can't
/// be tokenized can't be safely rewritten. Parse failures are fine:
/// the formatter never reparses.
pub fn format_source(src: &str) -> Result<String, Vec<Diag>> {
    let toks = lexer::lex(src)?;
    let mut f = Fmt {
        out: String::with_capacity(src.len() + 64),
        indent: 0,
        bol: true,
        need_nl: false,
        just_block_close: false,
    };
    let mut lit_stack: Vec<bool> = Vec::new();
    let mut paren = 0i32;
    let mut bracket = 0i32;
    let mut in_targs = 0i32;
    // An empty `{}` was emitted inline — consume the real `}`.
    let mut skip_rbrace = false;

    for (i, t) in toks.iter().enumerate() {
        // Leading trivia: comments + blank lines before this token.
        for tr in &t.leading {
            match tr {
                Trivia::Line { text, same_line } => {
                    if *same_line && !f.bol {
                        f.push("  ");
                        f.push(text.trim_end());
                        f.need_nl = true;
                    } else {
                        f.line_break();
                        f.push_indent();
                        f.push(text.trim_end());
                        f.need_nl = true;
                    }
                }
                Trivia::BlankLines(n) => {
                    if !f.bol && !f.out.is_empty() {
                        f.line_break();
                    }
                    if !f.out.is_empty() {
                        for _ in 0..(*n).min(1) {
                            f.out.push('\n');
                        }
                    }
                }
            }
        }
        if t.tok == Tok::Eof {
            break;
        }

        // Structural placement: does this token continue the line,
        // or open a new one?
        let lit_top = lit_stack.last().copied().unwrap_or(false);
        if skip_rbrace {
            if t.tok == Tok::RBrace {
                skip_rbrace = false;
                continue;
            }
            skip_rbrace = false; // trivia intervened — process normally
        }
        match &t.tok {
            Tok::RBrace if lit_top => {
                lit_stack.pop();
                if f.out.ends_with("{ ") {
                    f.out.pop(); // `Name { }`
                } else if !f.out.ends_with(' ') {
                    f.push(" ");
                }
                f.push("}");
            }
            Tok::RBrace => {
                lit_stack.pop();
                f.indent -= 1;
                f.line_break();
                f.push_indent();
                f.push("}");
                f.just_block_close = true;
            }
            Tok::LBrace => {
                let lit = is_lit_brace(&toks, i);
                lit_stack.push(lit);
                if lit {
                    if !f.out.ends_with(' ') && !f.out.ends_with('(') && !f.bol {
                        f.push(" ");
                    }
                    f.push("{ ");
                    // empty literal — `Name { }`: `}` will trim the space.
                } else if matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::RBrace))
                    && toks[i + 1].leading.is_empty()
                {
                    // empty block — `{}` inline.
                    if !f.bol && !f.out.ends_with(' ') {
                        f.push(" ");
                    }
                    f.push("{}");
                    lit_stack.pop();
                    skip_rbrace = true;
                } else {
                    if !f.bol && !f.out.ends_with(' ') {
                        f.push(" ");
                    }
                    f.push("{");
                    f.indent += 1;
                    f.need_nl = true;
                }
            }
            Tok::Semicolon => {
                f.push(";");
                if paren == 0 && bracket == 0 && in_targs == 0 && !lit_top {
                    f.need_nl = true;
                }
            }
            Tok::Comma => {
                f.push(",");
                if paren == 0 && bracket == 0 && in_targs == 0 && !lit_top {
                    f.need_nl = true; // match-arm separator
                } else {
                    f.push(" ");
                }
            }
            Tok::Else => {
                f.push(" else ");
                f.just_block_close = false;
            }
            Tok::FatArrow => {
                f.sp(" => ");
            }
            _ => {
                // Ordinary token: a newline is pending (statement end,
                // comment, or block boundary) → break; else apply the
                // spacing table.
                if f.need_nl || f.just_block_close {
                    f.line_break();
                    f.push_indent();
                } else if !f.bol {
                    f.push(gap(&toks, i, in_targs));
                }
                f.push(&t.text);
                match &t.tok {
                    Tok::LParen => paren += 1,
                    Tok::RParen => paren -= 1,
                    Tok::LBracket => bracket += 1,
                    Tok::RBracket => bracket -= 1,
                    Tok::Lt if is_targ_open(&toks, i) => in_targs += 1,
                    Tok::Gt if in_targs > 0 => in_targs -= 1,
                    _ => {}
                }
                f.just_block_close = false;
            }
        }
        // `{`-lit closing `}` spacing handled above; `,` after a block
        // `}` (arm separator) — `}` emitted, `,` lands next.
    }
    // Trailing newline.
    if !f.out.is_empty() && !f.out.ends_with('\n') {
        f.out.push('\n');
    }
    Ok(f.out)
}

struct Fmt {
    out: String,
    indent: i32,
    /// At the start of a line (nothing emitted since last `\n`).
    bol: bool,
    /// A statement/block ended — the next token starts a new line.
    need_nl: bool,
    /// A block `}` just closed — next token breaks unless `else`/`,`/`;`.
    just_block_close: bool,
}

impl Fmt {
    fn push(&mut self, s: &str) {
        self.out.push_str(s);
        if !s.is_empty() {
            self.bol = false;
        }
    }
    fn sp(&mut self, s: &str) {
        self.push(s);
    }
    fn push_indent(&mut self) {
        if self.bol {
            for _ in 0..self.indent.max(0) {
                self.out.push_str("    ");
            }
            if self.indent > 0 {
                self.bol = false;
            }
        }
    }
    fn line_break(&mut self) {
        // trim trailing spaces before the newline
        while self.out.ends_with(' ') {
            self.out.pop();
        }
        if !self.out.is_empty() && !self.out.ends_with('\n') {
            self.out.push('\n');
        }
        self.bol = true;
        self.need_nl = false;
        self.just_block_close = false;
    }
}

/// `{` is a struct/enum literal iff followed by `Ident :` — the same
/// lookahead the parser uses (`Name { f: v }` vs `if x { stmts }`).
/// A `struct`/`enum` DECLARATION also matches that lookahead
/// (`struct Point { x: i32 }`), so scan back for the decl keyword.
fn is_lit_brace(toks: &[Token], i: usize) -> bool {
    if !matches!(toks.get(i + 1).map(|t| &t.tok), Some(Tok::Ident(_)))
        || !matches!(toks.get(i + 2).map(|t| &t.tok), Some(Tok::Colon))
    {
        return false;
    }
    for j in (i.saturating_sub(6))..i {
        match &toks[j].tok {
            Tok::Struct => return false,
            Tok::Ident(n) if n == "enum" => return false,
            _ => {}
        }
    }
    true
}

/// Tokens that can appear inside a generic arg list `<...>`.
fn ty_tok(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_)
            | Tok::Mut
            | Tok::Int(_)
            | Tok::Str(_)
            | Tok::Comma
            | Tok::Lt
            | Tok::Gt
            | Tok::Amp
            | Tok::Star
            | Tok::LBracket
            | Tok::RBracket
            | Tok::ColonColon
            | Tok::Dot
            | Tok::Region
            | Tok::Fn
            | Tok::Question
    )
}

/// `a < b`: is this `<` opening a targ list? A balanced run of
/// type-grammar tokens ending in `>`. `f(a < b, c > d)` can
/// misidentify — same tokens, spacing-only consequence.
fn is_targ_open(toks: &[Token], i: usize) -> bool {
    let mut depth = 0i32;
    let mut j = i;
    while let Some(t) = toks.get(j) {
        match &t.tok {
            Tok::Lt => depth += 1,
            Tok::Gt => {
                depth -= 1;
                if depth == 0 {
                    return true;
                }
            }
            // `>>`-style: `>` can only decrement once per token here
            // (the lexer emits one Gt per `>`), nested `>>` shows as
            // two Gt — handled naturally.
            t if ty_tok(t) => {}
            _ => return false,
        }
        j += 1;
    }
    false
}

/// The space between the previous emitted token and `toks[i]` when
/// they share a line.
fn gap(toks: &[Token], i: usize, in_targs: i32) -> &'static str {
    let cur = &toks[i].tok;
    let prev = &toks[i - 1].tok;
    let prev2 = if i >= 2 { Some(&toks[i - 2].tok) } else { None };

    // `a << b` / `a >> b` — the lexer has no shift tokens; adjacent
    // `<<`/`>>` in the source glue back together.
    let adjacent = cur_has_adjacent(&toks[i - 1], &toks[i]);
    if adjacent
        && matches!((prev, cur), (Tok::Lt, Tok::Lt) | (Tok::Gt, Tok::Gt))
    {
        return "";
    }

    if in_targs > 0 {
        return match cur {
            Tok::Comma | Tok::Gt | Tok::Lt | Tok::RBracket | Tok::LBracket
            | Tok::Dot => "",
            _ => match prev {
                Tok::Lt | Tok::Amp | Tok::Star | Tok::ColonColon
                | Tok::LBracket | Tok::Region | Tok::Gt | Tok::Comma
                | Tok::Dot => "",
                _ => " ",
            },
        };
    }

    // `cur`-side rules.
    match cur {
        Tok::LParen => {
            // `f(`, `)(` glued; `if (`, `= (`, `-> (` spaced; prefix
            // ops glue `(-x)` — unless they were binary `- (` spaced.
            if operandish(prev) {
                return "";
            }
            return match prev {
                Tok::Bang | Tok::Tilde | Tok::Star | Tok::Amp | Tok::Minus => {
                    if prev2.map(operandish).unwrap_or(false) {
                        " "
                    } else {
                        ""
                    }
                }
                Tok::LParen | Tok::LBracket | Tok::Comma | Tok::Dot
                | Tok::ColonColon | Tok::Question | Tok::DotDot => "",
                _ => " ",
            };
        }
        Tok::LBracket => {
            // `a[i]` glued after a value; `[i32; 4]`/`&mut [i32]` take
            // the space that `:`, `mut`, `->` etc. provide.
            if operandish(prev) {
                return "";
            }
            return match prev {
                Tok::LParen | Tok::LBracket | Tok::Comma | Tok::Amp | Tok::Mut
                | Tok::Star | Tok::Dot | Tok::Bang | Tok::Tilde => "",
                _ => " ",
            };
        }
        Tok::RParen | Tok::RBracket | Tok::Comma | Tok::Semicolon | Tok::Dot
        | Tok::DotDot | Tok::ColonColon | Tok::Question | Tok::Colon => {
            return ""
        }
        Tok::Arrow | Tok::FatArrow => return " ",
        _ => {}
    }

    // `-`/`*`/`&`/`!`/`~` — binary after an operand, prefix otherwise.
    // `if !x`/`while !f()` — keywords take a space before prefix ops.
    if matches!(cur, Tok::Minus | Tok::Star | Tok::Amp | Tok::Bang | Tok::Tilde) {
        return if operandish(prev)
            || matches!(
                prev,
                Tok::Colon
                    | Tok::Arrow
                    | Tok::Assign
                    | Tok::If
                    | Tok::While
                    | Tok::Return
                    | Tok::Let
                    | Tok::Else
            ) {
            " "
        } else {
            ""
        };
    }

    // `<`/`>` — generic (glue) or comparison (space)?
    match cur {
        Tok::Lt if is_targ_open(toks, i) => return "",
        Tok::Lt | Tok::Gt => return " ",
        _ => {}
    }

    // Binary operators and assign — space on both sides.
    let bin = |t: &Tok| {
        matches!(
            t,
            Tok::Assign | Tok::Eq | Tok::Ne | Tok::Le | Tok::Ge | Tok::Plus
                | Tok::Slash | Tok::Percent | Tok::AmpAmp | Tok::PipePipe
                | Tok::Pipe | Tok::Caret | Tok::Lt | Tok::Gt
        )
    };
    if bin(cur) {
        return " ";
    }

    // `prev`-side rules for value-ish `cur`.
    match prev {
        Tok::LParen | Tok::LBracket | Tok::Dot | Tok::DotDot | Tok::ColonColon
        | Tok::Question | Tok::Comma | Tok::LBrace => return "",
        _ => {}
    }
    // `x, ` emitted its space already? — Comma pushes `, ` eagerly for
    // inline lists; a `,` that ended a line set need_nl instead.
    // Prefix/binary `- * & ! ~` on the left: space iff they were binary.
    if matches!(prev, Tok::Minus | Tok::Star | Tok::Amp | Tok::Bang | Tok::Tilde) {
        return if prev2.map(operandish).unwrap_or(false) {
            " "
        } else {
            ""
        };
    }
    " "
}

/// `cur` begins exactly where `prev` ends — same line, no gap.
fn cur_has_adjacent(prev: &Token, cur: &Token) -> bool {
    cur.span.line == prev.span.line
        && cur.span.col == prev.span.col + prev.text.chars().count() as u32
}

/// A token that can end an operand — `x -`, `f() -`, `a[i] -`.
fn operandish(t: &Tok) -> bool {
    matches!(
        t,
        Tok::Ident(_) | Tok::Int(_) | Tok::Str(_) | Tok::RParen | Tok::RBracket
            | Tok::True | Tok::False | Tok::Question | Tok::Gt
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::lexer::{lex, Tok};

    /// The semantic invariant: the formatted text produces the SAME
    /// token sequence — trivia and spans aside, nothing changes.
    fn same_tokens(a: &str, b: &str) -> bool {
        let strip = |s: &str| {
            lex(s)
                .unwrap()
                .into_iter()
                .map(|t| t.tok)
                .collect::<Vec<Tok>>()
        };
        strip(a) == strip(b)
    }

    #[test]
    fn comments_survive_once() {
        let src = "// head\nfn f() -> i32 {\n    let x = 1; // tail\n    // lone\n    return x;\n}\n// foot\n";
        let out = format_source(src).unwrap();
        assert_eq!(out.matches("// head").count(), 1);
        assert_eq!(out.matches("// tail").count(), 1);
        assert_eq!(out.matches("// lone").count(), 1);
        assert_eq!(out.matches("// foot").count(), 1);
        assert!(out.contains("x = 1;  // tail"), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn literals_respell_never() {
        let src = "fn f() -> i32 {\n    let a = 0xFF;\n    let b = 1_000;\n    let s = \"a\\x41b\";\n    return a;\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("0xFF"), "{out}");
        assert!(out.contains("1_000"), "{out}");
        assert!(out.contains("\"a\\x41b\""), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn blank_lines_capped_at_one() {
        let src = "fn a() -> i32 {\n    return 1;\n}\n\n\n\nfn b() -> i32 {\n    return 2;\n}\n";
        let out = format_source(src).unwrap();
        assert!(!out.contains("\n\n\n"), "{out}");
        assert!(out.contains("}\n\nfn b"), "{out}");
    }

    #[test]
    fn shift_glue() {
        let src = "fn f() -> i32 {\n    let x = 1 << 4;\n    let y = x >> 1;\n    return y;\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("1 << 4"), "{out}");
        assert!(out.contains("x >> 1"), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn literal_inline_block_multiline() {
        let src = "struct P {\n    x: i32,\n}\nfn f() -> P {\n    if true {\n        return P { x: 1 };\n    }\n    return P { x: 2 };\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("P { x: 1 }"), "{out}");
        assert!(out.contains("struct P {\n    x: i32,\n}"), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn generics_glue_comparison_spaces() {
        let src = "fn f() -> i32 {\n    let a: Vec<Vec<i32>>;\n    let b = 1 < 2;\n    return 0;\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("Vec<Vec<i32>>"), "{out}");
        assert!(out.contains("1 < 2"), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn qualified_paths_in_targs() {
        // N56.1 — `m.T<..>` inside targs must still read as generics.
        let src = "fn f(h: alloc) -> result.Result<omap.OMap<box slice u8>, i32> {\n    return result.Result<omap.OMap<box slice u8>, i32>::Ok(h);\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("result.Result<omap.OMap<box slice u8>, i32>"), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn range_before_paren_glues() {
        // N56.1 — `0..(*r).len`: `..` before `(` takes no space.
        let src = "fn f(r: &box slice u8) -> i32 {\n    return i32((*r)[0..(*r).len]);\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("[0..(*r).len]"), "{out}");
        assert!(same_tokens(src, &out));
    }

    #[test]
    fn idempotent_on_messy_input() {
        let src = "// c\nfn main( ) ->i32{let x=1;\nmatch x{A=>{return 1;}_=>{}}\nreturn 0;}\n";
        let once = format_source(src).unwrap();
        let twice = format_source(&once).unwrap();
        assert_eq!(once, twice, "{once}");
        assert!(same_tokens(src, &once));
    }

    #[test]
    fn utf8_survives() {
        let src = "// é 🚀\nfn f() -> i32 {\n    let s = \"héllo\";\n    return 0;\n}\n";
        let out = format_source(src).unwrap();
        assert!(out.contains("// é 🚀"), "{out}");
        assert!(out.contains("\"héllo\""), "{out}");
        assert!(same_tokens(src, &out));
    }
}
