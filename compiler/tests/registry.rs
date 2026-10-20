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
    // tmp+rename: the permissive server reads these files on its own
    // thread — a torn index must never surface as a flaky parse error.
    let tmp = p.with_extension("wtmp");
    std::fs::write(&tmp, s).unwrap();
    std::fs::rename(&tmp, p).unwrap();
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
    // 1.9.9 needs ourc ^9.0 — invisible to the running toolchain.
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

// ---- N60C: remote transport — same contract over GET ----

/// Read one HTTP request head — a single `read()` is NOT guaranteed
/// to return the full request line (segments can split), which used
/// to intermittently serve `stub`/404 for a partially-read path.
fn read_request_head(s: &mut std::net::TcpStream) -> Vec<u8> {
    use std::io::Read;
    let mut req = Vec::new();
    let mut buf = [0u8; 4096];
    while !req.windows(4).any(|w| w == b"\r\n\r\n") && req.len() < 65536 {
        match s.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => req.extend_from_slice(&buf[..n]),
        }
    }
    req
}

/// A minimal static-file HTTP server — enough to prove the transport
/// boundary is byte-identical to the directory layout.
fn serve(dir: PathBuf) -> (String, std::sync::mpsc::Sender<()>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || loop {
        l.set_nonblocking(true).unwrap();
        match l.accept() {
            Ok((mut s, _)) => {
                let req_bytes = read_request_head(&mut s);
                let req = String::from_utf8_lossy(&req_bytes);
                let rel = req.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
                let f = dir.join(rel);
                if rel.contains("..") || !f.is_file() {
                    let _ = s.write_all(b"HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\n\r\n");
                } else {
                    let bytes = std::fs::read(&f).unwrap();
                    let _ = s.write_all(
                        format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", bytes.len()).as_bytes(),
                    );
                    let _ = s.write_all(&bytes);
                }
            }
            Err(_) => {
                match stop_rx.try_recv() {
                    Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), stop_tx)
}

/// `files[]` + hash index entry — what a remote index needs.
fn index_files(reg: &Path, name: &str, entries: &[(&str, Option<&str>, bool)]) {
    fn walk(d: &Path, base: &Path, out: &mut Vec<String>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                let n = e.file_name().to_string_lossy().to_string();
                if n == "build" || n == ".git" {
                    continue;
                }
                walk(&p, base, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().to_string_lossy().replace('\\', "/"));
            }
        }
    }
    let mut s = String::new();
    for (ver, requires, yanked) in entries {
        let root = reg.join("src").join(name).join(ver);
        let mut files = Vec::new();
        walk(&root, &root, &mut files);
        files.sort();
        let hash = registry::tree_sha256(&root).unwrap();
        s.push_str("[[version]]\n");
        s.push_str(&format!("version = \"{ver}\"\nsha256 = \"{hash}\"\n"));
        let fl: Vec<String> = files.iter().map(|f| format!("\"{f}\"")).collect();
        s.push_str(&format!("files = [{}]\n", fl.join(", ")));
        if let Some(r) = requires {
            s.push_str(&format!("requires = \"{r}\"\n"));
        }
        if *yanked {
            s.push_str("yanked = true\n");
        }
    }
    write(&reg.join("index").join(format!("{name}.toml")), &s);
}

fn our_remote(dir: &Path, args: &[&str], url: &str, home: &Path) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_our"))
        .args(args)
        .current_dir(dir)
        .env("OUR_REGISTRY", url)
        .env("OUR_HOME", home)
        .output()
        .expect("spawn our")
}

#[test]
fn remote_fetch_resolve_run_and_offline() {
    let w = ws("remote");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 40, &[]);
    pkg(&reg, "lib", "1.2.0", 41, &[]);
    index_files(&reg, "lib", &[("1.0.0", None, false), ("1.2.0", None, false)]);
    let app = mk_app(&w, "lib = \"1.0\"");

    let (url, _stop) = serve(reg.clone());
    let r = our_remote(&app, &["run"], &url, &home);
    assert_eq!(r.status.code(), Some(42), "remote run: {}", out(&r));

    // The cache is the same verified tree a directory registry makes —
    // kill the server: --offline still builds from lock + cache alone.
    drop(_stop);
    let r = our_remote(&app, &["run", "--offline"], &url, &home);
    assert_eq!(r.status.code(), Some(42), "offline-after-remote: {}", out(&r));
    // …and without --offline the dead server is a fetch error, not a
    // resolution error.
    let r = our_remote(&app, &["build"], &url, &home);
    // warm cache + satisfied lock: build resolves lib again — the
    // index fetch IS attempted (no lock shortcut for selection), so
    // this must fail loudly rather than silently succeed.
    assert!(!r.status.success());
    assert!(out(&r).contains("fetch"), "expected fetch failure: {}", out(&r));
}

/// Permissive variant of `serve`: any path that isn't a real file
/// still gets a 200 — a hostile registry WILL answer whatever a
/// crafted `files[]` entry requests, so the fetch succeeding must
/// never be what a write relies on. (QA-001)
fn serve_permissive(dir: PathBuf) -> (String, std::sync::mpsc::Sender<()>) {
    use std::io::{Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = l.local_addr().unwrap().port();
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    std::thread::spawn(move || loop {
        l.set_nonblocking(true).unwrap();
        match l.accept() {
            Ok((mut s, _)) => {
                let req_bytes = read_request_head(&mut s);
                let req = String::from_utf8_lossy(&req_bytes);
                let rel = req.split_whitespace().nth(1).unwrap_or("/").trim_start_matches('/');
                let f = dir.join(rel);
                let body = if f.is_file() {
                    std::fs::read(&f).unwrap()
                } else {
                    b"stub".to_vec()
                };
                let _ = s.write_all(
                    format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", body.len()).as_bytes(),
                );
                let _ = s.write_all(&body);
            }
            Err(_) => {
                match stop_rx.try_recv() {
                    Ok(()) | Err(std::sync::mpsc::TryRecvError::Disconnected) => return,
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        std::thread::sleep(std::time::Duration::from_millis(5));
                    }
                }
            }
        }
    });
    (format!("http://127.0.0.1:{port}"), stop_tx)
}

#[test]
fn remote_traversal_rejected() {
    // QA-001 — a hostile index's `files[]` must never steer a write
    // outside `.tmp-<name>-<ver>`: every relpath is validated against
    // the component grammar BEFORE the first fetch, and validation
    // fails closed. Pre-fix, `../e1.txt` landed one level above tmp
    // inside the cache root; `sub/../../e2.txt` two levels up.
    let w = ws("remotetrv");
    let reg = w.join("registry");
    pkg(&reg, "lib", "1.0.0", 41, &[]);
    index_files(&reg, "lib", &[("1.0.0", None, false)]);
    let (url, _stop) = serve_permissive(reg.clone());
    let app = mk_app(&w, "lib = \"1.0\"");

    // `C:`-style drive prefixes and every traversal shape. (Raw
    // backslashes can't reach files[] — the index's TOML layer eats
    // them at parse — but the validator rejects them anyway: the
    // grammar, not any single parser, is the boundary.)
    let cases: &[&str] = &[
        "../e1.txt",
        "sub/../../e2.txt",
        "a/../../../e3.txt",
        "/abs/e4.txt",
        "C:/e5.txt",
        "./x/e7.txt",
        "a//b",
        "x/",
        "",
    ];
    for (i, f) in cases.iter().enumerate() {
        let home = w.join(format!("home{i}"));
        write(
            &reg.join("index/lib.toml"),
            &format!(
                "[[version]]\nversion = \"1.0.0\"\nsha256 = \"00\"\nfiles = [\"{f}\", \"project.toml\", \"src/util.lang\"]\n"
            ),
        );
        let r = our_remote(&app, &["build"], &url, &home);
        assert!(!r.status.success(), "payload `{f}` accepted: {}", out(&r));
        assert!(
            out(&r).contains("unsafe registry relpath"),
            "payload `{f}`: expected relpath rejection, got {}",
            out(&r)
        );
    }
    // Nothing may have escaped: no e*.txt anywhere outside a scratch
    // dir, and no half-staged tmp left behind.
    for i in 0..cases.len() {
        let home = w.join(format!("home{i}"));
        assert!(!home.join("registry/e1.txt").exists(), "e1 escaped into cache root");
        assert!(!home.join("e2.txt").exists(), "e2 escaped into OUR_HOME");
        assert!(!home.join("registry/.tmp-lib-1.0.0").exists(), "tmp left staged");
    }
    assert!(!w.join("e3.txt").exists(), "e3 escaped above OUR_HOME");

    // And the good case still works through the same grammar:
    // canonical relpaths fetch, hash, and promote normally.
    index_files(&reg, "lib", &[("1.0.0", None, false)]);
    let home = w.join("home-ok");
    let r = our_remote(&app, &["run"], &url, &home);
    assert_eq!(r.status.code(), Some(42), "clean remote run: {}", out(&r));
}

#[test]
fn remote_hash_mismatch_and_missing_files() {
    let w = ws("remotebad");
    let reg = w.join("registry");
    let home = w.join("home");
    pkg(&reg, "lib", "1.0.0", 41, &[]);
    index_files(&reg, "lib", &[("1.0.0", None, false)]);
    let (url, _stop) = serve(reg.clone());
    let app = mk_app(&w, "lib = \"1.0\"");

    // corrupt the served index hash — transport integrity must refuse
    write(
        &reg.join("index/lib.toml"),
        "[[version]]\nversion = \"1.0.0\"\nsha256 = \"0000000000000000000000000000000000000000000000000000000000000000\"\nfiles = [\"project.toml\", \"src/util.lang\"]\n",
    );
    let r = our_remote(&app, &["build"], &url, &home);
    assert!(!r.status.success());
    assert!(out(&r).contains("hash mismatch"), "remote integrity: {}", out(&r));

    // no files[] on a remote → named error, never a silent empty tree
    write(
        &reg.join("index/lib.toml"),
        "[[version]]\nversion = \"1.0.0\"\nsha256 = \"abc\"\n",
    );
    let home2 = w.join("home2"); // fresh cache — the entry must fetch
    let r = our_remote(&app, &["build"], &url, &home2);
    assert!(!r.status.success());
    assert!(out(&r).contains("no `files` list"), "missing files: {}", out(&r));
}
