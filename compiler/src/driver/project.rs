//! The project tool's view of the world (N21A): a `project.toml` at
//! the root names the package and points at the entry file. Deliberately
//! minimal — the manifest is a map, not a build system:
//!
//! ```toml
//! [project]
//! name    = "hello"
//! version = "0.1.0"
//!
//! [build]
//! entry = "src/main.lang"
//! profile = "debug"        # or "release" — `our build --release` overrides
//!
//! [dependencies]
//! lib = "../lib"           # a local path package: `import lib.util`
//!                          # resolves `<dep>/src/util.lang`
//! ```
//!
//! The parser accepts the TOML subset that expresses: `[table]`
//! headers, `key = "string"` pairs, `key = integer`/`boolean`, and `#`
//! comments. Unknown keys are ignored so the format can grow without
//! breaking older tools.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// A parsed `project.toml`.
pub struct Manifest {
    /// `[project] name` — used for the output binary name.
    pub name: String,
    /// `[project] version` — carried, not yet used.
    pub version: String,
    /// `[build] entry` — path to the root `.lang` file, relative to
    /// the project root.
    pub entry: PathBuf,
    /// `[build] profile` — `"debug"` (default) or `"release"`.
    /// `our build --release` overrides.
    pub profile: String,
    /// `[dependencies]` — package name → declared path, sorted by
    /// name for deterministic resolution. A dependency named `lib`
    /// becomes a module root: `import lib.util` resolves
    /// `<dep>/src/util.lang`.
    pub deps: Vec<(String, PathBuf)>,
}

/// A resolved path dependency: its canonical package root (where its
/// own `project.toml` lives) and the `src/` directory that serves as
/// its module root.
pub struct Dep {
    pub name: String,
    pub root: PathBuf,
    pub src: PathBuf,
}

/// `name → src/` pairs for the module loader — dep names behave like
/// `std`: the first `import` path segment selects the root.
pub fn module_roots(deps: &[Dep]) -> HashMap<String, PathBuf> {
    deps.iter()
        .map(|d| (d.name.clone(), d.src.clone()))
        .collect()
}

/// Resolve a manifest's `[dependencies]` (and transitively theirs)
/// into a name-sorted package list. Path deps declare paths relative
/// to the manifest that names them. A dep's own `project.toml` must
/// exist and declare the same name. Two paths claiming one package
/// name, and dependency cycles, are errors.
pub fn resolve_deps(root: &Path, manifest: &Manifest) -> Result<Vec<Dep>, String> {
    let mut out: Vec<Dep> = Vec::new();
    let mut by_name: HashMap<String, PathBuf> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    resolve_from(root, manifest, &mut stack, &mut by_name, &mut out)?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn resolve_from(
    root: &Path,
    manifest: &Manifest,
    stack: &mut Vec<String>,
    by_name: &mut HashMap<String, PathBuf>,
    out: &mut Vec<Dep>,
) -> Result<(), String> {
    for (name, rel) in &manifest.deps {
        if name == "std" {
            return Err(format!(
                "{}: `std` is a reserved package name",
                root.join("project.toml").display()
            ));
        }
        let pkg_root = std::fs::canonicalize(root.join(rel)).map_err(|_| {
            format!(
                "dependency `{name}`: no directory `{}` (from {})",
                root.join(rel).display(),
                root.join("project.toml").display()
            )
        })?;
        if let Some(prev) = by_name.get(name) {
            if prev != &pkg_root {
                return Err(format!(
                    "package `{name}` resolves to both `{}` and `{}`",
                    prev.display(),
                    pkg_root.display()
                ));
            }
            continue; // already resolved — shared path dep
        }
        let mf_path = pkg_root.join("project.toml");
        if !mf_path.is_file() {
            return Err(format!(
                "dependency `{name}`: `{}` has no project.toml",
                mf_path.display()
            ));
        }
        let sub = load(&mf_path)?;
        if sub.name != *name {
            return Err(format!(
                "dependency `{name}` at `{}` declares name `{}`",
                pkg_root.display(),
                sub.name
            ));
        }
        if stack.iter().any(|s| s == name) {
            let mut chain = stack.clone();
            chain.push(name.clone());
            return Err(format!("dependency cycle: {}", chain.join(" -> ")));
        }
        let src = pkg_root.join("src");
        if !src.is_dir() {
            return Err(format!(
                "dependency `{name}`: `{}` has no src/ directory",
                pkg_root.display()
            ));
        }
        stack.push(name.clone());
        by_name.insert(name.clone(), pkg_root.clone());
        out.push(Dep {
            name: name.clone(),
            root: pkg_root.clone(),
            src,
        });
        resolve_from(&pkg_root, &sub, stack, by_name, out)?;
        stack.pop();
    }
    Ok(())
}

/// FNV-1a over the parts of the input set that decide the build:
/// tool version, profile, manifest bytes, then every `.lang` file
/// under the project root (excluding `build/` and `tests/` — test
/// corpus changes never invalidate the binary) and under each dep's
/// root, in sorted-path order. Stable across runs and machines.
pub fn fingerprint(p: &Project, deps: &[Dep], profile: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    let mut mix = |bytes: &[u8]| {
        for b in bytes {
            h ^= *b as u64;
            h = h.wrapping_mul(0x100000001b3);
        }
    };
    mix(env!("CARGO_PKG_VERSION").as_bytes());
    mix(profile.as_bytes());
    if let Ok(mf) = std::fs::read(p.root.join("project.toml")) {
        mix(&mf);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    collect_lang(&p.root, &p.root, &mut files);
    for d in deps {
        collect_lang(&d.root, &d.root, &mut files);
    }
    files.sort();
    files.dedup();
    for f in files {
        mix(f.to_string_lossy().as_bytes());
        if let Ok(b) = std::fs::read(&f) {
            mix(&b);
        }
    }
    h
}

fn collect_lang(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = e.file_name();
            let n = n.to_string_lossy();
            if p == base.join("build") || n == "build" && p.parent() == Some(base) {
                continue;
            }
            if n == "tests" || n == ".git" {
                continue;
            }
            collect_lang(&p, base, out);
        } else if p.extension().map(|x| x == "lang").unwrap_or(false) {
            out.push(p);
        }
    }
}

/// `project.lock` — the resolved dependency set on record. Written on
/// every resolution; a path dep whose contents change shows up as a
/// hash drift when compared with `read_lock`.
pub fn write_lock(root: &Path, deps: &[Dep]) -> Result<(), String> {
    let mut s = String::from(
        "# project.lock — generated by `our`, do not edit\n[lock]\n",
    );
    s.push_str(&format!("tool = \"ourc {}\"\n", env!("CARGO_PKG_VERSION")));
    for d in deps {
        let rel = d
            .root
            .strip_prefix(root)
            .map(|r| r.display().to_string())
            .unwrap_or_else(|_| d.root.display().to_string());
        s.push_str(&format!("\n[dep.{}]\npath = \"{}\"\n", d.name, rel));
        let mut h: u64 = 0xcbf29ce484222325;
        let mut files = Vec::new();
        collect_lang(&d.root, &d.root, &mut files);
        files.sort();
        for f in files {
            for b in f.to_string_lossy().as_bytes() {
                h ^= *b as u64;
                h = h.wrapping_mul(0x100000001b3);
            }
            if let Ok(b) = std::fs::read(&f) {
                for b in &b {
                    h ^= *b as u64;
                    h = h.wrapping_mul(0x100000001b3);
                }
            }
        }
        s.push_str(&format!("hash = \"{h:016x}\"\n"));
    }
    std::fs::write(root.join("project.lock"), s).map_err(|e| e.to_string())
}

/// `build/build.info` — what produced the artifacts, so a build is
/// auditable: tool version, profile, target triple, input fingerprint.
pub fn write_build_info(build_dir: &Path, profile: &str, fp: u64) -> Result<(), String> {
    let s = format!(
        "tool = \"ourc {}\"\nprofile = \"{profile}\"\ntarget = \"{}\"\nfingerprint = \"{fp:016x}\"\n",
        env!("CARGO_PKG_VERSION"),
        crate::driver::pipeline::target_triple(),
    );
    std::fs::write(build_dir.join("build.info"), s).map_err(|e| e.to_string())
}

/// The stored fingerprint, if a previous build left one.
pub fn read_fingerprint(build_dir: &Path) -> Option<u64> {
    let s = std::fs::read_to_string(build_dir.join(".fingerprint")).ok()?;
    u64::from_str_radix(s.trim(), 16).ok()
}

pub fn write_fingerprint(build_dir: &Path, fp: u64) -> Result<(), String> {
    std::fs::write(build_dir.join(".fingerprint"), format!("{fp:016x}\n"))
        .map_err(|e| e.to_string())
}
pub struct Project {
    pub root: PathBuf,
    pub manifest: Manifest,
}

impl Project {
    /// The entry source file, absolutized.
    pub fn entry(&self) -> PathBuf {
        self.root.join(&self.manifest.entry)
    }

    /// `build/` beside `project.toml` — where objects, `.ll`s, and the
    /// final binary land.
    pub fn build_dir(&self) -> PathBuf {
        self.root.join("build")
    }

    /// `tests/` beside `project.toml` — `our test` runs the `.lang`
    /// corpus here (same `@exit`/`@compile-error` directives as the
    /// compiler's own native suite).
    pub fn tests_dir(&self) -> PathBuf {
        self.root.join("tests")
    }
}

/// Walk `from` and its ancestors looking for `project.toml`; parse the
/// first one found.
pub fn discover(from: &Path) -> Result<Project, String> {
    let start = std::fs::canonicalize(from).unwrap_or_else(|_| from.to_path_buf());
    let mut dir: &Path = &start;
    loop {
        let cand = dir.join("project.toml");
        if cand.is_file() {
            let manifest = load(&cand)?;
            return Ok(Project {
                root: dir.to_path_buf(),
                manifest,
            });
        }
        dir = dir.parent().ok_or_else(|| {
            format!(
                "no project.toml found in `{}` or any parent directory",
                from.display()
            )
        })?;
    }
}

/// Parse a `project.toml` file.
fn load(path: &Path) -> Result<Manifest, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut section = String::new();
    let mut name = None;
    let mut version = String::new();
    let mut entry = PathBuf::from("src/main.lang");
    let mut profile = String::from("debug");
    let mut deps: Vec<(String, PathBuf)> = Vec::new();
    for (i, raw) in src.lines().enumerate() {
        // `#` starts a comment — inside a quoted string it's data, so
        // strip comments only outside quotes.
        let line = strip_comment(raw).trim().to_string();
        if line.is_empty() {
            continue;
        }
        if line.starts_with('[') {
            if !(line.starts_with('[') && line.ends_with(']') && line.len() > 2) {
                return Err(format!("{path_display}:{ln}: malformed table header `{line}`",
                    path_display = path.display(),
                    ln = i + 1));
            }
            section = line[1..line.len() - 1].trim().to_string();
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!(
                "{}:{}: expected `key = value`",
                path.display(),
                i + 1
            ));
        };
        let key = format!("{}.{}", section, k.trim());
        let val = parse_value(v.trim()).map_err(|e| {
            format!("{}:{}: {e}", path.display(), i + 1)
        })?;
        match key.as_str() {
            "project.name" => name = Some(val),
            "project.version" => version = val,
            "build.entry" => entry = PathBuf::from(val),
            "build.profile" => {
                if val != "debug" && val != "release" {
                    return Err(format!(
                        "{}:{}: `profile` must be `debug` or `release`",
                        path.display(),
                        i + 1
                    ));
                }
                profile = val;
            }
            _ => {
                if let Some(dep) = key.strip_prefix("dependencies.") {
                    if dep.is_empty()
                        || !dep
                            .chars()
                            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
                    {
                        return Err(format!(
                            "{}:{}: malformed dependency name `{dep}`",
                            path.display(),
                            i + 1
                        ));
                    }
                    deps.push((dep.to_string(), PathBuf::from(val)));
                }
                // other unknown keys — forward-compat, not errors yet.
            }
        }
    }
    let name = name.ok_or_else(|| {
        format!(
            "{}: missing `name` — add it under [project]",
            path.display()
        )
    })?;
    deps.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(Manifest {
        name,
        version,
        entry,
        profile,
        deps,
    })
}

/// Remove a trailing `#`-comment — unless the `#` sits inside quotes.
fn strip_comment(line: &str) -> &str {
    let mut in_str = false;
    for (i, c) in line.char_indices() {
        match c {
            '"' => in_str = !in_str,
            '#' if !in_str => return &line[..i],
            _ => {}
        }
    }
    line
}

/// The value half of `key = value`: a `"quoted"` string (bare
/// escapes), a bare integer, or a bare boolean — all returned as their
/// source text; the manifest only consumes strings today.
fn parse_value(v: &str) -> Result<String, String> {
    if let Some(rest) = v.strip_prefix('"') {
        let mut out = String::new();
        let mut cs = rest.chars();
        while let Some(c) = cs.next() {
            match c {
                '"' => return Ok(out),
                '\\' => match cs.next() {
                    Some('n') => out.push('\n'),
                    Some('t') => out.push('\t'),
                    Some('r') => out.push('\r'),
                    Some('"') => out.push('"'),
                    Some('\\') => out.push('\\'),
                    Some(other) => {
                        out.push('\\');
                        out.push(other);
                    }
                    None => return Err("unterminated string escape".to_string()),
                },
                c => out.push(c),
            }
        }
        return Err("unterminated string".to_string());
    }
    if v.parse::<i64>().is_ok() || v == "true" || v == "false" {
        return Ok(v.to_string());
    }
    Err(format!("malformed value `{v}`"))
}

/// The `our new <name>` scaffold.
pub fn scaffold(root: &Path, name: &str) -> Result<(), String> {
    if root.exists() && root.read_dir().map(|mut d| d.next().is_some()).unwrap_or(false) {
        return Err(format!("`{}` already exists and is not empty", root.display()));
    }
    let src_dir = root.join("src");
    let tests_dir = root.join("tests");
    std::fs::create_dir_all(&src_dir).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&tests_dir).map_err(|e| e.to_string())?;
    std::fs::write(
        root.join("project.toml"),
        format!(
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n# profile = \"release\"     # or pass --release to build/run\n\n# [dependencies]\n# lib = \"../lib\"          # a path package — `import lib.util`\n#                          # resolves <dep>/src/util.lang\n"
        ),
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        src_dir.join("main.lang"),
        "// Hello through the C FFI — literals lower to private\n\
         // null-terminated globals, so `*const u8` params take them\n\
         // directly.\n\
         extern fn puts(s: *const u8) -> i32;\n\n\
         fn main() -> i32 {\n    \
             puts(\"hello\");\n    \
             return 0;\n\
         }\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        tests_dir.join("smoke.lang"),
        "// @exit 42\n// `our test` runs every .lang file in tests/\n\
         // with the same directives as the compiler's own suite.\n\n\
         fn main() -> i32 {\n    return 42;\n}\n",
    )
    .map_err(|e| e.to_string())?;
    Ok(())
}
