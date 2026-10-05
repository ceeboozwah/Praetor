//! N45 integration test — `our doc` renders a module's public API
//! from source: `pub` signatures, struct fields, enum variants and
//! `///` doc comments; private decls never appear. Runs against the
//! real binary in a temp workspace plus the shipped `stdlib/`.

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

fn stdout(o: &std::process::Output) -> String {
    String::from_utf8_lossy(&o.stdout).to_string()
}

fn ws() -> PathBuf {
    std::env::temp_dir().join(format!("ourc-doc-{}", std::process::id()))
}

/// `std.*` — the reference renders the stdlib's real surface:
/// generic signatures, `read`/`write`/`own` modes, cross-module
/// types qualified, doc comments lifted, private decls absent.
#[test]
fn doc_renders_stdlib_public_api() {
    let dir = std::env::temp_dir(); // no project needed for `std`
    let o = our(&dir, &["doc", "std.omap"]);
    assert!(o.status.success());
    let d = stdout(&o);
    assert!(d.starts_with("std.omap\n"), "header: {d}");
    // module doc — the owned-vs-borrowed key contract (N43A's hole).
    assert!(d.contains("OWNED-key"), "module doc: {d}");
    // generic signatures with modes preserved.
    assert!(d.contains("fn insert<V>(own self: OMap<V>, k: str, v: V) -> OPut<V>"), "insert: {d}");
    assert!(d.contains("fn get<V>(read self: OMap<V>, k: str)"), "get: {d}");
    // cross-module types stay qualified.
    assert!(d.contains("result.Result<OEntry<V>,i32>"), "result: {d}");
    // struct fields + enum variants shown.
    assert!(d.contains("struct OMap<V>") && d.contains("heap: alloc"), "fields: {d}");
    assert!(d.contains("Fresh(OMap<V>)"), "variants: {d}");

    let fs = our(&dir, &["doc", "std.fs"]);
    assert!(fs.status.success());
    let d = stdout(&fs);
    assert!(d.contains("fn slurp_r(heap: alloc, path: *const u8)"), "fs: {d}");
    assert!(d.contains("vec.Vec<u8>"), "fs generics: {d}");

    // deterministic — two runs byte-identical.
    let o2 = our(&dir, &["doc", "std.omap"]);
    assert_eq!(stdout(&o), stdout(&o2));

    // `our doc std` — bare package lists its modules, so the
    // surface is enumerable without reading the stdlib directory.
    let ls = our(&dir, &["doc", "std"]);
    assert!(ls.status.success());
    let d = stdout(&ls);
    assert!(d.contains("std.omap"), "module list: {d}");
    assert!(d.contains("std.fs"), "module list: {d}");
    assert!(d.contains("std.string"), "module list: {d}");
    // a missing module's error points back at the listing.
    let e = our(&dir, &["doc", "std.nothere"]);
    assert!(!e.status.success());
    let se = String::from_utf8_lossy(&e.stderr);
    assert!(se.contains("lists std's modules"), "hint: {se}");
}

/// A project's own modules doc exactly like `std.*` — `pub` only,
/// per-decl `///` comments, malformed docs can't break the module.
#[test]
fn doc_renders_project_module() {
    let ws = ws();
    let _ = std::fs::remove_dir_all(&ws);
    let app = ws.join("app");
    write(
        &app.join("project.toml"),
        "[project]\nname = \"app\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n",
    );
    write(
        &app.join("src/main.lang"),
        "fn main() -> i32 {\n    return 0;\n}\n",
    );
    write(
        &app.join("src/store.lang"),
        "/// Keeps things. Also a doc with `odd *chars` and trailing whitespace  \n\
         ///   that stays verbatim.\n\
         pub struct Store<T> {\n    h: alloc,\n    n: i64,\n}\n\
         /// Inserts one `T`.\n\
         pub fn put<T>(write self: Store<T>, v: T) -> i32 {\n    return 0;\n}\n\
         fn helper() -> i32 {\n    return 1;\n}\n\
         extern fn memcpy(dst: *void, src: *const void, n: u64) -> *void;\n",
    );

    let o = our(&app, &["doc", "app.store"]);
    assert!(o.status.success(), "{}", stdout(&o));
    let d = stdout(&o);
    assert!(d.starts_with("app.store\n"));
    assert!(d.contains("Keeps things. Also a doc with `odd *chars`"), "doc: {d}");
    assert!(d.contains("struct Store<T>"), "struct: {d}");
    assert!(d.contains("fn put<T>(write self: Store<T>, v: T) -> i32"), "sig: {d}");
    // private fn and extern decl absent.
    assert!(!d.contains("helper"), "private: {d}");
    assert!(!d.contains("memcpy"), "extern: {d}");

    // The same file still compiles — doc comments are trivia.
    let c = our(&app, &["check"]);
    assert!(c.status.success(), "check: {}", stdout(&c));

    // Unknown package / missing module name real errors.
    let e = our(&app, &["doc", "zed.mod"]);
    assert!(!e.status.success());
    assert!(stdout(&e).contains("not a known package") || String::from_utf8_lossy(&e.stderr).contains("not a known package"));
    let e = our(&app, &["doc", "app.nothere"]);
    assert!(!e.status.success());

    let _ = std::fs::remove_dir_all(&ws);
}
