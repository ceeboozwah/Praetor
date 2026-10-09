//! Dev tool — regenerates `selfhost/unicode.lang`: Unicode range
//! tables for the predicates the lexer delegates to `char::is_*`
//! (BOOTSTRAP.md: the self-hosted lexer must match Stage-0 on every
//! input, and `is_alphabetic`/`is_alphanumeric`/`is_whitespace` are
//! full-Unicode predicates — the honest reproduction is the same
//! table the oracle uses, extracted from this toolchain).
//!
//!   cargo run --bin ucdgen > selfhost/unicode.lang
//!
//! The generated file is checked in; regenerate after a Rust
//! toolchain bump (the Unicode version follows rustc).

use std::fmt::Write as _;

/// Coalesce the codepoints where `f` holds into (lo, hi) ranges.
fn ranges(f: impl Fn(char) -> bool) -> Vec<(u32, u32)> {
    let mut out = Vec::new();
    let mut lo = 0u32;
    let mut on = false;
    for cp in 0u32..0x110000 {
        let v = char::from_u32(cp).map(&f).unwrap_or(false);
        if v && !on {
            lo = cp;
            on = true;
        } else if !v && on {
            out.push((lo, cp - 1));
            on = false;
        }
    }
    if on {
        out.push((lo, 0x10FFFF));
    }
    out
}

/// Emit the ranges as a `.lang` string literal — packed LE u32 pairs
/// (`\xNN` escapes decode to arbitrary bytes; N68's lexer handles
/// them, which also dogfoods the escape path).
fn blob(rs: &[(u32, u32)]) -> String {
    let mut s = String::new();
    for (lo, hi) in rs {
        for b in lo.to_le_bytes().into_iter().chain(hi.to_le_bytes()) {
            write!(s, "\\x{b:02x}").unwrap();
        }
    }
    s
}

fn table(name: &str, rs: &[(u32, u32)]) -> String {
    let mut s = String::new();
    writeln!(
        s,
        "pub fn {name}() -> str {{\n    return \"{}\";\n}}",
        blob(rs)
    )
    .unwrap();
    s
}

fn main() {
    let alpha = ranges(|c| c.is_alphabetic());
    let alnum = ranges(|c| c.is_alphanumeric());
    let ws = ranges(|c| c.is_whitespace());
    let mut o = String::new();
    o.push_str(
        "//! GENERATED — do not edit. `cargo run --bin ucdgen`.\n\
         //! selfhost.unicode — the Unicode range tables behind the\n\
         //! lexer's char classes, extracted from the Stage-0\n\
         //! toolchain's own `char::is_*` so both lexers classify\n\
         //! every codepoint identically (N68). Each blob is packed\n\
         //! LE u32 (lo,hi) ranges; `lookup` binary-searches them.\n\n",
    );
    o.push_str(
        "fn rd32(b: slice u8, off: u64) -> u32 {\n\
         \x20   return u32(b[off]) | (u32(b[off + 1]) << 8) |\n\
         \x20       (u32(b[off + 2]) << 16) | (u32(b[off + 3]) << 24);\n\
         }\n\n\
         /// Binary search a packed range table for `cp`.\n\
         pub fn lookup(tab: str, cp: u32) -> bool {\n\
         \x20   let b = tab.bytes;\n\
         \x20   let lo: u64 = 0;\n\
         \x20   let hi: u64 = u64(tab.len) / 8;\n\
         \x20   while lo < hi {\n\
         \x20       let mid = (lo + hi) / 2;\n\
         \x20       let off = mid * 8;\n\
         \x20       let rl = rd32(b, off);\n\
         \x20       let rh = rd32(b, off + 4);\n\
         \x20       if cp < rl { hi = mid; }\n\
         \x20       if cp > rh { lo = mid + 1; }\n\
         \x20       if cp >= rl && cp <= rh { return true; }\n\
         \x20   }\n\
         \x20   return false;\n\
         }\n\n\
         /// `char::is_alphabetic` — Unicode Alphabetic property.\n\
         pub fn is_alphabetic(cp: u32) -> bool {\n\
         \x20   return lookup(alphabetic(), cp);\n\
         }\n\n\
         /// `char::is_alphanumeric` — Alphabetic | Nd | Nl | No.\n\
         pub fn is_alphanumeric(cp: u32) -> bool {\n\
         \x20   return lookup(alphanumeric(), cp);\n\
         }\n\n\
         /// `char::is_whitespace` — Unicode White_Space property\n\
         /// (tab..CR, space, NEL, NBSP, ogham, en/em quads and\n\
         /// friends, U+2028/29, narrow NBSP, medium math, ideographic).\n\
         pub fn is_whitespace(cp: u32) -> bool {\n\
         \x20   return lookup(whitespace(), cp);\n\
         }\n\n",
    );
    o.push_str(&table("alphabetic", &alpha));
    o.push('\n');
    o.push_str(&table("alphanumeric", &alnum));
    o.push('\n');
    o.push_str(&table("whitespace", &ws));
    print!("{o}");
}
