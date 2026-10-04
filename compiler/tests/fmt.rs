//! N39 corpus pin — the formatter's contract over every `.lang`
//! file in the repo:
//!
//! 1. Formatted output produces the SAME token sequence as the
//!    source (modulo trivia) — reformatting can't change meaning.
//! 2. `fmt(fmt(src)) == fmt(src)` — idempotent.
//! 3. Every `//` comment's exact text appears in the output exactly
//!    once.
//!
//! Files that fail to lex (deliberately malformed corpus tests) are
//! skipped — `fmt` refuses them, which is itself pinned by the
//! unit tests.

use ourc::driver::fmt::format_source;
use ourc::frontend::lexer::{lex, Tok};
use std::path::PathBuf;

fn lang_files(dir: &PathBuf, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            lang_files(&p, out);
        } else if p.extension().map(|x| x == "lang").unwrap_or(false) {
            out.push(p);
        }
    }
}

#[test]
fn corpus_formats_lossless() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let mut files = Vec::new();
    lang_files(&root.join("native-tests"), &mut files);
    lang_files(&root.join("stdlib"), &mut files);
    files.sort();
    assert!(files.len() > 600, "corpus not found: {}", files.len());

    let mut checked = 0;
    let mut skipped = 0;
    for f in &files {
        let src = std::fs::read_to_string(f).unwrap();
        let Ok(out) = format_source(&src) else {
            skipped += 1;
            continue; // deliberately unlexable test — fmt refuses
        };
        checked += 1;

        // 1 — same token sequence.
        let src_toks: Vec<Tok> = lex(&src).unwrap().into_iter().map(|t| t.tok).collect();
        let out_toks: Vec<Tok> = lex(&out).unwrap().into_iter().map(|t| t.tok).collect();
        assert_eq!(src_toks, out_toks, "token drift in {}", f.display());

        // 2 — idempotent.
        let twice = format_source(&out).unwrap();
        assert_eq!(out, twice, "not idempotent: {}", f.display());

        // 3 — every comment's exact text survives once: compare the
        // lexer's own trivia lists (no `//`-in-string false positives).
        let comments = |s: &str| -> Vec<String> {
            lex(s)
                .unwrap()
                .into_iter()
                .flat_map(|t| t.leading)
                .filter_map(|tr| match tr {
                    ourc::frontend::lexer::Trivia::Line { text, .. } => Some(text),
                    _ => None,
                })
                .collect()
        };
        assert_eq!(
            comments(&src),
            comments(&out),
            "comment drift in {}",
            f.display()
        );
    }
    println!("formatted {checked} files ({skipped} unlexable-skipped)");
}
