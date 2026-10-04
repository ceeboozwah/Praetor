//! Source-level metrics used to compare the ergonomic cost of each model.
//!
//! The lab uses ONE source language, so the raw annotation counts are shared.
//! What differs is *which* annotations each model actually relies on — so we
//! break the burden down by kind and let each model claim its relevant subset.

use crate::ast::*;

#[derive(Debug, Clone, Default)]
pub struct Annotations {
    /// `ref`/`mut` parameter modes (explicit access annotations; `own` is default).
    pub modes: u32,
    /// `&` / `&mut` reference-forming expressions.
    pub refs: u32,
    /// `region NAME { .. }` scopes plus `new(r)` region allocations.
    pub regions: u32,
    /// Explicit `free` statements.
    pub frees: u32,
    /// `promote` / `handle` persistence primitives.
    pub rehomes: u32,
    /// `spawn` / `join` task constructs (the concurrency vocabulary).
    pub spawns: u32,
}

impl Annotations {
    /// Annotation burden actually imposed by a given model: the sum of the
    /// annotation *kinds* that model relies on. (All models share the source, so
    /// this attributes the shared annotations to the models that consume them;
    /// a per-model rewrite study is future work.)
    pub fn for_model(&self, model: &str) -> u32 {
        match model {
            "ownership" => self.modes + self.refs + self.frees,
            "capability" => self.modes + self.refs,
            "region" => self.regions + self.rehomes,
            "affine" => self.frees + self.spawns,
            // Region + Capability draws on both lifetime and access annotations.
            "region+capability" => self.modes + self.refs + self.regions + self.rehomes,
            // Region + Ownership.
            "region+ownership" => {
                self.modes + self.refs + self.frees + self.regions + self.rehomes
            }
            // Region + Capability + Affine: everything except that affine uses
            // `free`, which F already counts via capability's modes+refs plus
            // region annotations plus consume points — plus the concurrency
            // vocabulary (spawn/join), which only the affine layer interprets.
            "region+cap+affine" => {
                self.modes + self.refs + self.regions + self.rehomes + self.frees + self.spawns
            }
            _ => self.modes + self.refs + self.regions + self.frees + self.rehomes + self.spawns,
        }
    }

    pub fn total(&self) -> u32 {
        self.modes + self.refs + self.regions + self.frees + self.rehomes + self.spawns
    }
}

#[derive(Debug, Clone, Default)]
pub struct SourceMetrics {
    pub source_lines: u32,
    pub ast_nodes: u32,
    /// Total lexical tokens in the source (excluding EOF; comments aren't tokens).
    pub total_tokens: u32,
    pub annotations: Annotations,
}

impl SourceMetrics {
    /// Annotation burden A = memory-management annotations / total source tokens,
    /// for a given model. A crude but comparable "how much of the program is
    /// devoted to talking to the memory model" figure.
    pub fn burden(&self, model: &str) -> f64 {
        if self.total_tokens == 0 {
            0.0
        } else {
            self.annotations.for_model(model) as f64 / self.total_tokens as f64
        }
    }
}

pub fn measure(program: &Program, source: &str) -> SourceMetrics {
    let mut m = SourceMetrics::default();
    m.source_lines = source
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with("//")
        })
        .count() as u32;
    m.total_tokens = crate::lexer::lex(source)
        .map(|toks| toks.len().saturating_sub(1) as u32) // drop the trailing Eof
        .unwrap_or(0);
    for item in &program.items {
        match item {
            Item::Fn(f) => count_fn(f, &mut m),
            Item::Struct(s) => {
                m.ast_nodes += 1 + s.fields.len() as u32;
            }
        }
    }
    m
}

fn count_fn(f: &FnDef, m: &mut SourceMetrics) {
    m.ast_nodes += 1;
    for p in &f.params {
        m.ast_nodes += 1;
        if matches!(p.mode, Mode::Ref | Mode::Mut) {
            m.annotations.modes += 1;
        }
    }
    count_block(&f.body, m);
}

fn count_block(b: &Block, m: &mut SourceMetrics) {
    for s in &b.stmts {
        count_stmt(s, m);
    }
}

fn count_stmt(s: &Stmt, m: &mut SourceMetrics) {
    m.ast_nodes += 1;
    match s {
        Stmt::Let { init, .. } => count_expr(init, m),
        Stmt::Assign { target, value, .. } => {
            count_expr(target, m);
            count_expr(value, m);
        }
        Stmt::Return { value: Some(v), .. } => count_expr(v, m),
        Stmt::Return { value: None, .. } => {}
        Stmt::If {
            cond,
            then_block,
            else_block,
            ..
        } => {
            count_expr(cond, m);
            count_block(then_block, m);
            if let Some(eb) = else_block {
                count_block(eb, m);
            }
        }
        Stmt::While { cond, body, .. } => {
            count_expr(cond, m);
            count_block(body, m);
        }
        Stmt::Region { body, .. } => {
            m.annotations.regions += 1;
            count_block(body, m);
        }
        Stmt::Free { value, .. } => {
            m.annotations.frees += 1;
            count_expr(value, m);
        }
        Stmt::Expr { expr, .. } => count_expr(expr, m),
    }
}

fn count_expr(e: &Expr, m: &mut SourceMetrics) {
    m.ast_nodes += 1;
    match e {
        Expr::Int(..) | Expr::Bool(..) | Expr::Var(..) => {}
        Expr::Ref(inner, _) => {
            m.annotations.refs += 1;
            count_expr(inner, m);
        }
        Expr::MutRef(inner, _) => {
            m.annotations.refs += 1;
            count_expr(inner, m);
        }
        Expr::Deref(inner, _) => count_expr(inner, m),
        Expr::Field(base, _, _) => count_expr(base, m),
        Expr::Alloc { region, .. } => {
            if region.is_some() {
                m.annotations.regions += 1;
            }
        }
        Expr::Persist { inner, .. } => {
            m.annotations.rehomes += 1;
            count_expr(inner, m);
        }
        Expr::StructLit { fields, .. } => {
            for (_, v) in fields {
                count_expr(v, m);
            }
        }
        Expr::Call { args, .. } => {
            for a in args {
                count_expr(a, m);
            }
        }
        Expr::Spawn { body, .. } => {
            m.annotations.spawns += 1;
            count_block(body, m);
        }
        Expr::Join { inner, .. } => {
            m.annotations.spawns += 1;
            count_expr(inner, m);
        }
        Expr::Binary { lhs, rhs, .. } => {
            count_expr(lhs, m);
            count_expr(rhs, m);
        }
    }
}
