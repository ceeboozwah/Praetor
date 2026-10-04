//! Capability-based memory model.
//!
//! Every binding carries a *capability* describing what may be done through it,
//! rather than tracking ownership transfer over time:
//!
//!   param `own` -> `own<T>`   full rights (read, write, hand off ownership)
//!   param `ref` -> `read<T>`  read-only
//!   param `mut` -> `write<T>` exclusive read/write
//!
//! References inherit/narrow capabilities:
//!   `&x`     yields a `read`  capability derived from x
//!   `&mut x` yields a `write` capability derived from x (illegal if x is read-only)
//!
//! What this model proves:
//!   * write-through-read      (mutating via a read-only capability)
//!   * capability escalation   (deriving `&mut` from a read-only binding)
//!   * conflicting capabilities (a live write cap alongside any other cap)
//!   * illegal ownership transfer (handing off a value you only borrowed)
//!
//! Divergence from the ownership model: capabilities describe *rights*, not a
//! move timeline. This model does NOT track use-after-move / use-after-free
//! (there is no "moved" state and `free` is ignored). Conversely it catches
//! write-through-a-shared-reference, which a purely move-oriented checker can
//! miss. The corpus exercises both directions.

use crate::ast::*;
use crate::check::{signature_modes, Diagnostic, MemoryModel};
use std::collections::HashMap;

pub struct Capability;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cap {
    Own,
    Read,
    Write,
}

impl Cap {
    fn can_write(self) -> bool {
        matches!(self, Cap::Own | Cap::Write)
    }
}

struct VarInfo {
    cap: Cap,
    /// If this binding is a reference, the (base, is_write) capability it holds.
    borrows: Option<(String, bool)>,
    /// Outstanding read/write capabilities derived from this variable.
    out_read: u32,
    out_write: u32,
}

impl VarInfo {
    fn new(cap: Cap) -> Self {
        VarInfo {
            cap,
            borrows: None,
            out_read: 0,
            out_write: 0,
        }
    }
}

struct Env<'a> {
    vars: HashMap<String, VarInfo>,
    scopes: Vec<Vec<String>>,
    sigs: &'a HashMap<String, Vec<Mode>>,
    diags: Vec<Diagnostic>,
}

impl<'a> Env<'a> {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        if let Some(names) = self.scopes.pop() {
            for n in names {
                if let Some(info) = self.vars.remove(&n) {
                    if let Some((base, is_write)) = info.borrows {
                        if let Some(b) = self.vars.get_mut(&base) {
                            if is_write {
                                b.out_write = b.out_write.saturating_sub(1);
                            } else {
                                b.out_read = b.out_read.saturating_sub(1);
                            }
                        }
                    }
                }
            }
        }
    }

    fn declare(&mut self, name: String, info: VarInfo) {
        if let Some(s) = self.scopes.last_mut() {
            s.push(name.clone());
        }
        self.vars.insert(name, info);
    }
}

fn mode_to_cap(m: Mode) -> Cap {
    match m {
        Mode::Own => Cap::Own,
        Mode::Ref => Cap::Read,
        Mode::Mut => Cap::Write,
    }
}

fn base_var(e: &Expr) -> Option<&str> {
    match e {
        Expr::Var(n, _) => Some(n),
        Expr::Field(b, _, _) | Expr::Deref(b, _) => base_var(b),
        _ => None,
    }
}

impl Capability {
    fn check_fn(env: &mut Env, f: &FnDef) {
        env.push_scope();
        for p in &f.params {
            env.declare(p.name.clone(), VarInfo::new(mode_to_cap(p.mode)));
        }
        Self::check_block(env, &f.body);
        env.pop_scope();
    }

    fn check_block(env: &mut Env, block: &Block) {
        env.push_scope();
        for stmt in &block.stmts {
            Self::check_stmt(env, stmt);
        }
        env.pop_scope();
    }

    fn check_stmt(env: &mut Env, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, init, .. } => match init {
                Expr::Ref(place, span) => {
                    Self::derive_ref(env, name.clone(), place, false, *span);
                }
                Expr::MutRef(place, span) => {
                    Self::derive_ref(env, name.clone(), place, true, *span);
                }
                other => {
                    Self::eval_value(env, other);
                    // You own what you construct locally.
                    env.declare(name.clone(), VarInfo::new(Cap::Own));
                }
            },
            Stmt::Assign { target, value, span } => {
                match target {
                    // Rebinding a *variable* is not a write through whatever
                    // capability the variable's old value held — `a = &y` where
                    // `a` was `&x` swaps the referent, it doesn't write `x`.
                    // The old borrow is released and the new value's
                    // capability is derived fresh.
                    Expr::Var(tname, _) => {
                        Self::eval_value(env, value);
                        let tname = tname.clone();
                        if let Some(info) = env.vars.remove(&tname) {
                            if let Some((base, is_write)) = info.borrows {
                                if let Some(b) = env.vars.get_mut(&base) {
                                    if is_write {
                                        b.out_write = b.out_write.saturating_sub(1);
                                    } else {
                                        b.out_read = b.out_read.saturating_sub(1);
                                    }
                                }
                            }
                        }
                        match value {
                            Expr::Ref(place, s) => {
                                Self::derive_ref(env, tname, place, false, *s)
                            }
                            Expr::MutRef(place, s) => {
                                Self::derive_ref(env, tname, place, true, *s)
                            }
                            _ => env.declare(tname, VarInfo::new(Cap::Own)),
                        }
                    }
                    _ => {
                        Self::eval_value(env, value);
                        Self::require_write(env, target, *span);
                    }
                }
            }
            Stmt::Return { value: Some(v), .. } => Self::eval_value(env, v),
            Stmt::Return { value: None, .. } => {}
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                Self::eval_value(env, cond);
                Self::check_block(env, then_block);
                if let Some(eb) = else_block {
                    Self::check_block(env, eb);
                }
            }
            Stmt::While { cond, body, .. } => {
                Self::eval_value(env, cond);
                Self::check_block(env, body);
            }
            Stmt::Region { body, .. } => Self::check_block(env, body),
            // No move timeline: `free` carries no capability meaning here.
            Stmt::Free { .. } => {}
            Stmt::Expr { expr, .. } => Self::eval_value(env, expr),
        }
    }

    fn eval_value(env: &mut Env, e: &Expr) {
        match e {
            Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) | Expr::Alloc { .. } => {}
            Expr::Persist { inner, .. } => Self::eval_value(env, inner),
            Expr::Ref(place, span) => Self::check_temp_cap(env, place, false, *span),
            Expr::MutRef(place, span) => Self::check_temp_cap(env, place, true, *span),
            Expr::Deref(inner, _) => Self::eval_value(env, inner),
            Expr::Field(base, _, _) => Self::eval_value(env, base),
            Expr::StructLit { fields, .. } => {
                for (_, v) in fields {
                    Self::eval_value(env, v);
                }
            }
            Expr::Call { name, args, span } => {
                let modes = env.sigs.get(name).cloned();
                for (i, arg) in args.iter().enumerate() {
                    match modes.as_ref().and_then(|m| m.get(i).copied()) {
                        Some(Mode::Mut) => Self::check_arg_cap(env, arg, true, *span),
                        Some(Mode::Ref) => Self::check_arg_cap(env, arg, false, *span),
                        Some(Mode::Own) => Self::require_own_transfer(env, arg, *span),
                        None => Self::eval_value(env, arg),
                    }
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                Self::eval_value(env, lhs);
                Self::eval_value(env, rhs);
            }
            // Sequential baseline: the task body runs inline — this model has
            // no task-capability partition, so concurrent races are invisible.
            Expr::Spawn { body, .. } => Self::check_block(env, body),
            Expr::Join { inner, .. } => Self::eval_value(env, inner),
        }
    }

    /// `let r = &[mut] place;` — derive and persist a capability.
    fn derive_ref(env: &mut Env, name: String, place: &Expr, want_write: bool, span: Span) {
        let base = match base_var(place) {
            Some(b) => b.to_string(),
            None => {
                Self::eval_value(env, place);
                env.declare(name, VarInfo::new(Cap::Own));
                return;
            }
        };
        let base_cap = env.vars.get(&base).map(|v| v.cap);
        if want_write {
            if let Some(cap) = base_cap {
                if !cap.can_write() {
                    env.diags.push(Diagnostic::new(
                        span,
                        "capability-escalation",
                        format!("cannot derive a `write` capability from read-only `{base}`"),
                    ));
                }
            }
            if let Some(info) = env.vars.get(&base) {
                if info.out_write > 0 || info.out_read > 0 {
                    env.diags.push(Diagnostic::new(
                        span,
                        "capability-conflict",
                        format!("exclusive `write` of `{base}` conflicts with a live capability"),
                    ));
                }
            }
            if let Some(info) = env.vars.get_mut(&base) {
                info.out_write += 1;
            }
            let mut vi = VarInfo::new(Cap::Write);
            vi.borrows = Some((base, true));
            env.declare(name, vi);
        } else {
            if let Some(info) = env.vars.get(&base) {
                if info.out_write > 0 {
                    env.diags.push(Diagnostic::new(
                        span,
                        "capability-conflict",
                        format!("cannot derive `read` of `{base}`: a live `write` capability exists"),
                    ));
                }
            }
            if let Some(info) = env.vars.get_mut(&base) {
                info.out_read += 1;
            }
            let mut vi = VarInfo::new(Cap::Read);
            vi.borrows = Some((base, false));
            env.declare(name, vi);
        }
    }

    /// A write is attempted on `target` (assignment LHS or `mut` argument).
    fn require_write(env: &mut Env, target: &Expr, span: Span) {
        if let Some(base) = base_var(target) {
            if let Some(info) = env.vars.get(base) {
                if !info.cap.can_write() {
                    env.diags.push(Diagnostic::new(
                        span,
                        "write-through-read",
                        format!("cannot write to `{base}` through a read-only capability"),
                    ));
                }
            }
        }
    }

    /// Temporary capability check for `&`/`&mut` in value position.
    fn check_temp_cap(env: &mut Env, place: &Expr, want_write: bool, span: Span) {
        if let Some(base) = base_var(place) {
            let (cap, out_read, out_write) = match env.vars.get(base) {
                Some(i) => (i.cap, i.out_read, i.out_write),
                None => return,
            };
            if want_write && !cap.can_write() {
                env.diags.push(Diagnostic::new(
                    span,
                    "capability-escalation",
                    format!("cannot take `&mut {base}`: only a read capability is held"),
                ));
            }
            // An exclusive write must not coexist with any live capability,
            // whether that capability was persisted or is another live borrow.
            if want_write && (out_read > 0 || out_write > 0) {
                env.diags.push(Diagnostic::new(
                    span,
                    "capability-conflict",
                    format!("exclusive `&mut {base}` conflicts with a live capability"),
                ));
            }
        } else {
            Self::eval_value(env, place);
        }
    }

    /// Passing an argument to a `ref`/`mut` parameter.
    fn check_arg_cap(env: &mut Env, arg: &Expr, want_write: bool, span: Span) {
        match arg {
            Expr::Ref(place, s) => Self::check_temp_cap(env, place, false, *s),
            Expr::MutRef(place, s) => Self::check_temp_cap(env, place, true, *s),
            Expr::Var(name, s) => {
                if want_write {
                    if let Some(info) = env.vars.get(name.as_str()) {
                        if !info.cap.can_write() {
                            env.diags.push(Diagnostic::new(
                                *s,
                                "write-through-read",
                                format!(
                                    "passing read-only `{name}` where a `write` capability is required"
                                ),
                            ));
                        }
                    }
                }
            }
            other => Self::eval_value(env, other),
        }
        let _ = span;
    }

    /// Passing an argument to an `own` parameter transfers ownership.
    fn require_own_transfer(env: &mut Env, arg: &Expr, span: Span) {
        if let Some(base) = base_var(arg) {
            if let Some(info) = env.vars.get(base) {
                if info.cap != Cap::Own {
                    env.diags.push(Diagnostic::new(
                        span,
                        "illegal-transfer",
                        format!(
                            "cannot transfer ownership of `{base}`: only a borrowed capability is held"
                        ),
                    ));
                }
            }
        } else {
            Self::eval_value(env, arg);
        }
    }
}

impl MemoryModel for Capability {
    fn name(&self) -> &'static str {
        "capability"
    }

    fn tagline(&self) -> &'static str {
        "read/write/own rights on references; catches permission violations"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            states: &["read-cap", "write-cap", "own-cap"],
            resource_states: &[],
            temporal_states: 0,
            lifetime_concepts: 0,
            move_rules: 1, // capability-conflict exclusivity
            rules: 4,
            path_sensitive: false,
            machinery: &["borrow-counts"],
        }
    }

    fn check(&self, program: &Program) -> Vec<Diagnostic> {
        let sigs = signature_modes(program);
        let mut env = Env {
            vars: HashMap::new(),
            scopes: Vec::new(),
            sigs: &sigs,
            diags: Vec::new(),
        };
        for f in program.functions() {
            Self::check_fn(&mut env, f);
        }
        env.diags
    }
}
