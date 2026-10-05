//! N38 integration test — drive the real `our` binary against a
//! two-package workspace in a temp dir:
//!
//! ```text
//! ws/
//!   lib/project.toml      package `lib` (a library — no main needed)
//!   lib/src/util.lang     pub fn answer
//!   lib/src/helpers.lang  `import lib.util` — a dep self-imports by name
//!   app/project.toml      [dependencies] lib = "../lib"
//!   app/src/main.lang     `import lib.util` / `import lib.helpers`
//!   app/tests/dep.lang    @exit pin that also imports `lib.util`
//! ```
//!
//! Pins: resolve → lockfile → build → run = 42; a second build is
//! `fresh`; editing the dep invalidates the fingerprint; `our test`
//! sees the dep; `clean`/`rebuild`; undeclared package roots get a
//! named error; `--release` produces a working binary.

use std::path::{Path, PathBuf};
use std::process::Command;

fn our(dir: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_our"))
        .args(args)
        .current_dir(dir)
        .output()
        .expect("spawn our")
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn out(o: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

#[test]
fn multi_package_project_builds_and_caches() {
    let ws = std::env::temp_dir().join(format!("ourc-n38-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let lib = ws.join("lib");
    let app = ws.join("app");

    write(&lib.join("project.toml"), "[project]\nname = \"lib\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/lib.lang\"\n");
    write(&lib.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 40;\n}\n");
    write(
        &lib.join("src/helpers.lang"),
        "import lib.util;\npub fn two() -> i32 {\n    return util.answer() - 38;\n}\n",
    );
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\nlib = \"../lib\"\n",
    );
    write(
        &app.join("src/main.lang"),
        "import lib.util;\nimport lib.helpers;\nfn main() -> i32 {\n    return util.answer() + helpers.two();\n}\n",
    );
    write(
        &app.join("tests/dep.lang"),
        "// @exit 41\nimport lib.util;\nfn main() -> i32 {\n    return util.answer() + 1;\n}\n",
    );

    // resolve + build + run: the dep's modules answer through the
    // `lib` package root, and the dep can name its own package too.
    let b = our(&app, &["build"]);
    assert!(b.status.success(), "build: {}", out(&b));
    assert!(app.join("project.lock").is_file(), "no lockfile: {}", out(&b));
    let lock = std::fs::read_to_string(app.join("project.lock")).unwrap();
    assert!(lock.contains("[dep.lib]"), "lockfile: {lock}");
    assert!(app.join("build/build.info").is_file());

    let r = our(&app, &["run"]);
    assert_eq!(r.status.code(), Some(42), "run: {}", out(&r));

    // same inputs → the fingerprint short-circuits codegen.
    let b2 = our(&app, &["build"]);
    assert!(out(&b2).contains("fresh"), "second build: {}", out(&b2));

    // a dep-source edit invalidates the fingerprint.
    write(&lib.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 41;\n}\n");
    let b3 = our(&app, &["build"]);
    assert!(out(&b3).contains("built"), "post-edit build: {}", out(&b3));
    let r3 = our(&app, &["run"]);
    assert_eq!(r3.status.code(), Some(44), "post-edit run: {}", out(&r3));
    write(&lib.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 40;\n}\n");

    // tests/ compiles against the same dep roots.
    let t = our(&app, &["test"]);
    assert!(t.status.success(), "test: {}", out(&t));
    assert!(out(&t).contains("1 passed"), "test: {}", out(&t));

    // deps + clean + rebuild.
    let d = our(&app, &["deps"]);
    assert!(out(&d).contains("lib"), "deps: {}", out(&d));
    let c = our(&app, &["clean"]);
    assert!(c.status.success());
    assert!(!app.join("build/app").exists());
    let rb = our(&app, &["rebuild"]);
    assert!(rb.status.success(), "rebuild: {}", out(&rb));

    // an undeclared package root names what's missing.
    write(&app.join("src/main.lang"), "import zed.x;\nfn main() -> i32 {\n    return 0;\n}\n");
    let bad = our(&app, &["build"]);
    assert!(!bad.status.success());
    assert!(
        out(&bad).contains("`zed` is not a declared package"),
        "undeclared: {}",
        out(&bad)
    );
    write(
        &app.join("src/main.lang"),
        "import lib.util;\nimport lib.helpers;\nfn main() -> i32 {\n    return util.answer() + helpers.two();\n}\n",
    );

    // release profile produces a working binary.
    let rel = our(&app, &["build", "--release"]);
    assert!(rel.status.success(), "release: {}", out(&rel));
    let info = std::fs::read_to_string(app.join("build/build.info")).unwrap();
    assert!(info.contains("profile = \"release\""), "build.info: {info}");
    let rr = Command::new(app.join("build/app")).output().unwrap();
    assert_eq!(rr.status.code(), Some(42));

    let _ = std::fs::remove_dir_all(&ws);
}

/// N44T — `tests/` reaches the project's own modules as
/// `import <pkg>.<mod>` with no `[dependencies]` entry, and a
/// package depending on itself is rejected outright (the two halves
/// of the N43A bug-chain: without the first, the self-dep hack was
/// the only workaround; without the second, it was legal).
#[test]
fn tests_import_own_src_and_self_dep_rejected() {
    let ws = std::env::temp_dir().join(format!("ourc-n44t-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&ws);
    let app = ws.join("app");

    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n",
    );
    write(
        &app.join("src/util.lang"),
        "pub fn answer() -> i32 {\n    return 42;\n}\n",
    );
    write(
        &app.join("src/main.lang"),
        "import app.util;\nfn main() -> i32 {\n    return util.answer();\n}\n",
    );
    write(
        &app.join("tests/selftest.lang"),
        "// @exit 42\nimport app.util;\nfn main() -> i32 {\n    return util.answer();\n}\n",
    );

    // `import app.util` resolves in src AND tests with no dep entry.
    let c = our(&app, &["check"]);
    assert!(c.status.success(), "check: {}", out(&c));
    let r = our(&app, &["run"]);
    assert_eq!(r.status.code(), Some(42), "run: {}", out(&r));
    let t = our(&app, &["test"]);
    assert!(t.status.success(), "test: {}", out(&t));
    assert!(out(&t).contains("1 passed"), "test: {}", out(&t));

    // a package may not depend on itself.
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\napp = \".\"\n",
    );
    let s = our(&app, &["check"]);
    assert!(!s.status.success());
    assert!(
        out(&s).contains("cannot depend on itself"),
        "self-dep: {}",
        out(&s)
    );
    let d = our(&app, &["deps"]);
    assert!(!d.status.success());

    let _ = std::fs::remove_dir_all(&ws);
}
