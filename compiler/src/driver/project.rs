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
//! ```
//!
//! The parser accepts the TOML subset that expresses: `[table]`
//! headers, `key = "string"` pairs, `key = integer`/`boolean`, and `#`
//! comments. Unknown keys are ignored so the format can grow without
//! breaking older tools.

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
}

/// A discovered project: the directory holding `project.toml` and its
/// parsed manifest.
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
            _ => {} // forward-compat: unknown keys are not errors yet.
        }
    }
    let name = name.ok_or_else(|| {
        format!(
            "{}: missing `name` — add it under [project]",
            path.display()
        )
    })?;
    Ok(Manifest {
        name,
        version,
        entry,
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
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n\n[build]\nentry = \"src/main.lang\"\n"
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
