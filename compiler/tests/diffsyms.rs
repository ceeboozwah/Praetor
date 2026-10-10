//! N71 — the BOOTSTRAP.md §4 resolver differential, executable:
//! `ourc --dump-symbols` (Stage 0) and `selfhost/dumpsyms` (the
//! .lang resolver) must emit byte-identical `(module-symbols …)`
//! dumps for every input: the whole `.lang` corpus plus fixtures
//! exercising generics, visibility, methods, match arms, and
//! resolution diagnostics.
//!
//! Decl names are module-qualified; paths follow the same rules as
//! the graph dump — fixture output is stable on any machine.

use ourc::driver::pipeline;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn workdir() -> PathBuf {
    let d = repo().join("target/diffsyms");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Build `selfhost/dumpsyms.lang` with the Stage-0 pipeline — the
/// binary under test is a product of the oracle itself.
fn selfhost_exe() -> PathBuf {
    pipeline::build_with(
        &repo().join("selfhost/dumpsyms.lang"),
        &workdir(),
        &Default::default(),
        false,
        false,
    )
    .unwrap_or_else(|e| panic!("selfhost dumpsyms must build: {e}"))
}

/// Run the self-hosted symbol dumper; returns stdout bytes. Both
/// sides use the repo stdlib — `OUR_STDLIB` pins it so the run does
/// not depend on the materialized `~/.our` tree's freshness.
fn selfhost_dump(exe: &Path, file: &Path) -> Vec<u8> {
    let out = Command::new(exe)
        .arg(file)
        .env("OUR_STDLIB", repo().join("stdlib"))
        .output()
        .unwrap();
    assert!(out.status.success(), "dumpsyms failed on {}", file.display());
    out.stdout
}

fn stage0_dump(file: &Path) -> Vec<u8> {
    pipeline::dump_symbols(file).into_bytes()
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

fn diff_one(exe: &Path, file: &Path) {
    let expected = stage0_dump(file);
    let actual = selfhost_dump(exe, file);
    if actual != expected {
        let pos = actual
            .iter()
            .zip(expected.iter())
            .position(|(a, b)| a != b)
            .unwrap_or(actual.len().min(expected.len()));
        panic!(
            "symbol dump diverged on {} at byte {pos}\n--- selfhosted ---\n{}\n--- stage0 ---\n{}",
            file.display(),
            String::from_utf8_lossy(&actual[..actual.len().min(pos + 400)]),
            String::from_utf8_lossy(&expected[..expected.len().min(pos + 400)]),
        );
    }
}

/// Write a fixture tree; returns its root file.
fn fixture(dir: &Path, files: &[(&str, &str)], root: &str) -> PathBuf {
    for (name, src) in files {
        let p = dir.join(name);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, src).unwrap();
    }
    dir.join(root)
}

#[test]
fn diffsyms() {
    // `stdlib_dir()` honors OUR_STDLIB — set it before first use so
    // both sides resolve `std.*` to the repo checkout.
    std::env::set_var("OUR_STDLIB", repo().join("stdlib"));

    let exe = selfhost_exe();
    let mut files = Vec::new();
    corpus(&repo(), &mut files);
    assert!(files.len() > 800, "corpus sweep found {}", files.len());

    // 1. The real corpus — pins, stdlib, programs, selfhost itself.
    //    Exercises every decl/body/generic/diag path the resolver
    //    emits.
    for f in &files {
        diff_one(&exe, f);
    }

    let fx = workdir().join("cases");
    std::fs::create_dir_all(&fx).unwrap();

    // 2. Cross-module generics — clone materialization with
    //    substituted fields/variants/params.
    let r = fixture(&fx.join("gen"), &[
        ("main.lang", "import g;\nfn main() -> i32 {\n    let p = g.Pair<i32, bool>::make(1, true);\n    return p.a;\n}\n"),
        ("g.lang", "module g;\npub struct Pair<A, B> { a: A, b: B }\nimpl Pair<A, B> {\n    pub fn make<A, B>(a: A, b: B) -> Pair<A, B> {\n        return Pair<A, B> { a: a, b: b };\n    }\n}\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 3. Private visibility — a private dep member named from the
    //    root resolves to a `private to its module` diag, not a
    //    crash.
    let r = fixture(&fx.join("priv"), &[
        ("main.lang", "import pv;\nfn main() -> i32 { return pv.hidden(); }\n"),
        ("pv.lang", "module pv;\nfn hidden() -> i32 { return 7; }\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 4. Methods on imported types — `v.push` desugars to the
    //    receiver module's fn.
    let r = fixture(&fx.join("meth"), &[
        ("main.lang", "import m;\nfn main() -> i32 {\n    let s = m.S { v: 1 };\n    return s.get();\n}\n"),
        ("m.lang", "module m;\npub struct S { v: i32 }\nimpl S {\n    pub fn get(own s: S) -> i32 { return s.v; }\n}\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 5. Match on an imported enum — variant resolution + arm binds.
    let r = fixture(&fx.join("match"), &[
        ("main.lang", "import e;\nfn main() -> i32 {\n    let x = e.E::B(3);\n    match x {\n        e.E::A => { return 0; }\n        e.E::B(v) => { return v; }\n    }\n}\n"),
        ("e.lang", "module e;\npub enum E { A, B(i32) }\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 6. Name collision — `f` exists in both modules; each resolves
    //    to its own qualifier.
    let r = fixture(&fx.join("collide"), &[
        ("main.lang", "import a;\nimport b;\nfn main() -> i32 { return a.f() + b.f(); }\n"),
        ("a.lang", "module a;\npub fn f() -> i32 { return 1; }\n"),
        ("b.lang", "module b;\npub fn f() -> i32 { return 2; }\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 7. Determinism — the symbol dump is a pure function of the fs.
    let a = selfhost_dump(&exe, &r);
    let b = selfhost_dump(&exe, &r);
    assert_eq!(a, b);
}
