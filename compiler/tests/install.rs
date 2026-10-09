//! N61 — the stranger test (INSTALL.md §7): the `our` binary, alone
//! on a machine. Isolated HOME/OUR_HOME, cwd outside the repo, and
//! `OUR_STDLIB`/`OUR_RUNTIME` force-removed so the embedded-resource
//! materialization path is the only thing that can work.

use ourc::driver::registry;
use std::path::{Path, PathBuf};
use std::process::Command;

fn ws(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("ourc-n61-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// `our` under a stranger's environment: HOME and OUR_HOME point at
/// the throwaway workspace, the dev overrides are gone. Nothing in
/// the environment points back at the repo.
fn our(dir: &Path, args: &[&str], home: &Path) -> std::process::Output {
    our_env(dir, args, home, None)
}

fn our_env(dir: &Path, args: &[&str], home: &Path, reg: Option<&Path>) -> std::process::Output {
    let mut c = Command::new(env!("CARGO_BIN_EXE_our"));
    c.args(args)
        .current_dir(dir)
        .env("HOME", home)
        .env("OUR_HOME", home.join(".our"))
        .env_remove("OUR_STDLIB")
        .env_remove("OUR_RUNTIME");
    if let Some(r) = reg {
        c.env("OUR_REGISTRY", r);
    }
    c.output().expect("spawn our")
}

fn out(o: &std::process::Output) -> String {
    format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr))
}

fn write(p: &Path, s: &str) {
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, s).unwrap();
}

/// Scaffold → test → run, entirely from embedded resources. Asserts
/// the materialized tree appears under the isolated $OUR_HOME/lib.
#[test]
fn stranger_new_test_run() {
    let w = ws("stranger");
    let home = w.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let desk = w.join("desk"); // a cwd with no relation to any checkout
    std::fs::create_dir_all(&desk).unwrap();

    let v = our(&desk, &["--version"], &home);
    assert!(v.status.success(), "--version: {}", out(&v));
    assert_eq!(
        String::from_utf8_lossy(&v.stdout).trim(),
        format!("our {}", env!("CARGO_PKG_VERSION"))
    );

    let r = our(&desk, &["new", "hello"], &home);
    assert!(r.status.success(), "new: {}", out(&r));

    let proj = desk.join("hello");
    // The scaffold's `puts` extern imports nothing — swap in a real
    // `import std` program so stdlib materialization is exercised.
    write(
        &proj.join("src/main.lang"),
        "import std.io;\nfn main() -> i32 {\n    io.print_i64(7);\n    return 42;\n}\n",
    );
    let r = our(&proj, &["test"], &home);
    assert!(r.status.success(), "test: {}", out(&r));
    let r = our(&proj, &["run"], &home);
    assert_eq!(r.status.code(), Some(42), "run: {}", out(&r));

    // The embedded resources must have materialized — proof the
    // binary carried them.
    let lib = home.join(".our").join("lib").join(env!("CARGO_PKG_VERSION"));
    assert!(lib.join("stdlib/string.lang").is_file(), "stdlib materialized: {}", lib.display());
    assert!(lib.join("runtime/mmlrt_common.c").is_file(), "runtime materialized");
    assert!(lib.join("runtime/mmlrt.h").is_file(), "runtime header materialized");

    // `ourc` — the single-file compiler — under the same isolation.
    write(
        &desk.join("one.lang"),
        "import std.io;\nfn main() -> i32 {\n    io.print_i64(9);\n    return 7;\n}\n",
    );
    let r = Command::new(env!("CARGO_BIN_EXE_ourc"))
        .args(["one.lang", "-o", "one"])
        .current_dir(&desk)
        .env("HOME", &home)
        .env("OUR_HOME", home.join(".our"))
        .env_remove("OUR_STDLIB")
        .env_remove("OUR_RUNTIME")
        .output()
        .expect("spawn ourc");
    assert!(r.status.success(), "ourc: {}", out(&r));
    let r = Command::new(desk.join("one")).output().unwrap();
    assert_eq!(r.status.code(), Some(7), "compiled binary: {}", out(&r));
}

/// Registry deps end to end on a stranger's machine — directory
/// registry here; the transport half is pinned in registry.rs.
#[test]
fn stranger_registry_dep() {
    let w = ws("dep");
    let home = w.join("home");
    let reg = w.join("registry");

    // Fixture package: answer() = 41.
    let root = reg.join("src").join("lib").join("1.0.0");
    write(
        &root.join("project.toml"),
        "[project]\nname = \"lib\"\nversion = \"1.0.0\"\n\n[build]\nentry = \"src/util.lang\"\n",
    );
    write(&root.join("src/util.lang"), "pub fn answer() -> i32 {\n    return 41;\n}\n");
    let hash = registry::tree_sha256(&root).unwrap();
    write(
        &reg.join("index").join("lib.toml"),
        &format!("[[version]]\nversion = \"1.0.0\"\nsha256 = \"{hash}\"\n"),
    );

    let app = w.join("app");
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n\n[dependencies]\nlib = \"1.0\"\n",
    );
    write(
        &app.join("src/main.lang"),
        "import lib.util;\nfn main() -> i32 {\n    return util.answer() + 1;\n}\n",
    );

    let r = our_env(&app, &["run"], &home, Some(&reg));
    assert_eq!(r.status.code(), Some(42), "registry run: {}", out(&r));

    // The cache fill is observable; deleting the registry must not
    // matter under --offline.
    assert!(home.join(".our/registry/lib/1.0.0/src/util.lang").is_file());
    std::fs::remove_dir_all(&reg).unwrap();
    let r = our_env(&app, &["run", "--offline"], &home, Some(&reg));
    assert_eq!(r.status.code(), Some(42), "offline after registry deleted: {}", out(&r));
}
