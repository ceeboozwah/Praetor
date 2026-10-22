//! N71 — the resolver pass: `name → declaration`, `syntax type →
//! canonical type`. Runs on `load_graph` output — post-qualify dep
//! programs plus the unqualified root — and emits the canonical
//! `(module-symbols …)` surface the self-hosted port must reproduce
//! byte-for-byte.
//!
//! What this pass is: the union of the checker's name-level facts and
//! mono's scan-level resolution. Decl tables, visibility (pub /
//! private-to-module), scope/binding resolution, the `ty_of`
//! best-effort evaluator that resolves method calls / match-arm
//! variants / associated fns / inferred type arguments, and the
//! instantiation *requests* (`want`s) each site raises.
//!
//! What it is not: no clone materialization (the `made` machinery is
//! a later milestone), no authority/init/consume, and no general
//! expression typing — `x.f` member resolution records the member
//! token but not a target, because that needs the real typer.

use crate::{Diag, Span};
use crate::frontend::ast::*;
use std::collections::{HashMap, HashSet};

// ------------------------------------------------------------------
// Tables — global decl registry across the whole loaded graph.
// Mirrors mono's `Tables`/`gens`/`concrete`: generic templates carry
// their owner module (privacy is enforced cross-module), concrete
// decls just occupy names.
// ------------------------------------------------------------------

enum Gen<'a> {
    F(usize, &'a FnDef),
    S(usize, &'a StructDecl),
    E(usize, &'a EnumDecl),
}

#[derive(Default)]
struct Tables<'a> {
    fns: HashMap<String, &'a FnDef>,
    structs: HashMap<String, &'a StructDecl>,
    enums: HashMap<String, &'a EnumDecl>,
    /// CHAN-004 S3b — checker-synthesized decls with no AST home:
    /// `result.Result<T,E>` is registered here (owned, since the
    /// borrowed tables can't hold it) so `send` on `bchannel` can
    /// name `Result<void,T>` and its variants resolve like a real
    /// generic decl.
    synth: HashMap<String, EnumDecl>,
}

/// Enum-decl lookup across the three homes: checker-synthesized
/// decls, concrete decls, generic decls.
fn edecl<'x, 'a>(
    tb: &'x Tables<'a>,
    gens: &'x HashMap<String, Gen<'a>>,
    n: &str,
) -> Option<&'x EnumDecl> {
    tb.synth.get(n).or_else(|| {
        tb.enums
            .get(n)
            .copied()
            .or_else(|| match gens.get(n) {
                Some(Gen::E(_, d)) => Some(*d),
                _ => None,
            })
    })
}

/// Is `n` an enum nameable from module `who` — synth decls are the
/// checker's own registrations: always visible.
fn enum_named<'a>(tb: &Tables<'a>, gens: &HashMap<String, Gen<'a>>, n: &str, who: usize) -> bool {
    tb.synth.contains_key(n)
        || match gens.get(n) {
            Some(Gen::E(o, d)) => *o == who || d.is_pub,
            _ => false,
        }
}

#[derive(Clone)]
struct Want {
    base: String,
    args: Vec<Ty>,
    span: Span,
}

type Sig = (Vec<(Option<Auth>, Ty)>, Ty, bool);

/// One module's visible-name sets — the checker's `imports`
/// interface: direct deps' `pub` names only, transitivity cannot
/// leak. `private` is the set that gets the "private to its module"
/// spelling instead of "undefined".
#[derive(Default)]
struct ModCtx {
    /// Callable: own fns (any visibility) + deps' pub non-template
    /// fns + deps' externs (externs always export). Full sigs ride
    /// along — the duplicate-fn check inserts locals into the same
    /// seeded map the checker uses.
    v_fns: HashMap<String, Sig>,
    /// check_ty membership: own + deps' pub non-template type names —
    /// enum names fold in as pseudo-structs like the checker.
    v_structs: HashSet<String>,
    v_enums: HashSet<String>,
    /// Deps' non-pub non-template decl names.
    private: HashSet<String>,
    /// Own fn names — the builtin-shadow set (N21B).
    local_fns: HashSet<String>,
}

// ------------------------------------------------------------------
// The resolver itself.
// ------------------------------------------------------------------

pub struct Resolver<'a> {
    tb: Tables<'a>,
    gens: HashMap<String, Gen<'a>>,
    concrete: HashSet<String>,
    /// `duplicate generic declaration` diags — global registry phase.
    reg_errs: Vec<Diag>,
    out: String,
}

impl<'a> Resolver<'a> {
    pub fn new() -> Self {
        Resolver {
            tb: Tables::default(),
            gens: HashMap::new(),
            concrete: HashSet::new(),
            reg_errs: Vec::new(),
            out: String::from("(module-symbols\n"),
        }
    }

    /// Register every program's decls into the global tables.
    /// `progs` is `(file, prog)` in scan order (postorder + root).
    pub fn register(&mut self, progs: &[(String, &'a Program)]) {
        for (i, (_, p)) in progs.iter().enumerate() {
            for s in &p.structs {
                self.tb.structs.insert(s.name.clone(), s);
                if s.tparams.is_empty() {
                    self.concrete.insert(s.name.clone());
                } else if self.gens.insert(s.name.clone(), Gen::S(i, s)).is_some() {
                    self.reg_errs.push(Diag::new(
                        s.span,
                        format!("duplicate generic declaration `{}`", s.name),
                    ));
                }
            }
            for e in &p.enums {
                self.tb.enums.insert(e.name.clone(), e);
                if e.tparams.is_empty() {
                    self.concrete.insert(e.name.clone());
                } else if self.gens.insert(e.name.clone(), Gen::E(i, e)).is_some() {
                    self.reg_errs.push(Diag::new(
                        e.span,
                        format!("duplicate generic declaration `{}`", e.name),
                    ));
                }
            }
            for f in &p.fns {
                self.tb.fns.insert(f.name.clone(), f);
                if f.tparams.is_empty() {
                    self.concrete.insert(f.name.clone());
                } else if self.gens.insert(f.name.clone(), Gen::F(i, f)).is_some() {
                    self.reg_errs.push(Diag::new(
                        f.span,
                        format!("duplicate generic declaration `{}`", f.name),
                    ));
                }
            }
        }
        // CHAN-004 S3b — `result.Result<T,E>` exists whenever a
        // `send` on a `bchannel` needs its conditional-ownership
        // return type; register it unless the program declares its
        // own (imported `std.result` counts — same decl shape).
        let rn = "result.Result";
        if !self.tb.enums.contains_key(rn) && !self.gens.contains_key(rn) {
            self.tb.synth.insert(
                rn.to_string(),
                EnumDecl {
                    name: rn.to_string(),
                    tparams: vec!["T".to_string(), "E".to_string()],
                    variants: vec![
                        ("Ok".to_string(), Some(Ty::Struct("T".to_string()))),
                        ("Err".to_string(), Some(Ty::Struct("E".to_string()))),
                    ],
                    is_pub: true,
                    span: Span { line: 0, col: 0 },
                },
            );
        }
        // `recv` on a `bchannel` needs `option.Option<T>` the same
        // way — synthesized unless the program declares its own.
        let on = "option.Option";
        if !self.tb.enums.contains_key(on) && !self.gens.contains_key(on) {
            self.tb.synth.insert(
                on.to_string(),
                EnumDecl {
                    name: on.to_string(),
                    tparams: vec!["T".to_string()],
                    variants: vec![
                        ("Some".to_string(), Some(Ty::Struct("T".to_string()))),
                        ("None".to_string(), None),
                    ],
                    is_pub: true,
                    span: Span { line: 0, col: 0 },
                },
            );
        }
    }

    /// Build one module's visibility sets from its own decls plus its
    /// direct deps' interfaces.
    fn visibility(p: &Program, deps: &[&Program]) -> ModCtx {
        let mut m = ModCtx::default();
        for f in &p.fns {
            if f.tparams.is_empty() {
                m.v_fns.insert(f.name.clone(), fn_sig_of(f));
            }
            m.local_fns.insert(f.name.clone());
        }
        for s in &p.structs {
            if s.tparams.is_empty() {
                m.v_structs.insert(s.name.clone());
            }
        }
        for e in &p.enums {
            if e.tparams.is_empty() {
                m.v_enums.insert(e.name.clone());
                m.v_structs.insert(e.name.clone());
            }
        }
        for dp in deps {
            for s in &dp.structs {
                if s.tparams.is_empty() {
                    if s.is_pub || s.name.contains('<') {
                        m.v_structs.insert(s.name.clone());
                    } else {
                        m.private.insert(s.name.clone());
                    }
                }
            }
            for e in &dp.enums {
                if e.tparams.is_empty() {
                    if e.is_pub || e.name.contains('<') {
                        m.v_enums.insert(e.name.clone());
                        m.v_structs.insert(e.name.clone());
                    } else {
                        m.private.insert(e.name.clone());
                    }
                }
            }
            for f in &dp.fns {
                if f.is_extern {
                    // Externs name global C symbols — every importer
                    // sees them, pub or not.
                    m.v_fns.insert(f.name.clone(), fn_sig_of(f));
                } else if !f.tparams.is_empty() {
                    // Templates export nothing — instantiating a
                    // private one is the want-path diag, not a
                    // `private` lookup hit.
                } else if f.is_pub {
                    m.v_fns.insert(f.name.clone(), fn_sig_of(f));
                } else {
                    m.private.insert(f.name.clone());
                }
            }
        }
        m
    }

    /// Resolve one module; appends its `(mod …)` / `(root …)` block.
    pub fn module(
        &mut self,
        tag: &str,
        qual: &str,
        path: &str,
        deps: &[String],
        p: &Program,
        dep_progs: &[&Program],
        me: usize,
    ) {
        let m = Self::visibility(p, dep_progs);
        self.out.push_str(&format!("  ({tag} \"{qual}\" \"{path}\"\n"));
        for d in deps {
            self.out.push_str(&format!("    (dep \"{d}\")\n"));
        }
        let mut errs: Vec<Diag> = Vec::new();
        self.registry(p, &m, &mut errs);
        // Decl-level wants — mono collects these from every concrete
        // decl's signature before any body runs.
        let mut wants: Vec<Want> = Vec::new();
        for s in &p.structs {
            if s.tparams.is_empty() {
                for (_, ft) in &s.fields {
                    wants_ty(ft, me, s.span, &mut wants);
                }
            }
        }
        for e in &p.enums {
            if e.tparams.is_empty() {
                for (_, vt) in &e.variants {
                    if let Some(t) = vt {
                        wants_ty(t, me, e.span, &mut wants);
                    }
                }
            }
        }
        for f in &p.fns {
            if f.tparams.is_empty() {
                for (_, _, t) in &f.params {
                    wants_ty(t, me, f.span, &mut wants);
                }
                wants_ty(&f.ret, me, f.span, &mut wants);
            }
        }
        // Bodies.
        let mut infer_failed: HashSet<(u32, u32)> = HashSet::new();
        for f in &p.fns {
            if f.is_extern || !f.tparams.is_empty() {
                continue;
            }
            self.fn_body(f, &m, me, &mut wants, &mut infer_failed, &mut errs);
        }
        // Instantiation worklist — each fresh (requester, canon) want
        // materializes a clone in this module; clone decls and bodies
        // scan like source decls, cascading further wants.
        self.materialize(me, &m, wants, &mut infer_failed, &mut errs);
        for d in errs {
            self.out.push_str(&format!(
                "    (err {} {} \"{}\")\n",
                d.span.line,
                d.span.col,
                esc(&d.msg)
            ));
        }
        self.out.push_str("  )\n");
    }

    /// Declaration phase — `check_module`'s registry order exactly:
    /// struct dups, enum dups+variants, enum cycles, struct fields,
    /// fn sigs+FFI, pub-exposes-private. Decl records interleave.
    fn registry(&mut self, p: &Program, m: &ModCtx, errs: &mut Vec<Diag>) {
        // Struct/enum decl records, source order — dup diagnostics
        // ride the same inserts the checker uses.
        let mut structs_seen: HashSet<String> = HashSet::new();
        let mut enums_seen: HashSet<String> = HashSet::new();
        for s in &p.structs {
            self.out.push_str(&format!(
                "    (struct \"{}\"{}{}{}{})\n",
                s.name,
                if s.is_pub { " pub" } else { "" },
                sp(&s.span),
                tparams_text(&s.tparams),
                s.fields
                    .iter()
                    .map(|(n, t)| format!(" (field \"{n}\" \"{t}\")"))
                    .collect::<String>()
            ));
            if !s.tparams.is_empty() {
                continue;
            }
            if !structs_seen.insert(s.name.clone()) {
                errs.push(Diag::new(s.span, format!("duplicate struct `{}`", s.name)));
            }
            let mut seen = HashSet::new();
            for (fn_, _) in &s.fields {
                if !seen.insert(fn_) {
                    errs.push(Diag::new(
                        s.span,
                        format!("duplicate field `{fn_}` in struct `{}`", s.name),
                    ));
                }
            }
        }
        for e in &p.enums {
            self.out.push_str(&format!(
                "    (enum \"{}\"{}{}{}{})\n",
                e.name,
                if e.is_pub { " pub" } else { "" },
                sp(&e.span),
                tparams_text(&e.tparams),
                e.variants
                    .iter()
                    .map(|(n, t)| match t {
                        Some(t) => format!(" (variant \"{n}\" \"{t}\")"),
                        None => format!(" (variant \"{n}\")"),
                    })
                    .collect::<String>()
            ));
            if !e.tparams.is_empty() {
                continue;
            }
            if !enums_seen.insert(e.name.clone()) {
                errs.push(Diag::new(
                    Span { line: 0, col: 0 },
                    format!("duplicate enum `{}`", e.name),
                ));
            }
        }
        for e in &p.enums {
            if !e.tparams.is_empty() {
                continue;
            }
            let mut seen = HashSet::new();
            for (vn, vt) in &e.variants {
                if !seen.insert(vn) {
                    errs.push(Diag::new(
                        e.span,
                        format!("duplicate variant `{vn}` in enum `{}`", e.name),
                    ));
                }
                if let Some(t) = vt {
                    check_ty(t, &m.v_structs, e.span, errs);
                }
            }
        }
        for e in &p.enums {
            if !e.tparams.is_empty() {
                continue;
            }
            for (_, vt) in &e.variants {
                if let Some(t) = vt {
                    if reaches(t, &e.name, &m.v_structs, &self.tb, &mut HashSet::new()) {
                        errs.push(Diag::new(
                            e.span,
                            format!(
                                "enum `{}` contains itself by value — wrap in `box`",
                                e.name
                            ),
                        ));
                    }
                }
            }
        }
        for s in &p.structs {
            if !s.tparams.is_empty() {
                continue;
            }
            for (fn_, ft) in &s.fields {
                check_ty(ft, &m.v_structs, s.span, errs);
                if reaches(ft, &s.name, &m.v_structs, &self.tb, &mut HashSet::new()) {
                    errs.push(Diag::new(
                        s.span,
                        format!("field `{fn_}` makes struct `{}` recursive", s.name),
                    ));
                }
            }
        }
        // The dup rule inserts locals into a map already seeded with
        // imported sigs — an imported extern redeclared with an
        // identical sig dedupes like a C redeclaration. `v_fns`
        // carries locals too, so seed with the imported slice only.
        let mut fns_seen: HashMap<String, Sig> = m
            .v_fns
            .iter()
            .filter(|(n, _)| !m.local_fns.contains(*n))
            .map(|(n, s)| (n.clone(), s.clone()))
            .collect();
        for f in &p.fns {
            let tag = if f.is_extern {
                "extern"
            } else if f.tparams.is_empty() {
                "fn"
            } else {
                "gen"
            };
            self.out.push_str(&format!(
                "    ({tag} \"{}\"{}{}{}{} (ret \"{}\"))\n",
                f.name,
                if f.is_pub { " pub" } else { "" },
                sp(&f.span),
                tparams_text(&f.tparams),
                f.params
                    .iter()
                    .map(|(n, a, t)| format!(" (param \"{n}\" {} \"{t}\")", auth_text(*a)))
                    .collect::<String>(),
                f.ret
            ));
            if !f.tparams.is_empty() {
                continue;
            }
            for (_, _, t) in &f.params {
                check_ty(t, &m.v_structs, f.span, errs);
            }
            check_ty(&f.ret, &m.v_structs, f.span, errs);
            if f.is_extern {
                for (pn, auth, t) in &f.params {
                    if auth.is_some() {
                        errs.push(Diag::new(
                            f.span,
                            format!(
                                "extern `{n}` parameter `{pn}` takes no capability — \
                                 use `*T` (writable) or `*const T`",
                                n = f.name
                            ),
                        ));
                    }
                    if !is_ffi_ty(t) {
                        errs.push(Diag::new(
                            f.span,
                            format!(
                                "extern `{n}` parameter `{pn}` has non-FFI type `{t}`",
                                n = f.name
                            ),
                        ));
                    }
                }
                if f.ret != Ty::Void && !is_ffi_ty(&f.ret) {
                    errs.push(Diag::new(
                        f.span,
                        format!("extern `{}` returns non-FFI type `{}`", f.name, f.ret),
                    ));
                }
            }
            let sig = fn_sig_of(f);
            match fns_seen.get(&f.name) {
                Some(prev) if f.is_extern && *prev == sig => {}
                _ => {
                    if fns_seen.insert(f.name.clone(), sig).is_some() {
                        errs.push(Diag::new(
                            f.span,
                            format!("duplicate function `{}`", f.name),
                        ));
                    }
                }
            }
        }
        let private_structs: HashSet<String> = p
            .structs
            .iter()
            .filter(|s| !s.is_pub && s.tparams.is_empty())
            .map(|s| s.name.clone())
            .chain(
                p.enums
                    .iter()
                    .filter(|e| !e.is_pub && e.tparams.is_empty())
                    .map(|e| e.name.clone()),
            )
            .collect();
        for f in &p.fns {
            if !f.is_pub || f.is_extern || !f.tparams.is_empty() {
                continue;
            }
            for t in f.params.iter().map(|(_, _, t)| t).chain(std::iter::once(&f.ret)) {
                if let Some(n) = ty_mentions(t, &private_structs) {
                    errs.push(Diag::new(
                        f.span,
                        format!(
                            "`pub fn {}` exposes private struct `{n}` in its signature",
                            f.name
                        ),
                    ));
                }
            }
        }
        for s in &p.structs {
            if !s.is_pub || !s.tparams.is_empty() {
                continue;
            }
            for (fn_, ft) in &s.fields {
                if let Some(n) = ty_mentions(ft, &private_structs) {
                    errs.push(Diag::new(
                        s.span,
                        format!(
                            "`pub struct {}` field `{fn_}` has private type `{n}`",
                            s.name
                        ),
                    ));
                }
            }
        }
    }

    /// One concrete fn body: seed `locals` from params (mono), push
    /// the param scope, then scan the body emitting records.
    fn fn_body(
        &mut self,
        f: &FnDef,
        m: &ModCtx,
        me: usize,
        wants: &mut Vec<Want>,
        infer_failed: &mut HashSet<(u32, u32)>,
        errs: &mut Vec<Diag>,
    ) {
        let mut locals: HashMap<String, Ty> = f
            .params
            .iter()
            .map(|(n, _, t)| (n.clone(), t.clone()))
            .collect();
        let mut recs = String::new();
        // Params sit in scope 0; the fn body itself is a block (the
        // checker's `check_fn` → `check_block` pushes one), so
        // top-level lets land at depth 2.
        let mut scopes: Vec<HashMap<String, (Span, bool)>> = vec![HashMap::new()];
        for (n, a, _) in &f.params {
            scopes[0].insert(n.clone(), (f.span, a.is_some()));
        }
        {
            let mut sc = Scan {
                who: me,
                tb: &self.tb,
                gens: &self.gens,
                m,
                locals: &mut locals,
                scopes: &mut scopes,
                recs: &mut recs,
                errs,
                wants,
                infer_failed,
                loop_depth: 0,
            };
            scan_block(&f.body, &mut sc);
        }
        self.out.push_str(&format!("    (body \"{}\"\n{recs}    )\n", f.name));
    }

    /// Instantiation worklist — mono's fixpoint in compact form: a
    /// fresh `(requester, canon)` want either reports its rejection
    /// (private / not-generic / arity / subst) or materializes a
    /// clone in the requester's module. Clone decls emit records and
    /// fn-clone bodies scan like source bodies, so their wants
    /// cascade into the same list.
    fn materialize(
        &mut self,
        me: usize,
        m: &ModCtx,
        mut wants: Vec<Want>,
        infer_failed: &mut HashSet<(u32, u32)>,
        errs: &mut Vec<Diag>,
    ) {
        let mut attempted: HashSet<String> = HashSet::new();
        let mut i = 0;
        while i < wants.len() {
            let w = wants[i].clone();
            i += 1;
            let key = canon(&w.base, &w.args);
            if !attempted.insert(key.clone()) {
                continue;
            }
            if let Some(decl) = self.tb.synth.get(&w.base) {
                match subst_variants(decl, &w.args) {
                    Err((sp, msg)) => errs.push(Diag::new(sp, msg)),
                    Ok(variants) => {
                        self.out.push_str(&format!(
                            "    (clone-enum \"{key}\"{})\n",
                            variants
                                .iter()
                                .map(|(n, t)| match t {
                                    Some(t) => format!(" (variant \"{n}\" \"{t}\")"),
                                    None => format!(" (variant \"{n}\")"),
                                })
                                .collect::<String>()
                        ));
                        for (_, vt) in &variants {
                            if let Some(t) = vt {
                                wants_ty(t, me, w.span, &mut wants);
                            }
                        }
                    }
                }
                continue;
            }
            match self.gens.get(&w.base) {
                Some(Gen::S(owner, decl)) => {
                    let (owner, decl) = (*owner, *decl);
                    if owner != me && !decl.is_pub {
                        errs.push(Diag::new(
                            w.span,
                            format!("`{}` is private to its module", w.base),
                        ));
                        continue;
                    }
                    match subst_fields(decl, &w.args) {
                        Err((sp, msg)) => errs.push(Diag::new(sp, msg)),
                        Ok(fields) => {
                            self.out.push_str(&format!(
                                "    (clone-struct \"{key}\"{})\n",
                                fields
                                    .iter()
                                    .map(|(n, t)| format!(" (field \"{n}\" \"{t}\")"))
                                    .collect::<String>()
                            ));
                            for (_, ft) in &fields {
                                wants_ty(ft, me, decl.span, &mut wants);
                            }
                        }
                    }
                }
                Some(Gen::E(owner, decl)) => {
                    let (owner, decl) = (*owner, *decl);
                    if owner != me && !decl.is_pub {
                        errs.push(Diag::new(
                            w.span,
                            format!("`{}` is private to its module", w.base),
                        ));
                        continue;
                    }
                    match subst_variants(decl, &w.args) {
                        Err((sp, msg)) => errs.push(Diag::new(sp, msg)),
                        Ok(variants) => {
                            self.out.push_str(&format!(
                                "    (clone-enum \"{key}\"{})\n",
                                variants
                                    .iter()
                                    .map(|(n, t)| match t {
                                        Some(t) => format!(" (variant \"{n}\" \"{t}\")"),
                                        None => format!(" (variant \"{n}\")"),
                                    })
                                    .collect::<String>()
                            ));
                            for (_, vt) in &variants {
                                if let Some(t) = vt {
                                    wants_ty(t, me, decl.span, &mut wants);
                                }
                            }
                        }
                    }
                }
                Some(Gen::F(owner, decl)) => {
                    let (owner, decl) = (*owner, *decl);
                    if owner != me && !decl.is_pub {
                        errs.push(Diag::new(
                            w.span,
                            format!("`{}` is private to its module", w.base),
                        ));
                        continue;
                    }
                    match subst_fn(decl, &w.args) {
                        Err((sp, msg)) => errs.push(Diag::new(sp, msg)),
                        Ok(cf) => {
                            self.out.push_str(&format!(
                                "    (clone-fn \"{key}\"{}{} (ret \"{}\"))\n",
                                if cf.is_pub { " pub" } else { "" },
                                cf.params
                                    .iter()
                                    .map(|(n, a, t)| {
                                        format!(" (param \"{n}\" {} \"{t}\")", auth_text(*a))
                                    })
                                    .collect::<String>(),
                                cf.ret
                            ));
                            // The clone is a concrete decl in this
                            // module — its sig types and body scan
                            // like source.
                            for (_, _, t) in &cf.params {
                                wants_ty(t, me, cf.span, &mut wants);
                            }
                            wants_ty(&cf.ret, me, cf.span, &mut wants);
                            if !cf.is_extern {
                                self.clone_body(&cf, &key, m, me, &mut wants, infer_failed, errs);
                            }
                        }
                    }
                }
                None => {
                    if self.concrete.contains(&w.base) {
                        errs.push(Diag::new(
                            w.span,
                            format!("`{}` is not generic", w.base),
                        ));
                    }
                }
            }
        }
    }

    /// A materialized fn clone's body — same scan as a source body,
    /// records under the canonical name.
    fn clone_body(
        &mut self,
        f: &FnDef,
        canon_name: &str,
        m: &ModCtx,
        me: usize,
        wants: &mut Vec<Want>,
        infer_failed: &mut HashSet<(u32, u32)>,
        errs: &mut Vec<Diag>,
    ) {
        let mut locals: HashMap<String, Ty> = f
            .params
            .iter()
            .map(|(n, _, t)| (n.clone(), t.clone()))
            .collect();
        let mut recs = String::new();
        let mut scopes: Vec<HashMap<String, (Span, bool)>> = vec![HashMap::new()];
        for (n, a, _) in &f.params {
            scopes[0].insert(n.clone(), (f.span, a.is_some()));
        }
        {
            let mut sc = Scan {
                who: me,
                tb: &self.tb,
                gens: &self.gens,
                m,
                locals: &mut locals,
                scopes: &mut scopes,
                recs: &mut recs,
                errs,
                wants,
                infer_failed,
                loop_depth: 0,
            };
            scan_block(&f.body, &mut sc);
        }
        self.out
            .push_str(&format!("    (body \"{canon_name}\"\n{recs}    )\n"));
    }

    pub fn finish(mut self) -> String {
        for d in self.reg_errs.drain(..) {
            self.out.push_str(&format!(
                "  (err {} {} \"{}\")\n",
                d.span.line,
                d.span.col,
                esc(&d.msg)
            ));
        }
        self.out.push_str(")\n");
        self.out
    }
}

// ------------------------------------------------------------------
// The scan — mono's scan_stmt/scan_expr discipline with canonical
// records and a real scope tree layered on. `locals` stays flat
// per-fn (mono's quirk: lets insert permanently, arm binds remove
// without restore) and feeds ty_of; `scopes` is the binding tree
// use-records resolve through.
// ------------------------------------------------------------------

struct Scan<'a> {
    who: usize,
    tb: &'a Tables<'a>,
    gens: &'a HashMap<String, Gen<'a>>,
    m: &'a ModCtx,
    locals: &'a mut HashMap<String, Ty>,
    scopes: &'a mut Vec<HashMap<String, (Span, bool)>>,
    recs: &'a mut String,
    errs: &'a mut Vec<Diag>,
    wants: &'a mut Vec<Want>,
    infer_failed: &'a mut HashSet<(u32, u32)>,
    loop_depth: u32,
}

fn sp(s: &Span) -> String {
    format!(" @{}:{}", s.line, s.col)
}

fn esc(s: &str) -> String {
    let mut o = String::new();
    for c in s.chars() {
        match c {
            '\\' => o.push_str("\\\\"),
            '"' => o.push_str("\\\""),
            '\n' => o.push_str("\\n"),
            _ => o.push(c),
        }
    }
    o
}

fn auth_text(a: Option<Auth>) -> &'static str {
    match a {
        Some(Auth::Read) => "read",
        Some(Auth::Write) => "write",
        Some(Auth::Own) => "own",
        _ => "-",
    }
}

fn tparams_text(tps: &[String]) -> String {
    tps.iter().map(|t| format!(" (tparam \"{t}\")")).collect()
}

fn canon(base: &str, args: &[Ty]) -> String {
    let inner: Vec<String> = args.iter().map(|a| format!("{a}")).collect();
    format!("{base}<{}>", inner.join(","))
}

fn is_ffi_ty(t: &Ty) -> bool {
    crate::semantics::check::is_ffi_ty(t)
}

fn fn_sig_of(f: &FnDef) -> Sig {
    (
        f.params.iter().map(|(_, a, t)| (*a, t.clone())).collect(),
        f.ret.clone(),
        f.is_extern,
    )
}

/// Builtin call names — unqualified only, and only when the module
/// doesn't declare the name itself (N21B). The checker's guarded arms.
fn builtin_name(n: &str) -> bool {
    crate::semantics::check::int_cast_ty(n).is_some()
        || crate::semantics::check::is_atomic_op(n)
        || crate::semantics::check::is_chan_op(n)
        || matches!(
            n,
            "slice"
                | "cstr"
                | "str_from_utf8"
                | "system_allocator"
                | "free"
                | "replace"
                | "swap"
                | "unreachable"
                | "lock"
                | "unlock"
                | "channel"
                | "bchannel"
        )
}

/// checker's `check_ty` — unknown-type-name validation through every
/// nested constructor. `App` never reaches the real checker either
/// (it has no arm — mono's want path owns generic applications);
/// `guard` is not a declarable type.
fn check_ty(t: &Ty, structs: &HashSet<String>, span: Span, errs: &mut Vec<Diag>) {
    match t {
        Ty::Struct(n) if !structs.contains(n) => {
            errs.push(Diag::new(span, format!("unknown struct `{n}`")));
        }
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Channel(e)
        | Ty::BChannel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => check_ty(e, structs, span, errs),
        Ty::Guard(_) => errs.push(Diag::new(
            span,
            "`guard` is not a declarable type — it comes from `lock`".to_string(),
        )),
        _ => {}
    }
}

/// `name` embedded by value — through fields, elements, payloads.
/// Refs, handles, tasks, ptrs, slices, boxes are all pointers and
/// break the cycle; `atomic`/`mutex` store inline.
fn reaches(
    t: &Ty,
    name: &str,
    vis: &HashSet<String>,
    tb: &Tables,
    seen: &mut HashSet<String>,
) -> bool {
    match t {
        Ty::Array(e, _) | Ty::Atomic(e) | Ty::Mutex(e) => reaches(e, name, vis, tb, seen),
        Ty::Struct(n) | Ty::App(n, _) => {
            if n == name {
                return true;
            }
            if !seen.insert(n.clone()) {
                return false;
            }
            if let Some(d) = tb.structs.get(n.as_str()) {
                return d
                    .fields
                    .iter()
                    .any(|(_, ft)| reaches(ft, name, vis, tb, seen));
            }
            tb.synth
                .get(n.as_str())
                .or_else(|| tb.enums.get(n.as_str()).copied())
                .map(|d| {
                    d.variants
                        .iter()
                        .filter_map(|(_, vt)| vt.as_ref())
                        .any(|ft| reaches(ft, name, vis, tb, seen))
                }) == Some(true)
        }
        _ => false,
    }
}

/// First private name reachable through `t` (N17's `ty_mentions`).
fn ty_mentions(t: &Ty, private: &HashSet<String>) -> Option<String> {
    match t {
        Ty::Struct(n) if private.contains(n) && !n.contains('<') => Some(n.clone()),
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::BChannel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => ty_mentions(e, private),
        _ => None,
    }
}

// ------------------------------------------------------------------
// Substitution — arity + `T<..>` errors that fire while materializing
// a would-be clone (the clone itself is a later milestone; the diag
// is a resolution fact).
// ------------------------------------------------------------------

fn arity(decl: &[String], args: &[Ty], name: &str, span: Span) -> Result<(), (Span, String)> {
    if decl.len() != args.len() {
        Err((
            span,
            format!(
                "`{name}` takes {} type argument(s), found {}",
                decl.len(),
                args.len()
            ),
        ))
    } else {
        Ok(())
    }
}

fn subst_fields(
    decl: &StructDecl,
    args: &[Ty],
) -> Result<Vec<(String, Ty)>, (Span, String)> {
    arity(&decl.tparams, args, &decl.name, decl.span)?;
    let map: HashMap<String, Ty> = decl
        .tparams
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    let mut fields = decl.fields.clone();
    for (_, ft) in &mut fields {
        subst_ty(ft, &map, decl.span)?;
    }
    Ok(fields)
}

fn subst_variants(
    decl: &EnumDecl,
    args: &[Ty],
) -> Result<Vec<(String, Option<Ty>)>, (Span, String)> {
    arity(&decl.tparams, args, &decl.name, decl.span)?;
    let map: HashMap<String, Ty> = decl
        .tparams
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    let mut variants = decl.variants.clone();
    for (_, vt) in &mut variants {
        if let Some(t) = vt {
            subst_ty(t, &map, decl.span)?;
        }
    }
    Ok(variants)
}

fn subst_fn(decl: &FnDef, args: &[Ty]) -> Result<FnDef, (Span, String)> {
    arity(&decl.tparams, args, &decl.name, decl.span)?;
    let map: HashMap<String, Ty> = decl
        .tparams
        .iter()
        .cloned()
        .zip(args.iter().cloned())
        .collect();
    let mut f = decl.clone();
    for (_, _, t) in &mut f.params {
        subst_ty(t, &map, decl.span)?;
    }
    subst_ty(&mut f.ret, &map, decl.span)?;
    for st in &mut f.body {
        subst_stmt(st, &map, decl.span)?;
    }
    Ok(f)
}

fn subst_ty(t: &mut Ty, map: &HashMap<String, Ty>, span: Span) -> Result<(), (Span, String)> {
    match t {
        Ty::Struct(n) => {
            if let Some(a) = map.get(n.as_str()) {
                *t = a.clone();
            }
        }
        Ty::App(n, args) => {
            if map.contains_key(n.as_str()) {
                return Err((span, format!("type parameter `{n}` takes no type arguments")));
            }
            for a in args {
                subst_ty(a, map, span)?;
            }
        }
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::BChannel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => subst_ty(e, map, span)?,
        _ => {}
    }
    Ok(())
}

fn subst_stmt(s: &mut Stmt, map: &HashMap<String, Ty>, dspan: Span) -> Result<(), (Span, String)> {
    match s {
        Stmt::Let { ty, init, span, .. } => {
            if let Some(t) = ty {
                subst_ty(t, map, *span)?;
            }
            if let Some(e) = init {
                subst_expr(e, map, dspan)?;
            }
        }
        Stmt::Assign { target, value, .. } => {
            subst_expr(target, map, dspan)?;
            subst_expr(value, map, dspan)?;
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                subst_expr(e, map, dspan)?;
            }
        }
        Stmt::If { cond, then_body, else_body, .. } => {
            subst_expr(cond, map, dspan)?;
            for s in then_body.iter_mut().chain(else_body.iter_mut().flat_map(|b| b.iter_mut())) {
                subst_stmt(s, map, dspan)?;
            }
        }
        Stmt::While { cond, body, .. } => {
            subst_expr(cond, map, dspan)?;
            for s in body {
                subst_stmt(s, map, dspan)?;
            }
        }
        Stmt::Expr { expr, .. } => subst_expr(expr, map, dspan)?,
        Stmt::Match { scrut, arms, .. } => {
            subst_expr(scrut, map, dspan)?;
            for a in arms {
                for s in &mut a.body {
                    subst_stmt(s, map, dspan)?;
                }
            }
        }
        Stmt::Region { body, .. } => {
            for s in body {
                subst_stmt(s, map, dspan)?;
            }
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
        Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => {
            subst_expr(target, map, dspan)?
        }
        Stmt::Defer { expr, .. } => subst_expr(expr, map, dspan)?,
    }
    Ok(())
}

fn subst_expr(e: &mut Expr, map: &HashMap<String, Ty>, dspan: Span) -> Result<(), (Span, String)> {
    match e {
        Expr::Call(_, targs, args, _) => {
            for t in targs {
                subst_ty(t, map, dspan)?;
            }
            for a in args {
                subst_expr(a, map, dspan)?;
            }
        }
        Expr::StructLit(_, targs, fields, _) => {
            for t in targs {
                subst_ty(t, map, dspan)?;
            }
            for (_, fe) in fields {
                subst_expr(fe, map, dspan)?;
            }
        }
        Expr::Unary(_, i, _) | Expr::Deref(i, _) | Expr::Handle(i, _) | Expr::Join(i, _)
        | Expr::Mutex(i, _) => subst_expr(i, map, dspan)?,
        Expr::Bin(_, a, b, _) => {
            subst_expr(a, map, dspan)?;
            subst_expr(b, map, dspan)?;
        }
        Expr::Field(b, ..) => subst_expr(b, map, dspan)?,
        Expr::Index(b, i, _) => {
            subst_expr(b, map, dspan)?;
            subst_expr(i, map, dspan)?;
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            subst_expr(base, map, dspan)?;
            for e in lo.iter_mut().chain(hi.iter_mut()) {
                subst_expr(e, map, dspan)?;
            }
        }
        Expr::Sizeof(t, _) => subst_ty(t, map, dspan)?,
        Expr::MethodCall { recv, targs, args, .. } => {
            subst_expr(recv, map, dspan)?;
            for t in targs {
                subst_ty(t, map, dspan)?;
            }
            for a in args {
                subst_expr(a, map, dspan)?;
            }
        }
        Expr::AssocFn { ty, args, .. } => {
            subst_ty(ty, map, dspan)?;
            for a in args {
                subst_expr(a, map, dspan)?;
            }
        }
        Expr::AddrOf { place, .. } => subst_expr(place, map, dspan)?,
        Expr::Spawn(body, _) => {
            for s in body {
                subst_stmt(s, map, dspan)?;
            }
        }
        Expr::Alloc { ty, alloc, count, .. } => {
            subst_ty(ty, map, dspan)?;
            subst_expr(alloc, map, dspan)?;
            if let Some(c) = count {
                subst_expr(c, map, dspan)?;
            }
        }
        Expr::EnumLit { ty, payload, .. } => {
            subst_ty(ty, map, dspan)?;
            if let Some(p) = payload {
                subst_expr(p, map, dspan)?;
            }
        }
        Expr::Try { inner, .. } => subst_expr(inner, map, dspan)?,
        Expr::Match { scrut, arms, .. } => {
            subst_expr(scrut, map, dspan)?;
            for a in arms {
                for s in &mut a.body {
                    subst_stmt(s, map, dspan)?;
                }
            }
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) | Expr::Str(..) => {}
    }
    Ok(())
}

// ------------------------------------------------------------------
// Statements — the scope discipline comes from the checker's block
// rules (every block pushes/pops a scope), the resolution walk
// mirrors mono's scan_stmt.
// ------------------------------------------------------------------

fn scan_block(stmts: &[Stmt], x: &mut Scan) {
    x.scopes.push(HashMap::new());
    for s in stmts {
        scan_stmt(s, x);
    }
    x.scopes.pop();
}

fn scan_stmt(s: &Stmt, x: &mut Scan) {
    match s {
        Stmt::Let { name, ty, init, span, name_span } => {
            if let Some(t) = ty {
                check_ty(t, &x.m.v_structs, *span, x.errs);
                tyuse(t, *span, x);
                wants_ty(t, x.who, *span, x.wants);
            }
            let mut ity = None;
            if let Some(e) = init {
                scan_expr(e, x);
                ity = ty_of(e, x.locals, x.tb);
                if let Some(t) = &ity {
                    wants_ty(t, x.who, *span, x.wants);
                }
            }
            let bt = ty.clone().or(ity);
            x.recs.push_str(&format!(
                "      (bind \"{name}\" @{}:{} d{} {})\n",
                name_span.line,
                name_span.col,
                x.scopes.len(),
                bt.as_ref().map(|t| format!("\"{t}\"")).unwrap_or_else(|| "?".into())
            ));
            if let Some(t) = bt {
                x.locals.insert(name.clone(), t);
            }
            x.scopes.last_mut().unwrap().insert(name.clone(), (*name_span, false));
        }
        Stmt::Assign { target, value, .. } => {
            scan_expr(target, x);
            scan_expr(value, x);
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                scan_expr(e, x);
            }
        }
        Stmt::If { cond, then_body, else_body, .. } => {
            scan_expr(cond, x);
            scan_block(then_body, x);
            if let Some(els) = else_body {
                scan_block(els, x);
            }
        }
        Stmt::While { cond, body, .. } => {
            scan_expr(cond, x);
            x.loop_depth += 1;
            scan_block(body, x);
            x.loop_depth -= 1;
        }
        Stmt::Break { span } | Stmt::Continue { span } => {
            let kw = if matches!(s, Stmt::Break { .. }) { "break" } else { "continue" };
            if x.loop_depth == 0 {
                x.errs.push(Diag::new(*span, format!("`{kw}` outside a loop")));
            }
        }
        Stmt::Expr { expr, .. } => scan_expr(expr, x),
        Stmt::Match { scrut, arms, span } => {
            scan_expr(scrut, x);
            let sty = ty_of(scrut, x.locals, x.tb);
            if let Some(t) = &sty {
                wants_ty(t, x.who, *span, x.wants);
            }
            scan_arms(sty, arms, span, x);
        }
        Stmt::Region { body, .. } => scan_block(body, x),
        Stmt::Promote { target, span } => {
            // Name-level promote: the binding moves one scope
            // outward (the region's parent). Ownership itself is
            // the checker's job.
            match target {
                Expr::Var(n, _) => {
                    scan_expr(target, x);
                    let hit = x
                        .scopes
                        .iter()
                        .rev()
                        .find_map(|sc| sc.get(n).copied());
                    match hit {
                        Some(b) if x.scopes.len() >= 2 => {
                            let i = x.scopes.len() - 2;
                            x.scopes[i].insert(n.clone(), b);
                        }
                        None => {
                            x.errs.push(Diag::new(*span, format!("undefined variable `{n}`")));
                        }
                        _ => {}
                    }
                }
                _ => {
                    scan_expr(target, x);
                    x.errs.push(Diag::new(
                        *span,
                        "promote requires a variable".to_string(),
                    ));
                }
            }
        }
        Stmt::Consume { target, .. } => scan_expr(target, x),
        Stmt::Defer { expr, .. } => scan_expr(expr, x),
    }
}

/// Per-arm walk for both stmt and expr `match` forms: variant
/// resolution against the scrutinee's enum (when `ty_of` supplies
/// one), payload binding, body block — and the checker's variant
/// validation when the enum is visible.
fn scan_arms(sty: Option<Ty>, arms: &[MatchArm], mspan: &Span, x: &mut Scan) {
    // Variant validation resolves when the scrutinee's enum is
    // nameable here — concrete pub enums, or a pub/local generic the
    // instantiation made concrete.
    let ename = sty.as_ref().and_then(|t| match t {
        Ty::Struct(n) | Ty::App(n, _)
            if x.m.v_enums.contains(n)
                || enum_named(x.tb, x.gens, n.as_str(), x.who) =>
        {
            Some(n.clone())
        }
        _ => None,
    });
    let variants: Option<Vec<(String, Option<Ty>)>> = ename
        .as_ref()
        .and_then(|n| edecl(x.tb, x.gens, n.as_str()).map(|d| d.variants.clone()));
    let mut covered: HashSet<&str> = HashSet::new();
    let mut wildcard = false;
    for a in arms {
        let is_wild = a.variant == "_";
        if is_wild {
            wildcard = true;
            if a.bind.is_some() {
                x.errs.push(Diag::new(
                    *mspan,
                    "`_` binds no payload".to_string(),
                ));
            }
        }
        let target = match &variants {
            Some(vs) if !is_wild => {
                match vs.iter().find(|(vn, _)| vn == &a.variant) {
                    Some((_, pt)) => {
                        if !covered.insert(a.variant.as_str()) {
                            x.errs.push(Diag::new(
                                *mspan,
                                format!("duplicate match arm `{}`", a.variant),
                            ));
                        }
                        if pt.is_none() && a.bind.is_some() {
                            x.errs.push(Diag::new(
                                *mspan,
                                format!(
                                    "variant `{}` of `{}` carries no payload to bind",
                                    a.variant,
                                    ename.as_ref().unwrap()
                                ),
                            ));
                        }
                        format!("\"{}::{}\"", ename.as_ref().unwrap(), a.variant)
                    }
                    None => {
                        x.errs.push(Diag::new(
                            *mspan,
                            format!(
                                "enum `{}` has no variant `{}`",
                                ename.as_ref().unwrap(),
                                a.variant
                            ),
                        ));
                        "?".to_string()
                    }
                }
            }
            _ if is_wild => "wildcard".to_string(),
            _ => "?".to_string(),
        };
        x.recs.push_str(&format!(
            "      (variant \"{}\" @{}:{} -> {target})\n",
            a.variant, mspan.line, mspan.col
        ));
        x.scopes.push(HashMap::new());
        if let Some(b) = &a.bind {
            let pt = sty
                .as_ref()
                .and_then(|sty| arm_bind_ty(sty, &a.variant, x.tb));
            x.recs.push_str(&format!(
                "      (armbind \"{b}\" @{}:{} d{} {})\n",
                mspan.line,
                mspan.col,
                x.scopes.len(),
                pt.as_ref().map(|t| format!("\"{t}\"")).unwrap_or_else(|| "?".into())
            ));
            if let Some(t) = pt {
                x.locals.insert(b.clone(), t);
            }
            x.scopes.last_mut().unwrap().insert(b.clone(), (*mspan, false));
        }
        for s in &a.body {
            scan_stmt(s, x);
        }
        x.scopes.pop();
        if let Some(b) = &a.bind {
            x.locals.remove(b); // mono's quirk — no shadow restore
        }
    }
    if let Some(vs) = &variants {
        let missing: Vec<&String> = vs
            .iter()
            .map(|(vn, _)| vn)
            .filter(|vn| !covered.contains(vn.as_str()) && vn.as_str() != "_")
            .collect();
        if !missing.is_empty() && !wildcard {
            x.errs.push(Diag::new(
                *mspan,
                format!(
                    "non-exhaustive `match` on `{}` — missing: {}",
                    ename.as_ref().unwrap(),
                    missing
                        .iter()
                        .map(|s| s.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            ));
        }
    }
}

// ------------------------------------------------------------------
// Expressions — resolution records for every name the node mentions.
// ------------------------------------------------------------------

fn scan_expr(e: &Expr, x: &mut Scan) {
    match e {
        Expr::Var(n, s) => {
            let hit = x
                .scopes
                .iter()
                .rev()
                .find_map(|sc| sc.get(n))
                .copied();
            match hit {
                Some((d, is_param)) => x.recs.push_str(&format!(
                    "      (use \"{n}\" @{}:{} -> {} @{}:{})\n",
                    s.line,
                    s.col,
                    if is_param { "param" } else { "local" },
                    d.line,
                    d.col
                )),
                None => {
                    x.recs.push_str(&format!(
                        "      (use \"{n}\" @{}:{} -> ?)\n",
                        s.line, s.col
                    ));
                    x.errs.push(Diag::new(*s, format!("undefined variable `{n}`")));
                }
            }
        }
        Expr::Match { scrut, arms, span } => {
            scan_expr(scrut, x);
            let sty = ty_of(scrut, x.locals, x.tb);
            if let Some(t) = &sty {
                wants_ty(t, x.who, *span, x.wants);
            }
            scan_arms(sty, arms, span, x);
        }
        Expr::Call(n, targs, args, span) => {
            for a in args {
                scan_expr(a, x);
            }
            // `a.b(..)` ambiguity — a bound variable wins and the call
            // is really a method call (mono's rewrite).
            if let Some((pre, meth)) = n.rsplit_once('.') {
                if x.locals.contains_key(pre) {
                    // The receiver reads like any other use.
                    let hit = x
                        .scopes
                        .iter()
                        .rev()
                        .find_map(|sc| sc.get(pre))
                        .copied();
                    if let Some((d, is_param)) = hit {
                        x.recs.push_str(&format!(
                            "      (use \"{pre}\" @{}:{} -> {} @{}:{})\n",
                            span.line,
                            span.col,
                            if is_param { "param" } else { "local" },
                            d.line,
                            d.col
                        ));
                    }
                    let rty = x.locals.get(pre).cloned();
                    resolve_method(
                        rty,
                        meth,
                        targs,
                        args,
                        Some(Expr::Var(pre.to_string(), *span)),
                        span,
                        x,
                    );
                    return;
                }
            }
            resolve_call(n, targs, args, span, x);
        }
        Expr::MethodCall { recv, name, targs, args, span } => {
            scan_expr(recv, x);
            for a in args {
                scan_expr(a, x);
            }
            let rty = ty_of(recv, x.locals, x.tb);
            resolve_method(rty, name, targs, args, Some((**recv).clone()), span, x);
        }
        Expr::AssocFn { ty, name, args, span } => {
            for a in args {
                scan_expr(a, x);
            }
            tyuse(ty, *span, x);
            let (base, tys) = match &**ty {
                Ty::App(b, a) => (b.clone(), a.clone()),
                Ty::Struct(b) => (b.clone(), Vec::new()),
                t => (format!("{t}"), Vec::new()),
            };
            let enum_decl = edecl(x.tb, x.gens, &base);
            if let Some(decl) = enum_decl {
                assoc_enum(decl, &base, name, tys, args, span, x);
                return;
            }
            let qn = match base.rfind('.') {
                Some(i) => format!("{}.{}", &base[..i], name),
                None => name.clone(),
            };
            if !x.tb.fns.contains_key(&qn) && !x.gens.contains_key(&qn) {
                x.recs.push_str(&format!(
                    "      (assoc \"{base}::{name}\" @{}:{} -> ?)\n",
                    span.line, span.col
                ));
                x.errs.push(Diag::new(
                    *span,
                    format!("no associated function `{qn}`"),
                ));
            } else {
                // Desugared `qn(tys…, args…)` — the call arm's rules
                // replay, including visibility on `qn` itself.
                x.recs.push_str(&format!(
                    "      (assoc \"{base}::{name}\" @{}:{} -> fn \"{qn}\")\n",
                    span.line, span.col
                ));
                resolve_call(&qn, &tys, args, span, x);
            }
        }
        Expr::EnumLit { ty, variant, payload, span } => {
            wants_ty(ty, x.who, *span, x.wants);
            tyuse(ty, *span, x);
            let (name, iargs) = match &**ty {
                Ty::Struct(n) => (n.clone(), Vec::new()),
                Ty::App(n, a) => (n.clone(), a.clone()),
                t => (format!("{t}"), Vec::new()),
            };
            enum_lit(&name, &iargs, variant, payload.is_some(), span, x);
            if let Some(p) = payload {
                scan_expr(p, x);
            }
        }
        Expr::Try { inner, span } => {
            if let Some(t) = ty_of(inner, x.locals, x.tb) {
                wants_ty(&t, x.who, *span, x.wants);
            }
            scan_expr(inner, x);
        }
        Expr::StructLit(n, targs, fields, span) => {
            for (_, fe) in fields {
                scan_expr(fe, x);
            }
            // `S<..>{..}` on a template: infer when bare (mono), then
            // either way the instantiation is a want — and the clone
            // is what the field checks resolve against.
            let mut eff_targs: Option<Vec<Ty>> = None;
            if targs.is_empty() {
                if let Some(Gen::S(_, decl)) = x.gens.get(n.as_str()) {
                    let decl = *decl;
                    if x.infer_failed.insert((span.line, span.col)) {
                        match infer_struct_targs(decl, fields, x.locals, x.tb) {
                            Ok(ts) => {
                                call_want(n, &ts, x.who, *span, x.wants);
                                eff_targs = Some(ts);
                            }
                            Err((sp, msg)) => x.errs.push(Diag::new(sp, msg)),
                        }
                    }
                }
            }
            let enum_like = x.m.v_enums.contains(n)
                || x.tb.synth.contains_key(n.as_str())
                || matches!(x.gens.get(n.as_str()), Some(Gen::E(..)));
            if enum_like {
                x.recs.push_str(&format!(
                    "      (use \"{n}\" @{}:{} -> ?)\n",
                    span.line, span.col
                ));
                x.errs.push(Diag::new(
                    *span,
                    format!("`{n}` is an enum — construct a variant: `{n}::V(..)`"),
                ));
            } else {
                let defs: Option<Vec<(String, Ty)>> = if x.m.v_structs.contains(n) {
                    x.tb.structs.get(n).map(|d| d.fields.clone())
                } else {
                    let ga = eff_targs
                        .clone()
                        .or(if targs.is_empty() { None } else { Some(targs.to_vec()) });
                    match (x.gens.get(n.as_str()), ga) {
                        (Some(Gen::S(owner, decl)), Some(ts))
                            if *owner == x.who || decl.is_pub =>
                        {
                            subst_fields(decl, &ts).ok()
                        }
                        _ => None,
                    }
                };
                match defs {
                    Some(defs) => {
                        let rname = match eff_targs.as_ref() {
                            Some(ts) => canon(n, ts),
                            None if !targs.is_empty() => canon(n, targs),
                            None => n.clone(),
                        };
                        x.recs.push_str(&format!(
                            "      (use \"{n}\" @{}:{} -> struct \"{rname}\")\n",
                            span.line, span.col
                        ));
                        lit_fields(&rname, n, &defs, fields, span, x);
                    }
                    None => {
                        let msg = if x.m.private.contains(n.as_str()) {
                            format!("`{n}` is private to its module")
                        } else {
                            format!("unknown struct `{n}`")
                        };
                        x.recs.push_str(&format!(
                            "      (use \"{n}\" @{}:{} -> ?)\n",
                            span.line, span.col
                        ));
                        x.errs.push(Diag::new(*span, msg));
                    }
                }
            }
            for t in targs {
                tyuse(t, *span, x);
            }
            if !targs.is_empty() {
                call_want(n, targs, x.who, *span, x.wants);
            }
        }
        Expr::Field(b, name, s) => {
            scan_expr(b, x);
            x.recs.push_str(&format!(
                "      (member \"{name}\" @{}:{})\n",
                s.line, s.col
            ));
        }
        Expr::Index(b, i, _) => {
            scan_expr(b, x);
            scan_expr(i, x);
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            scan_expr(base, x);
            for e in lo.iter().chain(hi.iter()) {
                scan_expr(e, x);
            }
        }
        Expr::Unary(_, i, _) | Expr::Deref(i, _) | Expr::Handle(i, _) | Expr::Join(i, _)
        | Expr::Mutex(i, _) => scan_expr(i, x),
        Expr::Bin(_, a, b, _) => {
            scan_expr(a, x);
            scan_expr(b, x);
        }
        Expr::AddrOf { place, .. } => scan_expr(place, x),
        Expr::Spawn(body, _) => {
            let saved = x.loop_depth;
            x.loop_depth = 0;
            scan_block(body, x);
            x.loop_depth = saved;
        }
        Expr::Alloc { ty, alloc, count, span } => {
            wants_ty(ty, x.who, *span, x.wants);
            tyuse(ty, *span, x);
            scan_expr(alloc, x);
            if let Some(c) = count {
                scan_expr(c, x);
            }
        }
        Expr::Sizeof(t, span) => {
            wants_ty(t, x.who, *span, x.wants);
            tyuse(t, *span, x);
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Str(..) => {}
    }
}

/// Ordinary-call resolution — the path mono's desugars funnel back
/// into and the checker's call dispatch: generic template → builtin
/// gate → visible fns → private/undefined. Emits the `use` record
/// and the instantiation want (explicit or inferred targs).
/// `infer_args` are the arguments inference sees — method replays
/// prepend the receiver like mono's rewritten `Call`.
fn resolve_call(n: &str, targs: &[Ty], infer_args: &[Expr], span: &Span, x: &mut Scan) {
    if let Some(Gen::F(_, decl)) = x.gens.get(n) {
        let decl = *decl;
        if targs.is_empty() {
            if x.infer_failed.insert((span.line, span.col)) {
                match infer_targs(decl, infer_args, x.locals, x.tb) {
                    Ok(ts) => {
                        x.recs.push_str(&format!(
                            "      (use \"{n}\" @{}:{} -> generic \"{}\" infer ({}))\n",
                            span.line,
                            span.col,
                            canon(n, &ts),
                            ts.iter().map(|t| format!("\"{t}\"")).collect::<Vec<_>>().join(" ")
                        ));
                        call_want(n, &ts, x.who, *span, x.wants);
                    }
                    Err((sp, msg)) => {
                        x.recs.push_str(&format!(
                            "      (use \"{n}\" @{}:{} -> generic \"{n}\" ?)\n",
                            span.line, span.col
                        ));
                        x.errs.push(Diag::new(sp, msg));
                    }
                }
            } else {
                x.recs.push_str(&format!(
                    "      (use \"{n}\" @{}:{} -> generic \"{n}\")\n",
                    span.line, span.col
                ));
            }
        } else {
            x.recs.push_str(&format!(
                "      (use \"{n}\" @{}:{} -> generic \"{}\")\n",
                span.line,
                span.col,
                canon(n, targs)
            ));
            call_want(n, targs, x.who, *span, x.wants);
        }
        return;
    }
    if !n.contains('.') && builtin_name(n) && !x.m.local_fns.contains(n) {
        x.recs.push_str(&format!(
            "      (use \"{n}\" @{}:{} -> builtin)\n",
            span.line, span.col
        ));
    } else if let Some(sig) = x.m.v_fns.get(n) {
        let tgt = if sig.2 {
            format!("extern \"{n}\"")
        } else {
            format!("fn \"{n}\"")
        };
        x.recs.push_str(&format!(
            "      (use \"{n}\" @{}:{} -> {tgt})\n",
            span.line, span.col
        ));
    } else {
        let msg = if x.m.private.contains(n) {
            format!("`{n}` is private to its module")
        } else {
            format!("undefined function `{n}`")
        };
        x.recs.push_str(&format!(
            "      (use \"{n}\" @{}:{} -> ?)\n",
            span.line, span.col
        ));
        x.errs.push(Diag::new(*span, msg));
    }
    if !targs.is_empty() {
        call_want(n, targs, x.who, *span, x.wants);
    }
}

/// `v.m(..)` — resolved like mono: receiver type via `ty_of`, the
/// target fn is `<mod>.<name>` in the receiver type's module, and it
/// must take `self` first. `pre` is the bound-variable form's root
/// (`v.push(..)`) — inference sees it as arg 0 like the desugared
/// `Call` mono rebuilds.
fn resolve_method(
    rty: Option<Ty>,
    name: &str,
    targs: &[Ty],
    args: &[Expr],
    recv: Option<Expr>,
    span: &Span,
    x: &mut Scan,
) {
    let rname = rty.as_ref().map_or("?".to_string(), |t| format!("{t}"));
    let out = rty.as_ref().and_then(|t| method_target(t, name)).map(|n| {
        let decl = x
            .tb
            .fns
            .get(&n)
            .copied()
            .or_else(|| match x.gens.get(&n) {
                Some(Gen::F(_, d)) => Some(*d),
                _ => None,
            });
        match decl {
            Some(d)
                if d
                    .params
                    .first()
                    .map(|(pn, _, _)| pn == "self")
                    .unwrap_or(false) =>
            {
                Ok((n, d))
            }
            Some(_) => Err(format!(
                "`{name}` is not a method — `{n}` takes no `self` receiver"
            )),
            None => Err(format!("no method `{name}` on `{rname}`")),
        }
    });
    match out {
        Some(Ok((qn, _))) => {
            x.recs.push_str(&format!(
                "      (method \"{name}\" @{}:{} -> fn \"{qn}\")\n",
                span.line, span.col
            ));
            // Re-scanned as `qn(targs…, recv, args…)` — inference sees
            // the receiver as argument 0.
            let mut all: Vec<Expr> = Vec::with_capacity(args.len() + 1);
            if let Some(rv) = recv {
                all.push(rv);
            }
            all.extend(args.iter().cloned());
            resolve_call(&qn, targs, &all, span, x);
        }
        _ => {
            x.recs.push_str(&format!(
                "      (method \"{name}\" @{}:{} -> ?)\n",
                span.line, span.col
            ));
            if x.infer_failed.insert((span.line, span.col)) {
                match out {
                    Some(Err(msg)) => x.errs.push(Diag::new(*span, msg)),
                    _ => x.errs.push(Diag::new(
                        *span,
                        format!("no method `{name}` on `{rname}`"),
                    )),
                }
            }
        }
    }
}

/// `T::f(..)` / `T<..>::f(..)` when `T` is an enum: variant
/// construction — validates the variant exists, payload arity, and
/// infers missing targs from the payload's type like mono.
/// `T::V` / `T::V(p)` in expression position — the enum branch of
/// `AssocFn` and bare `EnumLit` nodes alike. Variant existence,
/// payload arity, generic payload inference (mono's rules), then the
/// checker's visibility law: the enum must be nameable from this
/// module or the diagnostic is "private"/"unknown enum".
fn assoc_enum(
    decl: &EnumDecl,
    base: &str,
    name: &str,
    mut tys: Vec<Ty>,
    args: &[Expr],
    span: &Span,
    x: &mut Scan,
) {
    let tps: HashSet<String> = decl.tparams.iter().cloned().collect();
    let vpos = decl.variants.iter().position(|(vn, _)| vn == name);
    match vpos {
        None => {
            x.recs.push_str(&format!(
                "      (assoc \"{base}::{name}\" @{}:{} -> ?)\n",
                span.line, span.col
            ));
            x.errs.push(Diag::new(
                *span,
                format!("enum `{base}` has no variant `{name}`"),
            ));
        }
        Some(i) => {
            let wants_payload = decl.variants[i].1.is_some();
            let mut map: HashMap<String, Ty> = HashMap::new();
            if tys.is_empty() && !tps.is_empty() {
                if let (Some(templ), Some(a)) = (&decl.variants[i].1, args.first()) {
                    if let Some(at) = ty_of(a, x.locals, x.tb) {
                        match_ty(templ, &at, &tps, &mut map);
                    }
                }
                for tp in &decl.tparams {
                    if !map.contains_key(tp) {
                        x.errs.push(Diag::new(
                            *span,
                            format!("cannot infer `{tp}` for `{base}::{name}`"),
                        ));
                    }
                }
            }
            if tys.is_empty() {
                tys = decl
                    .tparams
                    .iter()
                    .map(|tp| map.get(tp).cloned().unwrap_or(Ty::Struct(tp.clone())))
                    .collect();
            }
            if args.len() != wants_payload as usize {
                x.recs.push_str(&format!(
                    "      (assoc \"{base}::{name}\" @{}:{} -> ?)\n",
                    span.line, span.col
                ));
                x.errs.push(Diag::new(
                    *span,
                    format!(
                        "variant `{name}` takes {} argument(s), found {}",
                        wants_payload as usize,
                        args.len()
                    ),
                ));
            } else {
                x.recs.push_str(&format!(
                    "      (assoc \"{base}::{name}\" @{}:{} -> variant \"{base}::{name}\")\n",
                    span.line, span.col
                ));
                enum_visible(base, span, x);
                let lit_ty = if tys.is_empty() {
                    Ty::Struct(base.to_string())
                } else {
                    Ty::App(base.to_string(), tys)
                };
                wants_ty(&lit_ty, x.who, *span, x.wants);
            }
        }
    }
}

/// The checker's EnumLit visibility law: nameable enums are the
/// pub/concrete ones plus generals the requester may instantiate;
/// misses spell "private" or "unknown enum".
fn enum_visible(name: &str, span: &Span, x: &mut Scan) {
    let ok = x.m.v_enums.contains(name)
        || x.tb.synth.contains_key(name)
        || match x.gens.get(name) {
            Some(Gen::E(owner, decl)) => *owner == x.who || decl.is_pub,
            _ => false,
        };
    if !ok {
        let msg = if x.m.private.contains(name) {
            format!("`{name}` is private to its module")
        } else {
            format!("unknown enum `{name}`")
        };
        x.errs.push(Diag::new(*span, msg));
    }
}

/// A bare `T::V` literal (`Expr::EnumLit`) — variant resolution plus
/// the payload-arity pair from the checker's EnumLit arm.
fn enum_lit(
    name: &str,
    iargs: &[Ty],
    variant: &str,
    has_payload: bool,
    span: &Span,
    x: &mut Scan,
) {
    let decl = edecl(x.tb, x.gens, name);
    let Some(decl) = decl else {
        x.recs.push_str(&format!(
            "      (variant \"{variant}\" @{}:{} -> ?)\n",
            span.line, span.col
        ));
        let msg = if x.m.private.contains(name) {
            format!("`{name}` is private to its module")
        } else {
            format!("unknown enum `{name}`")
        };
        x.errs.push(Diag::new(*span, msg));
        return;
    };
    enum_visible(name, span, x);
    let canon_name = if iargs.is_empty() {
        name.to_string()
    } else {
        canon(name, iargs)
    };
    match decl.variants.iter().find(|(vn, _)| vn == variant) {
        None => {
            x.recs.push_str(&format!(
                "      (variant \"{variant}\" @{}:{} -> ?)\n",
                span.line, span.col
            ));
            x.errs.push(Diag::new(
                *span,
                format!("enum `{name}` has no variant `{variant}`"),
            ));
        }
        Some((_, want)) => {
            x.recs.push_str(&format!(
                "      (variant \"{variant}\" @{}:{} -> \"{canon_name}::{variant}\")\n",
                span.line, span.col
            ));
            match (want, has_payload) {
                (Some(_), false) => x.errs.push(Diag::new(
                    *span,
                    format!("variant `{variant}` of `{name}` takes a payload"),
                )),
                (None, true) => x.errs.push(Diag::new(
                    *span,
                    format!("variant `{variant}` of `{name}` takes no payload"),
                )),
                _ => {}
            }
        }
    }
}

/// Struct-literal field names — the checker's duplicate/no-field/
/// missing checks, plus a `member` record resolving each name to its
/// declaration. `rname` is the literal's resolved (possibly
/// canonical) struct name; `n` is the name as written for diag text.
fn lit_fields(
    rname: &str,
    n: &str,
    defs: &[(String, Ty)],
    fields: &[(String, Expr)],
    span: &Span,
    x: &mut Scan,
) {
    let mut seen = HashSet::new();
    let mut missing: Vec<&String> = defs.iter().map(|(dn, _)| dn).collect();
    for (fn_, fe) in fields {
        if !seen.insert(fn_) {
            x.errs.push(Diag::new(
                fe.span(),
                format!("field `{fn_}` initialized twice in `{n}`"),
            ));
        }
        match missing.iter().position(|dn| *dn == fn_) {
            Some(i) => {
                missing.remove(i);
                x.recs.push_str(&format!(
                    "      (member \"{fn_}\" @{}:{} -> field \"{rname}.{fn_}\")\n",
                    fe.span().line,
                    fe.span().col
                ));
            }
            None => {
                x.recs.push_str(&format!(
                    "      (member \"{fn_}\" @{}:{} -> ?)\n",
                    fe.span().line,
                    fe.span().col
                ));
                x.errs.push(Diag::new(
                    fe.span(),
                    format!("no field `{fn_}` on struct `{n}`"),
                ));
            }
        }
    }
    for dn in missing {
        x.errs.push(Diag::new(
            *span,
            format!("missing field `{dn}` in `{n}` literal"),
        ));
    }
}

fn call_want(base: &str, targs: &[Ty], who: usize, span: Span, out: &mut Vec<Want>) {
    let _ = who;
    out.push(Want {
        base: base.to_string(),
        args: targs.to_vec(),
        span,
    });
    wants_targs(targs, who, span, out);
}

fn wants_targs(targs: &[Ty], who: usize, span: Span, out: &mut Vec<Want>) {
    for t in targs {
        wants_ty(t, who, span, out);
    }
}

fn wants_ty(t: &Ty, who: usize, span: Span, out: &mut Vec<Want>) {
    match t {
        Ty::App(base, args) => {
            out.push(Want {
                base: base.clone(),
                args: args.clone(),
                span,
            });
            for a in args {
                wants_ty(a, who, span, out);
            }
        }
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::BChannel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => wants_ty(e, who, span, out),
        _ => {}
    }
}

/// A type-syntax node's name-resolution record — `Struct`/`App` bases
/// resolve against visible types (or the enclosing decl's tparams,
/// which the scan tracks per-fn? — params aren't tracked here;
/// `check_ty` already flags genuinely unknown names).
fn tyuse(t: &Ty, span: Span, x: &mut Scan) {
    match t {
        Ty::Struct(n) => {
            let tgt = if x.m.v_structs.contains(n) {
                if x.m.v_enums.contains(n) {
                    format!("enum \"{n}\"")
                } else {
                    format!("struct \"{n}\"")
                }
            } else {
                "?".to_string()
            };
            x.recs.push_str(&format!(
                "      (tyuse \"{n}\" @{}:{} -> {tgt})\n",
                span.line, span.col
            ));
        }
        Ty::App(n, args) => {
            let tgt = if x.gens.contains_key(n) || x.tb.synth.contains_key(n) {
                format!("generic \"{n}\"")
            } else if x.m.v_structs.contains(n) {
                format!("struct \"{n}\"")
            } else {
                "?".to_string()
            };
            x.recs.push_str(&format!(
                "      (tyuse \"{n}\" @{}:{} -> {tgt})\n",
                span.line, span.col
            ));
            for a in args {
                tyuse(a, span, x);
            }
        }
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::BChannel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => tyuse(e, span, x),
        _ => {}
    }
}

// ------------------------------------------------------------------
// `ty_of` — mono's best-effort mini type evaluator. Declared types
// flow through `locals`; everything else is structural.
// ------------------------------------------------------------------

fn ty_of(e: &Expr, locals: &HashMap<String, Ty>, tb: &Tables) -> Option<Ty> {
    match e {
        Expr::Int(..) => Some(Ty::I32),
        Expr::Bool(..) => Some(Ty::Bool),
        Expr::Str(..) => Some(Ty::Str),
        Expr::Var(n, _) => locals.get(n).cloned(),
        Expr::EnumLit { ty, .. } => Some((**ty).clone()),
        Expr::Try { inner, .. } => {
            let t = ty_of(inner, locals, tb)?;
            let (base, args) = match &t {
                Ty::App(b, a) => (b.clone(), a.clone()),
                Ty::Struct(n) => (n.clone(), Vec::new()),
                _ => return None,
            };
            let decl = tb.enums.get(&base)?;
            let (tps, vs) = (decl.tparams.clone(), decl.variants.clone());
            let pt = vs.iter().find(|(vn, _)| vn == "Ok")?.1.clone()?;
            if tps.is_empty() || args.is_empty() {
                return Some(pt);
            }
            let map: HashMap<String, Ty> = tps
                .iter()
                .cloned()
                .zip(args.iter().cloned())
                .collect();
            let mut out = pt;
            subst_ty(&mut out, &map, Span { line: 0, col: 0 }).ok()?;
            Some(out)
        }
        Expr::StructLit(n, targs, fields, ..) => Some(if !targs.is_empty() {
            Ty::App(n.clone(), targs.clone())
        } else {
            // Mono mutates the literal to carry inferred targs before
            // `ty_of` runs; the resolver re-infers on demand.
            match tb.structs.get(n).filter(|d| !d.tparams.is_empty()) {
                Some(d) => infer_struct_targs(d, fields, locals, tb)
                    .map(|ts| Ty::App(n.clone(), ts))
                    .unwrap_or_else(|_| Ty::Struct(n.clone())),
                None => Ty::Struct(n.clone()),
            }
        }),
        Expr::Call(n, targs, args, _) => {
            // The `v.m(..)` sugar — mono rewrote it before typing, so
            // here the same node resolves as the method's desugared
            // `Call(qn, targs, v, args..)`.
            if let Some((pre, meth)) = n.rsplit_once('.') {
                if let Some(rt) = locals.get(pre) {
                    let qn = method_target(rt, meth)?;
                    let mut all = Vec::with_capacity(args.len() + 1);
                    all.push(Expr::Var(pre.to_string(), Span { line: 0, col: 0 }));
                    all.extend(args.iter().cloned());
                    return call_ret(&qn, targs, &all, locals, tb);
                }
            }
            call_ret(n, targs, args, locals, tb)
        }
        Expr::MethodCall { recv, name, targs, args, .. } => {
            let rty = ty_of(recv, locals, tb)?;
            let qn = method_target(&rty, name)?;
            let mut all = Vec::with_capacity(args.len() + 1);
            all.push((**recv).clone());
            all.extend(args.iter().cloned());
            call_ret(&qn, targs, &all, locals, tb)
        }
        Expr::AssocFn { ty, name, args, .. } => {
            let (base, tys) = match &**ty {
                Ty::App(b, a) => (b.clone(), a.clone()),
                Ty::Struct(b) => (b.clone(), Vec::new()),
                _ => return None,
            };
            if tb.enums.contains_key(&base) || tb.synth.contains_key(&base) {
                return Some(if tys.is_empty() {
                    Ty::Struct(base)
                } else {
                    Ty::App(base, tys)
                });
            }
            let qn = match base.rfind('.') {
                Some(i) => format!("{}.{}", &base[..i], name),
                None => name.clone(),
            };
            call_ret(&qn, &tys, args, locals, tb)
        }
        Expr::Field(b, f, _) => field_ty(&ty_of(b, locals, tb)?, f, tb),
        Expr::Index(b, ..) | Expr::IndexRange { base: b, .. } => {
            match ty_of(b, locals, tb)? {
                Ty::Array(e, _) | Ty::Slice { elem: e, .. } | Ty::BoxSlice(e) => Some(*e),
                _ => None,
            }
        }
        Expr::Deref(i, _) => match ty_of(i, locals, tb)? {
            Ty::Ref { elem, .. }
            | Ty::Ptr { elem, .. }
            | Ty::Box(elem)
            | Ty::Handle(elem)
            | Ty::Task(elem)
            | Ty::Guard(elem) => Some(*elem),
            other => Some(other),
        },
        Expr::AddrOf { mutable, place, .. } => Some(Ty::Ref {
            mutable: *mutable,
            elem: Box::new(ty_of(place, locals, tb)?),
        }),
        Expr::Handle(i, _) => Some(Ty::Handle(Box::new(ty_of(i, locals, tb)?))),
        Expr::Mutex(i, _) => Some(Ty::Mutex(Box::new(ty_of(i, locals, tb)?))),
        Expr::Alloc { ty, count, .. } => Some(if count.is_some() {
            Ty::BoxSlice(ty.clone())
        } else {
            Ty::Box(ty.clone())
        }),
        Expr::Sizeof(..) => Some(Ty::U64),
        Expr::Unary(_, i, _) => ty_of(i, locals, tb),
        Expr::Bin(op, a, _, _) => {
            if op.is_cmp() {
                Some(Ty::Bool)
            } else {
                ty_of(a, locals, tb)
            }
        }
        Expr::Join(i, _) => match ty_of(i, locals, tb)? {
            Ty::Task(e) => Some(*e),
            _ => None,
        },
        Expr::Match { scrut, arms, .. } => {
            let sty = ty_of(scrut, locals, tb);
            arms.iter().find(|a| a.expr_arm).and_then(|a| {
                let mut ovl = locals.clone();
                if let (Some(b), Some(sty)) = (&a.bind, &sty) {
                    if let Some(pt) = arm_bind_ty(sty, &a.variant, tb) {
                        ovl.insert(b.clone(), pt);
                    }
                }
                let mut ity = None;
                for s in &a.body {
                    match s {
                        Stmt::Let { name, ty, init, .. } => {
                            let t = ty.clone().or_else(|| {
                                init.as_ref().and_then(|e| ty_of(e, &ovl, tb))
                            });
                            if let Some(t) = t {
                                ovl.insert(name.clone(), t);
                            }
                        }
                        Stmt::Expr { expr, .. } => ity = ty_of(expr, &ovl, tb),
                        _ => {}
                    }
                }
                ity
            })
        }
        Expr::Spawn(..) => None,
    }
}

/// The return type a `Call(n, targs, args)` yields — builtin
/// special-cases first, then the fn table, then generic inference +
/// substituted return (mono's `ty_of` Call arm).
fn call_ret(
    n: &str,
    targs: &[Ty],
    args: &[Expr],
    locals: &HashMap<String, Ty>,
    tb: &Tables,
) -> Option<Ty> {
    if let Some(t) = crate::semantics::check::int_cast_ty(n) {
        return Some(t);
    }
    match n {
        "system_allocator" => return Some(Ty::Alloc),
        "str_from_utf8" => return Some(Ty::Str),
        "channel" => return Some(Ty::Channel(Box::new(Ty::Void))),
        "bchannel" => return Some(Ty::BChannel(Box::new(Ty::Void))),
        "recv" => match ty_of(args.first()?, locals, tb)? {
            Ty::Channel(e) => return Some(*e),
            // CHAN-004 S3b — bounded recv yields `option.Option<T>`.
            Ty::BChannel(e) => {
                return Some(Ty::App("option.Option".to_string(), vec![*e]))
            }
            _ => return None,
        },
        // CHAN-004 S3b — `send` on a `bchannel T` yields
        // `result.Result<void,T>`; on a `channel` it stays void and
        // the fn-table miss below returns `?` like before.
        "send" => match ty_of(args.first()?, locals, tb)? {
            Ty::BChannel(e) => {
                return Some(Ty::App(
                    "result.Result".to_string(),
                    vec![Ty::Void, *e],
                ))
            }
            _ => return None,
        },
        "lock" => match ty_of(args.first()?, locals, tb)? {
            Ty::Mutex(i) => return Some(Ty::Guard(i)),
            other => return Some(Ty::Guard(Box::new(other))),
        },
        "replace" if !tb.fns.contains_key(n) => {
            return ty_of(args.first()?, locals, tb)
        }
        _ => {}
    }
    let d = tb.fns.get(n)?;
    if d.tparams.is_empty() {
        return Some(d.ret.clone());
    }
    let ts = if targs.is_empty() {
        infer_targs(d, args, locals, tb).ok()?
    } else {
        targs.to_vec()
    };
    let map: HashMap<String, Ty> = d.tparams.iter().cloned().zip(ts).collect();
    let mut r = d.ret.clone();
    let _ = subst_ty(&mut r, &map, Span { line: 0, col: 0 });
    Some(r)
}

/// Declared field ty of `f` on struct `t` — generic field tys
/// substitute under the application's args (mono's `field_ty`).
fn field_ty(t: &Ty, f: &str, tb: &Tables) -> Option<Ty> {
    if let Ty::Guard(i) = t {
        return field_ty(i, f, tb);
    }
    let (n, args) = match t {
        Ty::Struct(n) => (n, &[][..]),
        Ty::App(n, a) => (n, &a[..]),
        _ => return None,
    };
    let d = tb.structs.get(n)?;
    let (_, ft) = d.fields.iter().find(|(fn_, _)| fn_ == f).cloned()?;
    let mut ft = ft.clone();
    if !d.tparams.is_empty() {
        let map: HashMap<String, Ty> =
            d.tparams.iter().cloned().zip(args.iter().cloned()).collect();
        let _ = subst_ty(&mut ft, &map, Span { line: 0, col: 0 });
    }
    Some(ft)
}

/// `recv.m(..)` → `<mod>.<name>` where `mod` is the receiver type's
/// module — `vec.Vec<i32>` → `vec.m`, a root-local `Pair` → bare `m`.
fn method_target(t: &Ty, name: &str) -> Option<String> {
    let base = match t {
        Ty::Struct(n) | Ty::App(n, _) => n.as_str(),
        _ => return None,
    };
    Some(match base.rfind('.') {
        Some(i) => format!("{}.{}", &base[..i], name),
        None => name.to_string(),
    })
}

/// The payload ty an arm binder gets on a scrutinee of enum `sty` —
/// generic payloads substitute under the instantiation's args.
fn arm_bind_ty(sty: &Ty, variant: &str, tb: &Tables) -> Option<Ty> {
    let (base, iargs) = match sty {
        Ty::Struct(n) => (n.clone(), Vec::new()),
        Ty::App(n, targs) => (n.clone(), targs.clone()),
        _ => return None,
    };
    let d = tb.synth.get(&base).or_else(|| tb.enums.get(&base).copied())?;
    let raw = d
        .variants
        .iter()
        .find(|(vn, _)| vn == &variant)
        .and_then(|(_, t)| t.clone())?;
    if d.tparams.is_empty() || iargs.is_empty() {
        return Some(raw);
    }
    let map: HashMap<String, Ty> = d
        .tparams
        .iter()
        .cloned()
        .zip(iargs.iter().cloned())
        .collect();
    let mut out = raw;
    subst_ty(&mut out, &map, Span { line: 0, col: 0 }).ok()?;
    Some(out)
}

/// Bind template tparams (bare `Struct("T")`) against a concrete arg
/// — structural recursion, false on shape mismatch or rebind.
fn match_ty(templ: &Ty, arg: &Ty, tps: &HashSet<String>, map: &mut HashMap<String, Ty>) -> bool {
    use Ty::*;
    match templ {
        Struct(n) if tps.contains(n) => match map.get(n) {
            Some(b) => b == arg,
            None => {
                map.insert(n.clone(), arg.clone());
                true
            }
        },
        App(b, ts) => match arg {
            App(b2, as_) if b == b2 && ts.len() == as_.len() => ts
                .iter()
                .zip(as_.iter())
                .all(|(x, y)| match_ty(x, y, tps, map)),
            _ => false,
        },
        Array(e, n) => matches!(arg, Array(e2, n2) if n == n2 && match_ty(e, e2, tps, map)),
        Ref { mutable: m, elem: e } => {
            matches!(arg, Ref { mutable: m2, elem: e2 } if m == m2 && match_ty(e, e2, tps, map))
        }
        Ptr { mutable: m, elem: e } => {
            matches!(arg, Ptr { mutable: m2, elem: e2 } if m == m2 && match_ty(e, e2, tps, map))
        }
        Slice { mutable: m, elem: e } => {
            matches!(arg, Slice { mutable: m2, elem: e2 } if m == m2 && match_ty(e, e2, tps, map))
        }
        Handle(e) => matches!(arg, Handle(e2) if match_ty(e, e2, tps, map)),
        Task(e) => matches!(arg, Task(e2) if match_ty(e, e2, tps, map)),
        Atomic(e) => matches!(arg, Atomic(e2) if match_ty(e, e2, tps, map)),
        Mutex(e) => matches!(arg, Mutex(e2) if match_ty(e, e2, tps, map)),
        Guard(e) => matches!(arg, Guard(e2) if match_ty(e, e2, tps, map)),
        Channel(e) => matches!(arg, Channel(e2) if match_ty(e, e2, tps, map)),
        Box(e) => matches!(arg, Box(e2) | BoxSlice(e2) if match_ty(e, e2, tps, map)),
        BoxSlice(e) => matches!(arg, BoxSlice(e2) if match_ty(e, e2, tps, map)),
        _ => templ == arg,
    }
}

/// Infer `f<..>`'s targs positionally from declared param tys vs
/// argument exprs. Every tparam must end bound.
fn infer_targs(
    decl: &FnDef,
    args: &[Expr],
    locals: &HashMap<String, Ty>,
    tb: &Tables,
) -> Result<Vec<Ty>, (Span, String)> {
    let tps: HashSet<String> = decl.tparams.iter().cloned().collect();
    let mut map: HashMap<String, Ty> = HashMap::new();
    for ((_, _, pt), a) in decl.params.iter().zip(args.iter()) {
        let Some(got) = ty_of(a, locals, tb) else {
            return Err((
                a.span(),
                format!(
                    "cannot infer `{}`'s type arguments — argument type not statically known; instantiate explicitly",
                    decl.name
                ),
            ));
        };
        if !match_ty(pt, &got, &tps, &mut map) {
            let msg = match pt {
                Ty::Struct(n) if tps.contains(n) && map.contains_key(n) => format!(
                    "type argument `{n}` for `{}` inferred as both `{}` and `{got}`",
                    decl.name, map[n]
                ),
                _ => format!(
                    "cannot infer `{}`'s type arguments — parameter `{pt}` vs argument `{got}`",
                    decl.name
                ),
            };
            return Err((a.span(), msg));
        }
    }
    decl.tparams
        .iter()
        .map(|p| {
            map.get(p).cloned().ok_or_else(|| {
                (
                    decl.span,
                    format!("cannot infer type argument `{p}` for `{}`", decl.name),
                )
            })
        })
        .collect()
}

/// Infer a generic struct literal's targs from its field values.
fn infer_struct_targs(
    decl: &StructDecl,
    fields: &[(String, Expr)],
    locals: &HashMap<String, Ty>,
    tb: &Tables,
) -> Result<Vec<Ty>, (Span, String)> {
    let tps: HashSet<String> = decl.tparams.iter().cloned().collect();
    let mut map: HashMap<String, Ty> = HashMap::new();
    for (fn_, fe) in fields {
        let Some((_, ft)) = decl.fields.iter().find(|(df, _)| df == fn_) else {
            continue;
        };
        let Some(got) = ty_of(fe, locals, tb) else {
            continue;
        };
        if !match_ty(ft, &got, &tps, &mut map) {
            return Err((
                fe.span(),
                format!(
                    "cannot infer `{}`'s type arguments — field `{fn_}` is `{ft}` vs `{got}`",
                    decl.name
                ),
            ));
        }
    }
    decl.tparams
        .iter()
        .map(|p| {
            map.get(p).cloned().ok_or_else(|| {
                (
                    decl.span,
                    format!("cannot infer type argument `{p}` for `{}`", decl.name),
                )
            })
        })
        .collect()
}
