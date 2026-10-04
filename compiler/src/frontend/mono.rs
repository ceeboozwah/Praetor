//! N19 — monomorphization as AST expansion, run after qualification
//! and before checking.
//!
//! Generic decls (`struct Vec<T>`, `fn id<T>`) are templates: the
//! checker and codegen never see them. This pass scans every CONCRETE
//! decl for instantiation sites —
//!   `Ty::App`           in any type position   (let v: Vec<i32>)
//!   `Call.targs`        on calls               (id<i32>(x))
//!   `StructLit.targs`   on struct literals     (Vec<i32>{..})
//! — clones the template with `T -> arg` substitution, and names the
//! clone canonically (`vec.Vec<i32>`). The clone lands in the
//! REQUESTER's program: instantiation means "as if the requester wrote
//! this code with these types", so arg layouts resolve locally,
//! requester-private types work as args, and the clone's own callees
//! must be importable (pub, or same-module when requester == owner).
//! Two modules asking for `vec.Vec<i32>` get two identical clones —
//! `merge_module` dedupes emitted symbols by name.
//! Iterate to a fixpoint: a clone's body may instantiate further
//! generics (`vec_push<T>` calls `vec_grow<T>`). Afterwards every
//! App/targs is normalized into its canonical name — downstream stages
//! see only `Struct("m.Vec<i32>")` and `Call("m.f<i32>")` keys,
//! exactly like N17's qualified names.

use super::ast::*;
use crate::{Diag, Span};
use std::collections::{HashMap, HashSet};

/// The canonical symbol an instantiation occupies — also the LLVM name
/// (sanitized at emit). `Vec<i32>` -> `vec.Vec<i32>`; args render via
/// `Display`, which is already the type's canonical spelling.
fn canon(base: &str, args: &[Ty]) -> String {
    let inner: Vec<String> = args.iter().map(|a| format!("{a}")).collect();
    format!("{base}<{}>", inner.join(","))
}

enum Gen {
    F(usize, FnDef),
    S(usize, StructDecl),
    E(usize, EnumDecl),
}

/// Instantiation request discovered while scanning a concrete decl.
struct Want {
    /// Which program asked — private generics can't cross modules.
    requester: usize,
    base: String,
    args: Vec<Ty>,
    span: Span,
}

/// Whole-program decl tables for the N20 mini type-evaluator —
/// generic and concrete decls alike, keyed by qualified name.
struct Tables {
    fns: HashMap<String, FnDef>,
    structs: HashMap<String, StructDecl>,
    enums: HashMap<String, EnumDecl>,
}

/// Instantiate every generic application used across `progs`
/// (`(filename, program)` — the root's symbols are unqualified).
/// Returns diagnostics with `.file` set for module attribution.
pub fn monomorphize(progs: &mut [(String, &mut Program)]) -> Vec<Diag> {
    let mut errs: Vec<Diag> = Vec::new();

    // Registry: qualified base name -> template. Concrete names are
    // tracked too, so `f<i32>` on a non-generic reports precisely.
    // `tables` mirrors every decl (generic or not) — inference and
    // method desugaring need ret/field types without touching `progs`.
    let mut gens: HashMap<String, Gen> = HashMap::new();
    let mut concrete: HashSet<String> = HashSet::new();
    let mut tables = Tables {
        fns: HashMap::new(),
        structs: HashMap::new(),
        enums: HashMap::new(),
    };
    for (i, (_, p)) in progs.iter().enumerate() {
        for s in &p.structs {
            tables.structs.insert(s.name.clone(), s.clone());
            if s.tparams.is_empty() {
                concrete.insert(s.name.clone());
            } else if gens
                .insert(s.name.clone(), Gen::S(i, s.clone()))
                .is_some()
            {
                errs.push(diag(
                    progs[i].0.clone(),
                    s.span,
                    format!("duplicate generic declaration `{}`", s.name),
                ));
            }
        }
        for e in &p.enums {
            tables.enums.insert(e.name.clone(), e.clone());
            if e.tparams.is_empty() {
                concrete.insert(e.name.clone());
            } else if gens
                .insert(e.name.clone(), Gen::E(i, e.clone()))
                .is_some()
            {
                errs.push(diag(
                    progs[i].0.clone(),
                    e.span,
                    format!("duplicate generic declaration `{}`", e.name),
                ));
            }
        }
        for f in &p.fns {
            tables.fns.insert(f.name.clone(), f.clone());
            if f.tparams.is_empty() {
                concrete.insert(f.name.clone());
            } else if gens
                .insert(f.name.clone(), Gen::F(i, f.clone()))
                .is_some()
            {
                errs.push(diag(
                    progs[i].0.clone(),
                    f.span,
                    format!("duplicate generic declaration `{}`", f.name),
                ));
            }
        }
    }

    // Fixpoint: each pass scans concrete decls for wants; a fresh
    // want clones the template into its requester's program — and that
    // clone is itself concrete, so next pass finds its own wants.
    // `attempted` dedupes diagnostics on keys that never instantiate
    // (rejected/unknown) — `made` holds only real instantiations.
    // Both key on (requester, canon): each module gets its own clone,
    // since the instantiation must resolve in ITS import context.
    // `infer_failed` keeps a rejected inference site from re-diagnosing
    // every pass.
    let mut made: HashSet<(usize, String)> = HashSet::new();
    let mut attempted: HashSet<(usize, String)> = HashSet::new();
    let mut infer_failed: HashSet<(usize, u32, u32)> = HashSet::new();
    loop {
        let mut wants = Vec::new();
        for (i, (file, p)) in progs.iter_mut().enumerate() {
            for s in &p.structs {
                if s.tparams.is_empty() {
                    for (_, ft) in &s.fields {
                        wants_ty(ft, i, s.span, &mut wants);
                    }
                }
            }
            for e in &p.enums {
                if e.tparams.is_empty() {
                    for (_, vt) in &e.variants {
                        if let Some(t) = vt {
                            wants_ty(t, i, e.span, &mut wants);
                        }
                    }
                }
            }
            for f in &mut p.fns {
                if f.tparams.is_empty() {
                    for (_, _, t) in &f.params {
                        wants_ty(t, i, f.span, &mut wants);
                    }
                    wants_ty(&f.ret, i, f.span, &mut wants);
                    // N20 — the locals map inference and method
                    // resolution need: param bindings, then each
                    // `let` in order (annotation or init-inferred).
                    let mut locals: HashMap<String, Ty> = f
                        .params
                        .iter()
                        .map(|(n, _, t)| (n.clone(), t.clone()))
                        .collect();
                    {
                        let mut sc = Scan {
                            who: i,
                            locals: &mut locals,
                            tb: &tables,
                            gens: &gens,
                            file,
                            wants: &mut wants,
                            errs: &mut errs,
                            infer_failed: &mut infer_failed,
                        };
                        for st in &mut f.body {
                            scan_stmt(st, &mut sc);
                        }
                    }
                }
            }
        }
        let mut progress = false;
        for w in wants {
            let key = canon(&w.base, &w.args);
            if made.contains(&(w.requester, key.clone()))
                || !attempted.insert((w.requester, key.clone()))
            {
                continue;
            }
            match gens.get(&w.base) {
                Some(Gen::S(owner, decl)) => {
                    if *owner != w.requester && !decl.is_pub {
                        errs.push(diag(
                            progs[w.requester].0.clone(),
                            w.span,
                            format!("`{}` is private to its module", w.base),
                        ));
                        continue;
                    }
                    match subst_fields(decl, &w.args) {
                        Ok(fields) => {
                            progs[w.requester].1.structs.push(StructDecl {
                                name: key.clone(),
                                tparams: Vec::new(),
                                fields,
                                is_pub: false,
                                span: decl.span,
                            });
                            made.insert((w.requester, key));
                            progress = true;
                        }
                        Err((sp, m)) => {
                            errs.push(diag(progs[w.requester].0.clone(), sp, m))
                        }
                    }
                }
                Some(Gen::E(owner, decl)) => {
                    if *owner != w.requester && !decl.is_pub {
                        errs.push(diag(
                            progs[w.requester].0.clone(),
                            w.span,
                            format!("`{}` is private to its module", w.base),
                        ));
                        continue;
                    }
                    match subst_variants(decl, &w.args) {
                        Ok(variants) => {
                            progs[w.requester].1.enums.push(EnumDecl {
                                name: key.clone(),
                                tparams: Vec::new(),
                                variants,
                                is_pub: false,
                                span: decl.span,
                            });
                            made.insert((w.requester, key));
                            progress = true;
                        }
                        Err((sp, m)) => {
                            errs.push(diag(progs[w.requester].0.clone(), sp, m))
                        }
                    }
                }
                Some(Gen::F(owner, decl)) => {
                    if *owner != w.requester && !decl.is_pub {
                        errs.push(diag(
                            progs[w.requester].0.clone(),
                            w.span,
                            format!("`{}` is private to its module", w.base),
                        ));
                        continue;
                    }
                    match subst_fn(decl, &w.args) {
                        Ok(f) => {
                            progs[w.requester].1.fns.push(f);
                            made.insert((w.requester, key));
                            progress = true;
                        }
                        Err((sp, m)) => {
                            errs.push(diag(progs[w.requester].0.clone(), sp, m))
                        }
                    }
                }
                None => {
                    if concrete.contains(&w.base) {
                        errs.push(diag(
                            progs[w.requester].0.clone(),
                            w.span,
                            format!("`{}` is not generic", w.base),
                        ));
                    }
                    // else: unknown name — the checker reports it as
                    // `base<a,b>`, which names the use just as well.
                }
            }
        }
        if !progress {
            break;
        }
    }

    // Normalize: fold every App / targs into canonical `Struct` /
    // name keys. Generic templates keep their (unbound) params —
    // they're never checked — but normalize anyway for uniformity.
    for (_, p) in progs.iter_mut() {
        for s in &mut p.structs {
            for (_, ft) in &mut s.fields {
                norm_ty(ft);
            }
        }
        for e in &mut p.enums {
            for (_, vt) in &mut e.variants {
                if let Some(t) = vt {
                    norm_ty(t);
                }
            }
        }
        for f in &mut p.fns {
            for (_, _, t) in &mut f.params {
                norm_ty(t);
            }
            norm_ty(&mut f.ret);
            for st in &mut f.body {
                norm_stmt(st);
            }
        }
    }
    errs
}

fn diag(file: String, span: Span, msg: String) -> Diag {
    let mut d = Diag::new(span, msg);
    d.file = Some(file);
    d
}

// ------------------------------------------------------------------
// Instantiation-site scan — mirrors qualify's traversal, recording
// every `App` and targs list where concrete code asks for an
// instantiation. Generic decls are never scanned: a `T` in their
// bodies is bound only inside a substituted clone.
// N20 turns this into a mutable walk: `f(args)` with empty targs on a
// generic `f` infers and FILLS the args, `recv.m(args)` desugars to an
// ordinary `Call`, and `Type::name(args)` to an associated fn.
// ------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
struct Scan<'a> {
    who: usize,
    locals: &'a mut HashMap<String, Ty>,
    tb: &'a Tables,
    gens: &'a HashMap<String, Gen>,
    file: &'a str,
    wants: &'a mut Vec<Want>,
    errs: &'a mut Vec<Diag>,
    infer_failed: &'a mut HashSet<(usize, u32, u32)>,
}

fn wants_ty(t: &Ty, who: usize, span: Span, out: &mut Vec<Want>) {
    match t {
        Ty::App(base, args) => {
            out.push(Want {
                requester: who,
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
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => wants_ty(e, who, span, out),
        _ => {}
    }
}

fn wants_targs(targs: &[Ty], who: usize, span: Span, out: &mut Vec<Want>) {
    for t in targs {
        wants_ty(t, who, span, out);
    }
}

fn scan_stmt(s: &mut Stmt, x: &mut Scan) {
    match s {
        Stmt::Let { name, ty, init, span } => {
            if let Some(t) = ty {
                wants_ty(t, x.who, *span, x.wants);
            }
            let mut ity = None;
            if let Some(e) = init {
                scan_expr(e, x);
                ity = ty_of(e, x.locals, x.tb);
                // An inferred binding type can be the only mention of
                // a generic instantiation — `let v = fs.slurp(..)` is
                // the importer's sole claim on `vec.Vec<u8>` (N21B).
                if let Some(t) = &ity {
                    wants_ty(t, x.who, *span, x.wants);
                }
            }
            if let Some(t) = ty.clone().or(ity) {
                x.locals.insert(name.clone(), t);
            }
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
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            scan_expr(cond, x);
            for s in then_body.iter_mut() {
                scan_stmt(s, x);
            }
            if let Some(els) = else_body {
                for s in els {
                    scan_stmt(s, x);
                }
            }
        }
        Stmt::While { cond, body, .. } => {
            scan_expr(cond, x);
            for s in body {
                scan_stmt(s, x);
            }
        }
        Stmt::Expr { expr, .. } => scan_expr(expr, x),
        Stmt::Match { scrut, arms, .. } => {
            scan_expr(scrut, x);
            let sty = ty_of(scrut, &x.locals, x.tb);
            for a in arms {
                // Bind the arm's payload local so methods/calls on it
                // resolve during this scan; the checker re-derives it.
                if let (Some(b), Some(sty)) = (&a.bind, &sty) {
                    // N24C/B2 — an instantiated generic enum arrives as
                    // `App(base, args)` — resolve the decl by base name
                    // and substitute tparams -> args in the payload ty,
                    // so `Ok(e)` yields `Entry`, not `T`.
                    let pair = match sty {
                        Ty::Struct(n) => Some((n.clone(), Vec::new())),
                        Ty::App(n, targs) => Some((n.clone(), targs.clone())),
                        _ => None,
                    };
                    let pt = pair.and_then(|(base, iargs)| {
                        let d = x.tb.enums.get(&base)?;
                        let raw = d
                            .variants
                            .iter()
                            .find(|(vn, _)| vn == &a.variant)
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
                    });
                    if let Some(pt) = pt {
                        x.locals.insert(b.clone(), pt);
                    }
                }
                for s in &mut a.body {
                    scan_stmt(s, x);
                }
                if let Some(b) = &a.bind {
                    x.locals.remove(b);
                }
            }
        }
        Stmt::Region { body, .. } => {
            for s in body {
                scan_stmt(s, x);
            }
        }
        Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => scan_expr(target, x),
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

/// Push the want an explicit (or just-inferred) targs list represents,
/// plus any `App`s nested inside the args themselves.
fn call_want(base: &str, targs: &[Ty], who: usize, span: Span, out: &mut Vec<Want>) {
    out.push(Want {
        requester: who,
        base: base.to_string(),
        args: targs.to_vec(),
        span,
    });
    wants_targs(targs, who, span, out);
}

fn scan_expr(e: &mut Expr, x: &mut Scan) {
    match e {
        Expr::Call(n, targs, args, span) => {
            for a in args.iter_mut() {
                scan_expr(a, x);
            }
            // `a.b(..)` ambiguity: the parser can't tell a qualified
            // call from a method call — `v.push(x)` and `m.f(x)` are
            // the same tokens. A bound VARIABLE always wins: `v` in
            // locals → rewrite to a MethodCall node and let that arm
            // resolve it. Modules can't shadow this — a module name
            // is never a var.
            if let Some((pre, meth)) = n.rsplit_once('.') {
                if x.locals.contains_key(pre) {
                    *e = Expr::MethodCall {
                        recv: Box::new(Expr::Var(pre.to_string(), *span)),
                        name: meth.to_string(),
                        targs: std::mem::take(targs),
                        args: std::mem::take(args),
                        span: *span,
                    };
                    scan_expr(e, x);
                    return;
                }
            }
            if targs.is_empty() {
                if let Some(Gen::F(_, decl)) = x.gens.get(n.as_str()) {
                    // `f(args)` on a generic — N20A inference. A
                    // failed site is remembered so the fixpoint's
                    // next pass doesn't re-diagnose it.
                    if x.infer_failed.insert((x.who, span.line, span.col)) {
                        match infer_targs(decl, args, x.locals, x.tb) {
                            Ok(ts) => *targs = ts,
                            Err((sp, m)) => x.errs.push(diag(x.file.to_string(), sp, m)),
                        }
                    }
                }
            }
            if !targs.is_empty() {
                call_want(n, targs, x.who, *span, x.wants);
            }
        }
        Expr::EnumLit { ty, payload, span, .. } => {
            wants_ty(ty, x.who, *span, x.wants);
            if let Some(p) = payload {
                scan_expr(p, x);
            }
        }
        Expr::Try { inner, span, .. } => {
            // `f()?`'s Result<T,E> may be the requester's ONLY mention
            // of that enum instantiation — the inner's inferred type
            // is a want, same rule as `let v = fs.slurp(..)` (N21B).
            if let Some(t) = ty_of(inner, x.locals, x.tb) {
                wants_ty(&t, x.who, *span, x.wants);
            }
            scan_expr(inner, x);
        }
        Expr::StructLit(n, targs, fields, span) => {
            for (_, fe) in fields.iter_mut() {
                scan_expr(fe, x);
            }
            if targs.is_empty() {
                if let Some(Gen::S(_, decl)) = x.gens.get(n.as_str()) {
                    if x.infer_failed.insert((x.who, span.line, span.col)) {
                        match infer_struct_targs(decl, fields, x.locals, x.tb) {
                            Ok(ts) => *targs = ts,
                            Err((sp, m)) => x.errs.push(diag(x.file.to_string(), sp, m)),
                        }
                    }
                }
            }
            if !targs.is_empty() {
                call_want(n, targs, x.who, *span, x.wants);
            }
        }
        Expr::MethodCall {
            recv,
            name,
            targs,
            args,
            span,
        } => {
            scan_expr(recv, x);
            for a in args.iter_mut() {
                scan_expr(a, x);
            }
            let rty = ty_of(recv, x.locals, x.tb);
            let rname = rty.as_ref().map_or("?".to_string(), |t| format!("{t}"));
            let mname = rty
                .as_ref()
                .and_then(|t| method_target(t, name))
                .map(|n| {
                    // The method fn must take `self` as its first
                    // parameter — the receiver is arg 0 after desugar.
                    let decl = x
                        .tb
                        .fns
                        .get(&n)
                        .cloned()
                        .or_else(|| match x.gens.get(&n) {
                            Some(Gen::F(_, d)) => Some(d.clone()),
                            _ => None,
                        });
                    match decl {
                        Some(d)
                            if d.params
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
            match mname {
                Some(Ok((qn, _))) => {
                    let rv = std::mem::replace(&mut **recv, Expr::Bool(false, *span));
                    let mut all = Vec::with_capacity(args.len() + 1);
                    all.push(rv);
                    all.append(args);
                    *e = Expr::Call(qn, std::mem::take(targs), all, *span);
                    scan_expr(e, x); // reprocess as an ordinary call
                }
                // Failed sites are deduped so the fixpoint's next
                // pass doesn't re-diagnose — a SUCCESS never consumes
                // the slot the Call arm's inference needs.
                _ if x.infer_failed.insert((x.who, span.line, span.col)) => match mname {
                    Some(Err(m)) => x.errs.push(diag(x.file.to_string(), *span, m)),
                    _ => x.errs.push(diag(
                        x.file.to_string(),
                        *span,
                        format!("no method `{name}` on `{rname}`"),
                    )),
                },
                _ => {}
            }
        }
        Expr::AssocFn { ty, name, args, span } => {
            for a in args.iter_mut() {
                scan_expr(a, x);
            }
            // `m.T<..>::f` / `T::f` — the fn lives in the type's module.
            let (base, tys) = match &**ty {
                Ty::App(b, a) => (b.clone(), a.clone()),
                Ty::Struct(b) => (b.clone(), Vec::new()),
                t => (format!("{t}"), Vec::new()),
            };
            // Enum target? `Maybe::Some(42)` / `Maybe<i32>::None`
            // (N22). `base` resolves against enum decls first —
            // `m.Maybe::Some` qualified, `Maybe::Some` in its own module.
            let enum_decl = x.tb.enums.get(&base).or_else(|| {
                match x.gens.get(&base) {
                    Some(Gen::E(_, d)) => Some(d),
                    _ => None,
                }
            });
            if let Some(decl) = enum_decl {
                let tps: HashSet<String> = decl.tparams.iter().cloned().collect();
                let vpos = decl.variants.iter().position(|(vn, _)| vn == name);
                match vpos {
                    None => x.errs.push(diag(
                        x.file.to_string(),
                        *span,
                        format!("enum `{base}` has no variant `{name}`"),
                    )),
                    Some(i) => {
                        let wants_payload = decl.variants[i].1.is_some();
                        // Infer tparams for `Maybe<T>::Some(42)` — the
                        // payload arg's type binds T via match_ty.
                        let mut map: HashMap<String, Ty> = HashMap::new();
                        if tys.is_empty() && !tps.is_empty() {
                            if let (Some(templ), Some(a)) =
                                (&decl.variants[i].1, args.first())
                            {
                                if let Some(at) = ty_of(a, &x.locals, x.tb) {
                                    match_ty(templ, &at, &tps, &mut map);
                                }
                            }
                            for tp in &decl.tparams {
                                if !map.contains_key(tp) {
                                    x.errs.push(diag(
                                        x.file.to_string(),
                                        *span,
                                        format!(
                                            "cannot infer `{tp}` for `{base}::{name}`",
                                        ),
                                    ));
                                }
                            }
                        }
                        let tys = if tys.is_empty() {
                            decl.tparams
                                .iter()
                                .map(|tp| {
                                    map.get(tp).cloned().unwrap_or(Ty::Struct(tp.clone()))
                                })
                                .collect::<Vec<Ty>>()
                        } else {
                            tys
                        };
                        if args.len() != wants_payload as usize {
                            x.errs.push(diag(
                                x.file.to_string(),
                                *span,
                                format!(
                                    "variant `{name}` takes {} argument(s), found {}",
                                    wants_payload as usize,
                                    args.len()
                                ),
                            ));
                        } else {
                            let mut all = std::mem::take(args);
                            let lit_ty = if tys.is_empty() {
                                Ty::Struct(base)
                            } else {
                                Ty::App(base, tys)
                            };
                            wants_ty(&lit_ty, x.who, *span, x.wants);
                            *e = Expr::EnumLit {
                                ty: Box::new(lit_ty),
                                variant: name.clone(),
                                payload: all.pop().map(Box::new),
                                span: *span,
                            };
                        }
                    }
                }
                return;
            }
            let qn = match base.rfind('.') {
                Some(i) => format!("{}.{}", &base[..i], name),
                None => name.clone(),
            };
            if !x.tb.fns.contains_key(&qn) && !x.gens.contains_key(&qn) {
                x.errs.push(diag(
                    x.file.to_string(),
                    *span,
                    format!("no associated function `{qn}`"),
                ));
            } else {
                let mut all = std::mem::take(args);
                *e = Expr::Call(qn, tys, std::mem::take(&mut all), *span);
                scan_expr(e, x);
            }
        }
        Expr::Unary(_, i, _) | Expr::Deref(i, _) | Expr::Handle(i, _) | Expr::Join(i, _)
        | Expr::Mutex(i, _) => scan_expr(i, x),
        Expr::Bin(_, a, b, _) => {
            scan_expr(a, x);
            scan_expr(b, x);
        }
        Expr::Field(b, ..) => scan_expr(b, x),
        Expr::Index(b, i, _) => {
            scan_expr(b, x);
            scan_expr(i, x);
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            scan_expr(base, x);
            for e in lo.iter_mut().chain(hi.iter_mut()) {
                scan_expr(e, x);
            }
        }
        Expr::Sizeof(t, span) => wants_ty(t, x.who, *span, x.wants),
        Expr::AddrOf { place, .. } => scan_expr(place, x),
        Expr::Spawn(body, _) => {
            for s in body {
                scan_stmt(s, x);
            }
        }
        Expr::Alloc { ty, alloc, count, span } => {
            wants_ty(ty, x.who, *span, x.wants);
            scan_expr(alloc, x);
            if let Some(c) = count {
                scan_expr(c, x);
            }
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) | Expr::Str(..) => {}
    }
}

// ------------------------------------------------------------------
// N20A — generic-argument inference, deliberately small and
// positional: match each declared parameter type against the
// AST-inferred type of the corresponding argument. A type variable
// may bind from several args; every binding must agree. Anything
// unbindable or conflicting rejects — explicit `f<T>(..)` always
// remains available. There is no global solver here: `ty_of` is a
// mini evaluator over the decl tables plus the enclosing fn's
// `let`/`param` bindings — the same information the reader has.
// ------------------------------------------------------------------

/// Statically-known type of an expression, or None. This is NOT the
/// checker's type system — it covers the syntactic forms whose type
/// is locally obvious, which is all positional inference needs.
fn ty_of(e: &Expr, locals: &HashMap<String, Ty>, tb: &Tables) -> Option<Ty> {
    match e {
        Expr::Int(..) => Some(Ty::I32),
        Expr::Bool(..) => Some(Ty::Bool),
        Expr::Str(..) => Some(Ty::Str),
        Expr::Var(n, _) => locals.get(n).cloned(),
        Expr::EnumLit { ty, .. } => Some((**ty).clone()),
        Expr::Try { inner, .. } => {
            // `e?` yields the `Ok` payload of e's Result — best-effort,
            // like every other arm here.
            let t = ty_of(inner, locals, tb)?;
            let (base, args) = match &t {
                Ty::App(b, a) => (b.clone(), a.clone()),
                Ty::Struct(n) => (n.clone(), Vec::new()),
                _ => return None,
            };
            let decl = tb.enums.get(&base).or_else(|| match tb.enums.get(&format!("{base}")) {
                d => d,
            })?;
            let (tps, vs) = (decl.tparams.clone(), decl.variants.clone());
            let pt = vs.iter().find(|(vn, _)| vn == "Ok")?.1.clone()?;
            if tps.is_empty() || args.is_empty() {
                return Some(pt);
            }
            let mut map: HashMap<String, Ty> = HashMap::new();
            for (i, tp) in tps.iter().enumerate() {
                if let Some(a) = args.get(i) {
                    map.insert(tp.clone(), a.clone());
                }
            }
            let mut out = pt;
            subst_ty(&mut out, &map, Span { line: 0, col: 0 }).ok()?;
            Some(out)
        }
        Expr::StructLit(n, targs, ..) => Some(if targs.is_empty() {
            Ty::Struct(n.clone())
        } else {
            Ty::App(n.clone(), targs.clone())
        }),
        Expr::Call(n, targs, args, _) => {
            // Builtin calls carry their own return type.
            match n.as_str() {
                "system_allocator" => return Some(Ty::Alloc),
                "channel" => return Some(Ty::Channel(Box::new(Ty::Void))),
                // `recv(ch)` yields the endpoint's payload;
                // `lock(m)` yields its guard.
                "recv" => match ty_of(args.first()?, locals, tb)? {
                    Ty::Channel(e) => return Some(*e),
                    _ => return None,
                },
                "lock" => {
                    return Some(Ty::Guard(Box::new(ty_of(args.first()?, locals, tb)?)))
                }
                _ => {}
            }
            let d = tb.fns.get(n)?;
            if d.tparams.is_empty() {
                return Some(d.ret.clone());
            }
            // A generic callee — use its explicit targs, or infer
            // recursively (`id(id(42))` binds through both layers).
            let ts = if targs.is_empty() {
                infer_targs(d, args, locals, tb).ok()?
            } else {
                targs.clone()
            };
            let map: HashMap<String, Ty> =
                d.tparams.iter().cloned().zip(ts).collect();
            let mut r = d.ret.clone();
            let _ = subst_ty(&mut r, &map, Span { line: 0, col: 0 });
            Some(r)
        }
        Expr::Field(b, f, _) => field_ty(&ty_of(b, locals, tb)?, f, tb),
        Expr::Index(b, ..) | Expr::IndexRange { base: b, .. } => {
            match ty_of(b, locals, tb)? {
                Ty::Array(e, _)
                | Ty::Slice { elem: e, .. }
                | Ty::BoxSlice(e) => Some(*e),
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
            _ => None,
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
        Expr::MethodCall { .. } | Expr::AssocFn { .. } | Expr::Spawn(..) => None,
    }
}

/// The declared type of field `f` on struct-typed `t` — for a generic
/// struct, field types substitute under the application's args.
fn field_ty(t: &Ty, f: &str, tb: &Tables) -> Option<Ty> {
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

/// `recv.m(..)` resolves to `mod.m` where `mod` is the module that
/// declared the receiver's struct type — `vec.Vec<i32>` → `vec`, a
/// root-local `Pair` → the unqualified `m`. Methods live with their
/// type; no cross-module dispatch exists.
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

/// Bind the template's type parameters (bare `Struct("T")` names)
/// against the concrete argument type. Structural recursion through
/// every wrapper; `App` matches `App` with equal bases. Returns false
/// on any shape mismatch or conflicting rebind.
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
        // `box slice T` is its own constructor — match element-wise.
        BoxSlice(e) => matches!(arg, BoxSlice(e2) if match_ty(e, e2, tps, map)),
        _ => templ == arg,
    }
}

/// Infer `f<..>`'s type arguments positionally from its declared
/// params vs the argument expressions. Every type parameter must end
/// bound; conflicts and unknown arg types reject.
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
                    decl.name,
                    map[n]
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

/// Infer a generic struct literal's targs from its field values —
/// `Boxed { x: 42 }` binds `T = i32` through field `x`'s declared ty.
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
            continue; // unknown field — the checker reports it
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

// ------------------------------------------------------------------
// Template instantiation — clone the decl, replace `T` with the
// concrete arg everywhere a type can appear. Errors are template-use
// errors: bad arity, `T<..>`, `T{..}` on a non-struct arg.
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

/// Enum payloads substitute exactly like struct fields (N22) — a
/// variant payload is an ordinary slot subtree, no special rules.
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
    f.tparams = Vec::new();
    f.is_pub = false;
    f.name = canon(&decl.name, args);
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
                return Err((
                    span,
                    format!("type parameter `{n}` takes no type arguments"),
                ));
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
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => subst_ty(e, map, span)?,
        _ => {}
    }
    Ok(())
}

fn subst_targs(targs: &mut Vec<Ty>, map: &HashMap<String, Ty>, span: Span) -> Result<(), (Span, String)> {
    for t in targs {
        subst_ty(t, map, span)?;
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
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
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
        Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => {
            subst_expr(target, map, dspan)?
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
    Ok(())
}

fn subst_expr(e: &mut Expr, map: &HashMap<String, Ty>, dspan: Span) -> Result<(), (Span, String)> {
    match e {
        Expr::Try { inner, .. } => return subst_expr(inner, map, dspan),
        Expr::Call(n, targs, args, span) => {
            if map.contains_key(n.as_str()) {
                return Err((*span, format!("type parameter `{n}` is not a function")));
            }
            subst_targs(targs, map, *span)?;
            for a in args {
                subst_expr(a, map, dspan)?;
            }
        }
        Expr::EnumLit { ty, payload, .. } => {
            subst_ty(ty, map, dspan)?;
            if let Some(p) = payload {
                subst_expr(p, map, dspan)?;
            }
        }
        Expr::StructLit(n, targs, fields, span) => {
            if let Some(a) = map.get(n.as_str()) {
                // `T{..}` — only meaningful when T lands on a struct.
                match a {
                    Ty::Struct(m) | Ty::App(m, _) => *n = m.clone(),
                    other => {
                        return Err((
                            *span,
                            format!("cannot construct non-struct type `{other}`"),
                        ))
                    }
                }
            }
            subst_targs(targs, map, *span)?;
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
        Expr::MethodCall { recv, targs, args, span, .. } => {
            subst_expr(recv, map, dspan)?;
            subst_targs(targs, map, *span)?;
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
        Expr::Sizeof(t, span) => subst_ty(t, map, *span)?,
        Expr::AddrOf { place, .. } => subst_expr(place, map, dspan)?,
        Expr::Spawn(body, _) => {
            for s in body {
                subst_stmt(s, map, dspan)?;
            }
        }
        Expr::Alloc { ty, alloc, count, span } => {
            subst_ty(ty, map, *span)?;
            subst_expr(alloc, map, dspan)?;
            if let Some(c) = count {
                subst_expr(c, map, dspan)?;
            }
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) | Expr::Str(..) => {}
    }
    Ok(())
}

// ------------------------------------------------------------------
// Normalization — every App and targs list folds into the canonical
// `base<args>` spelling that the checker's name-keyed maps use.
// ------------------------------------------------------------------

fn norm_ty(t: &mut Ty) {
    match t {
        Ty::App(n, args) => {
            for a in args.iter_mut() {
                norm_ty(a);
            }
            let key = canon(n, args);
            *t = Ty::Struct(key);
        }
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => norm_ty(e),
        _ => {}
    }
}

fn norm_stmt(s: &mut Stmt) {
    match s {
        Stmt::Let { ty, init, .. } => {
            if let Some(t) = ty {
                norm_ty(t);
            }
            if let Some(e) = init {
                norm_expr(e);
            }
        }
        Stmt::Assign { target, value, .. } => {
            norm_expr(target);
            norm_expr(value);
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                norm_expr(e);
            }
        }
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            norm_expr(cond);
            for s in then_body.iter_mut().chain(else_body.iter_mut().flat_map(|b| b.iter_mut())) {
                norm_stmt(s);
            }
        }
        Stmt::While { cond, body, .. } => {
            norm_expr(cond);
            for s in body {
                norm_stmt(s);
            }
        }
        Stmt::Expr { expr, .. } => norm_expr(expr),
        Stmt::Match { scrut, arms, .. } => {
            norm_expr(scrut);
            for a in arms {
                for s in &mut a.body {
                    norm_stmt(s);
                }
            }
        }
        Stmt::Region { body, .. } => {
            for s in body {
                norm_stmt(s);
            }
        }
        Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => norm_expr(target),
        Stmt::Break { .. } | Stmt::Continue { .. } => {}
    }
}

fn norm_expr(e: &mut Expr) {
    match e {
        Expr::Call(n, targs, args, _) => {
            if !targs.is_empty() {
                for t in targs.iter_mut() {
                    norm_ty(t);
                }
                *n = canon(n, targs);
                targs.clear();
            }
            for a in args {
                norm_expr(a);
            }
        }
        Expr::StructLit(n, targs, fields, _) => {
            if !targs.is_empty() {
                for t in targs.iter_mut() {
                    norm_ty(t);
                }
                *n = canon(n, targs);
                targs.clear();
            }
            for (_, fe) in fields {
                norm_expr(fe);
            }
        }
        Expr::Unary(_, i, _) | Expr::Deref(i, _) | Expr::Handle(i, _) | Expr::Join(i, _)
        | Expr::Mutex(i, _) => norm_expr(i),
        Expr::Bin(_, a, b, _) => {
            norm_expr(a);
            norm_expr(b);
        }
        Expr::Field(b, ..) => norm_expr(b),
        Expr::Index(b, i, _) => {
            norm_expr(b);
            norm_expr(i);
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            norm_expr(base);
            for e in lo.iter_mut().chain(hi.iter_mut()) {
                norm_expr(e);
            }
        }
        Expr::MethodCall { recv, targs, args, .. } => {
            norm_expr(recv);
            for t in targs.iter_mut() {
                norm_ty(t);
            }
            for a in args {
                norm_expr(a);
            }
        }
        Expr::AssocFn { ty, args, .. } => {
            norm_ty(ty);
            for a in args {
                norm_expr(a);
            }
        }
        Expr::Sizeof(t, _) => norm_ty(t),
        Expr::AddrOf { place, .. } => norm_expr(place),
        Expr::Spawn(body, _) => {
            for s in body {
                norm_stmt(s);
            }
        }
        Expr::Alloc { ty, alloc, count, .. } => {
            norm_ty(ty);
            norm_expr(alloc);
            if let Some(c) = count {
                norm_expr(c);
            }
        }
        Expr::Try { inner, .. } => norm_expr(inner),
        Expr::EnumLit { ty, payload, .. } => {
            norm_ty(ty);
            if let Some(p) = payload {
                norm_expr(p);
            }
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) | Expr::Str(..) => {}
    }
}
