//! `our` — the project tool (N21A). `ourc` is the compiler; `our` is
//! the driver a developer lives in:
//!
//! ```text
//! our new hello        scaffold project.toml + src/main.lang + tests/
//! our check            parse → load → monomorphize → check (no codegen)
//! our build            full pipeline → build/<entry-stem> executable
//! our run              build, then run the binary (args after -- pass through)
//! our test             run tests/*.lang — same @exit/@compile-error corpus
//! ```
//!
//! Everything resolves the project by walking up from the cwd for
//! `project.toml`, so commands work from any subdirectory.

use ourc::driver::{ntest, pipeline, project};
use std::path::PathBuf;
use std::process::Command;

fn usage() -> ! {
    eprintln!("usage: our <new|check|build|run|test> [args]");
    eprintln!("       our new <name>   scaffold a project");
    eprintln!("       our check        semantic pipeline only — no codegen");
    eprintln!("       our build        compile to build/<name>");
    eprintln!("       our run [-- args]  build, then execute");
    eprintln!("       our test         run tests/*.lang");
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
        "build" => cmd_build(),
        "run" => cmd_run(&args[1..]),
        "test" => cmd_test(),
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

fn cmd_check() {
    let p = cwd_project();
    let entry = p.entry();
    match pipeline::check_path(&entry) {
        Ok(()) => println!("checked {}", entry.display()),
        Err(ds) => {
            for d in &ds {
                match &d.file {
                    Some(f) => {
                        eprintln!("{f}:{}:{}: error: {}", d.span.line, d.span.col, d.msg)
                    }
                    None => eprintln!("{}:{}: error: {}", d.span.line, d.span.col, d.msg),
                }
            }
            std::process::exit(1);
        }
    }
}

/// Build the project and rename the binary to `build/<project-name>`
/// (the pipeline names it after the entry file's stem, which is always
/// `main` in the scaffolded layout).
fn build_project(p: &project::Project) -> Result<PathBuf, String> {
    let exe = pipeline::build(&p.entry(), &p.build_dir())?;
    let named = p.build_dir().join(&p.manifest.name);
    if exe != named {
        std::fs::rename(&exe, &named).map_err(|e| e.to_string())?;
    }
    Ok(named)
}

fn cmd_build() {
    let p = cwd_project();
    match build_project(&p) {
        Ok(exe) => println!("built {}", exe.display()),
        Err(msg) => {
            eprint!("{msg}");
            std::process::exit(1);
        }
    }
}

fn cmd_run(args: &[String]) {
    let p = cwd_project();
    // `our run -- a b` forwards `a b` to the program.
    let fwd: &[String] = match args.first().map(String::as_str) {
        Some("--") => &args[1..],
        _ => args,
    };
    let exe = match build_project(&p) {
        Ok(e) => e,
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
    let dir = p.tests_dir();
    if !dir.is_dir() {
        eprintln!("our test: no `tests/` directory in {}", p.root.display());
        std::process::exit(1);
    }
    let (pass, fail, report) = ntest::run_dir(&dir);
    print!("{report}");
    std::process::exit(if fail == 0 && pass > 0 { 0 } else { 1 });
}
