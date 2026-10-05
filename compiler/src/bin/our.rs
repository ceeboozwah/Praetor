//! `our` — the project tool (N21A+N38). `ourc` is the compiler; `our`
//! is the driver a developer lives in:
//!
//! ```text
//! our new hello        scaffold project.toml + src/main.lang + tests/
//! our check            parse → load → monomorphize → check (no codegen)
//! our build [--release] compile to build/<name> (skips when fresh)
//! our run [-- args]    build, then run the binary
//! our test             run tests/*.lang — same @exit/@compile-error corpus
//! our clean            remove build/
//! our rebuild          clean + build
//! our deps             show the resolved dependency graph + lockfile
//! our fmt [--check]    format src/ + tests/ .lang files in place
//! ```
//!
//! Everything resolves the project by walking up from the cwd for
//! `project.toml`, so commands work from any subdirectory.
//!
//! [dependencies] entries name module roots: `lib = "../lib"` makes
//! `import lib.util` resolve `<dep>/src/util.lang` (N38). Resolution
//! is transitive and deterministic (name-sorted); `project.lock`
//! records the resolved set, `build/.fingerprint` short-circuits
//! unchanged builds, `build/build.info` records the tool/triple/
//! profile that produced the artifacts.
//!
//! A project's own modules are always visible as `<pkg>.<mod>` —
//! `import app.util` works in `src/` and `tests/` alike with no
//! `[dependencies]` entry, which is how tests reach project code
//! (N44T). A package may not depend on itself.

use ourc::driver::{ntest, pipeline, project, render};
use std::path::PathBuf;
use std::process::Command;

fn usage() -> ! {
    eprintln!("usage: our <new|check|build|run|test|clean|rebuild|deps> [args]");
    eprintln!("       our new <name>        scaffold a project");
    eprintln!("       our check             semantic pipeline only — no codegen");
    eprintln!("       our build [--release] compile to build/<name>");
    eprintln!("       our run [--release] [-- args]  build, then execute");
    eprintln!("       our test              run tests/*.lang");
    eprintln!("       our clean             remove build/");
    eprintln!("       our rebuild           clean, then build");
    eprintln!("       our deps              resolved dependency graph");
    eprintln!("       our fmt [--check]     format src/ + tests/ in place");
    std::process::exit(2);
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(cmd) = args.first().map(String::as_str) else {
        usage();
    };
    match cmd {
        "new" => cmd_new(&args[1..]),
        "check" => cmd_check(),
        "build" => cmd_build(&args[1..]),
        "run" => cmd_run(&args[1..]),
        "test" => cmd_test(),
        "clean" => cmd_clean(),
        "rebuild" => {
            cmd_clean_quiet();
            cmd_build(&args[1..]);
        }
        "deps" => cmd_deps(),
        "fmt" => cmd_fmt(&args[1..]),
        _ => usage(),
    }
}

fn cmd_new(args: &[String]) {
    let Some(name) = args.first() else {
        eprintln!("our new: missing project name");
        std::process::exit(2);
    };
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        eprintln!("our new: `{name}` is not a valid project name");
        std::process::exit(2);
    }
    let root = PathBuf::from(name);
    match project::scaffold(&root, name) {
        Ok(()) => {
            println!("created `{name}` — cd {name} && our run");
        }
        Err(e) => {
            eprintln!("our new: {e}");
            std::process::exit(1);
        }
    }
}

/// The project containing the cwd, or a clean failure.
fn cwd_project() -> project::Project {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    match project::discover(&cwd) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("our: {e}");
            std::process::exit(1);
        }
    }
}

/// Resolve the project's declared dependencies (transitively), refresh
/// `project.lock`, and return them. Every command that sees source
/// resolves through here so all of them agree on the package set.
fn resolved_deps(p: &project::Project) -> Vec<project::Dep> {
    match project::resolve_deps(&p.root, &p.manifest) {
        Ok(ds) => {
            if let Err(e) = project::write_lock(&p.root, &ds) {
                eprintln!("our: cannot write project.lock: {e}");
                std::process::exit(1);
            }
            ds
        }
        Err(e) => {
            eprintln!("our: {e}");
            std::process::exit(1);
        }
    }
}

fn cmd_check() {
    let p = cwd_project();
    let deps = resolved_deps(&p);
    let roots = project::roots_with_self(&p, &deps);
    let entry = p.entry();
    match pipeline::check_path_roots(&entry, &roots) {
        Ok(()) => println!("checked {}", entry.display()),
        Err(ds) => {
            eprint!("{}", render::render_all(&ds));
            std::process::exit(1);
        }
    }
}

/// `--release` anywhere in the flag list, or `[build] profile` in the
/// manifest, selects the release profile.
fn release_profile(args: &[String], p: &project::Project) -> bool {
    args.iter().any(|a| a == "--release") || p.manifest.profile == "release"
}

/// Build the project and rename the binary to `build/<project-name>`
/// (the pipeline names it after the entry file's stem, which is always
/// `main` in the scaffolded layout). The fingerprint short-circuit
/// skips codegen entirely when nothing the binary depends on changed.
fn build_project(p: &project::Project, release: bool) -> Result<(PathBuf, bool), String> {
    let deps = resolved_deps(p);
    let roots = project::roots_with_self(&p, &deps);
    let profile = if release { "release" } else { "debug" };
    let named = p.build_dir().join(&p.manifest.name);
    let fp = project::fingerprint(p, &deps, profile);
    if named.is_file() && project::read_fingerprint(&p.build_dir()) == Some(fp) {
        return Ok((named, true));
    }
    let exe = pipeline::build_with(&p.entry(), &p.build_dir(), &roots, release)?;
    if exe != named {
        std::fs::rename(&exe, &named).map_err(|e| e.to_string())?;
    }
    project::write_fingerprint(&p.build_dir(), fp)?;
    project::write_build_info(&p.build_dir(), profile, fp)?;
    Ok((named, false))
}

fn cmd_build(args: &[String]) {
    let p = cwd_project();
    let release = release_profile(args, &p);
    match build_project(&p, release) {
        Ok((exe, fresh)) => {
            println!("{} {}", if fresh { "fresh" } else { "built" }, exe.display())
        }
        Err(msg) => {
            eprint!("{msg}");
            std::process::exit(1);
        }
    }
}

fn cmd_run(args: &[String]) {
    let p = cwd_project();
    // `our run -- a b` forwards `a b` to the program.
    // `our run -- a b` forwards `a b` to the program; without `--`,
    // flags `our` understands (just `--release`) are filtered out.
    let fwd_owned: Vec<String> = match args.iter().position(|a| a == "--") {
        Some(i) => args[i + 1..].to_vec(),
        None => args.iter().filter(|a| *a != "--release").cloned().collect(),
    };
    let fwd: &[String] = &fwd_owned;
    let release = release_profile(args, &p);
    let exe = match build_project(&p, release) {
        Ok((e, _)) => e,
        Err(msg) => {
            eprint!("{msg}");
            std::process::exit(1);
        }
    };
    let status = match Command::new(&exe).args(fwd).status() {
        Ok(s) => s,
        Err(e) => {
            eprintln!("our run: {e}");
            std::process::exit(1);
        }
    };
    std::process::exit(status.code().unwrap_or(1));
}

fn cmd_test() {
    let p = cwd_project();
    let deps = resolved_deps(&p);
    let roots = project::roots_with_self(&p, &deps);
    let dir = p.tests_dir();
    if !dir.is_dir() {
        eprintln!("our test: no `tests/` directory in {}", p.root.display());
        std::process::exit(1);
    }
    let (pass, fail, report) = ntest::run_dir(&dir, &roots);
    print!("{report}");
    std::process::exit(if fail == 0 && pass > 0 { 0 } else { 1 });
}

fn cmd_clean_quiet() {
    let p = cwd_project();
    if p.build_dir().exists() {
        let _ = std::fs::remove_dir_all(p.build_dir());
    }
}

fn cmd_clean() {
    let p = cwd_project();
    let dir = p.build_dir();
    if dir.exists() {
        match std::fs::remove_dir_all(&dir) {
            Ok(()) => println!("removed {}", dir.display()),
            Err(e) => {
                eprintln!("our clean: {e}");
                std::process::exit(1);
            }
        }
    } else {
        println!("nothing to clean");
    }
}

fn cmd_deps() {
    let p = cwd_project();
    let deps = resolved_deps(&p);
    if deps.is_empty() {
        println!("{}: no dependencies", p.manifest.name);
        return;
    }
    for d in &deps {
        println!("{} {} -> {}", p.manifest.name, d.name, d.root.display());
    }
}

/// `our fmt [--check]` — reformat every `.lang` under `src/` and
/// `tests/` in place. `--check` lists files that would change and
/// exits 1 without writing (CI mode).
fn cmd_fmt(args: &[String]) {
    let check = args.iter().any(|a| a == "--check");
    let p = cwd_project();
    let mut files = Vec::new();
    for dir in [p.root.join("src"), p.tests_dir()] {
        collect_lang_files(&dir, &mut files);
    }
    files.sort();
    let mut dirty = 0;
    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("our fmt: {}: {e}", f.display());
                std::process::exit(1);
            }
        };
        match ourc::driver::fmt::format_source(&src) {
            Ok(out) if out != src => {
                dirty += 1;
                if check {
                    println!("would reformat {}", f.display());
                } else {
                    if let Err(e) = std::fs::write(f, &out) {
                        eprintln!("our fmt: {}: {e}", f.display());
                        std::process::exit(1);
                    }
                    println!("formatted {}", f.display());
                }
            }
            Ok(_) => {}
            Err(ds) => {
                eprint!("{}", render::render_all(&ds));
                std::process::exit(1);
            }
        }
    }
    if check {
        if dirty > 0 {
            eprintln!("{dirty} file(s) would change");
            std::process::exit(1);
        }
        println!("{} file(s) already formatted", files.len());
    }
}

fn collect_lang_files(dir: &std::path::Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let path = e.path();
        if path.is_dir() {
            collect_lang_files(&path, out);
        } else if path.extension().map(|x| x == "lang").unwrap_or(false) {
            out.push(path);
        }
    }
}
