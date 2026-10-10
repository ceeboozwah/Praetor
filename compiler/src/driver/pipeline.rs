//! The pipeline:
//!   .lang → tokens → AST → typecheck → our IR → LLVM IR → .o → exe
//! Every stage's product is inspectable — that is the point of the
//! project, so nothing here hides a representation.

use crate::index;
use crate::backend_llvm::emit;
use crate::frontend::{ast, module, parser};
use crate::ir::{gen, ir::Module};
use crate::driver::render;
use crate::semantics::check::{self, ModuleInterface};
use crate::semantics::resolve;
use crate::{Diag, Span};
use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

/// Everything the compiler produced, in inspectable form.
pub struct Artifacts {
    pub ast: String,
    pub ir: String,
    pub llvm: String,
    pub module: Module,
}

/// The front half for a single source string — no imports can resolve
/// without a file on disk.
pub fn compile(src: &str) -> Result<Artifacts, Vec<Diag>> {
    render::register_source("<source>", src);
    render::register_root("<source>");
    let prog = parser::parse(src)?;
    if let Some(imp) = prog.imports.first() {
        return Err(vec![Diag::new(
            imp.span,
            "`import` requires compiling from a file path".to_string(),
        )]);
    }
    check::check(&prog)?;
    let binds = fold_arm_binds();
    let strings = RefCell::new(Vec::new());
    let module = gen::gen(&prog, &ModuleInterface::default(), &strings, &binds, "<source>");
    let module = Module {
        strings: strings.into_inner(),
        ..module
    };
    let llvm = emit::emit(&module, target_triple(), false);
    Ok(Artifacts {
        ast: ast::show_ast(&prog),
        ir: module.to_string(),
        llvm,
        module,
    })
}

/// What the front half produces — everything up to and including the
/// semantic check. `our check` stops here; `compile_path` continues
/// into codegen.
struct Frontend {
    prog: ast::Program,
    mods: Vec<LoadedModule>,
    root_deps: Vec<String>,
    ifaces: HashMap<String, ModuleInterface>,
    root_bind_modes: std::collections::HashMap<
        Span,
        std::collections::HashMap<String, check::ArmBind>,
    >,
}

/// Fold `take_arm_binds`' tuples into the map codegen consumes.
fn fold_arm_binds() -> std::collections::HashMap<
    Span,
    std::collections::HashMap<String, check::ArmBind>,
> {
    let mut out = std::collections::HashMap::new();
    for (sp, v, m) in check::take_arm_binds() {
        out.entry(sp).or_insert_with(std::collections::HashMap::new).insert(v, m);
    }
    out
}

/// Parse → load imports → monomorphize → check. Shared by `check_path`
/// and `compile_path` so the two can never drift (N21A).
fn frontend_path(
    src_path: &Path,
    roots: &HashMap<String, PathBuf>,
) -> Result<Frontend, Vec<Diag>> {
    let src = std::fs::read_to_string(src_path).map_err(|e| {
        vec![Diag::new(
            Span { line: 1, col: 1 },
            format!("{}: {e}", src_path.display()),
        )]
    })?;
    let mut occs = Vec::new();
    frontend_source(src_path, &src, &mut occs, roots)
}

/// The frontend with the root text provided by the caller — the LSP
/// server (N36) checks editor buffers that may never hit disk.
/// Imports still resolve from the filesystem relative to `src_path`.
/// `roots` maps package names to module roots (N38 dependencies).
fn frontend_source(
    src_path: &Path,
    src: &str,
    occs: &mut Vec<index::Occ>,
    roots: &HashMap<String, PathBuf>,
) -> Result<Frontend, Vec<Diag>> {
    index::reset(); // drop state left by a previous failed check
    let (mut prog, root_deps, mut loader) = load_graph(src_path, src, occs, roots)?;

    // N19 — monomorphize BEFORE checking: every `App`/`targs` site in
    // any module instantiates its template (qualified names make the
    // registry global), clones land in the requester's program, and the
    // whole graph is normalized to canonical `m.f<a,b>` keys.
    {
        let mut prog_refs: Vec<(String, &mut ast::Program)> = loader
            .mods
            .iter_mut()
            .map(|m| (m.file.display().to_string(), &mut m.prog))
            .collect();
        prog_refs.push((src_path.display().to_string(), &mut prog));
        let ds = crate::frontend::mono::monomorphize(&mut prog_refs);
        if !ds.is_empty() {
            occs.extend(index::drain());
            return Err(ds);
        }
    }

    // N24A — rvalue-place lowering: hoist non-place bases and non-place
    // call args into `let $rvN = ..` temps, so both checker and codegen
    // see only named-root places. Post-mono so MethodCall/AssocFn have
    // already desugared. `write`-param args keep their place contract,
    // so callee auths are collected across every module first.
    let mut sig_progs: Vec<&ast::Program> =
        loader.mods.iter().map(|m| &m.prog).collect();
    sig_progs.push(&prog);
    let fn_auths = crate::frontend::lower::collect_fn_auths(&sig_progs);
    for m in loader.mods.iter_mut() {
        crate::frontend::lower::lower_program(&mut m.prog, &fn_auths);
    }
    crate::frontend::lower::lower_program(&mut prog, &fn_auths);

    // Check each module against only its direct imports' interfaces —
    // transitivity does NOT leak: m can name `d.x` but not `d`'s own
    // dependencies.
    let mut ifaces: HashMap<String, ModuleInterface> = HashMap::new();
    for m in loader.mods.iter_mut() {
        let imports = join_ifaces(&m.deps, &ifaces);
        let mut iface = ModuleInterface::default();
        let res = check::check_module(&m.prog, &imports, &mut iface);
        // N36B — stamp the module's file onto its occurrence batch and
        // its exported def sites, even when the check failed (the
        // index is still valid for the parts that resolved).
        let fname = m.file.display().to_string();
        for o in index::drain() {
            let mut o = o;
            o.file = Some(fname.clone());
            occs.push(o);
        }
        for v in iface.locs.values_mut() {
            v.file = Some(fname.clone());
        }
        ifaces.insert(m.name.clone(), iface);
        res.map_err(|ds| {
            ds.into_iter()
                .map(|mut d| {
                    d.file = Some(fname.clone());
                    d
                })
                .collect::<Vec<_>>()
        })?;
        m.bind_modes = fold_arm_binds();
    }
    let root_imports = join_ifaces(&root_deps, &ifaces);
    let res = check::check_module(&prog, &root_imports, &mut ModuleInterface::default());
    occs.extend(index::drain());
    res?;
    Ok(Frontend {
        prog,
        mods: loader.mods,
        root_deps,
        ifaces,
        root_bind_modes: fold_arm_binds(),
    })
}

/// Parse the root and load its whole import graph — the loader phase
/// shared by `frontend_source` (which continues into mono/check) and
/// `--dump-modules` (which stops here), so the two can never drift.
/// Returns the unqualified root program, its direct dep qualifiers,
/// and the populated loader (postorder `mods`).
fn load_graph(
    src_path: &Path,
    src: &str,
    occs: &mut Vec<index::Occ>,
    roots: &HashMap<String, PathBuf>,
) -> Result<(ast::Program, Vec<String>, Loader), Vec<Diag>> {
    let src = src.to_string();
    render::register_source(&src_path.display().to_string(), &src);
    render::register_root(&src_path.display().to_string());
    let prog = parser::parse(&src)?;
    let root_dir = src_path
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Load the import graph: postorder DFS — every module lands in
    // `mods` after all of its own dependencies.
    let mut loader = Loader {
        roots: roots.clone(),
        ..Loader::default()
    };
    let mut root_deps = Vec::new();
    for imp in &prog.imports {
        let f = loader.resolve(imp, &root_dir)?;
        root_deps.push(loader.visit(&f, imp.span, imp)?);
        // Straight into the root batch — `index::record` would be
        // drained by the imported module's check and stamped with ITS
        // file instead of the root's.
        occs.push(index::Occ {
            span: Span {
                line: imp.span.line,
                col: imp.span.col + 7, // past `import `
            },
            name: imp.path.iter().map(String::as_str).collect::<Vec<_>>().join("."),
            kind: "module",
            hover: format!("module `{}`", imp.path.iter().map(String::as_str).collect::<Vec<_>>().join(".")),
            def: Some((Span { line: 1, col: 1 }, Some(f.display().to_string()))),
            file: None,
        });
    }
    Ok((prog, root_deps, loader))
}

/// N70 — the canonical module-graph surface for the self-hosted
/// loader differential: `(module-graph …)` on success, `!` records
/// on failure. Both sides run the identical resolution law: postorder
/// DFS, canonical-path dedup, qualifier collision checks.
///
/// Paths normalize for machine-independence: under the entry dir →
/// relative, under the stdlib root → `<std>/…`, under a package root
/// → `<pkg:name>/…`, else absolute. Error `file` slots normalize the
/// same way; `-` marks root-attributed diagnostics.
pub fn dump_modules(src_path: &Path) -> String {
    let mut out = String::new();
    let run = || -> Result<(ast::Program, Vec<String>, Loader), Vec<Diag>> {
        let src = std::fs::read_to_string(src_path).map_err(|e| {
            vec![Diag::new(
                Span { line: 1, col: 1 },
                format!("{}: {e}", src_path.display()),
            )]
        })?;
        let mut occs = Vec::new();
        load_graph(src_path, &src, &mut occs, &HashMap::new())
    };
    let base = src_path
        .parent()
        .and_then(|d| d.canonicalize().ok())
        .unwrap_or_else(|| PathBuf::from("."));
    let stdlib = stdlib_dir().canonicalize().unwrap_or_else(|_| stdlib_dir());
    let norm = |p: &str| -> String { norm_path(Path::new(p), &base, &stdlib, &HashMap::new()) };
    match run() {
        Ok((_prog, root_deps, loader)) => {
            out.push_str("(module-graph\n");
            out.push_str(&format!("  (root \"{}\"\n", src_path.display()));
            for d in &root_deps {
                out.push_str(&format!("    (dep \"{d}\")\n"));
            }
            out.push_str("  )\n");
            for m in &loader.mods {
                out.push_str(&format!(
                    "  (mod \"{}\" \"{}\"\n",
                    m.name,
                    norm(&m.file.display().to_string())
                ));
                for d in &m.deps {
                    out.push_str(&format!("    (dep \"{d}\")\n"));
                }
                out.push_str("  )\n");
            }
            out.push_str(")\n");
        }
        Err(ds) => {
            for d in &ds {
                let f = d.file.as_deref().map(norm).unwrap_or_else(|| "-".into());
                out.push_str(&format!("! {f} {} {} {}\n", d.span.line, d.span.col, d.msg));
            }
        }
    }
    out
}

/// `<std>`/`<pkg>`/entry-relative path spelling for the canonical
/// surfaces — the resolved path's *origin* is the semantic fact,
/// the machine-specific prefix is not.
fn norm_path(
    p: &Path,
    entry_dir: &Path,
    stdlib: &Path,
    roots: &HashMap<String, PathBuf>,
) -> String {
    if let Ok(rel) = p.strip_prefix(entry_dir) {
        return rel.display().to_string();
    }
    if let Ok(rel) = p.strip_prefix(stdlib) {
        return format!("<std>/{}", rel.display());
    }
    let mut pkgs: Vec<_> = roots.iter().collect();
    pkgs.sort_by(|a, b| a.0.cmp(b.0));
    for (name, root) in pkgs {
        let canon_root = root.canonicalize().unwrap_or_else(|_| root.clone());
        if let Ok(rel) = p.strip_prefix(&canon_root) {
            return format!("<pkg:{name}>/{}", rel.display());
        }
    }
    p.display().to_string()
}

/// N71 — the canonical symbol/resolution surface for the self-hosted
/// resolver differential: `(module-symbols …)` on success, `!`
/// records on a load failure (same contract as `--dump-modules`).
/// Decl tables, visibility, binds/uses, and resolution diagnostics —
/// no authority facts, no clone materialization.
pub fn dump_symbols(src_path: &Path) -> String {
    let src = match std::fs::read_to_string(src_path) {
        Ok(s) => s,
        Err(e) => return format!("! - 1 1 {}: {e}\n", src_path.display()),
    };
    let mut occs = Vec::new();
    let (prog, root_deps, loader) = match load_graph(src_path, &src, &mut occs, &Default::default()) {
        Ok(t) => t,
        Err(ds) => {
            let entry_dir = src_path
                .parent()
                .map(|d| d.to_path_buf())
                .unwrap_or_else(|| PathBuf::from("."));
            let stdlib = stdlib_dir();
            let mut out = String::new();
            for d in ds {
                let f = d
                    .file
                    .as_ref()
                    .map(|f| {
                        norm_path(
                            Path::new(f),
                            &entry_dir,
                            &stdlib,
                            &HashMap::new(),
                        )
                    })
                    .unwrap_or_else(|| "-".to_string());
                out.push_str(&format!("! {f} {} {} {}\n", d.span.line, d.span.col, d.msg));
            }
            return out;
        }
    };
    let entry_dir = src_path
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));
    let stdlib = stdlib_dir();
    let mut res = resolve::Resolver::new();
    let owned: Vec<(String, &ast::Program)> = {
        let mut v: Vec<(String, &ast::Program)> = Vec::new();
        for m in loader.mods.iter() {
            v.push((m.file.display().to_string(), &m.prog));
        }
        v.push((src_path.display().to_string(), &prog));
        v
    };
    res.register(&owned.iter().map(|(f, p)| (f.clone(), *p)).collect::<Vec<_>>());
    // Each module's direct-dep programs, resolved by qualifier.
    let prog_of = |name: &str| -> Option<&ast::Program> {
        loader
            .mods
            .iter()
            .position(|m| m.name == name)
            .map(|i| owned[i].1)
    };
    for (i, m) in loader.mods.iter().enumerate() {
        let dep_progs: Vec<&ast::Program> =
            m.deps.iter().filter_map(|d| prog_of(d)).collect();
        let norm = norm_path(&m.file, &entry_dir, &stdlib, &HashMap::new());
        res.module("mod", &m.name, &norm, &m.deps, &m.prog, &dep_progs, i);
    }
    let root_dep_progs: Vec<&ast::Program> = root_deps
        .iter()
        .filter_map(|d| prog_of(d))
        .collect();
    res.module(
        "root",
        "",
        &src_path.display().to_string(),
        &root_deps,
        &prog,
        &root_dep_progs,
        loader.mods.len(),
    );
    res.finish()
}

/// The LSP server's per-change entry point: check an editor buffer
/// (`src`) as if it were the file at `src_path`. Diagnostics on the
/// root keep `file: None` — the server maps them to the doc's URI.
pub fn check_source(
    src_path: &Path,
    src: &str,
    occs: &mut Vec<index::Occ>,
) -> Result<(), Vec<Diag>> {
    check_source_roots(src_path, src, occs, &HashMap::new())
}

/// `check_source` with declared package roots (N38) — the LSP server
/// resolves the enclosing project's dependencies once per session.
pub fn check_source_roots(
    src_path: &Path,
    src: &str,
    occs: &mut Vec<index::Occ>,
    roots: &HashMap<String, PathBuf>,
) -> Result<(), Vec<Diag>> {
    frontend_source(src_path, src, occs, roots).map(|_| ())
}

/// `our check` — the semantic pipeline only (N21A): parse, resolve
/// imports, monomorphize, check every module. No IR, no LLVM, no link.
pub fn check_path(src_path: &Path) -> Result<(), Vec<Diag>> {
    check_path_roots(src_path, &HashMap::new())
}

pub fn check_path_roots(
    src_path: &Path,
    roots: &HashMap<String, PathBuf>,
) -> Result<(), Vec<Diag>> {
    frontend_path(src_path, roots).map(|_| ())
}

/// The full path-driven compile (N17): resolve `import`s from disk,
/// qualify each module's symbols, check modules dependency-first so
/// importers replay callee summaries — never callee bodies — and emit
/// every module's code into one LLVM module.
pub fn compile_path(src_path: &Path) -> Result<Artifacts, Vec<Diag>> {
    compile_path_roots(src_path, &HashMap::new(), false)
}

pub fn compile_path_roots(
    src_path: &Path,
    roots: &HashMap<String, PathBuf>,
    debug: bool,
) -> Result<Artifacts, Vec<Diag>> {
    let fe = frontend_path(src_path, roots)?;
    let Frontend {
        prog,
        mods,
        root_deps,
        ifaces,
        root_bind_modes,
    } = fe;

    // Codegen: one IR module for everything — qualified names keep the
    // namespaces separate at the symbol level. The string table is
    // shared so StrAddr indices stay consistent across modules.
    let strings = RefCell::new(Vec::new());
    let mut module = Module {
        structs: Vec::new(),
        fns: Vec::new(),
        externs: Vec::new(),
        strings: Vec::new(),
    };
    let mut extern_sigs: HashMap<String, ()> = HashMap::new();
    let root_imports = join_ifaces(&root_deps, &ifaces);
    for m in &mods {
        let imports = join_ifaces(&m.deps, &ifaces);
        merge_module(
            &mut module,
            gen::gen(
                &m.prog,
                &imports,
                &strings,
                &m.bind_modes,
                &m.file.display().to_string(),
            ),
            &mut extern_sigs,
        );
    }
    let root_file = std::fs::canonicalize(src_path)
        .unwrap_or_else(|_| src_path.to_path_buf())
        .display()
        .to_string();
    merge_module(
        &mut module,
        gen::gen(&prog, &root_imports, &strings, &root_bind_modes, &root_file),
        &mut extern_sigs,
    );
    module.strings = strings.into_inner();
    let llvm = emit::emit(&module, target_triple(), debug);
    Ok(Artifacts {
        ast: ast::show_ast(&prog),
        ir: module.to_string(),
        llvm,
        module,
    })
}

/// Union of the direct dependencies' interfaces — what one module's
/// imports contribute to its check/gen context.
fn join_ifaces(deps: &[String], ifaces: &HashMap<String, ModuleInterface>) -> ModuleInterface {
    let mut out = ModuleInterface::default();
    for d in deps {
        if let Some(i) = ifaces.get(d) {
            out.fns.extend(i.fns.iter().map(|(k, v)| (k.clone(), v.clone())));
            out.structs
                .extend(i.structs.iter().map(|(k, v)| (k.clone(), v.clone())));
            out.sums.extend(i.sums.iter().map(|(k, v)| (k.clone(), v.clone())));
            out.private.extend(i.private.iter().cloned());
            out.enums
                .extend(i.enums.iter().map(|(k, v)| (k.clone(), v.clone())));
            out.locs
                .extend(i.locs.iter().map(|(k, v)| (k.clone(), v.clone())));
        }
    }
    out
}

/// Fold one module's IR products into the shared module. Externs
/// dedupe by name — two modules declaring `memcpy` name the same C
/// symbol. Instantiated generics dedupe likewise: two requesters of
/// `vec.Vec<i32>` produce byte-identical clones (N19), so struct defs
/// and fn bodies merge by canonical name.
fn merge_module(into: &mut Module, m: Module, seen_ext: &mut HashMap<String, ()>) {
    for st in m.structs {
        if !into.structs.iter().any(|s| s.name == st.name) {
            into.structs.push(st);
        }
    }
    for f in m.fns {
        if !into.fns.iter().any(|g| g.name == f.name) {
            into.fns.push(f);
        }
    }
    for e in m.externs {
        if seen_ext.insert(e.name.clone(), ()).is_none() {
            into.externs.push(e);
        }
    }
    // `strings` merged by the caller — the shared RefCell means every
    // module's StrAddr indices already index one global table.
}

/// The stdlib root — `import std.vec_i32` resolves here. N61:
/// embedded in the binary and materialized under `$OUR_HOME/lib`
/// (`OUR_STDLIB` overrides) so an installed tool needs no checkout.
pub fn stdlib_dir() -> PathBuf {
    super::resources::stdlib_dir()
}

/// A module parsed and name-qualified, sitting after its deps in the
/// load order.
struct LoadedModule {
    /// The qualifier — the import path's leaf (`import lib.math` → `math`).
    name: String,
    /// Where it was found — for diagnostics.
    file: PathBuf,
    /// Direct dependencies' qualifiers.
    deps: Vec<String>,
    /// The parsed program with every local symbol already renamed to
    /// `name.*` — checking sees qualified names only.
    prog: ast::Program,
    /// `(match-span, variant) -> bind mode` the checker recorded —
    /// codegen reads it to bind arm payloads by value or by `&`/`&mut`.
    bind_modes: std::collections::HashMap<
        Span,
        std::collections::HashMap<String, check::ArmBind>,
    >,
}

/// Depth-first module loader (N17). The graph is a DAG by construction:
/// a back-edge in `visiting` is a cyclic import, rejected. `done` maps
/// canonical paths to qualifiers so shared deps load once; `names`
/// keeps two different files from claiming the same qualifier.
/// `roots` (N38) maps package names to module roots — `import lib.util`
/// resolves under the `lib` package's `src/`.
#[derive(Default)]
struct Loader {
    visiting: Vec<PathBuf>,
    done: HashMap<PathBuf, String>,
    names: HashMap<String, PathBuf>,
    mods: Vec<LoadedModule>,
    roots: HashMap<String, PathBuf>,
}

impl Loader {
    /// `import a.b.c` → `a/b/c.lang` under the importer's directory;
    /// `std.*` roots at the repo's `stdlib/`; `pkg.*` roots at the
    /// declared dependency's `src/` (N38).
    fn resolve(&self, imp: &ast::Import, dir: &Path) -> Result<PathBuf, Vec<Diag>> {
        let rel: PathBuf = imp.path.iter().collect();
        let base = if imp.path.first().map(String::as_str) == Some("std") {
            stdlib_dir().join(imp.path[1..].iter().collect::<PathBuf>())
        } else if let Some(root) = imp
            .path
            .first()
            .and_then(|seg| self.roots.get(seg.as_str()))
        {
            root.join(imp.path[1..].iter().collect::<PathBuf>())
        } else {
            dir.join(&rel)
        };
        Ok(base.with_extension("lang"))
    }

    /// Load one module (and transitively its imports). Returns its
    /// qualifier. Postorder push means `mods` is already a valid
    /// check order when this returns.
    fn visit(&mut self, file: &Path, site: Span, imp: &ast::Import) -> Result<String, Vec<Diag>> {
        let canon = std::fs::canonicalize(file)
            .ok()
            .filter(|p| p.is_file())
            .ok_or_else(|| {
                let mut msg =
                    format!("cannot resolve import — no file `{}`", file.display());
                // Name what `pkg.` resolved to, or what it failed to
                // match — the dependency boundary is otherwise
                // invisible in the error (N38).
                if let Some(first) = imp.path.first() {
                    if first == "std" {
                        msg.push_str(" (the `std` root is the compiler's stdlib)");
                    } else if let Some(root) = self.roots.get(first.as_str()) {
                        msg.push_str(&format!(
                            " (package `{first}` at `{}`)",
                            root.display()
                        ));
                    } else if imp.path.len() > 1
                        && !file.parent().map(|d| d.is_dir()).unwrap_or(false)
                    {
                        msg.push_str(&format!(
                            " (`{first}` is not a declared package — \
                             add it under [dependencies] in project.toml)"
                        ));
                    }
                }
                vec![Diag::new(site, msg)]
            })?;
        if let Some(n) = self.done.get(&canon) {
            return Ok(n.clone());
        }
        let name = canon
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();
        if let Some(at) = self.visiting.iter().position(|p| p == &canon) {
            let mut chain: Vec<String> = self.visiting[at..]
                .iter()
                .map(|p| {
                    p.file_stem()
                        .map(|s| s.to_string_lossy().to_string())
                        .unwrap_or_default()
                })
                .collect();
            chain.push(name);
            return Err(vec![Diag::new(
                site,
                format!("cyclic module dependency: {}", chain.join(" -> ")),
            )]);
        }
        if let Some(prev) = self.names.get(&name) {
            if prev != &canon {
                return Err(vec![Diag::new(
                    site,
                    format!(
                        "module name `{name}` is claimed by both `{}` and `{}`",
                        prev.display(),
                        canon.display()
                    ),
                )]);
            }
        }
        let src = std::fs::read_to_string(&canon).map_err(|e| {
            vec![Diag::new(site, format!("{}: {e}", canon.display()))]
        })?;
        render::register_source(&canon.display().to_string(), &src);
        let mut prog = parser::parse(&src).map_err(|ds| {
            ds.into_iter()
                .map(|mut d| {
                    d.file = Some(canon.display().to_string());
                    d
                })
                .collect::<Vec<_>>()
        })?;
        if let Some(m) = &prog.module {
            if m != &name {
                return Err(vec![Diag::new(
                    site,
                    format!("`module {m}` declared, but the file is `{name}.lang`"),
                )]);
            }
        }
        self.names.insert(name.clone(), canon.clone());
        self.visiting.push(canon.clone());
        let mut deps = Vec::new();
        let dir = canon
            .parent()
            .map(|d| d.to_path_buf())
            .unwrap_or_else(|| PathBuf::from("."));
        for imp in &prog.imports {
            let f = self.resolve(imp, &dir)?;
            deps.push(self.visit(&f, imp.span, imp)?);
        }
        self.visiting.pop();
        module::qualify(&mut prog, &name);
        self.done.insert(canon.clone(), name.clone());
        self.mods.push(LoadedModule {
            name: name.clone(),
            file: canon,
            deps,
            prog,
            bind_modes: std::collections::HashMap::new(),
        });
        Ok(name)
    }
}

/// The platform triple for `target triple = "..."`.
pub fn target_triple() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "arm64-apple-darwin"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "x86_64-apple-darwin"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "x86_64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "aarch64-unknown-linux-gnu"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "x86_64-pc-windows-msvc"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "aarch64-pc-windows-msvc"
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        "unknown-unknown-unknown"
    }
}

/// Resolve a C toolchain driver that accepts `.ll`: on macOS `/usr/bin/clang`
/// may be a stub without the toolchain — `xcrun` finds the real one.
pub fn cc() -> String {
    if cfg!(target_os = "macos") {
        if let Ok(o) = Command::new("xcrun").args(["-f", "clang"]).output() {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !p.is_empty() && Path::new(&p).exists() {
                return p;
            }
        }
    }
    for cand in ["clang", "cc", "gcc"] {
        if Command::new(cand).arg("--version").output().is_ok() {
            return cand.to_string();
        }
    }
    "clang".to_string()
}

/// Extra args a platform driver needs — on macOS the SDK root, since the
/// linker can't find `libSystem` without it.
fn cc_args() -> Vec<String> {
    let mut v = Vec::new();
    if cfg!(target_os = "macos") {
        if let Ok(o) = Command::new("xcrun").args(["--show-sdk-path"]).output() {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !p.is_empty() {
                v.push("-isysroot".to_string());
                v.push(p);
            }
        }
    }
    v
}

fn run(cmd: &mut Command) -> Result<(), String> {
    let out = cmd
        .output()
        .map_err(|e| format!("failed to spawn {:?}: {e}", cmd.get_program()))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{:?} failed:\n{}",
            cmd.get_program(),
            String::from_utf8_lossy(&out.stderr)
        ))
    }
}

/// Compile `src_path` to a native executable in `out_dir`, leaving the
/// `.ll` and `.o` next to it (transparency: every stage's product is a
/// file you can read).
pub fn build(src_path: &Path, out_dir: &Path) -> Result<PathBuf, String> {
    build_with(src_path, out_dir, &HashMap::new(), false, false)
}

/// `build` with declared package roots (N38) and a profile switch:
/// `release` asks the C driver for `-O2` on both the `.ll`→`.o`
/// compile and the link; debug emits no opt flag. `debug` attaches
/// the N57 DWARF line tables to the emitted `.ll` (C's `-g` — `our`
/// dev builds pass it, test/inspection paths don't so textual IR
/// stays metadata-free).
pub fn build_with(
    src_path: &Path,
    out_dir: &Path,
    roots: &HashMap<String, PathBuf>,
    release: bool,
    debug: bool,
) -> Result<PathBuf, String> {
    let stem = src_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let ll = out_dir.join(format!("{stem}.ll"));
    let obj = out_dir.join(format!("{stem}.o"));
    let exe = if cfg!(target_os = "windows") {
        out_dir.join(format!("{stem}.exe"))
    } else {
        out_dir.join(&stem)
    };

    let art = match compile_path_roots(src_path, roots, debug) {
        Ok(a) => a,
        Err(ds) => return Err(render::render_all(&ds)),
    };
    // `main` must exist and return an integer (its low bits are the exit
    // status — a contract with the platform, stated plainly). Params:
    // none, or `args: slice str` — the runtime's argv view (N26A).
    match art.module.fns.iter().find(|f| f.name == "main") {
        Some(f) if f.ret.is_int() => match &f.params[..] {
            [] => {}
            [(_, None, crate::frontend::ast::Ty::Slice { mutable: false, elem })]
                if **elem == crate::frontend::ast::Ty::Str => {}
            _ => {
                return Err(
                    "error: `main` takes no parameters, or `args: slice str`".to_string()
                )
            }
        },
        Some(_) => return Err("error: `main` must return an integer type".to_string()),
        None => return Err("error: no `main` function".to_string()),
    }
    std::fs::write(&ll, &art.llvm).map_err(|e| e.to_string())?;
    let cc = cc();
    let extra = cc_args();
    let mut opt = extra.clone();
    if release {
        opt.push("-O2".to_string());
    }
    run(Command::new(&cc)
        .args(&opt)
        .args(["-c"])
        .arg(&ll)
        .args(["-o"])
        .arg(&obj))?;
    let rt = rt_objs()?;
    let mut link = Command::new(&cc);
    link.args(&opt).arg(&obj);
    for r in &rt {
        link.arg(r);
    }
    link.args(["-o"]).arg(&exe);
    if cfg!(target_os = "linux") {
        link.arg("-lpthread");
    }
    if cfg!(target_os = "windows") {
        // lld links the MSVC target; sockets live in ws2_32.
        link.args(["-fuse-ld=lld", "-lws2_32"]);
    }
    run(&mut link)?;
    Ok(exe)
}

/// `runtime/mmlrt_common.c` + the platform half compiled once per
/// process — the N13 boundary: tasks, mutexes, channels. N58C-2 split
/// the runtime: common (checks/strings/alloc/argv) + mmlrt_posix.c or
/// mmlrt_win32.c chosen by host. N61: sources are embedded in the
/// binary and materialized under `$OUR_HOME/lib` (`OUR_RUNTIME`
/// overrides) — an installed tool needs no checkout.
fn rt_objs() -> Result<Vec<PathBuf>, String> {
    static CACHE: OnceLock<Result<Vec<PathBuf>, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let rt_dir = super::resources::runtime_dir();
            let dir = std::env::temp_dir().join("ourc-rt");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let plat = if cfg!(target_os = "windows") {
                "mmlrt_win32.c"
            } else {
                "mmlrt_posix.c"
            };
            let mut objs = Vec::new();
            for name in ["mmlrt_common.c", plat] {
                let src = rt_dir.join(name);
                let obj = dir.join(name.replace(".c", ".o"));
                run(Command::new(cc())
                    .args(&cc_args())
                    .args(["-O2", "-c"])
                    .arg(&src)
                    .args(["-o"])
                    .arg(&obj))?;
                objs.push(obj);
            }
            Ok(objs)
        })
        .clone()
}

/// `--show-asm`: the same `.ll` we emitted, disassembled to native asm.
pub fn asm(ll_path: &Path) -> Result<String, String> {
    let out = Command::new(cc())
        .args(["-S", "-x", "ir"])
        .arg(ll_path)
        .args(["-o", "-"])
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).to_string())
    }
}

#[cfg(test)]
mod graph_tests {
    use super::*;

    /// A two-module fixture plus error cases — pins the canonical
    /// `(module-graph …)` / `!` shapes for the N70 differential.
    #[test]
    fn dump_modules_shape() {
        let dir = std::env::temp_dir().join("ourc_graph_pin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("sub")).unwrap();
        std::fs::write(
            dir.join("main.lang"),
            "import util;\nimport std.vec;\nfn main() {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("util.lang"),
            "module util;\nimport sub.leaf;\npub fn u() {}\n",
        )
        .unwrap();
        std::fs::write(dir.join("sub/leaf.lang"), "pub fn l() {}\n").unwrap();
        std::fs::write(dir.join("cyc.lang"), "import cyc;\n").unwrap();
        std::fs::write(dir.join("gone.lang"), "import absent;\n").unwrap();
        std::fs::write(dir.join("badmod.lang"), "module other;\n").unwrap();
        std::fs::write(dir.join("usesbad.lang"), "import badmod;\n").unwrap();

        let got = dump_modules(&dir.join("main.lang"));
        let main_lang = dir.join("main.lang").display().to_string();
        let expected = format!(
            "(module-graph\n  (root \"{main_lang}\"\n    (dep \"util\")\n    (dep \"vec\")\n  )\n  (mod \"leaf\" \"sub/leaf.lang\"\n  )\n  (mod \"util\" \"util.lang\"\n    (dep \"leaf\")\n  )\n  (mod \"vec\" \"<std>/vec.lang\"\n  )\n)\n"
        );
        assert_eq!(got, expected);

        let cyc = dump_modules(&dir.join("cyc.lang"));
        assert!(
            cyc.contains("cyclic module dependency: cyc -> cyc"),
            "cycle diag: {cyc}"
        );
        assert!(cyc.starts_with("! - 1 1 "), "cycle is unattributed: {cyc}");

        let gone = dump_modules(&dir.join("gone.lang"));
        assert!(
            gone.contains("cannot resolve import — no file"),
            "resolve diag: {gone}"
        );

        let bad = dump_modules(&dir.join("usesbad.lang"));
        assert!(
            bad.contains("`module other` declared, but the file is `badmod.lang`"),
            "module-name diag: {bad}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// N71 — pins the canonical `(module-symbols …)` record forms:
    /// decl tables, binds/uses, method + assoc-fn resolution, match
    /// variants, generic inference and the clone it materializes,
    /// and the private-use diagnostic.
    #[test]
    fn dump_symbols_shape() {
        let dir = std::env::temp_dir().join("ourc_syms_pin");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("dep.lang"),
            "module dep;\n\
             pub struct S { x: i32, y: u8 }\n\
             pub enum E { A, B(i32) }\n\
             pub fn make() -> S { return S { x: 1, y: 2 }; }\n\
             fn hidden() {}\n\
             pub fn id<T>(x: T) -> T { return x; }\n\
             pub fn touch(read self: S) -> i32 { return self.x; }\n\
             extern fn cwrite(fd: i32, buf: *const u8, n: u64) -> i64;\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("main.lang"),
            "import dep;\n\
             fn main() {\n\
             \x20   let s = dep.make();\n\
             \x20   let n = s.touch();\n\
             \x20   let e = dep.E::B(3);\n\
             \x20   match e {\n\
             \x20       A => {}\n\
             \x20       B(v) => { let w = v; }\n\
             \x20       _ => {}\n\
             \x20   }\n\
             \x20   let g = dep.id(7);\n\
             \x20   dep.hidden();\n\
             }\n",
        )
        .unwrap();

        let got = dump_symbols(&dir.join("main.lang"));
        let dep_path = dir.join("dep.lang").display().to_string();
        let main_path = dir.join("main.lang").display().to_string();
        // Canonicalize like `load_graph` does — the dep's identity is
        // its canonical path, the root's is as-given.
        let dep_canon = std::fs::canonicalize(&dep_path)
            .unwrap()
            .display()
            .to_string();
        let _ = main_path;
        let expected = format!(
            "(module-symbols\n  (mod \"dep\" \"{dep_canon}\"\n    (struct \"dep.S\" pub @2:5 (field \"x\" \"i32\") (field \"y\" \"u8\"))\n    (enum \"dep.E\" pub @3:5 (variant \"A\") (variant \"B\" \"i32\"))\n    (fn \"dep.make\" pub @4:5 (ret \"dep.S\"))\n    (fn \"dep.hidden\" @5:1 (ret \"void\"))\n    (gen \"dep.id\" pub @6:5 (tparam \"T\") (param \"x\" - \"T\") (ret \"T\"))\n    (fn \"dep.touch\" pub @7:5 (param \"self\" read \"dep.S\") (ret \"i32\"))\n    (extern \"cwrite\" @8:8 (param \"fd\" - \"i32\") (param \"buf\" - \"*const u8\") (param \"n\" - \"u64\") (ret \"i64\"))\n    (body \"dep.make\"\n      (use \"dep.S\" @4:29 -> struct \"dep.S\")\n      (member \"x\" @4:36 -> field \"dep.S.x\")\n      (member \"y\" @4:42 -> field \"dep.S.y\")\n    )\n    (body \"dep.hidden\"\n    )\n    (body \"dep.touch\"\n      (use \"self\" @7:44 -> param @7:5)\n      (member \"x\" @7:48)\n    )\n  )\n  (root \"\" \"{main_path}\"\n    (dep \"dep\")\n    (fn \"main\" @2:1 (ret \"void\"))\n    (body \"main\"\n      (use \"dep.make\" @3:13 -> fn \"dep.make\")\n      (bind \"s\" @3:9 d2 \"dep.S\")\n      (use \"s\" @4:13 -> local @3:9)\n      (method \"touch\" @4:13 -> fn \"dep.touch\")\n      (use \"dep.touch\" @4:13 -> fn \"dep.touch\")\n      (bind \"n\" @4:9 d2 \"i32\")\n      (tyuse \"dep.E\" @5:13 -> enum \"dep.E\")\n      (assoc \"dep.E::B\" @5:13 -> variant \"dep.E::B\")\n      (bind \"e\" @5:9 d2 \"dep.E\")\n      (use \"e\" @6:11 -> local @5:9)\n      (variant \"A\" @6:5 -> \"dep.E::A\")\n      (variant \"B\" @6:5 -> \"dep.E::B\")\n      (armbind \"v\" @6:5 d3 \"i32\")\n      (use \"v\" @8:27 -> local @6:5)\n      (bind \"w\" @8:23 d3 \"i32\")\n      (variant \"_\" @6:5 -> wildcard)\n      (use \"dep.id\" @11:13 -> generic \"dep.id<i32>\" infer (\"i32\"))\n      (bind \"g\" @11:9 d2 \"i32\")\n      (use \"dep.hidden\" @12:5 -> ?)\n    )\n    (clone-fn \"dep.id<i32>\" pub (param \"x\" - \"i32\") (ret \"i32\"))\n    (body \"dep.id<i32>\"\n      (use \"x\" @6:34 -> local @6:5)\n    )\n    (err 12 5 \"`dep.hidden` is private to its module\")\n  )\n)\n"
        );
        assert_eq!(got, expected);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
