//! N69 — the BOOTSTRAP.md §4 parser differential, executable:
//! `ourc --dump-ast` (Stage 0) and `selfhost/dumpast` (the .lang
//! parser) must emit byte-identical canonical dumps for every input
//! — the whole `.lang` corpus, malformed fixtures, truncations,
//! mutations, and a seeded token-soup fuzz. The error surface is
//! canonical too: `! <line> <col> <msg>` lines, unescaped.
//!
//! Layer discipline (SELF-HOSTING.md): the parser earns the right to
//! feed the next port (module loader, N70) only while this stays
//! green. Token equivalence is already proven by `difftokens`.

use ourc::driver::pipeline;
use ourc::frontend::{dumpast, parser};
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn workdir() -> PathBuf {
    // Same latent join artifact as diffgraph — component-wise.
    let d = repo().join("target").join("diffast");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Build `selfhost/dumpast.lang` with the Stage-0 pipeline — the
/// binary under test is a product of the oracle itself.
fn selfhost_exe() -> PathBuf {
    pipeline::build_with(
        &repo().join("selfhost").join("dumpast.lang"),
        &workdir(),
        &Default::default(),
        false,
        false,
    )
    .unwrap_or_else(|e| panic!("selfhost dumpast must build: {e}"))
}

/// Run the self-hosted dump binary; returns (stdout bytes, success).
fn selfhost_dump(exe: &Path, file: &Path) -> (Vec<u8>, bool) {
    let out = Command::new(exe).arg(file).output().unwrap();
    (out.stdout, out.status.success())
}

/// The Stage-0 canonical surface: the S-expression program, or the
/// `!` diagnostics alone (lex errors short-circuit before parsing,
/// matching `parse`'s `lex(src)?`).
fn stage0_dump(src: &str) -> Vec<u8> {
    match parser::parse(src) {
        Ok(p) => dumpast::dump_ast(&p).into_bytes(),
        Err(ds) => ds
            .iter()
            .map(|d| format!("! {} {} {}\n", d.span.line, d.span.col, d.msg))
            .collect::<String>()
            .into_bytes(),
    }
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
    let utf8 = std::str::from_utf8(src_bytes).is_ok();
    let expected = match std::str::from_utf8(src_bytes) {
        Ok(s) => stage0_dump(s),
        Err(_) => Vec::new(),
    };
    let (actual, ok) = selfhost_dump(exe, file);
    assert_eq!(ok, utf8, "exit status diverged on {}", file.display());
    if actual != expected {
        let pos = actual
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        panic!(
            "AST dump diverged on {} at byte {pos}\n--- selfhosted ---\n{}\n--- stage0 ---\n{}",
            file.display(),
            String::from_utf8_lossy(&actual[..actual.len().min(pos + 300)]),
            String::from_utf8_lossy(&expected[..expected.len().min(pos + 300)]),
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

/// Token soup for the fuzz batch — grammar keywords, operators that
/// stress the `<`-speculation paths, and fragments that truncate
/// productions mid-way.
const FRAGS: &[&str] = &[
    "fn", "let", "return", "if", "else", "while", "match", "struct",
    "enum", "import", "module", "pub", "region", "mut", "as",
    "break", "continue", "true", "false", "x", "y", "foo", "T",
    "alloc", "slice", "box", "u8", "i32", "bool",
    "0", "42", "0x1F", "1_000", "\"s\"", "\"a\\n\"",
    "(", ")", "{", "}", "[", "]", "<", ">", ",", ";", ":", "::",
    "->", ".", "..", "=", "==", "=>", "!=", "<=", ">=", "+", "-",
    "*", "/", "%", "!", "?", "|", "||", "^", "~", "&&", "&",
    " ", "\n",
];

/// One malformed case per parser diagnostic, plus the speculation
/// and truncation edges the corpus does not cover.
const BAD: &[&str] = &[
    // Every `eat`/`expect`/`err` production.
    "fn main( {\n}\n",
    "fn main() { let x = ; }\n",
    "pub 5 fn f() {}\n",
    "struct S { x: u8 }\nfn f() -> [abc]u8 { }\n",
    "fn f() -> [0]u8 { }\n",
    "fn f() -> [5000000000]u8 { }\n",
    "fn f<T, T>(x: T) -> T { return x; }\n",
    "enum E { }\n",
    "fn f() { let a = alloc(u8, u8); }\n",
    "fn f() { let a = alloc_slice(u8); }\n",
    "fn f() { x + 1 return 0; }\n",
    "fn f() {\n",
    "fn\n",
    "fn f() { let x = 1 }\n",
    "import a.\n",
    "module 9;\n",
    "fn f() -> { }\n",
    "fn f() { if x { } else }\n",
    "struct S { x: }\n",
    "fn f() { match x { A => , } }\n",
    "fn f() { while { } }\n",
    // `<` speculation: generic vs comparison, in every nesting.
    "fn f() { let a = i < n; let b = x < y < z; let c = a < b > c; }\n",
    "fn f() { let v = Vec<u8>::new(); let w = Vec<Vec<u8>>::new(); }\n",
    "fn f() { let x = f<u8>(a); let y = h<u8, u16>(c); }\n",
    "fn f() { let p = S<u8> { x: 1 }; let q = m.S<u8> { x: 2 }; }\n",
    "fn f() { let r = o.m<u8>(x); let s = o.m < x; }\n",
    "fn f() { let a = x<y; let b = x<y>; let c = x < y > (z); }\n",
    "fn f() { while i < n { } if a < b && c < d { } let e = 1 < 2 == 3 < 4; }\n",
    "fn f() { let x = T::A; let y = m::T::B; let z = a.b.c.d; }\n",
    "fn f() { let a = x[i]; let b = x[i..j]; let c = x[..j]; let d = x[i..]; }\n",
    "fn f() { let a = x.y.z(); let b = x.y.z.w(); let c = a.b::<u8>.c; }\n",
    "fn f() { match m.x { A => 1, B(y) => y, C { q } => q, _ => 0 } }\n",
    // Error messages whose text contains backslashes — the `!`
    // surface prints messages RAW, unlike escaped token payloads.
    "fn f() { let s = \"a\\q\"; }\n",
    "fn f() { let s = \"a\\x\"; }\n",
    "fn f() { let s = \"unterminated\n; }\n",
];

#[test]
fn diffast() {
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

    // 2. Malformed + speculation fixtures.
    for (i, src) in BAD.iter().enumerate() {
        let f = tmp.join(format!("bad{i}.lang"));
        std::fs::write(&f, src).unwrap();
        diff_one(&exe, &f, src.as_bytes());
    }

    // 3. Byte-prefix truncations of a grammar-dense source — every
    //    cut lands mid-production somewhere.
    let dense = "module m;\nimport a.b;\nstruct Pt<T> { x: T, y: u64 }\n\
        enum E { A, B(u8), C { q: i32 } }\n\
        pub fn f<T: alloc>(p: *mut Pt<T>, n: u64) -> Result<T, E> !io {\n\
        \x20   let mut i = 0;\n\x20   while i < n {\n\
        \x20       match p.x {\n\x20           Ok(v) => { return v; }\n\
        \x20           Err(e) => { if i % 2 == 0 { continue; } break; }\n\
        \x20       }\n\x20       i = i + 1;\n\x20   }\n\
        \x20   let s = \"a\\nb\\x41\";\n\x20   return Err(E::B(7));\n}\n";
    for i in 1..dense.len() {
        let f = tmp.join("trunc.lang");
        std::fs::write(&f, &dense[..i]).unwrap();
        diff_one(&exe, &f, dense[..i].as_bytes());
    }

    // 4. Single-byte mutations of the same source — swap each byte
    //    for a rotating grammar-significant replacement.
    let subs: &[u8] = b"<>;{}()=!?|&.,:[]\"\\_x9 <";
    let bytes = dense.as_bytes();
    for pos in 0..bytes.len() {
        let sb = subs[pos % subs.len()];
        if bytes[pos] == sb {
            continue;
        }
        let mut m = bytes.to_vec();
        m[pos] = sb;
        if let Ok(s) = std::str::from_utf8(&m) {
            let f = tmp.join("mut.lang");
            std::fs::write(&f, s).unwrap();
            diff_one(&exe, &f, s.as_bytes());
        }
    }

    // 5. Seeded token-soup fuzz — 400 concatenations.
    let mut rng = Rng(0x069);
    for i in 0..400 {
        let n = 3 + (rng.next() % 60) as usize;
        let mut src = String::new();
        for _ in 0..n {
            src.push_str(rng.pick(FRAGS));
            src.push(' ');
        }
        let f = tmp.join(format!("fuzz{i}.lang"));
        std::fs::write(&f, &src).unwrap();
        diff_one(&exe, &f, src.as_bytes());
    }

    // 6. Determinism — the self-hosted dump is a pure function.
    let f = &files[0];
    let (a, _) = selfhost_dump(&exe, f);
    let (b, _) = selfhost_dump(&exe, f);
    assert_eq!(a, b);
}
