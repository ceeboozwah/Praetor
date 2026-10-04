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
        eprintln!("usage: ourc <file.lang> [-o out] [--show-ast|--show-ir|--show-llvm|--show-asm] [--diag=short]");
        eprintln!("       ourc test <dir>");
        std::process::exit(if args.is_empty() { 2 } else { 0 });
    }
    if args[0] == "test" {
        let dir = args.get(1).map(String::as_str).unwrap_or("native-tests");
        let (pass, fail, report) = ntest::run_dir(Path::new(dir));
        print!("{report}");
        std::process::exit(if fail == 0 && pass > 0 { 0 } else { 1 });
    }

    let src_path = PathBuf::from(&args[0]);
    let mut out_name: Option<String> = None;
    let mut show: Vec<String> = Vec::new();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                out_name = args.get(i + 1).cloned();
                i += 1;
            }
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
        let art = match pipeline::compile_path(&src_path) {
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
    let exe = match pipeline::build(&src_path, &out_dir) {
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
