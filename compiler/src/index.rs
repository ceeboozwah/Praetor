//! N36B — the symbol index: occurrences the checker actually
//! resolved, recorded as a side channel (the `SOURCES` pattern) so no
//! pipeline or `check_module` signature changes. The LSP consumes
//! this — it is compiler truth, not a second resolver.
//!
//! One `Occ` = a declaration or a use: its own span, the resolved
//! name, a kind tag, hover text built from the resolved type or
//! signature, and — for uses — the declaration site it resolves to.
//! `file` is stamped by the pipeline after each module's check (the
//! checker never knows which file it's in); `def.1` carries the
//! declaring file for imported symbols.

use crate::Span;
use std::cell::RefCell;

/// A declaration site exported through `ModuleInterface::locs` —
/// what an imported name's occurrences resolve TO.
#[derive(Debug, Clone)]
pub struct DefLoc {
    pub span: Span,
    /// Set by the pipeline once the module's file is known.
    pub file: Option<String>,
    /// Hover text, e.g. `fn push(own self: Vec<T>, x: T) -> void`.
    pub sig: String,
    pub kind: &'static str,
    /// Struct fields `(name, rendered type)` — N36C member completion.
    pub fields: Vec<(String, String)>,
    /// Enum variant names — N36C `E::` completion.
    pub variants: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct Occ {
    /// The occurrence's own site (the token's start).
    pub span: Span,
    /// Display name — source form for locals, qualified for imports.
    pub name: String,
    /// "fn" | "struct" | "enum" | "variant" | "field" | "local" |
    /// "param" | "module" | "builtin"
    pub kind: &'static str,
    /// Rendered from the RESOLVED type/signature — never re-derived.
    pub hover: String,
    /// `use` occurrences: the declaration site + its file (`None` =
    /// same file as the occurrence). `None` on declarations — they
    /// are the site.
    pub def: Option<(Span, Option<String>)>,
    /// The file this occurrence appears in — pipeline-stamped.
    pub file: Option<String>,
}

thread_local! {
    static OCCS: RefCell<Vec<Occ>> = RefCell::new(Vec::new());
}

/// Record one resolved occurrence. Internal names (`$rv0`, `$arg1`,
/// `$tmp`) and synthetic zero-position spans never enter the index.
pub fn record(o: Occ) {
    if o.span.line == 0 || o.name.starts_with('$') {
        return;
    }
    OCCS.with(|v| v.borrow_mut().push(o));
}

/// Take every occurrence recorded since the last drain — the
/// pipeline calls this after each module's `check_module` to stamp
/// `file` onto that module's batch.
pub fn drain() -> Vec<Occ> {
    OCCS.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

// The declaration table for the module currently being checked —
// local decls merged with the direct imports' exported `DefLoc`s.
// `check_module` installs it at entry; use-sites resolve through it.
// Same thread-local discipline as `OCCS`.
thread_local! {
    static LOCS: RefCell<std::collections::HashMap<String, DefLoc>> =
        RefCell::new(std::collections::HashMap::new());
}

/// Install `locs` as the current module's declaration table.
pub fn enter_module(locs: std::collections::HashMap<String, DefLoc>) {
    LOCS.with(|m| *m.borrow_mut() = locs);
}

/// The declaration site + signature for a resolved name, if any.
pub fn def(name: &str) -> Option<DefLoc> {
    LOCS.with(|m| m.borrow().get(name).cloned())
}

/// Every name in the last-installed module table — N36C completion
/// enumerates the candidates the checker considered in scope.
pub fn defs() -> Vec<(String, DefLoc)> {
    LOCS.with(|m| m.borrow().iter().map(|(k, v)| (k.clone(), v.clone())).collect())
}

/// Drop BOTH channels — pipeline entry calls this so a parse-failed
/// buffer can't inherit the previous check's declaration table.
pub fn reset() {
    drain();
    LOCS.with(|m| m.borrow_mut().clear());
}
