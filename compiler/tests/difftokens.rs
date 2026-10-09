//! N68 — the BOOTSTRAP.md §4 differential, executable:
//! `ourc --dump-tokens` (Stage 0) and `selfhost/dumptokens` (the
//! .lang lexer) must emit byte-identical canonical dumps for every
//! input — the whole `.lang` corpus, synthetic edge cases, and a
//! seeded fuzz batch. The self-hosted layer earns the right to
//! feed the next port (parser, N69) only while this stays green.

use ourc::driver::pipeline;
use ourc::frontend::lexer;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn workdir() -> PathBuf {
    let d = repo().join("target/difftokens");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Build `selfhost/dumptokens.lang` with the Stage-0 pipeline — the
/// binary under test is a product of the oracle itself.
fn selfhost_exe() -> PathBuf {
    pipeline::build_with(
        &repo().join("selfhost/dumptokens.lang"),
        &workdir(),
        &Default::default(),
        false,
        false,
    )
    .unwrap_or_else(|e| panic!("selfhost dumptokens must build: {e}"))
}

/// Run the self-hosted dump binary; returns stdout bytes.
fn selfhost_dump(exe: &Path, file: &Path) -> (Vec<u8>, bool) {
    let out = Command::new(exe).arg(file).output().unwrap();
    (out.stdout, out.status.success())
}

fn stage0_dump(src: &str) -> Vec<u8> {
    lexer::dump_tokens(src).into_bytes()
}

/// Every `.lang` file under the repo, excluding build output.
fn corpus(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        if p.is_dir() {
            if name != "target" && !name.starts_with('.') {
                corpus(&p, out);
            }
        } else if name.ends_with(".lang") {
            out.push(p);
        }
    }
}

fn diff_one(exe: &Path, file: &Path, src_bytes: &[u8]) {
    let expected = match std::str::from_utf8(src_bytes) {
        Ok(s) => stage0_dump(s),
        // Non-UTF-8 input: both sides refuse before lexing — the
        // canonical stdout is empty and the exit is a failure.
        Err(_) => Vec::new(),
    };
    let (actual, ok) = selfhost_dump(exe, file);
    let utf8 = std::str::from_utf8(src_bytes).is_ok();
    assert_eq!(ok, utf8, "exit status diverged on {}", file.display());
    if actual != expected {
        let pos = actual
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        panic!(
            "token dump diverged on {} at byte {pos}\n--- selfhosted ---\n{}\n--- stage0 ---\n{}",
            file.display(),
            String::from_utf8_lossy(&actual[..actual.len().min(pos + 200)]),
            String::from_utf8_lossy(&expected[..expected.len().min(pos + 200)]),
        );
    }
}

/// Deterministic xorshift64 — the fuzz batch is reproducible.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    fn pick<'a>(&mut self, v: &'a [&'a str]) -> &'a str {
        v[(self.next() as usize) % v.len()]
    }
}

/// Fragments chosen to stress boundaries: two-char prefixes, escape
/// edges, separator/pathological numbers, Unicode classes, trivia.
const FRAGS: &[&str] = &[
    "fn", "let", "return", "if", "else", "while", "struct", "mut",
    "region", "ident", "x", "_", "_9", "0x", "0b", "1_000", "..",
    "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "+", "-",
    "*", "/", "%", "&", "|", "^", "~", "!", "?", ".", ",", ";", ":",
    "(", ")", "{", "}", "[", "]", " ", "  ", "\t", "\n", "\r\n",
    "// comment\n", "//comment no space\n", "//\n", "\n\n", "\n\n\n",
    "\"str\"", "\"esc\\n\"", "\"\\x41\"", "\"unterm", "\"a\\q\"",
    "\"\\x\"", "\"\\xg\"", "\"\\x4g\"", "\"a\rb\"",
    "λ", "变量", "é", "\u{a0}", "\u{3000}", "\u{2028}", "\u{85}",
    "@", "#", "`", "$", "€", "٣", "\u{feff}",
    "0xdeadBEEF", "0XFF", "0B11", "99999999999999999999",
    "9223372036854775807", "9223372036854775808", "_ident_9", "1_",
    "0x_", "1__2", "\\", "'", "\u{b}", "\u{c}", "\u{2007}",
];

#[test]
fn difftokens() {
    let exe = selfhost_exe();
    let mut files = Vec::new();
    corpus(&repo(), &mut files);
    assert!(files.len() > 700, "corpus sweep found {}", files.len());

    let tmp = workdir().join("cases");
    std::fs::create_dir_all(&tmp).unwrap();

    // 1. The real corpus — pins, stdlib, programs, selfhost itself.
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        diff_one(&exe, f, &bytes);
    }

    // 2. Synthetic edge cases.
    let cases: &[&str] = &[
        "", "   \n\n\t\n  ", "﻿fn main() {}\n",
        "// a\n// b\n\n\nfn x() {}\n", "fn a() {} // trail",
        "let s = \"a\\x4\";", "let s = \"a\\", "\"",
        "let x = 0x_5;", "let x = 1_;", "let x = 1__2;",
        "let x = 0b2;", "let x = 0xG;",
        "fn a() {}\r\nfn b() {}\r\n",
    ];
    for (i, src) in cases.iter().enumerate() {
        let f = tmp.join(format!("edge{i}.lang"));
        std::fs::write(&f, src).unwrap();
        diff_one(&exe, &f, src.as_bytes());
    }

    // 3. A non-UTF-8 file — refused identically (empty stdout, exit 2).
    let bad = tmp.join("not_utf8.lang");
    std::fs::write(&bad, b"let x = \xff\xfe;\n").unwrap();
    diff_one(&exe, &bad, &std::fs::read(&bad).unwrap());

    // 4. Seeded fuzz — 300 fragment concatenations.
    let mut rng = Rng(0x068);
    for i in 0..300 {
        let n = (rng.next() % 40) as usize;
        let mut src = String::new();
        for _ in 0..n {
            src.push_str(rng.pick(FRAGS));
        }
        let f = tmp.join(format!("fuzz{i}.lang"));
        std::fs::write(&f, &src).unwrap();
        diff_one(&exe, &f, src.as_bytes());
    }

    // 5. Determinism — the self-hosted dump is a pure function.
    let f = &files[0];
    let bytes = std::fs::read(f).unwrap();
    let (a, _) = selfhost_dump(&exe, f);
    let (b, _) = selfhost_dump(&exe, f);
    assert_eq!(a, b);
}
