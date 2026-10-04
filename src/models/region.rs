//! Region-based memory model (escape analysis).
//!
//! Allocations are tied to a lexical `region` scope. When the region ends, all
//! of its allocations are released together — no per-object lifetime tracking.
//! The single obligation the programmer must satisfy is: *a reference into a
//! region must not outlive that region.*
//!
//!   `new(r) T` -> allocate in region `r`
//!   `new T`    -> heap allocation (outlives all regions; always safe to keep)
//!   `region r { .. }` -> opens region `r` for the duration of the block
//!
//! What this model proves:
//!   * region escape via assignment to a longer-lived variable
//!   * region escape via `return` of a region pointer
//!   * allocation into an unknown/closed region
//!
//! What it deliberately does NOT reason about (an honest divergence from the
//! ownership model): aliasing and manual `free`. Regions make `free`
//! unnecessary, so `free` is ignored here and use-after-free of a manual
//! allocation is *not* caught — a difference the corpus is designed to expose.
//!
//! The mental model is intentionally lighter than ownership: there are no
//! moves, no borrows, no exclusive-vs-shared distinction — only "does this
//! reference stay inside its region?".

use crate::ast::*;
use crate::check::{Diagnostic, MemoryModel};
use std::collections::HashMap;

pub struct Region;

/// Region level 0 means "not in any region" (a plain value or a heap
/// allocation that outlives every region). Level N (>=1) is the Nth currently
/// open region, counting from the outermost.
type Level = usize;

struct VarInfo {
    /// Which region the variable's value points into (0 = none / heap / plain).
    points_to: Level,
    /// Deepest region reachable through this value's *stored* fields
    /// (0 = no edge into any region). Used to prove `promote` edge-closure:
    /// a promoted object may not carry edges into regions that still die.
    edges_to: Level,
    /// This binding is a reference — `promote` cannot move its referent.
    is_ref: bool,
    /// Region depth in effect when the variable was declared.
    decl_depth: Level,
}

struct Env {
    vars: HashMap<String, VarInfo>,
    scopes: Vec<Vec<String>>,
    /// Stack of open region names; `region_stack.len()` is the current depth.
    region_stack: Vec<String>,
    diags: Vec<Diagnostic>,
}

impl Env {
    fn depth(&self) -> Level {
        self.region_stack.len()
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        if let Some(names) = self.scopes.pop() {
            for n in names {
                self.vars.remove(&n);
            }
        }
    }

    fn declare(&mut self, name: String, points_to: Level) {
        let decl_depth = self.depth();
        if let Some(s) = self.scopes.last_mut() {
            s.push(name.clone());
        }
        self.vars.insert(
            name,
            VarInfo {
                points_to,
                edges_to: 0,
                is_ref: false,
                decl_depth,
            },
        );
    }

    fn region_level(&self, name: &str) -> Option<Level> {
        self.region_stack
            .iter()
            .position(|r| r == name)
            .map(|i| i + 1)
    }
}

impl Region {
    fn check_fn(env: &mut Env, f: &FnDef) {
        env.push_scope();
        for p in &f.params {
            // Parameters come from the caller; they never point into a local region.
            env.declare(p.name.clone(), 0);
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
                let level = Self::level_of(env, init);
                let edges = Self::edge_level_of(env, init);
                let is_ref = Self::is_ref_expr(env, init);
                env.declare(name.clone(), level);
                if let Some(i) = env.vars.get_mut(name) {
                    i.edges_to = edges;
                    i.is_ref = is_ref;
                }
            }
            Stmt::Assign { target, value, span } => {
                let vlevel = Self::level_of(env, value);
                let vedges = Self::edge_level_of(env, value);
                if let Some(base) = base_var(target) {
                    if let Some(info) = env.vars.get(base) {
                        // The target outlives region `vlevel` if it was declared
                        // at a shallower region depth than that region.
                        if vlevel > info.decl_depth {
                            let rname = env
                                .region_stack
                                .get(vlevel - 1)
                                .cloned()
                                .unwrap_or_default();
                            env.diags.push(Diagnostic::new(
                                *span,
                                "region-escape",
                                format!(
                                    "`{base}` outlives region `{rname}` but is assigned a \
                                     reference into it"
                                ),
                            ));
                        } else if matches!(target, Expr::Var(..)) {
                            // Whole-variable rebinding: replace the value, its
                            // edges, and whether it is a borrowed alias.
                            let v_is_ref = Self::is_ref_expr(env, value);
                            let i = env.vars.get_mut(base).unwrap();
                            i.points_to = vlevel;
                            i.edges_to = vedges;
                            i.is_ref = v_is_ref;
                        } else {
                            // Field store (`x.f = v`): x gains an edge reaching
                            // as deep as v's stored content.
                            env.vars.get_mut(base).unwrap().edges_to =
                                info.edges_to.max(vedges);
                        }
                    }
                }
            }
            Stmt::Return { value: Some(v), span } => {
                let vlevel = Self::level_of(env, v);
                if vlevel >= 1 {
                    let rname = env.region_stack.get(vlevel - 1).cloned().unwrap_or_default();
                    env.diags.push(Diagnostic::new(
                        *span,
                        "region-escape",
                        format!("returning a reference into local region `{rname}`"),
                    ));
                }
            }
            Stmt::Return { value: None, .. } => {}
            Stmt::If {
                cond,
                then_block,
                else_block,
                ..
            } => {
                let _ = Self::level_of(env, cond);
                Self::check_block(env, then_block);
                if let Some(eb) = else_block {
                    Self::check_block(env, eb);
                }
            }
            Stmt::While { cond, body, .. } => {
                let _ = Self::level_of(env, cond);
                Self::check_block(env, body);
            }
            Stmt::Region { name, body, .. } => {
                env.region_stack.push(name.clone());
                Self::check_block(env, body);
                env.region_stack.pop();
            }
            // Regions auto-free; manual `free` is ignored by this model.
            Stmt::Free { .. } => {}
            Stmt::Expr { expr, .. } => {
                let _ = Self::level_of(env, expr);
            }
        }
    }

    /// Compute the region level of the value produced by an expression.
    fn level_of(env: &mut Env, e: &Expr) -> Level {
        match e {
            Expr::Alloc { region: Some(r), span, .. } => match env.region_level(r) {
                Some(l) => l,
                None => {
                    env.diags.push(Diagnostic::new(
                        *span,
                        "unknown-region",
                        format!("allocation into unknown or closed region `{r}`"),
                    ));
                    0
                }
            },
            Expr::Alloc { region: None, .. } => 0, // heap: outlives all regions
            // `promote` re-homes a value out of its region — but only if the
            // object carries no forbidden edges. Edge-closure rule (added for
            // the graph-surgery corpus, scenarios 62/63/68):
            //   * a reference binding into a region cannot be promoted — the
            //     referent does not move, so the promoted ref would dangle
            //   * a value that still stores edges into a region cannot be
            //     promoted — those edges die with the region
            // `handle e` needs no such rule: a handle is a generation-checked
            // lookup, not a lifetime-tracked pointer.
            Expr::Persist { kind, inner, span } => {
                let lvl = Self::level_of(env, inner);
                if *kind == PersistKind::Promote {
                    let bad = match base_var(inner) {
                        Some(b) => env.vars.get(b).map(|i| {
                            (i.is_ref && i.points_to > 0) || i.edges_to > 0
                        }).unwrap_or(false),
                        None => lvl > 0 || Self::edge_level_of(env, inner) > 0,
                    };
                    if bad {
                        env.diags.push(Diagnostic::new(
                            *span,
                            "dangling-edge-after-promote",
                            "promote would leave edges pointing into a dead region"
                                .to_string(),
                        ));
                    }
                }
                0
            }
            // A variable's escape level is the deepest region reachable through
            // it: its own target (points_to) OR any stored field edge
            // (edges_to). Returning/storing it must keep both alive.
            Expr::Var(name, _) => env
                .vars
                .get(name)
                .map(|v| v.points_to.max(v.edges_to))
                .unwrap_or(0),
            Expr::Ref(inner, _) | Expr::MutRef(inner, _) | Expr::Deref(inner, _) => {
                Self::level_of(env, inner)
            }
            Expr::Field(base, _, _) => Self::level_of(env, base),
            Expr::StructLit { fields, .. } => fields
                .iter()
                .map(|(_, v)| Self::level_of(env, v))
                .max()
                .unwrap_or(0),
            Expr::Binary { lhs, rhs, .. } => {
                Self::level_of(env, lhs).max(Self::level_of(env, rhs))
            }
            // Call return values are conservatively treated as region-free.
            Expr::Call { args, .. } => {
                for a in args {
                    let _ = Self::level_of(env, a);
                }
                0
            }
            // Sequential baseline: the task body is checked where it textually
            // appears; the join handle and its result are region-free.
            Expr::Spawn { body, .. } => {
                Self::check_block(env, body);
                0
            }
            Expr::Join { inner, .. } => {
                let _ = Self::level_of(env, inner);
                0
            }
            Expr::Int(..) | Expr::Bool(..) => 0,
        }
    }

    /// Deepest region level reachable by *storing* this value into a field —
    /// i.e. the level of the references it carries. Distinct from `level_of`,
    /// which is where the value's storage lives.
    fn edge_level_of(env: &mut Env, e: &Expr) -> Level {
        match e {
            Expr::Ref(place, _) | Expr::MutRef(place, _) => Self::level_of(env, place),
            Expr::Var(name, _) => env
                .vars
                .get(name)
                .map(|i| {
                    if i.is_ref {
                        i.points_to.max(i.edges_to)
                    } else {
                        i.edges_to
                    }
                })
                .unwrap_or(0),
            Expr::Field(base, _, _) | Expr::Deref(base, _) => Self::edge_level_of(env, base),
            Expr::StructLit { fields, .. } => fields
                .iter()
                .map(|(_, v)| Self::edge_level_of(env, v).max(Self::level_of(env, v)))
                .max()
                .unwrap_or(0),
            Expr::Persist { .. } => 0, // re-homed: carries no region edges
            _ => 0,
        }
    }

    /// Is this expression a reference-typed value?
    fn is_ref_expr(env: &Env, e: &Expr) -> bool {
        match e {
            Expr::Ref(..) | Expr::MutRef(..) => true,
            Expr::Var(name, _) => env.vars.get(name).map(|i| i.is_ref).unwrap_or(false),
            Expr::Field(base, _, _) | Expr::Deref(base, _) => Self::is_ref_expr(env, base),
            _ => false,
        }
    }
}

fn base_var(e: &Expr) -> Option<&str> {
    match e {
        Expr::Var(n, _) => Some(n),
        Expr::Field(b, _, _) | Expr::Deref(b, _) => base_var(b),
        _ => None,
    }
}

impl MemoryModel for Region {
    fn name(&self) -> &'static str {
        "region"
    }

    fn tagline(&self) -> &'static str {
        "lexical allocation scopes; catches references escaping their region"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            states: &["region-alive", "region-dead"],
            resource_states: &[],
            temporal_states: 0,
            lifetime_concepts: 1, // region nesting
            move_rules: 0,
            rules: 3, // escape + unknown-region + dangling-edge-after-promote
            path_sensitive: false,
            machinery: &["edge-levels(points_to/edges_to)"],
        }
    }

    fn check(&self, program: &Program) -> Vec<Diagnostic> {
        let mut env = Env {
            vars: HashMap::new(),
            scopes: Vec::new(),
            region_stack: Vec::new(),
            diags: Vec::new(),
        };
        for f in program.functions() {
            Self::check_fn(&mut env, f);
        }
        env.diags
    }
}
