//! N70 — the BOOTSTRAP.md §4 module-graph differential, executable:
//! `ourc --dump-modules` (Stage 0) and `selfhost/dumpgraph` (the
//! .lang loader) must emit byte-identical `(module-graph …)` dumps
//! — or identical `!` diagnostic lines — for every input: the whole
//! `.lang` corpus plus multi-file fixtures exercising cycles, name
//! collisions, missing/package/std imports, module-decl mismatches,
//! dep parse/lex/read errors, and diamonds.
//!
//! Canonical paths under the entry dir print relative, under the
//! stdlib `<std>/…`, else absolute — so fixture output is stable on
//! any machine.

use ourc::driver::pipeline;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn workdir() -> PathBuf {
    let d = repo().join("target/diffgraph");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Build `selfhost/dumpgraph.lang` with the Stage-0 pipeline — the
/// binary under test is a product of the oracle itself.
fn selfhost_exe() -> PathBuf {
    pipeline::build_with(
        &repo().join("selfhost/dumpgraph.lang"),
        &workdir(),
        &Default::default(),
        false,
        false,
    )
    .unwrap_or_else(|e| panic!("selfhost dumpgraph must build: {e}"))
}

/// Run the self-hosted graph dumper; returns stdout bytes. Both
/// sides use the repo stdlib — `OUR_STDLIB` pins it so the run does
/// not depend on the materialized `~/.our` tree's freshness.
fn selfhost_dump(exe: &Path, file: &Path) -> Vec<u8> {
    let out = Command::new(exe)
        .arg(file)
        .env("OUR_STDLIB", repo().join("stdlib"))
        .output()
        .unwrap();
    assert!(out.status.success(), "dumpgraph failed on {}", file.display());
    out.stdout
}

fn stage0_dump(file: &Path) -> Vec<u8> {
    pipeline::dump_modules(file).into_bytes()
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
            "module graph diverged on {} at byte {pos}\n--- selfhosted ---\n{}\n--- stage0 ---\n{}",
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
fn diffgraph() {
    // `stdlib_dir()` honors OUR_STDLIB — set it before first use so
    // both sides resolve `std.*` to the repo checkout.
    std::env::set_var("OUR_STDLIB", repo().join("stdlib"));

    let exe = selfhost_exe();
    let mut files = Vec::new();
    corpus(&repo(), &mut files);
    assert!(files.len() > 800, "corpus sweep found {}", files.len());

    // 1. The real corpus — pins, stdlib, programs, selfhost itself.
    //    Every file's import graph is exercised, including all the
    //    `import std.*` edges and missing-import diagnostics in
    //    native-tests/.
    for f in &files {
        diff_one(&exe, f);
    }

    let fx = workdir().join("cases");
    std::fs::create_dir_all(&fx).unwrap();

    // 2. Diamonds — shared deps dedup on canonical path.
    let r = fixture(&fx.join("diamond"), &[
        ("main.lang", "import a;\nimport b;\nfn main() -> i32 { return 0; }\n"),
        ("a.lang", "module a;\nimport leaf;\n"),
        ("b.lang", "module b;\nimport leaf;\n"),
        ("leaf.lang", "module leaf;\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 3. Cycles — 2-cycle, 3-cycle, self-import.
    let r = fixture(&fx.join("cyc2"), &[
        ("main.lang", "import a;\n"),
        ("a.lang", "module a;\nimport b;\n"),
        ("b.lang", "module b;\nimport a;\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("cyc3"), &[
        ("main.lang", "import a;\n"),
        ("a.lang", "module a;\nimport b;\n"),
        ("b.lang", "module b;\nimport c;\n"),
        ("c.lang", "module c;\nimport a;\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("selfcyc"), &[
        ("main.lang", "import s;\n"),
        ("s.lang", "module s;\nimport s;\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 4. Name collisions — two files claiming one qualifier.
    let r = fixture(&fx.join("collide"), &[
        ("main.lang", "import dup;\nimport sub.dup;\n"),
        ("dup.lang", "module dup;\n"),
        ("sub/dup.lang", "module dup;\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 5. Resolution failures — missing leaf, `pkg.` hint, `std.*`.
    let r = fixture(&fx.join("missing"), &[
        ("main.lang", "import gone;\nimport pk.gx;\nimport std.nope;\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    // Only the first missing import errors — one import per root.
    let r = fixture(&fx.join("missleaf"), &[
        ("main.lang", "import gone;\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("misspkg"), &[
        ("main.lang", "import pk.gx;\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("missstd"), &[
        ("main.lang", "import std.nope;\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("stdalone"), &[
        ("main.lang", "import std;\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 6. `module m` mismatch against the file stem.
    let r = fixture(&fx.join("mismatch"), &[
        ("main.lang", "import wrongname;\n"),
        ("wrongname.lang", "module wrong;\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 7. Dep-file errors — parse error, multi-lex-error, non-UTF-8,
    //    unreadable (dir at .lang path).
    let r = fixture(&fx.join("depparse"), &[
        ("main.lang", "import broken;\n"),
        ("broken.lang", "fn broken( {\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("deplex"), &[
        ("main.lang", "import bad;\n"),
        ("bad.lang", "let @ bad ^ chars ~ more $ here #\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("deputf8"), &[
        ("main.lang", "import n;\n"),
    ], "main.lang");
    std::fs::write(r.parent().unwrap().join("n.lang"), b"\xff\xfe x\n").unwrap();
    diff_one(&exe, &r);
    let r = fixture(&fx.join("depdir"), &[
        ("main.lang", "import d;\n"),
    ], "main.lang");
    std::fs::create_dir_all(r.parent().unwrap().join("d.lang")).unwrap();
    diff_one(&exe, &r);

    // 8. Root-side failures — parse error, lex errors, missing file,
    //    non-UTF-8 file, empty file, no imports.
    let r = fixture(&fx.join("rootparse"), &[
        ("main.lang", "fn main( {\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("rootlex"), &[
        ("main.lang", "let @ x ~ $ #\n"),
    ], "main.lang");
    diff_one(&exe, &r);
    diff_one(&exe, &fx.join("rootparse/nonexistent.lang"));
    let r = fixture(&fx.join("rootutf8"), &[("m.lang", "fn a() {}\n")], "m.lang");
    std::fs::write(&r, b"\xff\xfe\n").unwrap();
    diff_one(&exe, &r);
    let r = fixture(&fx.join("empty"), &[("m.lang", "")], "m.lang");
    diff_one(&exe, &r);
    let r = fixture(&fx.join("noimports"), &[
        ("main.lang", "module main;\nfn main() -> i32 { return 0; }\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 9. Nested dirs + stdlib transitive graph.
    let r = fixture(&fx.join("nested"), &[
        ("main.lang", "import sub.deep;\nimport std.vec;\nimport std.process;\n"),
        ("sub/deep.lang", "module deep;\nimport std.string;\nimport mid;\n"),
        ("sub/mid.lang", "module mid;\nimport std.result;\n"),
    ], "main.lang");
    diff_one(&exe, &r);

    // 10. Determinism — the graph dump is a pure function of the fs.
    let a = selfhost_dump(&exe, &r);
    let b = selfhost_dump(&exe, &r);
    assert_eq!(a, b);
}
