//! `ourc` — the native compiler driver.
//!
//! ```text
//! ourc hello.lang              → hello.ll, hello.o, hello  (every stage
//!                                 leaves a readable artifact)
//! ourc hello.lang --show-ast   → print each intermediate form
//! ourc hello.lang --show-ir
//! ourc hello.lang --show-llvm
//! ourc hello.lang --show-asm
//! ourc hello.lang -o out
//! ourc test native-tests/      → run the native suite
//! ```

use ourc::driver::{ntest, pipeline};
use std::path::{Path, PathBuf};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "--help" || args[0] == "-h" {
        eprintln!("usage: ourc <file.lang> [-o out] [-g|--debug] [--show-ast|--show-ir|--show-llvm|--show-asm] [--diag=short]");
        eprintln!("       ourc --explain <CODE>   print an error family's entry");
        eprintln!("       ourc --emit-errors-doc  regenerate ERRORS.md from the registry");
        eprintln!("       ourc lsp                language server over stdio (publishDiagnostics)");
        eprintln!("       ourc test <dir>");
        eprintln!("       ourc --version        toolchain version (gates registry `requires`)");
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    if args[0] == "--explain" {
        match args.get(1).and_then(|c| ourc::errors::explain(c)) {
            Some(f) => println!("{} — {}\n\n{}", f.code, f.title, f.doc),
            None => {
                eprintln!("ourc: no error code `{}`", args.get(1).map(String::as_str).unwrap_or(""));
                std::process::exit(2);
            }
        }
        return;
    }
    if args[0] == "--emit-errors-doc" {
        print!("{}", ourc::errors::markdown());
        return;
    }
    if args[0] == "--version" || args[0] == "-V" {
        println!("ourc {}", env!("CARGO_PKG_VERSION"));
        return;
    }
    if args[0] == "lsp" {
        ourc::driver::lsp::serve();
        return;
    }
    if args[0] == "test" {
        let dir = args.get(1).map(String::as_str).unwrap_or("native-tests");
        let (pass, fail, report) = ntest::run_dir(Path::new(dir), &Default::default());
        print!("{report}");
        std::process::exit(if fail == 0 && pass > 0 { 0 } else { 1 });
    }
    // `ourc fmt <file>` — formatted source to stdout (single-file;
    // `our fmt` is the project-wide in-place driver).
    if args[0] == "fmt" {
        let Some(f) = args.get(1) else {
            eprintln!("usage: ourc fmt <file.lang>");
            std::process::exit(2);
        };
        let src = std::fs::read_to_string(f).unwrap_or_else(|e| {
            eprintln!("{f}: {e}");
            std::process::exit(1);
        });
        match ourc::driver::fmt::format_source(&src) {
            Ok(out) => print!("{out}"),
            Err(ds) => {
                eprint!("{}", ourc::driver::render::render_all(&ds));
                std::process::exit(1);
            }
        }
        return;
    }

    let src_path = PathBuf::from(&args[0]);
    let mut out_name: Option<String> = None;
    let mut show: Vec<String> = Vec::new();
    let mut debug = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out_name = args.get(i + 1).cloned();
                i += 1;
            }
            "-g" | "--debug" => debug = true,
            "--diag=short" => ourc::driver::render::set_short(true),
            s if s.starts_with("--show-") => show.push(s.to_string()),
            other => {
                eprintln!("ourc: unknown flag `{other}`");
                std::process::exit(2);
            }
        }
        i += 1;
    }

    // Compile once for inspectable artifacts only when a --show flag
    // needs them — `build` compiles internally; running both would
    // parse/check/emit the whole program twice.
    let needs_art = show
        .iter()
        .any(|s| matches!(s.as_str(), "--show-ast" | "--show-ir" | "--show-llvm"));
    if needs_art {
        let art = match pipeline::compile_path_roots(&src_path, &Default::default(), debug) {
            Ok(a) => a,
            Err(ds) => {
                eprintln!("{}", ourc::driver::render::render_all(&ds));
                std::process::exit(1);
            }
        };
        for flag in &show {
            match flag.as_str() {
                "--show-ast" => println!("{}", art.ast),
                "--show-ir" => println!("{}", art.ir),
                "--show-llvm" => println!("{}", art.llvm),
                _ => {}
            }
        }
    }

    // Output naming: `ourc hello.lang` → hello.{ll,o,exe} beside the
    // source unless -o redirects.
    let (out_dir, stem) = match &out_name {
        Some(name) => {
            let p = PathBuf::from(name);
            let dir = p.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            let st = p.file_stem().unwrap().to_string_lossy().to_string();
            (dir, st)
        }
        None => {
            let dir = src_path.parent().map(|d| d.to_path_buf()).unwrap_or_else(|| PathBuf::from("."));
            (dir, src_path.file_stem().unwrap().to_string_lossy().to_string())
        }
    };
    let exe = match pipeline::build_with(&src_path, &out_dir, &Default::default(), false, debug) {
        Ok(e) => e,
        Err(msg) => {
            eprint!("{msg}");
            std::process::exit(1);
        }
    };
    if show.iter().any(|s| s == "--show-asm") {
        match pipeline::asm(&out_dir.join(format!("{stem}.ll"))) {
            Ok(a) => println!("{a}"),
            Err(e) => eprintln!("ourc: --show-asm failed: {e}"),
        }
    }
    if show.is_empty() {
        println!("built {}", exe.display());
    }
}
