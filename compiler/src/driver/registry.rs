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
}

/// The registry root: `$OUR_REGISTRY`, else `<project>/registry`
/// when it exists. Registry deps with no registry at all is a
/// resolve-time error, not a silent miss.
pub fn registry_root(project_root: &Path) -> Option<PathBuf> {
    if let Ok(r) = std::env::var("OUR_REGISTRY") {
        let p = PathBuf::from(r);
        if p.is_dir() {
            return Some(p);
        }
    }
    let local = project_root.join("registry");
    if local.is_dir() {
        return Some(local);
    }
    None
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
/// `key = value` lines using the manifest's TOML subset.
pub fn load_index(reg: &Path, name: &str) -> Result<Vec<IndexEntry>, String> {
    let path = reg.join("index").join(format!("{name}.toml"));
    let src = std::fs::read_to_string(&path)
        .map_err(|_| format!("package `{name}` is not in the registry index"))?;
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
            });
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Err(format!("{}:{}: expected `key = value`", path.display(), i + 1));
        };
        let Some(e) = cur.as_mut() else {
            return Err(format!(
                "{}:{}: `{line}` outside a `[[version]]` block",
                path.display(),
                i + 1
            ));
        };
        let v = v.trim();
        let s = parse_scalar(v)
            .map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
        match k.trim() {
            "version" => {
                e.version = semver::parse_ver(&s)
                    .map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?;
            }
            "sha256" => e.sha256 = s,
            "requires" => {
                e.requires = Some(
                    semver::parse_req(&s)
                        .map_err(|e| format!("{}:{}: {e}", path.display(), i + 1))?,
                );
            }
            "yanked" => e.yanked = s == "true",
            _ => {} // forward-compat
        }
    }
    if let Some(e) = cur.take() {
        out.push(e);
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
/// copy from `registry/src` on miss, verify the tree hash either
/// way. Returns the cached package root.
pub fn ensure_cached(
    reg: &Path,
    name: &str,
    e: &IndexEntry,
) -> Result<PathBuf, String> {
    let cache = cache_root().join(name).join(e.version.to_string());
    if !cache.join("project.toml").is_file() {
        let src = reg.join("src").join(name).join(e.version.to_string());
        if !src.join("project.toml").is_file() {
            return Err(format!(
                "`{name}` {}: index has no `src/{}/{}` tree",
                e.version, name, e.version
            ));
        }
        let tmp = cache_root().join(format!(".tmp-{name}-{}", e.version));
        let _ = std::fs::remove_dir_all(&tmp);
        copy_tree(&src, &tmp)
            .map_err(|e2| format!("cache `{name}` {}: {e2}", e.version))?;
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
