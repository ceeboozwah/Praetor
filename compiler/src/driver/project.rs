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

use crate::driver::registry;
use crate::driver::semver::{self, Req, Ver};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// One `[dependencies]` row (PACKAGES.md §2): a registry version
/// requirement, a local path, or a path with a version assertion.
pub enum DepSpec {
    /// `name = { path = "../lib" }` — or a bare string that fails
    /// `parse_req` (back-compat: `"../lib"` was the only v0 form).
    Path(PathBuf),
    /// `{ path = "../lib", version = "1.2" }` — the dep's manifest
    /// version must satisfy `req`.
    PathChecked { path: PathBuf, req: Req },
    /// `name = "1.2"` / `"^1.2"` / `"=1.2.3"` — registry dep.
    Registry(Req),
}

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
    /// `[dependencies]` — package name → declared spec, sorted by
    /// name for deterministic resolution. A dependency named `lib`
    /// becomes a module root: `import lib.util` resolves
    /// `<dep>/src/util.lang`.
    pub deps: Vec<(String, DepSpec)>,
    /// `[project] toolchain` — a req against the toolchain version
    /// (CHANNELS.md §3). `None` = run whatever was invoked.
    pub toolchain: Option<Req>,
}

/// Resolution flags — `--locked` (the lockfile must match the
/// resolved set exactly; drift is an error) and `--offline` (no
/// registry reads; resolve from `project.lock` + the dep cache).
#[derive(Default)]
pub struct ResolveOpts {
    pub locked: bool,
    pub offline: bool,
}

/// A resolved dependency: its canonical package root (a live dir for
/// path deps, the verified cache copy for registry deps) and the
/// `src/` directory that serves as its module root.
pub struct Dep {
    pub name: String,
    pub root: PathBuf,
    pub src: PathBuf,
    /// `"registry"` or `"path"` — the lockfile's `source`.
    pub source: &'static str,
    /// The chosen version — the dep manifest's own version for path
    /// deps, the index-selected one for registry deps.
    pub version: Ver,
    /// Canonical tree hash, computed at resolve time.
    pub sha256: String,
    /// Path deps record the manifest-relative path for the lockfile.
    pub rel_path: Option<PathBuf>,
}

/// `name → src/` pairs for the module loader — dep names behave like
/// `std`: the first `import` path segment selects the root.
pub fn module_roots(deps: &[Dep]) -> HashMap<String, PathBuf> {
    deps.iter()
        .map(|d| (d.name.clone(), d.src.clone()))
        .collect()
}

/// `module_roots` plus the project's own package name → `src/`, so
/// `import pkg.mod` resolves a project's own modules — which is how
/// `tests/` reaches the code under test (N44T).
pub fn roots_with_self(p: &Project, deps: &[Dep]) -> HashMap<String, PathBuf> {
    let mut roots = module_roots(deps);
    roots.insert(p.manifest.name.clone(), p.root.join("src"));
    roots
}

/// One `project.lock` `[dep.*]` block.
struct LockEntry {
    source: String,
    version: Option<Ver>,
    path: Option<String>,
    sha256: String,
}

/// Resolve a manifest's `[dependencies]` (and transitively theirs)
/// into a name-sorted package list. Registry deps select the highest
/// satisfying version from the index and land in the dep cache;
/// `opts.locked` diffs the result against `project.lock`,
/// `opts.offline` resolves from the lock + cache without touching
/// the registry.
pub fn resolve_deps(
    root: &Path,
    manifest: &Manifest,
    opts: &ResolveOpts,
) -> Result<Vec<Dep>, String> {
    let locks = read_lock(root)?;
    let mut out: Vec<Dep> = Vec::new();
    let mut by_name: HashMap<String, (PathBuf, Ver)> = HashMap::new();
    let mut stack: Vec<String> = Vec::new();
    resolve_from(root, root, manifest, &mut stack, &mut by_name, &mut out, &locks, opts)?;
    out.sort_by(|a, b| a.name.cmp(&b.name));
    if opts.locked {
        check_locked(root, &out, &locks)?;
    }
    Ok(out)
}

#[allow(clippy::too_many_arguments)]
fn resolve_from(
    top_root: &Path,
    root: &Path,
    manifest: &Manifest,
    stack: &mut Vec<String>,
    by_name: &mut HashMap<String, (PathBuf, Ver)>,
    out: &mut Vec<Dep>,
    locks: &HashMap<String, LockEntry>,
    opts: &ResolveOpts,
) -> Result<(), String> {
    // Path deps resolve before registry deps: a `path` override must
    // win over whatever the index would pick, independent of how the
    // names sort (PACKAGES.md §2).
    let ordered = manifest
        .deps
        .iter()
        .filter(|(_, s)| !matches!(s, DepSpec::Registry(_)))
        .chain(
            manifest
                .deps
                .iter()
                .filter(|(_, s)| matches!(s, DepSpec::Registry(_))),
        );
    for (name, spec) in ordered {
        if name == "std" {
            return Err(format!(
                "{}: `std` is a reserved package name",
                root.join("project.toml").display()
            ));
        }
        // Dedup BEFORE resolving: a name already picked (path override
        // or earlier registry selection) must never touch the index
        // again — the new spec is a constraint on the pick, nothing
        // more (PACKAGES.md §4 — no backtracking).
        if let Some((prev, prev_ver)) = by_name.get(name) {
            match spec {
                DepSpec::Registry(req) => {
                    if !req.satisfies(prev_ver) {
                        return Err(format!(
                            "package `{name}` resolved to {prev_ver} but another dep requires `{r}` — pin `x.y.z` in the top-level manifest to choose",
                            r = req.display()
                        ));
                    }
                }
                DepSpec::Path(rel) | DepSpec::PathChecked { path: rel, .. } => {
                    let want = std::fs::canonicalize(root.join(rel))
                        .unwrap_or_else(|_| root.join(rel));
                    if want != *prev {
                        return Err(format!(
                            "package `{name}` resolves to both `{}` and `{}`",
                            prev.display(),
                            want.display()
                        ));
                    }
                    if let DepSpec::PathChecked { req, .. } = spec {
                        if !req.satisfies(prev_ver) {
                            return Err(format!(
                                "package `{name}` resolved to {prev_ver} but `{p}` requires `{r}`",
                                p = rel.display(),
                                r = req.display()
                            ));
                        }
                    }
                }
            }
            continue; // already resolved — shared dep
        }
        let (pkg_root, source, picked, sha256, rel) = match spec {
            DepSpec::Registry(req) => registry_dep(top_root, name, req, locks, opts)?,
            DepSpec::Path(rel) | DepSpec::PathChecked { path: rel, .. } => {
                let pkg_root = std::fs::canonicalize(root.join(rel)).map_err(|_| {
                    format!(
                        "dependency `{name}`: no directory `{}` (from {})",
                        root.join(rel).display(),
                        root.join("project.toml").display()
                    )
                })?;
                (pkg_root, "path", Ver(0, 0, 0), String::new(), Some(rel.clone()))
            }
        };
        let self_root =
            std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
        if pkg_root == self_root {
            return Err(format!(
                "{}: package `{name}` cannot depend on itself — own modules are already visible as `{}.<mod>`",
                root.join("project.toml").display(),
                manifest.name
            ));
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
        let mf_ver = semver::parse_ver(&sub.version).unwrap_or(Ver(0, 0, 0));
        let dep_ver = if source == "registry" { picked } else { mf_ver };
        if let DepSpec::PathChecked { req, .. } = spec {
            if !req.satisfies(&dep_ver) {
                return Err(format!(
                    "dependency `{name}` at `{}` is version {dep_ver}, which does not satisfy `{r}`",
                    pkg_root.display(),
                    r = req.display()
                ));
            }
        }
        let sha256 = if sha256.is_empty() {
            registry::tree_sha256(&pkg_root)?
        } else {
            sha256
        };
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
        by_name.insert(name.clone(), (pkg_root.clone(), dep_ver));
        out.push(Dep {
            name: name.clone(),
            root: pkg_root.clone(),
            src,
            source,
            version: dep_ver,
            sha256,
            rel_path: rel,
        });
        resolve_from(top_root, &pkg_root, &sub, stack, by_name, out, locks, opts)?;
        stack.pop();
    }
    Ok(())
}

/// Pick + cache a registry dep. `--offline` resolves from the
/// lockfile + cache only — the index is never read.
fn registry_dep(
    top_root: &Path,
    name: &str,
    req: &Req,
    locks: &HashMap<String, LockEntry>,
    opts: &ResolveOpts,
) -> Result<(PathBuf, &'static str, Ver, String, Option<PathBuf>), String> {
    if opts.offline {
        let le = locks.get(name).ok_or_else(|| {
            format!("`{name}`: not in project.lock — cannot resolve --offline")
        })?;
        if le.source != "registry" {
            return Err(format!(
                "`{name}`: lockfile says `{}` but the manifest declares a version req",
                le.source
            ));
        }
        let ver = le
            .version
            .ok_or_else(|| format!("`{name}`: lockfile entry has no version"))?;
        if !req.satisfies(&ver) {
            return Err(format!(
                "`{name}`: locked at {ver} but the manifest requires `{r}`",
                r = req.display()
            ));
        }
        let cache = registry::cache_root().join(name).join(ver.to_string());
        if !cache.join("project.toml").is_file() {
            return Err(format!(
                "`{name}` {ver}: not in dep cache `{}` — cannot resolve --offline",
                cache.display()
            ));
        }
        let actual = registry::tree_sha256(&cache)?;
        if actual != le.sha256 {
            return Err(format!(
                "`{name}` {ver}: cache hash mismatch vs lockfile — tree is {}",
                &actual[..12]
            ));
        }
        return Ok((cache, "registry", ver, le.sha256.clone(), None));
    }
    let reg = registry::registry_root(top_root).ok_or_else(|| {
        format!(
            "`{name}`: no registry — set OUR_REGISTRY or create `{}`",
            top_root.join("registry").display()
        )
    })?;
    let idx = registry::load_index(&reg, name)?;
    let locked = locks.get(name).and_then(|le| le.version);
    let e = registry::select(&idx, name, req, locked.as_ref())?;
    let cache = registry::ensure_cached(&reg, name, e)?;
    Ok((cache, "registry", e.version, e.sha256.clone(), None))
}

/// `--locked`: the resolved set must equal the lockfile — same names,
/// sources, versions, and hashes. Drift in any of them is an error.
fn check_locked(
    root: &Path,
    deps: &[Dep],
    locks: &HashMap<String, LockEntry>,
) -> Result<(), String> {
    if locks.is_empty() && !deps.is_empty() {
        return Err(format!(
            "{}: --locked but no usable project.lock exists",
            root.join("project.lock").display()
        ));
    }
    for d in deps {
        let Some(le) = locks.get(&d.name) else {
            return Err(format!(
                "--locked: `{}` resolved but is absent from project.lock",
                d.name
            ));
        };
        if le.source != d.source || le.version != Some(d.version) || le.sha256 != d.sha256 {
            return Err(format!(
                "--locked: `{}` drifted — lock says {}@{} ({}…)",
                d.name,
                le.source,
                le.version.map(|v| v.to_string()).unwrap_or_default(),
                &le.sha256[..12.min(le.sha256.len())]
            ));
        }
    }
    for name in locks.keys() {
        if !deps.iter().any(|d| &d.name == name) {
            return Err(format!(
                "--locked: `{name}` is locked but no longer a dependency"
            ));
        }
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

/// `project.lock` v2 (PACKAGES.md §5) — the resolved dependency set
/// on record. Every dep carries its canonical SHA-256 tree hash;
/// registry deps record the selected version, path deps their
/// manifest-relative path. Written on every resolution (except
/// `--locked`, which verifies instead).
/// The lockfile format this toolchain reads and writes
/// (VERSIONING.md §4). Absent `format` = 2 — the field did not
/// exist when 2 was introduced, so all pre-existing files are 2.
const LOCK_FORMAT: u64 = 2;

pub fn write_lock(root: &Path, deps: &[Dep]) -> Result<(), String> {
    let mut s = String::from(
        "# project.lock — generated by `our`, do not edit\n[lock]\n",
    );
    s.push_str(&format!("format = {LOCK_FORMAT}\n"));
    s.push_str(&format!("tool = \"ourc {}\"\n", env!("CARGO_PKG_VERSION")));
    for d in deps {
        s.push_str(&format!("\n[dep.{}]\nsource = \"{}\"\n", d.name, d.source));
        s.push_str(&format!("version = \"{}\"\n", d.version));
        if let Some(rel) = &d.rel_path {
            s.push_str(&format!("path = \"{}\"\n", rel.display()));
        }
        s.push_str(&format!("sha256 = \"{}\"\n", d.sha256));
    }
    std::fs::write(root.join("project.lock"), s).map_err(|e| e.to_string())
}

/// Parse `project.lock` back into `name → LockEntry`. A missing or
/// malformed file, or an *older* format, yields an empty map — a
/// stale lock is an absence, not an error. A `format` newer than
/// `LOCK_FORMAT` is a hard error (VERSIONING.md §4): a newer
/// lockfile is never silently misread.
fn read_lock(root: &Path) -> Result<HashMap<String, LockEntry>, String> {
    let src = match std::fs::read_to_string(root.join("project.lock")) {
        Ok(s) => s,
        Err(_) => return Ok(HashMap::new()),
    };
    let mut out: HashMap<String, LockEntry> = HashMap::new();
    let mut cur: Option<(String, LockEntry)> = None;
    for raw in src.lines() {
        let line = strip_comment(raw).trim();
        if line.is_empty() || line == "[lock]" {
            continue;
        }
        if let Some(name) = line
            .strip_prefix("[dep.")
            .and_then(|s| s.strip_suffix(']'))
        {
            if let Some((n, e)) = cur.take() {
                out.insert(n, e);
            }
            cur = Some((
                name.to_string(),
                LockEntry {
                    source: String::new(),
                    version: None,
                    path: None,
                    sha256: String::new(),
                },
            ));
            continue;
        }
        let Some((k, v)) = line.split_once('=') else {
            return Ok(HashMap::new());
        };
        let Some((_, e)) = cur.as_mut() else {
            // [lock] header keys — only `format` gates.
            if k.trim() == "format" {
                let n: u64 = v.trim().trim_matches('"').parse().unwrap_or(LOCK_FORMAT);
                if n > LOCK_FORMAT {
                    return Err(format!(
                        "{}: lockfile format {n} is newer than this toolchain \
                         understands ({LOCK_FORMAT}) — upgrade `our`",
                        root.join("project.lock").display()
                    ));
                }
                if n < LOCK_FORMAT {
                    return Ok(HashMap::new()); // older format → re-resolve
                }
            }
            continue;
        };
        let Some(v) = parse_value(v.trim()).ok() else {
            return Ok(HashMap::new());
        };
        match k.trim() {
            "source" => e.source = v,
            "version" => e.version = semver::parse_ver(&v).ok(),
            "path" => e.path = Some(v),
            "sha256" => e.sha256 = v,
            "hash" => {} // pre-v2 lockfiles — stale by source drift
            _ => {}
        }
    }
    if let Some((n, e)) = cur.take() {
        out.insert(n, e);
    }
    // A lock missing source/sha256 is unusable — treat as absent.
    if out.values().any(|e| e.source.is_empty()) {
        return Ok(HashMap::new());
    }
    Ok(out)
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
/// `ourc --dump-project` — the canonical project-resolution dump and
/// the N81 self-host oracle: manifest fields, declared dependency
/// specs in name order, the fully resolved set (or the resolve
/// error), and the lockfile's recorded state. Deterministic across
/// runs on the same tree — every list is name-sorted.
///
/// Resolution is read-only for the project tree: `project.lock` is
/// never written here (that's the caller commands' job), though a
/// registry dep may still populate the dep cache.
pub fn dump_project(p: &Project, opts: &ResolveOpts) -> Result<String, String> {
    let m = &p.manifest;
    let mut s = String::new();
    s.push_str(&format!("project.name = \"{}\"\n", m.name));
    s.push_str(&format!("project.version = \"{}\"\n", m.version));
    s.push_str(&format!("project.entry = \"{}\"\n", m.entry.display()));
    s.push_str(&format!("project.profile = \"{}\"\n", m.profile));
    match &m.toolchain {
        Some(r) => s.push_str(&format!("project.toolchain = \"{}\"\n", r.display())),
        None => s.push_str("project.toolchain = none\n"),
    }
    for (n, sp) in &m.deps {
        match sp {
            DepSpec::Path(rel) => {
                s.push_str(&format!("decl {n} = path \"{}\"\n", rel.display()))
            }
            DepSpec::PathChecked { path: rel, req } => s.push_str(&format!(
                "decl {n} = path \"{}\" req \"{}\"\n",
                rel.display(),
                req.display()
            )),
            DepSpec::Registry(req) => {
                s.push_str(&format!("decl {n} = req \"{}\"\n", req.display()))
            }
        }
    }
    match resolve_deps(&p.root, m, opts) {
        Ok(deps) => {
            for d in &deps {
                s.push_str(&format!(
                    "dep {name} source={src} version={v} sha256={sha} root={root}\n",
                    name = d.name,
                    src = d.source,
                    v = d.version,
                    sha = d.sha256,
                    root = d.root.display(),
                ));
            }
        }
        Err(e) => s.push_str(&format!("resolve.error = \"{e}\"\n")),
    }
    match read_lock(&p.root) {
        Ok(locks) => {
            if locks.is_empty() {
                s.push_str("lock = none\n");
            } else {
                let mut names: Vec<&String> = locks.keys().collect();
                names.sort();
                for n in names {
                    let e = &locks[n];
                    s.push_str(&format!(
                        "lock {n} source={src} version={v} path={p} sha256={sha}\n",
                        src = e.source,
                        v = e
                            .version
                            .map(|v| v.to_string())
                            .unwrap_or_else(|| "none".to_string()),
                        p = e.path.as_deref().unwrap_or("none"),
                        sha = e.sha256,
                    ));
                }
            }
        }
        Err(e) => s.push_str(&format!("lock.error = \"{e}\"\n")),
    }
    Ok(s)
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
    let mut deps: Vec<(String, DepSpec)> = Vec::new();
    let mut toolchain = None;
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
        if section == "dependencies" {
            let dep = k.trim();
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
            let spec = dep_spec(v.trim()).map_err(|e| {
                format!("{}:{}: {e}", path.display(), i + 1)
            })?;
            deps.push((dep.to_string(), spec));
            continue;
        }
        let val = parse_value(v.trim()).map_err(|e| {
            format!("{}:{}: {e}", path.display(), i + 1)
        })?;
        match key.as_str() {
            "project.name" => name = Some(val),
            "project.version" => version = val,
            "project.toolchain" => {
                toolchain = Some(semver::parse_req(&val).map_err(|e| {
                    format!("{}:{}: `toolchain` — {e}", path.display(), i + 1)
                })?);
            }
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
            _ => {} // unknown keys — forward-compat, not errors yet.
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
        toolchain,
    })
}

/// One `[dependencies]` value (PACKAGES.md §2): `{ path = "..",
/// version = "1.2" }` inline tables, or a bare string — a version
/// requirement means a registry dep; anything else is a path
/// (back-compat: `"../lib"` was the only v0 form, and weird literal
/// paths can always write `{ path = "..." }` explicitly).
fn dep_spec(v: &str) -> Result<DepSpec, String> {
    if let Some(inner) = v.strip_prefix('{').and_then(|s| s.strip_suffix('}')) {
        let mut path: Option<PathBuf> = None;
        let mut req: Option<Req> = None;
        for part in inner.split(',') {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let Some((k, val)) = part.split_once('=') else {
                return Err(format!("malformed dependency entry `{part}`"));
            };
            let s = parse_value(val.trim())?;
            match k.trim() {
                "path" => path = Some(PathBuf::from(s)),
                "version" => req = Some(semver::parse_req(&s)?),
                other => {
                    return Err(format!(
                        "unknown dependency key `{other}` — expected `path` or `version`"
                    ))
                }
            }
        }
        let Some(path) = path else {
            return Err("dependency entry `{ .. }` needs `path = \"...\"`".to_string());
        };
        return Ok(match req {
            Some(req) => DepSpec::PathChecked { path, req },
            None => DepSpec::Path(path),
        });
    }
    let s = parse_value(v)?;
    match semver::parse_req(&s) {
        Ok(req) => Ok(DepSpec::Registry(req)),
        Err(_) => Ok(DepSpec::Path(PathBuf::from(s))),
    }
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
            "[project]\nname = \"{name}\"\nversion = \"0.1.0\"\n# toolchain = \"0.1\"       # pin the toolchain this project builds\n#                          # with — `our` re-execs a versioned\n#                          # sibling or errors (CHANNELS.md)\n\n[build]\nentry = \"src/main.lang\"\n# profile = \"release\"     # or pass --release to build/run\n\n# Own modules are always importable as `{name}.<mod>` —\n# `import {name}.util` works in src/ and tests/ alike.\n#\n# [dependencies]\n# lib = \"../lib\"          # a path package — `import lib.util`\n#                          # resolves <dep>/src/util.lang\n# net = \"1.2\"             # a registry package (OUR_REGISTRY)\n# pinned = {{ path = \"../lib\", version = \"1.2\" }}\n"
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
