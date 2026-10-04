//! N24A — general rvalue-place lowering.
//!
//! The semantic/place machinery keys on `place_id`: `Var`/`Field`/
//! `Index`/`IndexRange`/`Deref` chains rooted in a named local. An
//! expression like `f().x` or `get_slice()[i]` has a non-place base —
//! codegen used to collapse it to a literal `0`, and the checker's
//! `read`/`write` param contracts rejected it outright.
//!
//! This pass removes the distinction: any place-position operand whose
//! base is not itself a place is materialized once into a synthesized
//! local —
//!
//! ```text
//!     f().x          →  let $rv0 = f();  $rv0.x
//!     recv(ch).field →  let $rv1 = recv(ch);  $rv1.field
//!     get()?.value   →  let $rv2 = get();  let $rv3 = $rv2?;  $rv3.value
//! ```
//!
//! Because the materialization is an ordinary `let`, EVERY side-channel
//! (init, edges, caps, packets, tags, domains) attaches to the temp
//! exactly as if the programmer had written the binding — and because
//! each source expression is hoisted at most once, the evaluate-
//! exactly-once invariant holds by construction: the checker sees one
//! eval, the IR sees one eval, the machine executes one eval.
//!
//! Runs after `monomorphize`: `MethodCall`/`AssocFn` have already been
//! rewritten to `Call`/`EnumLit`, so receiver-in-arg-position is just
//! the arg rule.
//!
//! `write` is the exception: a write to a temp dies with the statement
//! and can never be observed, so `write`-param args are NOT hoisted —
//! the checker's "takes a place" diagnostic still rejects
//! `inspect(write n, x + 1)`. `read`/`own`/by-value params hoist
//! (borrowing or moving a statement temp is sound — C++'s `const T&`
//! vs `T&` line).
//!
//! Loop conds re-evaluate per iteration, so hoisted temps get a
//! matching re-eval at the end of the body:
//!
//! ```text
//!     while f().x { b }  →  let $rv0 = f();
//!                         while $rv0.x { b  $rv0 = f(); }
//! ```

use crate::frontend::ast::*;
use crate::Span;
use std::collections::HashMap;

/// Callee param auths by function name — collected across every
/// loaded module post-mono so method receivers (arg0) resolve too.
pub type FnAuths = HashMap<String, Vec<Option<Auth>>>;

/// Gather `name → param auths` for every fn in `progs`.
pub fn collect_fn_auths(progs: &[&Program]) -> FnAuths {
    let mut m = HashMap::new();
    for p in progs {
        for f in &p.fns {
            m.insert(
                f.name.clone(),
                f.params.iter().map(|(_, a, _)| *a).collect(),
            );
        }
    }
    m
}

/// The syntactic place forms — `place_id`'s domain. Everything else in
/// a place position rides through a temp first.
fn is_place(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Var(..)
            | Expr::Field(..)
            | Expr::Index(..)
            | Expr::IndexRange { .. }
            | Expr::Deref(..)
    )
}

/// Call args that keep their dedicated arg-position semantics instead
/// of moving through a temp: `&`/`&mut` derivation (transient holder
/// edges, init rules), `handle`/`mutex`/`spawn` side-channels, and
/// atoms that can never be place-relevant.
fn arg_needs_temp(e: &Expr) -> bool {
    !is_place(e)
        && !matches!(
            e,
            Expr::AddrOf { .. }
                | Expr::Handle(..)
                | Expr::Mutex(..)
                | Expr::Spawn(..)
                | Expr::Int(..)
                | Expr::Bool(..)
                | Expr::Str(..)
                | Expr::Sizeof(..)
        )
}

struct Lower<'a> {
    next: u64,
    fns: &'a FnAuths,
    /// Temps created while lowering a `while` cond — they need a
    /// re-eval `Assign` appended at the end of the loop body.
    cond_depth: u32,
    cond_refresh: Vec<(String, Expr, Span)>,
}

impl Lower<'_> {
    /// `let $rvN = <e>` into `pre`; `e` is replaced by the temp var.
    fn materialize(&mut self, e: &mut Expr, pre: &mut Vec<Stmt>) {
        if is_place(e) {
            return;
        }
        let name = format!("$rv{}", self.next);
        self.next += 1;
        let span = e.span();
        let init = std::mem::replace(e, Expr::Var(name.clone(), span));
        if self.cond_depth > 0 {
            self.cond_refresh.push((name.clone(), init.clone(), span));
        }
        pre.push(Stmt::Let {
            name,
            ty: None,
            init: Some(init),
            span,
        });
    }

    /// A place-position operand: lower its interior bases, then hoist
    /// the operand itself if it isn't place-shaped.
    fn place_operand(&mut self, e: &mut Expr, pre: &mut Vec<Stmt>) {
        self.expr(e, pre);
        self.materialize(e, pre);
    }

    /// A call-arg operand: lower interior bases; hoist only when the
    /// arg is a value producer with no dedicated arg-position rule and
    /// the callee's param isn't `write` (a write into a dying temp can
    /// never be observed — keep the "takes a place" rejection).
    fn arg_operand(&mut self, a: &mut Expr, pre: &mut Vec<Stmt>, must_place: bool) {
        self.expr(a, pre);
        if !must_place && arg_needs_temp(a) {
            self.materialize(a, pre);
        }
    }

    /// Args of a call to `n` — `write` params keep their place contract.
    fn call_args(&mut self, n: &str, args: &mut [Expr], pre: &mut Vec<Stmt>) {
        let auths = self.fns.get(n);
        for (i, a) in args.iter_mut().enumerate() {
            let must_place = matches!(
                auths.and_then(|ps| ps.get(i)),
                Some(Some(Auth::Write))
            );
            self.arg_operand(a, pre, must_place);
        }
    }

    fn block(&mut self, stmts: &mut Vec<Stmt>) {
        let mut out = Vec::with_capacity(stmts.len());
        for mut s in std::mem::take(stmts).into_iter() {
            let mut pre = Vec::new();
            self.stmt(&mut s, &mut pre);
            out.extend(pre);
            out.push(s);
        }
        *stmts = out;
    }

    fn stmt(&mut self, s: &mut Stmt, pre: &mut Vec<Stmt>) {
        match s {
            Stmt::Let { init, .. } => {
                if let Some(i) = init {
                    self.expr(i, pre);
                }
            }
            Stmt::Assign { target, value, .. } => {
                // The TARGET itself never materializes — `f() = v`
                // stays a "not a place" rejection. Bases inside a
                // place target still hoist: `(*f()).x = v` works.
                self.expr(target, pre);
                self.expr(value, pre);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    self.expr(v, pre);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                // An `if` cond evaluates exactly once — its bases hoist
                // to before the branch like any other statement's.
                self.expr(cond, pre);
                self.block(then_body);
                if let Some(e) = else_body {
                    self.block(e);
                }
            }
            Stmt::While { cond, body, .. } => {
                // The cond re-evaluates each iteration: hoist its
                // temps before the loop AND re-eval them at body end.
                self.cond_depth += 1;
                self.expr(cond, pre);
                self.cond_depth -= 1;
                let refresh = std::mem::take(&mut self.cond_refresh);
                self.block(body);
                for (name, init, span) in refresh {
                    body.push(Stmt::Assign {
                        target: Expr::Var(name, span),
                        value: init,
                        span,
                    });
                }
            }
            Stmt::Expr { expr, .. } => self.expr(expr, pre),
            Stmt::Match { scrut, arms, .. } => {
                self.place_operand(scrut, pre);
                for a in arms.iter_mut() {
                    self.block(&mut a.body);
                }
            }
            Stmt::Region { body, .. } => self.block(body),
            Stmt::Consume { target, .. } | Stmt::Promote { target, .. } => {
                self.expr(target, pre)
            }
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
        }
    }

    /// Lower every interior place-position base inside `e`. The expr
    /// itself is a VALUE position — the caller decides hoisting.
    fn expr(&mut self, e: &mut Expr, pre: &mut Vec<Stmt>) {
        match e {
            Expr::Int(..)
            | Expr::Bool(..)
            | Expr::Var(..)
            | Expr::Str(..)
            | Expr::Sizeof(..) => {}
            Expr::Unary(_, o, _) => self.expr(o, pre),
            Expr::Bin(_, l, r, _) => {
                self.expr(l, pre);
                self.expr(r, pre);
            }
            Expr::Call(n, _, args, _) => {
                self.call_args(n, args, pre);
            }
            Expr::MethodCall { recv, args, .. } => {
                // Pre-mono safety — post-mono receivers are Call arg0.
                self.place_operand(recv, pre);
                for a in args.iter_mut() {
                    self.arg_operand(a, pre, false);
                }
            }
            Expr::AssocFn { name, args, .. } => {
                self.call_args(name, args, pre);
            }
            Expr::Field(b, ..) => self.place_operand(b, pre),
            Expr::Index(b, i, _) => {
                self.place_operand(b, pre);
                self.expr(i, pre);
            }
            Expr::IndexRange { base, lo, hi, .. } => {
                self.place_operand(base, pre);
                if let Some(l) = lo {
                    self.expr(l, pre);
                }
                if let Some(h) = hi {
                    self.expr(h, pre);
                }
            }
            Expr::Deref(b, _) => self.place_operand(b, pre),
            Expr::AddrOf { place, .. } => self.place_operand(place, pre),
            Expr::Handle(p, _) => self.place_operand(p, pre),
            Expr::Try { inner, .. } => {
                // `f()?` → `let $rv = f(); $rv?` — the temp is a real
                // place, so the Ok-payload move and tag tracking apply
                // to it like any scrutinee.
                self.place_operand(inner, pre);
            }
            Expr::StructLit(_, _, fields, _) => {
                for (_, fe) in fields.iter_mut() {
                    self.expr(fe, pre);
                }
            }
            Expr::EnumLit { payload, .. } => {
                if let Some(p) = payload {
                    self.expr(p, pre);
                }
            }
            Expr::Spawn(body, _) => self.block(body),
            Expr::Join(b, _) | Expr::Mutex(b, _) => self.expr(b, pre),
            Expr::Alloc { alloc, count, .. } => {
                self.expr(alloc, pre);
                if let Some(c) = count {
                    self.expr(c, pre);
                }
            }
        }
    }
}

/// Lower every function body in `p`.
pub fn lower_program(p: &mut Program, fns: &FnAuths) {
    let mut lw = Lower {
        next: 0,
        fns,
        cond_depth: 0,
        cond_refresh: Vec::new(),
    };
    for f in p.fns.iter_mut() {
        lw.block(&mut f.body);
    }
}
