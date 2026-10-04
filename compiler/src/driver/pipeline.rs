//! The pipeline:
//!   .lang → tokens → AST → typecheck → our IR → LLVM IR → .o → exe
//! Every stage's product is inspectable — that is the point of the
//! project, so nothing here hides a representation.

use crate::backend_llvm::emit;
use crate::frontend::{ast, module, parser};
use crate::ir::{gen, ir::Module};
use crate::driver::render;
use crate::semantics::check::{self, ModuleInterface};
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
    let strings = RefCell::new(Vec::new());
    let module = gen::gen(&prog, &ModuleInterface::default(), &strings);
    let module = Module {
        strings: strings.into_inner(),
        ..module
    };
    let llvm = emit::emit(&module, target_triple());
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
}

/// Parse → load imports → monomorphize → check. Shared by `check_path`
/// and `compile_path` so the two can never drift (N21A).
fn frontend_path(src_path: &Path) -> Result<Frontend, Vec<Diag>> {
    let src = std::fs::read_to_string(src_path).map_err(|e| {
        vec![Diag::new(
            Span { line: 1, col: 1 },
            format!("{}: {e}", src_path.display()),
        )]
    })?;
    render::register_source(&src_path.display().to_string(), &src);
    render::register_root(&src_path.display().to_string());
    let mut prog = parser::parse(&src)?;
    let root_dir = src_path
        .parent()
        .map(|d| d.to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."));

    // Load the import graph: postorder DFS — every module lands in
    // `mods` after all of its own dependencies.
    let mut loader = Loader::default();
    let mut root_deps = Vec::new();
    for imp in &prog.imports {
        let f = loader.resolve(imp, &root_dir)?;
        root_deps.push(loader.visit(&f, imp.span)?);
    }

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
    for m in &loader.mods {
        let imports = join_ifaces(&m.deps, &ifaces);
        let mut iface = ModuleInterface::default();
        check::check_module(&m.prog, &imports, &mut iface).map_err(|ds| {
            ds.into_iter()
                .map(|mut d| {
                    d.file = Some(m.file.display().to_string());
                    d
                })
                .collect::<Vec<_>>()
        })?;
        ifaces.insert(m.name.clone(), iface);
    }
    let root_imports = join_ifaces(&root_deps, &ifaces);
    check::check_module(&prog, &root_imports, &mut ModuleInterface::default())?;
    Ok(Frontend {
        prog,
        mods: loader.mods,
        root_deps,
        ifaces,
    })
}

/// `our check` — the semantic pipeline only (N21A): parse, resolve
/// imports, monomorphize, check every module. No IR, no LLVM, no link.
pub fn check_path(src_path: &Path) -> Result<(), Vec<Diag>> {
    frontend_path(src_path).map(|_| ())
}

/// The full path-driven compile (N17): resolve `import`s from disk,
/// qualify each module's symbols, check modules dependency-first so
/// importers replay callee summaries — never callee bodies — and emit
/// every module's code into one LLVM module.
pub fn compile_path(src_path: &Path) -> Result<Artifacts, Vec<Diag>> {
    let fe = frontend_path(src_path)?;
    let Frontend {
        prog,
        mods,
        root_deps,
        ifaces,
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
            gen::gen(&m.prog, &imports, &strings),
            &mut extern_sigs,
        );
    }
    merge_module(
        &mut module,
        gen::gen(&prog, &root_imports, &strings),
        &mut extern_sigs,
    );
    module.strings = strings.into_inner();
    let llvm = emit::emit(&module, target_triple());
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

/// The stdlib root — `import std.vec_i32` resolves here. Lives beside
/// the crate so the path is independent of the caller's cwd.
fn stdlib_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
        .join("stdlib")
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
}

/// Depth-first module loader (N17). The graph is a DAG by construction:
/// a back-edge in `visiting` is a cyclic import, rejected. `done` maps
/// canonical paths to qualifiers so shared deps load once; `names`
/// keeps two different files from claiming the same qualifier.
#[derive(Default)]
struct Loader {
    visiting: Vec<PathBuf>,
    done: HashMap<PathBuf, String>,
    names: HashMap<String, PathBuf>,
    mods: Vec<LoadedModule>,
}

impl Loader {
    /// `import a.b.c` → `a/b/c.lang` under the importer's directory;
    /// `std.*` roots at the repo's `stdlib/`.
    fn resolve(&self, imp: &ast::Import, dir: &Path) -> Result<PathBuf, Vec<Diag>> {
        let rel: PathBuf = imp.path.iter().collect();
        let base = if imp.path.first().map(String::as_str) == Some("std") {
            stdlib_dir().join(imp.path[1..].iter().collect::<PathBuf>())
        } else {
            dir.join(&rel)
        };
        Ok(base.with_extension("lang"))
    }

    /// Load one module (and transitively its imports). Returns its
    /// qualifier. Postorder push means `mods` is already a valid
    /// check order when this returns.
    fn visit(&mut self, file: &Path, site: Span) -> Result<String, Vec<Diag>> {
        let canon = std::fs::canonicalize(file)
            .ok()
            .filter(|p| p.is_file())
            .ok_or_else(|| {
                vec![Diag::new(
                    site,
                    format!("cannot resolve import — no file `{}`", file.display()),
                )]
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
            deps.push(self.visit(&f, imp.span)?);
        }
        self.visiting.pop();
        module::qualify(&mut prog, &name);
        self.done.insert(canon.clone(), name.clone());
        self.mods.push(LoadedModule {
            name: name.clone(),
            file: canon,
            deps,
            prog,
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
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
    let stem = src_path
        .file_stem()
        .unwrap()
        .to_string_lossy()
        .to_string();
    std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
    let ll = out_dir.join(format!("{stem}.ll"));
    let obj = out_dir.join(format!("{stem}.o"));
    let exe = out_dir.join(&stem);

    let art = match compile_path(src_path) {
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
    run(Command::new(&cc)
        .args(&extra)
        .args(["-c"])
        .arg(&ll)
        .args(["-o"])
        .arg(&obj))?;
    let rt = rt_obj()?;
    let mut link = Command::new(&cc);
    link.args(&extra).arg(&obj).arg(&rt).args(["-o"]).arg(&exe);
    if cfg!(target_os = "linux") {
        link.arg("-lpthread");
    }
    run(&mut link)?;
    Ok(exe)
}

/// `runtime/mmlrt.c` compiled once per process — the N13 boundary:
/// tasks, mutexes, channels on pthreads. The source ships inside the
/// compiler crate, so this path is relative to the crate, not cwd.
fn rt_obj() -> Result<PathBuf, String> {
    static CACHE: OnceLock<Result<PathBuf, String>> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("runtime/mmlrt.c");
            let dir = std::env::temp_dir().join("ourc-rt");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let obj = dir.join("mmlrt.o");
            run(Command::new(cc())
                .args(&cc_args())
                .args(["-O2", "-c"])
                .arg(&src)
                .args(["-o"])
                .arg(&obj))?;
            Ok(obj)
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
