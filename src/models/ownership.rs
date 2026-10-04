//! Ownership + borrowing model (a deliberately simplified take on Rust).
//!
//! Interpretation of the shared annotation vocabulary:
//!   param `own` -> value is *moved* into the callee
//!   param `ref` -> callee receives a shared (`&`) borrow
//!   param `mut` -> callee receives an exclusive (`&mut`) borrow
//!
//! What this model proves at compile time:
//!   * use-after-move            (reading a value that was moved away)
//!   * use-after-free            (`free` consumes; later use is caught)
//!   * double-free               (`free` of an already-consumed value)
//!   * aliasing violations       (`&mut` while another borrow is live, etc.)
//!   * move/mutate while borrowed
//!
//! Simplifications (documented honestly, per the lab's philosophy):
//!   * Borrows last for the lexical scope of the binding that holds them
//!     (no non-lexical-lifetime shortening).
//!   * Control flow is analysed sequentially; both arms of an `if` are checked
//!     but state is not merged with full path sensitivity.
//! These are exactly the kinds of reasoning a programmer must internalise, so
//! their *presence* is the cost this model imposes — which is what we measure.

use crate::ast::*;
use crate::check::{signature_modes, Diagnostic, MemoryModel};
use std::collections::HashMap;

pub struct Ownership;

#[derive(Debug)]
struct VarInfo {
    /// Copy types (primitives) are read rather than moved.
    copy: bool,
    /// `Some(span)` once the value has been moved/consumed at that location.
    moved: Option<Span>,
    /// If this binding is a reference, the (base variable, is_mut) it borrows.
    borrows: Option<(String, bool)>,
    /// Outstanding shared borrows *of* this variable.
    shared: u32,
    /// Outstanding mutable borrows *of* this variable.
    mutb: u32,
}

impl VarInfo {
    fn owned(copy: bool) -> Self {
        VarInfo {
            copy,
            moved: None,
            borrows: None,
            shared: 0,
            mutb: 0,
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
            for name in names {
                if let Some(info) = self.vars.remove(&name) {
                    // Releasing a reference binding frees the borrow it held.
                    if let Some((base, is_mut)) = info.borrows {
                        if let Some(b) = self.vars.get_mut(&base) {
                            if is_mut {
                                b.mutb = b.mutb.saturating_sub(1);
                            } else {
                                b.shared = b.shared.saturating_sub(1);
                            }
                        }
                    }
                }
            }
        }
    }

    fn declare(&mut self, name: String, info: VarInfo) {
        if let Some(scope) = self.scopes.last_mut() {
            scope.push(name.clone());
        }
        self.vars.insert(name, info);
    }

    /// Is a value produced by this expression a Copy value?
    fn is_copy_expr(&self, e: &Expr) -> bool {
        match e {
            Expr::Int(..) | Expr::Bool(..) => true,
            Expr::Binary { .. } => true,
            Expr::Ref(..) => true, // a shared reference is itself Copy
            Expr::Deref(..) => true,
            Expr::Field(..) => true,
            Expr::Var(name, _) => self.vars.get(name).map(|v| v.copy).unwrap_or(true),
            // Owned resources:
            Expr::MutRef(..)
            | Expr::Alloc { .. }
            | Expr::Persist { .. }
            | Expr::StructLit { .. }
            | Expr::Spawn { .. }
            | Expr::Join { .. }
            | Expr::Call { .. } => false,
        }
    }

    /// Extract the base variable name of a place expression (`x`, `x.f`, `*x`).
    fn base_var(e: &Expr) -> Option<&str> {
        match e {
            Expr::Var(n, _) => Some(n),
            Expr::Field(b, _, _) | Expr::Deref(b, _) => Self::base_var(b),
            _ => None,
        }
    }
}

impl Ownership {
    fn check_fn(env: &mut Env, f: &FnDef) {
        env.push_scope();
        for p in &f.params {
            // A `ref`/`mut` parameter is a borrow the caller owns; treat the
            // local binding as a (copy-able) reference so it can be read freely.
            let copy = matches!(p.mode, Mode::Ref | Mode::Mut) || is_primitive(&p.ty);
            env.declare(p.name.clone(), VarInfo::owned(copy));
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
            Stmt::Let { name, init, .. } => {
                // Persistent borrow if the initialiser is directly a reference.
                match init {
                    Expr::Ref(place, span) => {
                        Self::take_borrow(env, place, false, *span);
                        let borrows = Env::base_var(place).map(|b| (b.to_string(), false));
                        let mut info = VarInfo::owned(true);
                        info.borrows = borrows;
                        env.declare(name.clone(), info);
                    }
                    Expr::MutRef(place, span) => {
                        Self::take_borrow(env, place, true, *span);
                        let borrows = Env::base_var(place).map(|b| (b.to_string(), true));
                        let mut info = VarInfo::owned(true);
                        info.borrows = borrows;
                        env.declare(name.clone(), info);
                    }
                    other => {
                        let copy = env.is_copy_expr(other);
                        Self::eval_value(env, other);
                        env.declare(name.clone(), VarInfo::owned(copy));
                    }
                }
            }
            Stmt::Assign { target, value, span } => {
                Self::eval_value(env, value);
                // Writing to a place requires it not be borrowed by others.
                if let Some(base) = Env::base_var(target) {
                    if let Some(info) = env.vars.get(base) {
                        if info.shared > 0 || info.mutb > 0 {
                            env.diags.push(Diagnostic::new(
                                *span,
                                "alias",
                                format!("cannot assign to `{base}` while it is borrowed"),
                            ));
                        }
                    }
                    // Assignment re-initialises a (possibly moved) variable.
                    if let Some(info) = env.vars.get_mut(base) {
                        info.moved = None;
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
            Stmt::Region { body, .. } => {
                // Ownership model ignores region boundaries entirely.
                Self::check_block(env, body);
            }
            Stmt::Free { value, span } => {
                // `free` consumes the value like a move; guards against
                // double-free and later use-after-free.
                if let Some(base) = Env::base_var(value) {
                    match env.vars.get(base) {
                        Some(info) if info.moved.is_some() => {
                            env.diags.push(Diagnostic::new(
                                *span,
                                "double-free",
                                format!("`{base}` was already consumed"),
                            ));
                        }
                        Some(info) if info.shared > 0 || info.mutb > 0 => {
                            env.diags.push(Diagnostic::new(
                                *span,
                                "alias",
                                format!("cannot free `{base}` while it is borrowed"),
                            ));
                        }
                        _ => {}
                    }
                    if let Some(info) = env.vars.get_mut(base) {
                        info.moved = Some(*span);
                    }
                } else {
                    Self::eval_value(env, value);
                }
            }
            Stmt::Expr { expr, .. } => Self::eval_value(env, expr),
        }
    }

    /// Evaluate an expression in value position, recording reads and moves.
    fn eval_value(env: &mut Env, e: &Expr) {
        match e {
            Expr::Int(..) | Expr::Bool(..) => {}
            Expr::Var(name, span) => Self::use_var(env, name, *span),
            Expr::Ref(place, span) => Self::take_temp_borrow(env, place, false, *span),
            Expr::MutRef(place, span) => Self::take_temp_borrow(env, place, true, *span),
            Expr::Deref(inner, _) => Self::eval_value(env, inner),
            Expr::Field(base, _, span) => {
                // Reading a field is a shared read of the base variable.
                if let Some(b) = Env::base_var(base) {
                    Self::read_var(env, b, *span);
                } else {
                    Self::eval_value(env, base);
                }
            }
            Expr::Alloc { .. } => {}
            // `promote e` re-homes `e` — a move. `handle e` only reads `e` to
            // mint a generation-checked ticket; ownership stays put.
            Expr::Persist { kind: PersistKind::Promote, inner, .. } => {
                Self::eval_value(env, inner)
            }
            Expr::Persist { kind: PersistKind::Handle, inner, span } => {
                match Env::base_var(inner) {
                    Some(b) => Self::read_var(env, b, *span),
                    None => Self::eval_value(env, inner),
                }
            }
            // `mutex e` moves `e` into the wrapper — same cost as a move.
            Expr::Persist { kind: PersistKind::Mutex, inner, span } => {
                match Env::base_var(inner) {
                    Some(b) => Self::use_var(env, b, *span),
                    None => Self::eval_value(env, inner),
                }
            }
            Expr::StructLit { fields, .. } => {
                for (_, v) in fields {
                    Self::eval_value(env, v);
                }
            }
            // Lock builtins: `lock(m)` borrows the mutex (the guard's
            // authority is affine machinery, not an ownership transfer);
            // `unlock(g)` consumes the guard outright.
            Expr::Call { name, args, span } if name == "lock" => {
                if let Some(a) = args.first() {
                    match Env::base_var(a) {
                        Some(b) => Self::read_var(env, b, *span),
                        None => Self::eval_value(env, a),
                    }
                }
            }
            Expr::Call { name, args, span } if name == "unlock" => {
                if let Some(a) = args.first() {
                    match Env::base_var(a) {
                        Some(b) => Self::use_var(env, b, *span),
                        None => Self::eval_value(env, a),
                    }
                }
            }
            // Channel builtins: `send` borrows the endpoint and moves the
            // payload (authority transfer); `recv` borrows; `close`
            // consumes the endpoint.
            Expr::Call { name, args, span } if name == "send" => {
                if let Some(a) = args.first() {
                    match Env::base_var(a) {
                        Some(b) => Self::read_var(env, b, *span),
                        None => Self::eval_value(env, a),
                    }
                }
                for a in &args[1..] {
                    Self::eval_value(env, a);
                }
            }
            Expr::Call { name, args, span } if name == "recv" => {
                if let Some(a) = args.first() {
                    match Env::base_var(a) {
                        Some(b) => Self::read_var(env, b, *span),
                        None => Self::eval_value(env, a),
                    }
                }
            }
            Expr::Call { name, args, span } if name == "close" => {
                if let Some(a) = args.first() {
                    match Env::base_var(a) {
                        Some(b) => Self::use_var(env, b, *span),
                        None => Self::eval_value(env, a),
                    }
                }
            }
            Expr::Call { name, args, span } => {
                let modes = env.sigs.get(name).cloned();
                for (i, arg) in args.iter().enumerate() {
                    let mode = modes.as_ref().and_then(|m| m.get(i).copied());
                    match mode {
                        Some(Mode::Ref) => {
                            Self::arg_borrow(env, arg, false, *span);
                        }
                        Some(Mode::Mut) => {
                            Self::arg_borrow(env, arg, true, *span);
                        }
                        // `own` (or unknown callee) => move/consume the argument.
                        _ => Self::eval_value(env, arg),
                    }
                }
            }
            // Sequential baseline: the task body runs inline; `join` just
            // reads the handle. Concurrency semantics live in the affine
            // checker — this model shows what sequential reasoning sees.
            Expr::Spawn { body, .. } => Self::check_block(env, body),
            Expr::Join { inner, .. } => Self::eval_value(env, inner),
            Expr::Binary { lhs, rhs, .. } => {
                Self::eval_value(env, lhs);
                Self::eval_value(env, rhs);
            }
        }
    }

    /// A borrow passed as a `ref`/`mut` call argument: check but don't persist.
    fn arg_borrow(env: &mut Env, arg: &Expr, is_mut: bool, span: Span) {
        match arg {
            Expr::Ref(place, s) => Self::take_temp_borrow(env, place, false, *s),
            Expr::MutRef(place, s) => Self::take_temp_borrow(env, place, true, *s),
            // Passing a plain variable to a borrowing parameter borrows it.
            Expr::Var(name, s) => {
                Self::check_borrow_legal(env, name, is_mut, *s);
            }
            other => Self::eval_value(env, other),
        }
        let _ = span;
    }

    fn use_var(env: &mut Env, name: &str, span: Span) {
        let (copy, moved) = match env.vars.get(name) {
            Some(v) => (v.copy, v.moved),
            None => return, // unknown var; type/scope checking is out of scope here
        };
        if let Some(mspan) = moved {
            env.diags.push(Diagnostic::new(
                span,
                "use-after-move",
                format!("`{name}` used here but was moved/freed at {mspan}"),
            ));
            return;
        }
        if copy {
            // Reading a Copy value: illegal only if exclusively borrowed.
            if env.vars.get(name).map(|v| v.mutb > 0).unwrap_or(false) {
                env.diags.push(Diagnostic::new(
                    span,
                    "alias",
                    format!("cannot use `{name}` while it is mutably borrowed"),
                ));
            }
        } else {
            // Moving a non-Copy value: illegal if borrowed at all.
            let info = env.vars.get(name).unwrap();
            if info.shared > 0 || info.mutb > 0 {
                env.diags.push(Diagnostic::new(
                    span,
                    "alias",
                    format!("cannot move `{name}` while it is borrowed"),
                ));
            }
            env.vars.get_mut(name).unwrap().moved = Some(span);
        }
    }

    fn read_var(env: &mut Env, name: &str, span: Span) {
        match env.vars.get(name) {
            Some(v) if v.moved.is_some() => {
                let mspan = v.moved.unwrap();
                env.diags.push(Diagnostic::new(
                    span,
                    "use-after-move",
                    format!("`{name}` read here but was moved/freed at {mspan}"),
                ));
            }
            Some(v) if v.mutb > 0 => {
                env.diags.push(Diagnostic::new(
                    span,
                    "alias",
                    format!("cannot read `{name}` while it is mutably borrowed"),
                ));
            }
            _ => {}
        }
    }

    /// Register a persistent borrow held by a `let` binding.
    fn take_borrow(env: &mut Env, place: &Expr, is_mut: bool, span: Span) {
        if let Some(base) = Env::base_var(place) {
            if Self::check_borrow_legal(env, base, is_mut, span) {
                if let Some(info) = env.vars.get_mut(base) {
                    if is_mut {
                        info.mutb += 1;
                    } else {
                        info.shared += 1;
                    }
                }
            }
        }
    }

    /// A temporary borrow (function argument, etc.): checked, not persisted.
    fn take_temp_borrow(env: &mut Env, place: &Expr, is_mut: bool, span: Span) {
        if let Some(base) = Env::base_var(place) {
            Self::check_borrow_legal(env, base, is_mut, span);
        } else {
            Self::eval_value(env, place);
        }
    }

    /// Returns true if the borrow is legal (and records no error).
    fn check_borrow_legal(env: &mut Env, base: &str, is_mut: bool, span: Span) -> bool {
        let info = match env.vars.get(base) {
            Some(i) => i,
            None => return true,
        };
        if let Some(mspan) = info.moved {
            env.diags.push(Diagnostic::new(
                span,
                "use-after-move",
                format!("cannot borrow `{base}`: moved/freed at {mspan}"),
            ));
            return false;
        }
        if is_mut {
            if info.shared > 0 || info.mutb > 0 {
                env.diags.push(Diagnostic::new(
                    span,
                    "alias",
                    format!("cannot mutably borrow `{base}`: it is already borrowed"),
                ));
                return false;
            }
        } else if info.mutb > 0 {
            env.diags.push(Diagnostic::new(
                span,
                "alias",
                format!("cannot borrow `{base}`: it is already mutably borrowed"),
            ));
            return false;
        }
        true
    }
}

fn is_primitive(ty: &Type) -> bool {
    matches!(ty, Type::Named(n) if is_primitive_name(n))
}

fn is_primitive_name(n: &str) -> bool {
    matches!(
        n,
        "u8" | "u16" | "u32" | "u64" | "i8" | "i16" | "i32" | "i64" | "bool"
    )
}

impl MemoryModel for Ownership {
    fn name(&self) -> &'static str {
        "ownership"
    }

    fn tagline(&self) -> &'static str {
        "moves + borrows; catches use-after-free and aliasing (Rust-like)"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            states: &[
                "owned/live",
                "moved-out",
                "shared-borrowed",
                "mutably-borrowed",
                "borrow-expired",
                "consumed (freed)",
            ],
            resource_states: &[],
            temporal_states: 1, // moved-out is the only temporal-ish state
            lifetime_concepts: 1, // borrow-scope tracking
            move_rules: 4,      // move-out, borrow-shared, borrow-mut, consume
            rules: 3,
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
