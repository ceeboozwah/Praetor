//! The directory registry (PACKAGES.md §6/§7). A registry is a
//! directory: `index/<name>.toml` lists versions with their
//! canonical tree hash, toolchain requirement, and yank flag;
//! `src/<name>/<version>/` holds the unpacked package trees. The
//! download cache lives at `$OUR_HOME/registry` (default
//! `~/.our/registry`). Everything here is local I/O — a remote
//! registry later changes only the transport.

use crate::driver::semver::{self, Req, Ver};
use crate::driver::sha256::sha256_hex;
use std::path::{Path, PathBuf};

/// One `[[version]]` block in an index file.
pub struct IndexEntry {
    pub version: Ver,
    pub sha256: String,
    /// Toolchain req against the running `ourc` version — a package
    /// whose `requires` excludes the tool is invisible to selection.
    pub requires: Option<Req>,
    pub yanked: bool,
    /// The canonical relpath list — required for remote registries
    /// (you can't list a directory over GET); a directory registry
    /// ignores it and walks the tree instead.
    pub files: Vec<String>,
}

/// A registry root: a local directory or a remote base URL. Same
/// layout either way — `index/` + `src/` — only the transport
/// differs (PACKAGES.md §10).
pub enum Root {
    Dir(PathBuf),
    Remote(String),
}

/// The registry root: `$OUR_REGISTRY` (a directory or `http(s)://`
/// base), else `<project>/registry` when it exists. Registry deps
/// with no registry at all is a resolve-time error, not a silent miss.
pub fn registry_root(project_root: &Path) -> Option<Root> {
    if let Ok(r) = std::env::var("OUR_REGISTRY") {
        if r.starts_with("http://") || r.starts_with("https://") {
            return Some(Root::Remote(r.trim_end_matches('/').to_string()));
        }
        let p = PathBuf::from(r);
        if p.is_dir() {
            return Some(Root::Dir(p));
        }
    }
    let local = project_root.join("registry");
    if local.is_dir() {
        return Some(Root::Dir(local));
    }
    None
}

/// `GET` over `curl` — zero crate deps is a project invariant, and
/// `curl`/`curl.exe` ships on macOS, Linux, and Windows 10+. Transport
/// failures surface verbatim; integrity is the tree hash's job, not
/// TLS's (PACKAGES.md §10).
fn fetch(url: &str) -> Result<Vec<u8>, String> {
    let out = std::process::Command::new("curl")
        .args(["-fsSL", "--max-time", "30", url])
        .output()
        .map_err(|e| format!("fetch `{url}`: cannot spawn curl: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "fetch `{url}`: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    Ok(out.stdout)
}

/// The dep cache root: `$OUR_HOME/registry`, default
/// `~/.our/registry`.
pub fn cache_root() -> PathBuf {
    if let Ok(h) = std::env::var("OUR_HOME") {
        return PathBuf::from(h).join("registry");
    }
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".our").join("registry")
}

/// Parse `index/<name>.toml` — `[[version]]` blocks of
/// `key = value` lines using the manifest's TOML subset, plus the
/// `files = [...]` array remote registries need.
pub fn load_index(root: &Root, name: &str) -> Result<Vec<IndexEntry>, String> {
    let where_ = match root {
        Root::Dir(d) => d.join("index").join(format!("{name}.toml")).display().to_string(),
        Root::Remote(base) => format!("{base}/index/{name}.toml"),
    };
    let bytes = match root {
        Root::Dir(d) => std::fs::read(d.join("index").join(format!("{name}.toml")))
            .map_err(|_| format!("package `{name}` is not in the registry index"))?,
        Root::Remote(base) => fetch(&format!("{base}/index/{name}.toml"))
            .map_err(|e| format!("package `{name}`: {e}"))?,
    };
    let src = String::from_utf8(bytes)
        .map_err(|_| format!("{where_}: index is not UTF-8"))?;
    let mut out: Vec<IndexEntry> = Vec::new();
    let mut cur: Option<IndexEntry> = None;
    for (i, raw) in src.lines().enumerate() {
        let line = strip_comment(raw).trim();
        if line.is_empty() {
            continue;
        }
        if line == "[[version]]" {
            if let Some(e) = cur.take() {
                out.push(e);
            }
            cur = Some(IndexEntry {
                version: Ver(0, 0, 0),
                sha256: String::new(),
                requires: None,
                yanked: false,
                files: Vec::new(),
            });
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("{where_}:{}: expected `key = value`", i + 1));
        };
        let Some(e) = cur.as_mut() else {
            return Err(format!(
                "{where_}:{}: `{line}` outside a `[[version]]` block",
                i + 1
            ));
        };
        let v = v.trim();
        match k.trim() {
            "files" => {
                e.files = parse_str_array(v)
                    .map_err(|e| format!("{where_}:{}: {e}", i + 1))?;
            }
            "version" => {
                e.version = semver::parse_ver(
                    &parse_scalar(v).map_err(|e| format!("{where_}:{}: {e}", i + 1))?,
                )
                .map_err(|e| format!("{where_}:{}: {e}", i + 1))?;
            }
            "sha256" => {
                e.sha256 = parse_scalar(v).map_err(|e| format!("{where_}:{}: {e}", i + 1))?;
            }
            "requires" => {
                e.requires = Some(
                    semver::parse_req(
                        &parse_scalar(v).map_err(|e| format!("{where_}:{}: {e}", i + 1))?,
                    )
                    .map_err(|e| format!("{where_}:{}: {e}", i + 1))?,
                );
            }
            "yanked" => {
                e.yanked = parse_scalar(v).map_err(|e| format!("{where_}:{}: {e}", i + 1))? == "true";
            }
            _ => {} // forward-compat
        }
    }
    if let Some(e) = cur.take() {
        out.push(e);
    }
    Ok(out)
}

/// `["a", "b", …]` — the index's one array-valued key.
fn parse_str_array(v: &str) -> Result<Vec<String>, String> {
    let inner = v
        .strip_prefix('[')
        .and_then(|s| s.strip_suffix(']'))
        .ok_or_else(|| format!("expected `[\"a\", \"b\", …]`, found `{v}`"))?;
    let mut out = Vec::new();
    for part in inner.split(',') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        out.push(parse_scalar(part)?);
    }
    Ok(out)
}

/// The highest version satisfying `req`, visible to this toolchain,
/// and not yanked. `locked` re-admits a yanked version already in
/// the lockfile (a lock never silently revokes — PACKAGES.md §4).
pub fn select<'a>(
    entries: &'a [IndexEntry],
    name: &str,
    req: &Req,
    locked: Option<&Ver>,
) -> Result<&'a IndexEntry, String> {
    let tool = semver::parse_ver(env!("CARGO_PKG_VERSION")).unwrap_or(Ver(0, 0, 0));
    let mut best: Option<&IndexEntry> = None;
    let mut any = false;
    for e in entries {
        if !req.satisfies(&e.version) {
            continue;
        }
        any = true;
        if e.yanked && locked != Some(&e.version) {
            continue;
        }
        if let Some(r) = &e.requires {
            if !r.satisfies(&tool) {
                continue;
            }
        }
        if best.map_or(true, |b| e.version > b.version) {
            best = Some(e);
        }
    }
    best.ok_or_else(|| {
        if any {
            format!("`{name}`: every version satisfying `{r}` is yanked or needs a newer ourc", r = req.display())
        } else {
            format!("`{name}`: no version satisfies `{r}`", r = req.display())
        }
    })
}

/// SHA-256 over `relpath \0 bytes \0` for every file under `root`,
/// lexicographic order — `.git/` and `build/` excluded, `tests/`
/// included (shipped, hashed, never built). PACKAGES.md §6.
pub fn tree_sha256(root: &Path) -> Result<String, String> {
    let mut files = Vec::new();
    collect_tree(root, root, &mut files);
    files.sort();
    let mut buf = Vec::new();
    for f in files {
        buf.extend_from_slice(f.to_string_lossy().as_bytes());
        buf.push(0);
        let bytes =
            std::fs::read(root.join(&f)).map_err(|e| format!("{}: {e}", f.display()))?;
        buf.extend_from_slice(&bytes);
        buf.push(0);
    }
    Ok(sha256_hex(&buf))
}

fn collect_tree(dir: &Path, base: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            let n = e.file_name().to_string_lossy().to_string();
            if (n == "build" || n == ".git") && p.parent() == Some(base) {
                continue;
            }
            collect_tree(&p, base, out);
        } else if let Ok(rel) = p.strip_prefix(base) {
            out.push(rel.to_path_buf());
        }
    }
}

/// Ensure `<cache>/<name>/<version>` exists and matches `sha256`:
/// a directory registry copies `src/<name>/<ver>/`; a remote registry
/// fetches every `files[]` member over GET into the same scratch
/// tree. The canonical hash verifies the assembled bytes either way.
/// Returns the cached package root.
pub fn ensure_cached(
    root: &Root,
    name: &str,
    e: &IndexEntry,
) -> Result<PathBuf, String> {
    let cache = cache_root().join(name).join(e.version.to_string());
    if !cache.join("project.toml").is_file() {
        let tmp = cache_root().join(format!(".tmp-{name}-{}", e.version));
        let _ = std::fs::remove_dir_all(&tmp);
        match root {
            Root::Dir(reg) => {
                let src = reg.join("src").join(name).join(e.version.to_string());
                if !src.join("project.toml").is_file() {
                    return Err(format!(
                        "`{name}` {}: index has no `src/{}/{}` tree",
                        e.version, name, e.version
                    ));
                }
                copy_tree(&src, &tmp)
                    .map_err(|e2| format!("cache `{name}` {}: {e2}", e.version))?;
            }
            Root::Remote(base) => {
                if e.files.is_empty() {
                    return Err(format!(
                        "`{name}` {}: remote index entry has no `files` list",
                        e.version
                    ));
                }
                for f in &e.files {
                    let bytes = fetch(&format!(
                        "{base}/src/{name}/{}/{f}",
                        e.version
                    ))?;
                    let to = tmp.join(f);
                    if let Some(p) = to.parent() {
                        std::fs::create_dir_all(p).map_err(|e2| e2.to_string())?;
                    }
                    std::fs::write(&to, &bytes).map_err(|e2| e2.to_string())?;
                }
            }
        }
        if let Some(parent) = cache.parent() {
            std::fs::create_dir_all(parent).map_err(|e2| e2.to_string())?;
        }
        if cache.exists() {
            let _ = std::fs::remove_dir_all(&cache);
        }
        std::fs::rename(&tmp, &cache).map_err(|e2| e2.to_string())?;
    }
    let actual = tree_sha256(&cache)?;
    if actual != e.sha256 {
        return Err(format!(
            "`{name}` {}: cache hash mismatch — index says {}, tree is {}",
            e.version,
            &e.sha256[..12.min(e.sha256.len())],
            &actual[..12]
        ));
    }
    Ok(cache)
}

fn copy_tree(src: &Path, dst: &Path) -> Result<(), String> {
    std::fs::create_dir_all(dst).map_err(|e| e.to_string())?;
    for e in std::fs::read_dir(src).map_err(|e| e.to_string())?.flatten() {
        let p = e.path();
        let to = dst.join(e.file_name());
        if p.is_dir() {
            let n = e.file_name().to_string_lossy().to_string();
            if n == "build" || n == ".git" {
                continue;
            }
            copy_tree(&p, &to)?;
        } else {
            std::fs::copy(&p, &to).map_err(|e2| e2.to_string())?;
        }
    }
    Ok(())
}

/// Loose index/manifest scalar — shares the manifest's subset rules
/// (quoted string, integer, or bool; the string form wins).
fn parse_scalar(v: &str) -> Result<String, String> {
    if let Some(rest) = v.strip_prefix('"') {
        let mut out = String::new();
        for c in rest.chars() {
            match c {
                '"' => return Ok(out),
                '\\' => {}
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
