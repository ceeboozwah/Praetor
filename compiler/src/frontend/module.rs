//! N17 — module-name qualification, applied at load time.
//!
//! A module's local symbols are renamed to `m.f` / `m.T` BEFORE the
//! program is checked, so the checker, IR gen, and LLVM emit never see
//! an unqualified name twice. This is the whole isolation mechanism:
//! summaries, signatures, and struct layouts all key on the qualified
//! name — a module interface is just "the `pub` slice of those maps."
//!
//! N19 note: generic BASE names qualify (`App("Vec",..)` →
//! `App("m.Vec",..)`, `f<T>` calls → `m.f<T>`), but a decl's own
//! `tparams` never do — `T` stays `Ty::Struct("T")` so the monomorph
//! pass can substitute it.

use super::ast::*;
use std::collections::HashSet;

/// Rename `p`'s module-local symbols to `module.name` form. Private
/// symbols get qualified too — they simply aren't exported through the
/// interface, so a private `m.helper` can never collide with another
/// module's `helper` at emit time. `extern` fns stay unqualified: they
/// name global C symbols, not module members.
pub fn qualify(p: &mut Program, module: &str) {
    let fn_names: HashSet<String> = p
        .fns
        .iter()
        .filter(|f| !f.is_extern)
        .map(|f| f.name.clone())
        .collect();
    let struct_names: HashSet<String> = p.structs.iter().map(|s| s.name.clone()).collect();
    // Enum names qualify exactly like struct names (N22) — the merged
    // set is `locals` for type qualification.
    let enum_names: HashSet<String> = p.enums.iter().map(|e| e.name.clone()).collect();
    let ty_names: HashSet<String> = struct_names.union(&enum_names).cloned().collect();

    for s in &mut p.structs {
        let tparams = s.tparams.clone();
        for (_, ft) in &mut s.fields {
            qualify_ty(ft, module, &ty_names, &tparams);
        }
        s.name = format!("{module}.{}", s.name);
    }
    for e in &mut p.enums {
        let tparams = e.tparams.clone();
        for (_, vt) in &mut e.variants {
            if let Some(t) = vt {
                qualify_ty(t, module, &ty_names, &tparams);
            }
        }
        e.name = format!("{module}.{}", e.name);
    }
    for f in &mut p.fns {
        let tparams = f.tparams.clone();
        for (_, _, t) in &mut f.params {
            qualify_ty(t, module, &ty_names, &tparams);
        }
        qualify_ty(&mut f.ret, module, &ty_names, &tparams);
        for stmt in &mut f.body {
            qualify_stmt(stmt, module, &fn_names, &ty_names, &tparams);
        }
        if !f.is_extern {
            f.name = format!("{module}.{}", f.name);
        }
    }
}

/// `Ty::Struct(n)` → `Ty::Struct("m.n")` when `n` is module-local —
/// recurses through every type constructor that can nest a struct name.
/// `App` bases qualify the same way; a name in `params` is a type
/// parameter of the enclosing decl and is left bare for substitution.
fn qualify_ty(t: &mut Ty, module: &str, locals: &HashSet<String>, params: &[String]) {
    match t {
        Ty::Struct(n) if !params.contains(n) && locals.contains(n.as_str()) => {
            *n = format!("{module}.{n}");
        }
        Ty::App(n, args) => {
            if !params.contains(n) && locals.contains(n.as_str()) {
                *n = format!("{module}.{n}");
            }
            for a in args {
                qualify_ty(a, module, locals, params);
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
        | Ty::BoxSlice(e) => qualify_ty(e, module, locals, params),
        _ => {}
    }
}

fn qualify_stmt(
    s: &mut Stmt,
    module: &str,
    fns: &HashSet<String>,
    structs: &HashSet<String>,
    params: &[String],
) {
    match s {
        Stmt::Let { ty, init, .. } => {
            if let Some(t) = ty {
                qualify_ty(t, module, structs, params);
            }
            if let Some(e) = init {
                qualify_expr(e, module, fns, structs, params);
            }
        }
        Stmt::Assign { target, value, .. } => {
            qualify_expr(target, module, fns, structs, params);
            qualify_expr(value, module, fns, structs, params);
        }
        Stmt::Return { value, .. } => {
            if let Some(e) = value {
                qualify_expr(e, module, fns, structs, params);
            }
        }
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            qualify_expr(cond, module, fns, structs, params);
            for s in then_body.iter_mut().chain(else_body.iter_mut().flat_map(|b| b.iter_mut())) {
                qualify_stmt(s, module, fns, structs, params);
            }
        }
        Stmt::While { cond, body, .. } => {
            qualify_expr(cond, module, fns, structs, params);
            for s in body {
                qualify_stmt(s, module, fns, structs, params);
            }
        }
        Stmt::Expr { expr, .. } => qualify_expr(expr, module, fns, structs, params),
        Stmt::Match { scrut, arms, .. } => {
            qualify_expr(scrut, module, fns, structs, params);
            for a in arms {
                for s in &mut a.body {
                    qualify_stmt(s, module, fns, structs, params);
                }
            }
        }
        Stmt::Region { body, .. } => {
            for s in body {
                qualify_stmt(s, module, fns, structs, params);
            }
        }
        Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => {
            qualify_expr(target, module, fns, structs, params)
        }
    }
}

fn qualify_expr(
    e: &mut Expr,
    module: &str,
    fns: &HashSet<String>,
    structs: &HashSet<String>,
    params: &[String],
) {
    match e {
        Expr::Call(n, targs, args, _) => {
            if fns.contains(n.as_str()) {
                *n = format!("{module}.{n}");
            }
            for t in targs {
                qualify_ty(t, module, structs, params);
            }
            for a in args {
                qualify_expr(a, module, fns, structs, params);
            }
        }
        Expr::StructLit(n, targs, fields, _) => {
            if structs.contains(n.as_str()) {
                *n = format!("{module}.{n}");
            }
            for t in targs {
                qualify_ty(t, module, structs, params);
            }
            for (_, fe) in fields {
                qualify_expr(fe, module, fns, structs, params);
            }
        }
        Expr::Unary(_, i, _) | Expr::Deref(i, _) | Expr::Handle(i, _) | Expr::Join(i, _)
        | Expr::Mutex(i, _) => qualify_expr(i, module, fns, structs, params),
        Expr::Bin(_, a, b, _) => {
            qualify_expr(a, module, fns, structs, params);
            qualify_expr(b, module, fns, structs, params);
        }
        Expr::Field(b, ..) => qualify_expr(b, module, fns, structs, params),
        Expr::Index(b, i, _) => {
            qualify_expr(b, module, fns, structs, params);
            qualify_expr(i, module, fns, structs, params);
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            qualify_expr(base, module, fns, structs, params);
            for e in lo.iter_mut().chain(hi.iter_mut()) {
                qualify_expr(e, module, fns, structs, params);
            }
        }
        Expr::Sizeof(t, _) => qualify_ty(t, module, structs, params),
        Expr::MethodCall { recv, args, .. } => {
            qualify_expr(recv, module, fns, structs, params);
            for a in args {
                qualify_expr(a, module, fns, structs, params);
            }
        }
        Expr::AssocFn { ty, args, .. } => {
            qualify_ty(ty, module, structs, params);
            for a in args {
                qualify_expr(a, module, fns, structs, params);
            }
        }
        Expr::AddrOf { place, .. } => qualify_expr(place, module, fns, structs, params),
        Expr::Spawn(body, _) => {
            for s in body {
                qualify_stmt(s, module, fns, structs, params);
            }
        }
        Expr::Alloc { ty, alloc, count, .. } => {
            qualify_ty(ty, module, structs, params);
            qualify_expr(alloc, module, fns, structs, params);
            if let Some(c) = count {
                qualify_expr(c, module, fns, structs, params);
            }
        }
        Expr::EnumLit { ty, payload, .. } => {
            qualify_ty(ty, module, structs, params);
            if let Some(p) = payload {
                qualify_expr(p, module, fns, structs, params);
            }
        }
        Expr::Try { inner, .. } => qualify_expr(inner, module, fns, structs, params),
        Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) | Expr::Str(..) => {}
    }
}
