//! N63 adversarial pins — toolchain selection & dispatch
//! (CHANNELS.md): manifest pins, OUR_TOOLCHAIN override, the
//! sibling pool, the loop guard against mislabeled binaries, and
//! the explicit-error-never-fallback rule.

use std::path::{Path, PathBuf};
use std::process::Command;

fn ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ourc-n63-{tag}-{}", std::process::id()));
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

fn our(dir: &Path, args: &[&str], extra: &[(&str, &str)]) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_our"));
    c.args(args).current_dir(dir);
    for (k, v) in extra {
        c.env(k, v);
    }
    c.output().expect("spawn our")
}

fn mk_app(dir: &Path, toolchain_line: &str) {
    write(
        &dir.join("project.toml"),
        &format!(
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\n{toolchain_line}\n\n[build]\nentry = \"src/main.lang\"\n"
        ),
    );
    write(&dir.join("src/main.lang"), "fn main() -> i32 {\n    return 0;\n}\n");
}

/// A req the running binary satisfies — zero dispatch, zero noise.
#[test]
fn pin_satisfied_runs_normally() {
    let w = ws("satisfied");
    let app = w.join("app");
    mk_app(&app, "toolchain = \"0.1\"");
    let r = our(&app, &["check"], &[]);
    assert!(r.status.success(), "check: {}", out(&r));
    assert!(!out(&r).contains("dispatching"), "no dispatch happened: {}", out(&r));
}

/// A req nothing satisfies — hard error naming the req, never a
/// silent fallback to whatever's installed (§3).
#[test]
fn pin_unsatisfiable_hard_fails() {
    let w = ws("unsat");
    let app = w.join("app");
    mk_app(&app, "toolchain = \"=9.9.9\"");
    let r = our(&app, &["check"], &[]);
    assert!(!r.status.success(), "must fail: {}", out(&r));
    assert!(out(&r).contains("requires toolchain"), "named the req: {}", out(&r));
    assert!(out(&r).contains("install a satisfying"), "told the user what to do: {}", out(&r));
    // And it must fail identically for a build — the pin binds every
    // project command, not just check.
    let r = our(&app, &["build"], &[]);
    assert!(!r.status.success(), "build must fail too: {}", out(&r));
}

/// `OUR_TOOLCHAIN` beats the manifest — a satisfied pin is
/// overridden into failure (§3, order of evaluation).
#[test]
fn env_overrides_manifest() {
    let w = ws("envwins");
    let app = w.join("app");
    mk_app(&app, "toolchain = \"0.1\"");
    let r = our(&app, &["check"], &[("OUR_TOOLCHAIN", "=9.9.9")]);
    assert!(!r.status.success(), "env req must win: {}", out(&r));
    assert!(out(&r).contains("requires toolchain `=9.9.9`"), "env req named: {}", out(&r));

    // Malformed env req is a config error, not an ignore.
    let r = our(&app, &["check"], &[("OUR_TOOLCHAIN", "banana")]);
    assert!(!r.status.success(), "malformed env req: {}", out(&r));
    assert!(out(&r).contains("OUR_TOOLCHAIN"), "named the var: {}", out(&r));
}

/// The adversarial case: a sibling named `our-0.1.9` that's really
/// 0.1.0 inside. Dispatch must fire (the filename satisfies) — and
/// the child must refuse to re-dispatch, naming the lie (§4).
/// Without the guard this test would hang on an exec loop.
#[test]
fn mislabeled_sibling_cannot_loop() {
    let w = ws("mislabel");
    let app = w.join("app");
    mk_app(&app, "toolchain = \"^0.1.5\""); // 0.1.0 fails; a "0.1.9" satisfies
    let exe = Path::new(env!("CARGO_BIN_EXE_our"));
    let fake = exe.with_file_name("our-0.1.9");
    std::fs::copy(exe, &fake).unwrap();

    let r = our(&app, &["check"], &[]);
    let _ = std::fs::remove_file(&fake);

    assert!(!r.status.success(), "must fail: {}", out(&r));
    assert!(
        out(&r).contains("dispatching to") && out(&r).contains("our-0.1.9"),
        "parent dispatched on the filename claim: {}",
        out(&r)
    );
    assert!(
        out(&r).contains("reports 0.1.0") && out(&r).contains("filename does not match"),
        "child caught the lie instead of looping: {}",
        out(&r)
    );
}

/// `our toolchain` — the inspectable surface: running version, the
/// manifest's req, the sibling pool.
#[test]
fn toolchain_subcommand_reports() {
    let w = ws("report");
    let app = w.join("app");
    mk_app(&app, "toolchain = \"0.2\"");
    let r = our(&app, &["toolchain"], &[]);
    assert!(r.status.success(), "toolchain: {}", out(&r));
    assert!(
        out(&r).contains(&format!("our {}", env!("CARGO_PKG_VERSION"))),
        "running version: {}",
        out(&r)
    );
    assert!(out(&r).contains("toolchain req"), "req reported: {}", out(&r));
}

/// A project with no pin runs whatever was invoked — the req is
/// opt-in, never inferred.
#[test]
fn no_pin_no_dispatch() {
    let w = ws("nopin");
    let app = w.join("app");
    mk_app(&app, "");
    let r = our(&app, &["toolchain"], &[]);
    assert!(out(&r).contains("(none"), "no req reported: {}", out(&r));
    let r = our(&app, &["check"], &[]);
    assert!(r.status.success(), "check: {}", out(&r));
}
