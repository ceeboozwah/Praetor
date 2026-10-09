//! N62 adversarial pins — the versioning/compatibility contract
//! (VERSIONING.md): lockfile format gates, version identity,
//! cross-version resource isolation, and reserved manifest names.

use ourc::driver::registry;
use std::path::{Path, PathBuf};
use std::process::Command;

fn ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ourc-n62-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

fn out(o: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn our(dir: &Path, args: &[&str], home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_our"))
        .args(args)
        .current_dir(dir)
        .env("OUR_HOME", home)
        .env("OUR_STDLIB", repo("stdlib"))
        .env("OUR_RUNTIME", repo("compiler/runtime"))
        .output()
        .expect("spawn our")
}

fn repo(rel: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).parent().unwrap().join(rel)
}

fn mk_app(dir: &Path, deps: &str) {
    write(
        &dir.join("project.toml"),
        &format!(
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n{deps}"
        ),
    );
    write(
        &dir.join("src/main.lang"),
        "fn main() -> i32 {\n    return 42;\n}\n",
    );
}

fn registry_app(dir: &Path, reg: &Path) {
    // One registry package so the lockfile has something to hold.
    let root = reg.join("src").join("lib").join("1.0.0");
    write(
        &root.join("project.toml"),
        "[project]\nname = \"lib\"\nversion = \"1.0.0\"\n\n[build]\nentry = \"src/util.lang\"\n",
    );
    write(&root.join("src/util.lang"), "pub fn f() -> i32 {\n    return 41;\n}\n");
    let hash = registry::tree_sha256(&root).unwrap();
    write(
        &reg.join("index").join("lib.toml"),
        &format!("[[version]]\nversion = \"1.0.0\"\nsha256 = \"{hash}\"\n"),
    );
    write(
        &dir.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\nlib = \"1.0\"\n",
    );
    write(
        &dir.join("src/main.lang"),
        "import lib.util;\nfn main() -> i32 {\n    return util.f() + 1;\n}\n",
    );
}

fn our_reg(dir: &Path, args: &[&str], home: &Path, reg: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_our"))
        .args(args)
        .current_dir(dir)
        .env("OUR_HOME", home)
        .env("OUR_REGISTRY", reg)
        .env("OUR_STDLIB", repo("stdlib"))
        .env("OUR_RUNTIME", repo("compiler/runtime"))
        .output()
        .expect("spawn our")
}

/// `our --version` and `ourc --version` agree and match the crate —
/// one source of truth (§3).
#[test]
fn version_identity_is_one_number() {
    let w = ws("ver");
    let home = w.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let a = our(&w, &["--version"], &home);
    let b = Command::new(env!("CARGO_BIN_EXE_ourc"))
        .arg("--version")
        .output()
        .unwrap();
    assert_eq!(
        String::from_utf8_lossy(&a.stdout).trim(),
        format!("our {}", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        String::from_utf8_lossy(&b.stdout).trim(),
        format!("ourc {}", env!("CARGO_PKG_VERSION"))
    );
}

/// `format = 99` → hard error naming the lockfile (§4). A newer
/// lockfile is never silently misread.
#[test]
fn newer_lockfile_format_hard_fails() {
    let w = ws("locknew");
    let home = w.join("home");
    let app = w.join("app");
    registry_app(&app, &w.join("registry"));
    write(
        &app.join("project.lock"),
        "# forged by a future toolchain\n[lock]\nformat = 99\n\n[dep.lib]\nsource = \"registry\"\nversion = \"1.0.0\"\nsha256 = \"0\"\n",
    );
    let r = our_reg(&app, &["check"], &home, &w.join("registry"));
    assert!(!r.status.success(), "must fail: {}", out(&r));
    assert!(
        out(&r).contains("newer than this toolchain"),
        "named the format gate: {}",
        out(&r)
    );
}

/// `format = 1` → stale: re-resolves and rewrites at format 2 (§4),
/// and `--locked` still treats it as drift.
#[test]
fn older_lockfile_format_reresolves() {
    let w = ws("lockold");
    let home = w.join("home");
    let reg = w.join("registry");
    let app = w.join("app");
    registry_app(&app, &reg);
    write(
        &app.join("project.lock"),
        "[lock]\nformat = 1\n\n[dep.lib]\nsource = \"registry\"\nversion = \"1.0.0\"\nsha256 = \"bogus\"\n",
    );
    // --locked: an unreadable/stale lock is drift → error, no rewrite.
    let r = our_reg(&app, &["check", "--locked"], &home, &reg);
    assert!(!r.status.success(), "stale lock under --locked: {}", out(&r));
    // Normal: re-resolves, rewrites at format 2 with the real hash.
    let r = our_reg(&app, &["check"], &home, &reg);
    assert!(r.status.success(), "re-resolve: {}", out(&r));
    let lock = std::fs::read_to_string(app.join("project.lock")).unwrap();
    assert!(lock.contains("format = 2"), "rewrote format: {lock}");
    assert!(!lock.contains("bogus"), "real hash replaced: {lock}");
}

/// Another toolchain version's materialized tree is read-never,
/// write-never (§5). Spawns with NO dev overrides — the embedded-
/// resource materialization path is the thing under test.
#[test]
fn foreign_version_resources_untouched() {
    let w = ws("coexist");
    let home = w.join("home");
    let marker = home.join("lib/9.9.9/stdlib/EVIL.lang");
    write(&marker, "a future toolchain's file\n");
    let app = w.join("app");
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n",
    );
    write(
        &app.join("src/main.lang"),
        "import std.io;\nfn main() -> i32 {\n    io.print_i64(1);\n    return 0;\n}\n",
    );
    // `our run` — check alone doesn't link, so it wouldn't touch the
    // runtime materialization path.
    let r = Command::new(env!("CARGO_BIN_EXE_our"))
        .args(["run"])
        .current_dir(&app)
        .env("OUR_HOME", &home)
        .env_remove("OUR_STDLIB")
        .env_remove("OUR_RUNTIME")
        .output()
        .expect("spawn our");
    assert!(r.status.success(), "run: {}", out(&r));
    // Own version's trees materialized; the foreign one untouched.
    assert!(
        home.join(format!("lib/{}/runtime/mmlrt.h", env!("CARGO_PKG_VERSION"))).is_file(),
        "own runtime materialized"
    );
    assert!(
        home.join(format!("lib/{}/stdlib/io.lang", env!("CARGO_PKG_VERSION"))).is_file(),
        "own stdlib materialized"
    );
    assert_eq!(
        std::fs::read_to_string(&marker).unwrap(),
        "a future toolchain's file\n",
        "foreign version's files mutated"
    );
}

/// `toolchain` in `[project]` is a reserved name, not a silently
/// ignored unknown key (§6).
#[test]
fn project_toolchain_is_reserved() {
    let w = ws("reserved");
    let home = w.join("home");
    let app = w.join("app");
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\ntoolchain = \"1.0\"\n\n[build]\nentry = \"src/main.lang\"\n",
    );
    write(&app.join("src/main.lang"), "fn main() -> i32 {\n    return 0;\n}\n");
    let r = our(&app, &["check"], &home);
    assert!(!r.status.success(), "must fail: {}", out(&r));
    assert!(out(&r).contains("reserved"), "named the reservation: {}", out(&r));
}

/// But a genuinely unknown key stays ignored — the forward-compat
/// seam (§7).
#[test]
fn unknown_manifest_key_ignored() {
    let w = ws("fwdcompat");
    let home = w.join("home");
    let app = w.join("app");
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\nedition = \"2049\"\nstability = \"experimental\"\n\n[build]\nentry = \"src/main.lang\"\n\n[some_future_table]\nx = \"y\"\nn = 7\nok = true\n",
    );
    write(&app.join("src/main.lang"), "fn main() -> i32 {\n    return 0;\n}\n");
    let r = our(&app, &["check"], &home);
    assert!(r.status.success(), "unknown keys must not block: {}", out(&r));
}

/// No prereleases anywhere (§3): dep reqs reject `1.2.3-rc1`.
#[test]
fn prerelease_req_rejected() {
    let w = ws("prerelease");
    let home = w.join("home");
    let app = w.join("app");
    mk_app(&app, "[dependencies]\nlib = \"1.2.3-rc1\"\n");
    let r = our(&app, &["check"], &home);
    assert!(!r.status.success(), "prerelease must fail: {}", out(&r));
}
