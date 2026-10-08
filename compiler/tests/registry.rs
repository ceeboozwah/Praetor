//! N60B adversarial pins — the directory registry end to end against
//! the real `our` binary (PACKAGES.md). Fixture layout:
//!
//! ```text
//! ws/
//!   registry/index/<name>.toml     version list (sha256 computed)
//!   registry/src/<name>/<ver>/     package trees
//!   home/registry/                 the OUR_HOME dep cache
//!   app/project.toml + src/        the consumer
//! ```

use ourc::driver::registry;
use std::path::{Path, PathBuf};
use std::process::Command;

fn our(dir: &Path, args: &[&str], reg: &Path, home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_our"))
        .args(args)
        .current_dir(dir)
        .env("OUR_REGISTRY", reg)
        .env("OUR_HOME", home)
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

fn ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ourc-n60-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// A package tree at `registry/src/<name>/<ver>/` — `answer()` returns
/// `ret` so version selection is observable through exit codes.
fn pkg(reg: &Path, name: &str, ver: &str, ret: i32, deps: &[(&str, &str)]) {
    let root = reg.join("src").join(name).join(ver);
    let mut mf = format!(
        "[project]\nname = \"{name}\"\nversion = \"{ver}\"\n\n[build]\nentry = \"src/util.lang\"\n"
    );
    if !deps.is_empty() {
        mf.push_str("\n[dependencies]\n");
        for (n, r) in deps {
            mf.push_str(&format!("{n} = \"{r}\"\n"));
        }
    }
    write(&root.join("project.toml"), &mf);
    write(
        &root.join("src/util.lang"),
        &format!("pub fn answer() -> i32 {{\n    return {ret};\n}}\n"),
    );
}

/// `index/<name>.toml` with the real canonical hash per version.
fn index(reg: &Path, name: &str, entries: &[(&str, Option<&str>, bool)]) {
    let mut s = String::new();
    for (ver, requires, yanked) in entries {
        let hash = registry::tree_sha256(&reg.join("src").join(name).join(ver)).unwrap();
        s.push_str("[[version]]\n");
        s.push_str(&format!("version = \"{ver}\"\nsha256 = \"{hash}\"\n"));
        if let Some(r) = requires {
            s.push_str(&format!("requires = \"{r}\"\n"));
        }
        if *yanked {
            s.push_str("yanked = true\n");
        }
    }
    write(&reg.join("index").join(format!("{name}.toml")), &s);
}

fn mk_app(ws: &Path, dep_line: &str) -> PathBuf {
    let app = ws.join("app");
    write(
        &app.join("project.toml"),
        &format!(
            "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\n{dep_line}\n"
        ),
    );
    write(
        &app.join("src/main.lang"),
        "import lib.util;\nfn main() -> i32 {\n    return util.answer() + 1;\n}\n",
    );
    app
}

#[test]
fn highest_satisfying_wins() {
    let w = ws("highest");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 40, &[]);
    pkg(&reg, "lib", "1.2.0", 41, &[]);
    index(&reg, "lib", &[("1.0.0", None, false), ("1.2.0", None, false)]);
    let app = mk_app(&w, "lib = \"1.0\"");
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(42), "run: {}", out(&r));
    let lock = std::fs::read_to_string(app.join("project.lock")).unwrap();
    assert!(lock.contains("version = \"1.2.0\""), "lock: {lock}");
    assert!(lock.contains("source = \"registry\""), "lock: {lock}");
}

#[test]
fn yanked_excluded_unless_locked() {
    let w = ws("yank");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 40, &[]);
    pkg(&reg, "lib", "1.2.0", 41, &[]);
    // 1.2.0 yanked → 1.0.0 selected.
    index(&reg, "lib", &[("1.0.0", None, false), ("1.2.0", None, true)]);
    let app = mk_app(&w, "lib = \"1.0\"");
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(41), "yanked must lose: {}", out(&r));

    // Un-yank → 1.2.0 locks; re-yank → the lock still builds it.
    index(&reg, "lib", &[("1.0.0", None, false), ("1.2.0", None, false)]);
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(42), "run: {}", out(&r));
    index(&reg, "lib", &[("1.0.0", None, false), ("1.2.0", None, true)]);
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(42), "yanked+locked must keep building: {}", out(&r));
}

#[test]
fn toolchain_requires_filters() {
    let w = ws("requires");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 40, &[]);
    pkg(&reg, "lib", "1.9.9", 41, &[]);
    // 1.9.9 needs ourc ^9.0 — invisible to ourc 0.1.0.
    index(&reg, "lib", &[("1.0.0", None, false), ("1.9.9", Some("^9.0"), false)]);
    let app = mk_app(&w, "lib = \"1.0\"");
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(41), "requires must filter: {}", out(&r));

    // And when EVERY satisfying version is invisible → named error.
    index(&reg, "lib", &[("1.9.9", Some("^9.0"), false)]);
    let r = our(&app, &["build"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("yanked or needs a newer ourc"), "{}", out(&r));
}

#[test]
fn hash_mismatch_hard_fails() {
    let w = ws("hash");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 40, &[]);
    // a plausible-but-wrong index hash — the cache verify must refuse
    write(
        &reg.join("index/lib.toml"),
        "[[version]]\nversion = \"1.0.0\"\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\n",
    );
    let app = mk_app(&w, "lib = \"1.0\"");
    let r = our(&app, &["build"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("hash mismatch"), "expected integrity failure: {}", out(&r));
}

#[test]
fn offline_uses_lock_and_cache_only() {
    let w = ws("offline");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 41, &[]);
    index(&reg, "lib", &[("1.0.0", None, false)]);
    let app = mk_app(&w, "lib = \"1.0\"");

    // cold cache + --offline → hard fail naming the cache miss
    let r = our(&app, &["build", "--offline"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("cannot resolve --offline"), "{}", out(&r));

    // warm the cache, then delete the registry entirely: --offline
    // proves zero index/tree reads — there is nothing left to read.
    let r = our(&app, &["build"], &reg, &home);
    assert!(r.status.success(), "{}", out(&r));
    std::fs::remove_dir_all(&reg).unwrap();
    let r = our(&app, &["run", "--offline"], &reg, &home);
    assert_eq!(r.status.code(), Some(42), "offline run: {}", out(&r));
    // …and without --offline the missing registry IS an error.
    let r = our(&app, &["build"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("no registry"), "{}", out(&r));
}

#[test]
fn locked_detects_drift() {
    let w = ws("locked");
    let reg = w.join("registry");
    let home = w.join("home");
    let lib = w.join("lib");
    write(&lib.join("project.toml"), "[project]\nname = \"lib\"\nversion = \"1.0.0\"\n\n[build]\nentry = \"src/util.lang\"\n");
    write(&lib.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 41;\n}\n");
    let app = mk_app(&w, "lib = { path = \"../lib\" }");

    // resolve once → lock exists → --locked is happy
    let r = our(&app, &["build"], &reg, &home);
    assert!(r.status.success(), "{}", out(&r));
    let r = our(&app, &["build", "--locked"], &reg, &home);
    assert!(r.status.success(), "--locked clean: {}", out(&r));

    // edit the dep → sha drift → --locked refuses
    write(&lib.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 99;\n}\n");
    let r = our(&app, &["build", "--locked"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("--locked"), "expected drift error: {}", out(&r));
    // …and the lockfile was NOT rewritten to bless the drift.
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(100), "unlocked picks it up: {}", out(&r));
}

#[test]
fn transitive_registry_and_path_override() {
    let w = ws("transitive");
    let reg = w.join("registry");
    let home = w.join("home");
    // mid (registry) → lib "=1.2.0" (registry)
    pkg(&reg, "lib", "1.0.0", 40, &[]);
    pkg(&reg, "lib", "1.2.0", 41, &[]);
    index(&reg, "lib", &[("1.0.0", None, false), ("1.2.0", None, false)]);
    // mid re-exports lib.answer via its own module — named
    // `mid_api`, not `util`: module qualifiers are leaf names, so
    // `mid/util.lang` and `lib/util.lang` would collide (the loader
    // rejects shared leaf names across the graph).
    let mid = reg.join("src/mid/1.0.0");
    write(&mid.join("project.toml"),
        "[project]\nname = \"mid\"\nversion = \"1.0.0\"\n\n[build]\nentry = \"src/mid_api.lang\"\n\n[dependencies]\nlib = \"=1.2.0\"\n");
    write(&mid.join("src/mid_api.lang"),
        "import lib.util;\npub fn via() -> i32 {\n    return util.answer();\n}\n");
    index(&reg, "mid", &[("1.0.0", None, false)]);

    // app uses mid only; lib arrives transitively pinned =1.2.0
    let app = w.join("app");
    write(&app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\nmid = \"1.0\"\n");
    write(&app.join("src/main.lang"),
        "import mid.mid_api;\nfn main() -> i32 {\n    return mid_api.via() + 1;\n}\n");
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(42), "transitive: {}", out(&r));
    let lock = std::fs::read_to_string(app.join("project.lock")).unwrap();
    assert!(lock.contains("[dep.lib]"), "lock: {lock}");
    assert!(lock.contains("version = \"1.2.0\""), "lock: {lock}");

    // path override satisfying the transitive req → wins over index
    let locallib = w.join("lib");
    write(&locallib.join("project.toml"), "[project]\nname = \"lib\"\nversion = \"1.2.0\"\n\n[build]\nentry = \"src/util.lang\"\n");
    write(&locallib.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 50;\n}\n");
    write(&app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\nlib = { path = \"../lib\", version = \"1.2\" }\nmid = \"1.0\"\n");
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(51), "override: {}", out(&r));

    // path override that violates the transitive req → resolve error
    write(&app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\nlib = { path = \"../lib\" }\nmid = \"1.0\"\n");
    write(&locallib.join("project.toml"), "[project]\nname = \"lib\"\nversion = \"0.9.0\"\n\n[build]\nentry = \"src/util.lang\"\n");
    let r = our(&app, &["build"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("lib"), "expected named conflict: {}", out(&r));
}

#[test]
fn build_and_git_dirs_never_affect_hash() {
    let w = ws("hashdirs");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 41, &[]);
    // junk that must not exist in the canonical tree
    write(&reg.join("src/lib/1.0.0/build/junk.o"), "garbage");
    write(&reg.join("src/lib/1.0.0/.git/HEAD"), "garbage");
    // a shipped test that must be hashed but never built
    write(&reg.join("src/lib/1.0.0/tests/broken.lang"), "this is not even lang syntax {{{");
    index(&reg, "lib", &[("1.0.0", None, false)]);
    let app = mk_app(&w, "lib = \"1.0\"");
    let r = our(&app, &["run"], &reg, &home);
    assert_eq!(r.status.code(), Some(42), "run: {}", out(&r));
    let cache = home.join("registry/lib/1.0.0");
    assert!(!cache.join("build").exists(), "build/ must not be copied");
    assert!(!cache.join(".git").exists(), ".git/ must not be copied");
    // the shipped-but-never-built test DID land (it's hashed input)
    assert!(cache.join("tests/broken.lang").is_file());
}

#[test]
fn missing_package_and_bad_req_names() {
    let w = ws("errors");
    let reg = w.join("registry");
    let home = w.join("home");
    std::fs::create_dir_all(reg.join("index")).unwrap();
    let app = mk_app(&w, "ghost = \"1.0\"");
    let r = our(&app, &["build"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("not in the registry index"), "{}", out(&r));

    let app2 = mk_app(&w, "lib = \"nonsense-req-string\"");
    // unparseable string falls back to a path dep — which then fails
    // honestly as a missing directory.
    let r = our(&app2, &["build"], &reg, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("no directory"), "{}", out(&r));
}
