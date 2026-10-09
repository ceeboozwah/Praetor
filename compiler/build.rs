//! Embeds the toolchain's runtime resources into the `our` binary
//! (N61): every file under `stdlib/` and `compiler/runtime/` becomes
//! a `(&str, &[u8])` entry in `resources::FILES`, materialized to
//! `$OUR_HOME/lib/<ver>/` on first use — so an installed binary
//! needs nothing beyond itself.

use std::path::{Path, PathBuf};

fn collect(dir: &Path, group: &str, out: &mut Vec<(String, PathBuf)>) {
    let mut entries: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read {}: {e}", dir.display()))
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "lang" || x == "c" || x == "h"))
        .collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().unwrap().to_string_lossy().to_string();
        out.push((format!("{group}/{name}"), p));
    }
}

fn main() {
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let repo = manifest.parent().unwrap().to_path_buf();
    let mut files = Vec::new();
    collect(&repo.join("stdlib"), "stdlib", &mut files);
    collect(&manifest.join("runtime"), "runtime", &mut files);

    let mut gen = String::from(
        "pub const FILES: &[(&str, &[u8])] = &[\n",
    );
    for (name, path) in &files {
        gen.push_str(&format!(
            "    ({name:?}, include_bytes!({path:?})),\n",
            name = name,
            path = path.display().to_string()
        ));
    }
    gen.push_str("];\n");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("resources.rs");
    std::fs::write(&out, gen).unwrap();
    println!("cargo:rerun-if-changed={}", repo.join("stdlib").display());
    println!("cargo:rerun-if-changed={}", manifest.join("runtime").display());
}
