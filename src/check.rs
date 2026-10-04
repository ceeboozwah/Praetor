//! Shared checker infrastructure: diagnostics and the `MemoryModel` trait.
//!
//! Every memory model is a static analysis that consumes the *same* parsed
//! `Program` and returns a set of `Diagnostic`s. An empty diagnostic set means
//! the model *accepts* the program; any diagnostic means it *rejects* it.
//!
//! Because all models share one source language, differences in their output on
//! the same file are precisely the experimental signal the lab exists to study.

use crate::ast::{FnDef, Mode, Program, Span};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub span: Span,
    /// Short category, e.g. "use-after-move", "region-escape", "alias".
    pub kind: &'static str,
    pub message: String,
}

impl Diagnostic {
    pub fn new(span: Span, kind: &'static str, message: impl Into<String>) -> Self {
        Diagnostic {
            span,
            kind,
            message: message.into(),
        }
    }
}

#[derive(Debug, Clone)]
pub struct CheckResult {
    pub model: &'static str,
    pub diagnostics: Vec<Diagnostic>,
}

impl CheckResult {
    pub fn accepted(&self) -> bool {
        self.diagnostics.is_empty()
    }
}

/// Design-complexity profile of a model — the "state-space burden" (B_S) and
/// the machinery each checker carries. These are properties of the model's
/// *design*, not of any program, so they're constant per model.
///
///   - `states`: programmer-visible memory states for ORDINARY memory
///   - `resource_states`: EXTRA states only consumables carry (may be empty;
///     the F hypothesis is that temporal state needn't apply to all values)
///   - `temporal_states`: how many of those are temporal (consume-timeline)
///   - `lifetime_concepts`: lifetime-like notions the programmer must hold
///     (region nesting counts; Rust-style inferred lifetimes would count too)
///   - `move_rules`: ownership/borrow-tracking rules in the checker's semantics
///   - `path_sensitive`: whether the checker merges state across control flow
#[derive(Debug, Clone)]
pub struct Profile {
    pub states: &'static [&'static str],
    pub resource_states: &'static [&'static str],
    pub temporal_states: usize,
    pub lifetime_concepts: usize,
    pub move_rules: usize,
    /// Number of distinct reject-rules the checker enforces (its diagnostic
    /// kinds). The rule-growth metric: each frontier that forces new rules
    /// should show up here, so we can see whether complexity stays bounded.
    pub rules: usize,
    pub path_sensitive: bool,
    /// Compiler-internal machinery the model carries that is NOT a
    /// programmer-visible rule (e.g. "alias-edge-graph"). Honest accounting:
    /// precision bought inside the compiler still costs compiler complexity,
    /// and this field keeps that visible instead of hiding it.
    pub machinery: &'static [&'static str],
}

impl Profile {
    /// Total programmer-visible state count (B_S).
    pub fn state_count(&self) -> usize {
        self.states.len() + self.resource_states.len()
    }
}

/// A compile-time memory-safety model under evaluation.
pub trait MemoryModel {
    fn name(&self) -> &'static str;
    /// One-line description of what this model reasons about.
    fn tagline(&self) -> &'static str;
    fn check(&self, program: &Program) -> Vec<Diagnostic>;

    /// The model's complexity profile — the cognitive + machinery cost it
    /// imposes, independent of any particular program.
    fn profile(&self) -> Profile;

    fn run(&self, program: &Program) -> CheckResult {
        CheckResult {
            model: self.name(),
            diagnostics: self.check(program),
        }
    }
}

/// Signature info shared by checkers: maps a function name to its parameter modes.
pub fn signature_modes(program: &Program) -> HashMap<String, Vec<Mode>> {
    program
        .functions()
        .map(|f: &FnDef| {
            (
                f.name.clone(),
                f.params.iter().map(|p| p.mode).collect::<Vec<_>>(),
            )
        })
        .collect()
}
