//! Minimal affine temporal checker — the "temporal state" component of
//! Prototype F.
//!
//! The hypothesis under test is that temporal safety (use-after-close,
//! double-close, move-after-consume) does NOT require a full move/borrow
//! system. A consumable value needs only:
//!
//!     Live ──free/consume──▶ Consumed ──any use──▶ ERROR
//!
//! plus one more state that exists ONLY because control flow joins:
//!
//!     MaybeConsumed = merge(Live, Consumed)
//!
//! i.e. the state after `if c { free f }` — cannot be proven Live, so any use
//! or re-consume is rejected.
//!
//! Deliberately minimal — what this model is NOT:
//!   * no borrow counting (that's the capability/ownership models' job)
//!   * no move-out tracking on plain memory (region values aren't consumable)
//!   * no lifetime parameters — a consumed binding just can't be used
//!
//! Extra rules that fall out of the decomposition:
//!   * `free` of region-managed memory is a category error (regions free
//!     collectively; individual free would double-release)
//!   * consuming through an alias binding (`let a = &f; free a`) is illegal
//!   * consuming while a *stored* alias exists (`h.r = &f; free f`) is illegal
//!   * `handle e` does NOT prevent `free e` — generation-checked handles are
//!     designed to outlive their referent
//!
//! Aliases are tracked as an EDGE GRAPH, not a counter: each live alias is an
//! edge (holder, field?, target) that DIES when its holder's scope ends, when
//! the holder is rebound, when the field is overwritten, or when the target is
//! consumed. Branch merges take the union of per-path edge sets — a
//! compiler-internal "possible alias" that is NOT a programmer-visible state.
//!
//! INTERPROCEDURAL: each function is summarized once as a compact Effect —
//! edges it creates/kills on its parameters, params it (maybe) consumes, and
//! which params its return value may alias. Callers apply the summary to
//! their own graph: a call is a graph transformation, not a re-analysis.
//! The call graph decomposes into SCCs; recursive components iterate a
//! monotone union to a fixed point.
//!
//! CONCURRENCY: `spawn { body }` analyses the body in a forked env and
//! records a TaskResult — which shared vars it holds (Shared = read,
//! Write = written, Transferred = moved/consumed), its definite kills, its
//! surviving edges, its return value. `join t` applies that result as a
//! deferred graph transform. A var a live task still holds cannot go out
//! of scope; read-vs-Shared is the only legal overlap inside a window.
//!
//! Path sensitivity IS provided — this is the model's one real machinery cost.
//! `if`/`else` states merge pointwise; loops re-check the body under the merged
//! (back-edge) state so `while c { free f }` is caught as a second-iteration
//! double-consume.

use crate::ast::*;
use crate::check::{Diagnostic, MemoryModel};
use std::collections::{HashMap, HashSet};

/// Why a value is being consumed — the distinction matters: only `free`
/// requires the target to be a genuine resource, while moves/promotes of
/// Copy-typed values are plain reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Op {
    /// Explicit `free`/`close`/`release`.
    Free,
    /// Ownership transfer (move into a callee, `promote`, rebinding).
    Move,
    /// `unlock g` — releases a lock guard's synchronized authority.
    Unlock,
    /// `close ch` — consumes a channel endpoint.
    Close,
}

impl Op {
    fn name(self) -> &'static str {
        match self {
            Op::Free => "free",
            Op::Move => "move",
            Op::Unlock => "unlock",
            Op::Close => "close",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Temp {
    Live,
    Consumed,
    /// Cannot prove Live: produced by merging control-flow paths.
    Maybe,
}

fn merge_temp(a: Temp, b: Temp) -> Temp {
    if a == b {
        a
    } else {
        Temp::Maybe
    }
}

#[derive(Debug, Clone)]
struct Info {
    state: Temp,
    /// This binding IS a reference — it cannot be consumed, only read.
    is_ref: bool,
    /// Copy-typed (primitive): consume rules don't apply.
    copy: bool,
    /// Allocated with `new(r)` — region-managed, so individual `free` is illegal.
    region_managed: bool,
    /// This binding is a live task handle — the id keys `env.tasks`.
    task_id: Option<u32>,
    /// This binding is a handle (persistent identity ticket). Handles are
    /// copyable like plain values but writes/reads THROUGH them resolve to
    /// the capability-edge target — `h.fd = 1` writes the target, not `h`.
    is_handle: bool,
    /// Declared `atomic<T>` (or `new atomic<T>`): this storage may be
    /// accessed through the atomic builtins. Ordinary access stays under
    /// normal capability/race rules — atomicity is an operation-level
    /// permission, not a property that relaxes the storage.
    is_atomic: bool,
    /// Declared `mutex<T>` (or `mutex e`): this storage is a
    /// synchronization domain. Its interior is reachable ONLY through a
    /// live guard derived by `lock` — the mutex never exposes `T` directly.
    is_mutex: bool,
    /// This binding is a live lock guard — the temporary authority token
    /// `lock` derives. Affine: `unlock`/scope-exit consumes it like any
    /// resource. Its writes go through `sync_writes` under `guard_domain`.
    is_guard: bool,
    /// The synchronization domain this guard holds (e.g. `m`, `s.a`).
    guard_domain: Option<String>,
    /// Declared `channel<T>` (or `channel()` result): an endpoint that
    /// transports authority packets.
    is_channel: bool,
    /// The binding's named struct type (`S`), when known — used to resolve
    /// `s.cnt` against `atomic_fields` for field-level atomicity.
    ty_name: Option<String>,
}

impl Info {
    fn live() -> Self {
        Info {
            state: Temp::Live,
            is_ref: false,
            copy: false,
            region_managed: false,
            task_id: None,
            is_handle: false,
            is_atomic: false,
            is_mutex: false,
            is_guard: false,
            guard_domain: None,
            is_channel: false,
            ty_name: None,
        }
    }
}

/// (param modes, does it return a reference?)
type SigMap = HashMap<String, (Vec<Mode>, bool)>;

fn signatures(program: &Program) -> SigMap {
    program
        .functions()
        .map(|f| {
            (
                f.name.clone(),
                (
                    f.params.iter().map(|p| p.mode).collect::<Vec<_>>(),
                    matches!(f.ret, Some(Type::Ref(_)) | Some(Type::MutRef(_))),
                ),
            )
        })
        .collect()
}

/// A live alias edge: `(holder, field, target)`.
///   * `field = None`  — the binding itself is the alias (`let a = &x`)
///   * `field = Some(p)` — a reference lives in `holder.p` (`h.r = &x`)
/// Compiler-internal only: no programmer-visible "aliased" state exists.
type AliasEdge = (String, Option<String>, String);

/// A function's memory-effect summary — its caller-visible transformation of
/// the alias/temporal graph, expressed in parameter indices. Computed once,
/// callee-first; a call applies it as `G_before -> G_after`.
#[derive(Debug, Clone, Default)]
pub struct Effect {
    /// `(param, path)`: field paths overwritten on a param holder
    /// (path "*" = the whole binding was rebound).
    pub kills: Vec<(usize, String)>,
    /// `(holder-param, field, target-param)`: edges live at function exit
    /// that bridge two parameters.
    pub creates: Vec<(usize, Option<String>, usize)>,
    /// Params left `Consumed`.
    pub consumes: Vec<usize>,
    /// Params left `MaybeConsumed` (consumed on some paths only).
    pub maybe: Vec<usize>,
    /// Params the return value may alias.
    pub ret_alias: Vec<usize>,
    /// `(param, domain-suffix)`: writes the callee performed under a lock
    /// guard derived from that param. The suffix is the domain minus the
    /// param's own name (`""` = the param itself, `".a"` = its field).
    /// Callers replay these as synchronized writes on the actual's
    /// resolved mutex root — a `mut` arg with a sync fact does NOT count
    /// as an unsynchronized write.
    pub syncs: Vec<(usize, String)>,
}

impl Effect {
    /// The summary-complexity metric: how many facts a caller must apply.
    pub fn fact_count(&self) -> usize {
        self.kills.len() + self.creates.len() + self.consumes.len()
            + self.maybe.len() + self.ret_alias.len() + self.syncs.len()
    }

    /// Union-absorb another pass's facts into this summary (the fixpoint's
    /// monotone join: every fact class is a may-set, so facts only ever
    /// accumulate). Returns true if anything was added.
    fn absorb(&mut self, other: &Effect) -> bool {
        fn join<T: PartialEq + Clone>(dst: &mut Vec<T>, src: &[T]) -> bool {
            let mut grew = false;
            for f in src {
                if !dst.contains(f) {
                    dst.push(f.clone());
                    grew = true;
                }
            }
            grew
        }
        let mut grew = join(&mut self.kills, &other.kills);
        grew |= join(&mut self.creates, &other.creates);
        grew |= join(&mut self.consumes, &other.consumes);
        grew |= join(&mut self.maybe, &other.maybe);
        grew |= join(&mut self.ret_alias, &other.ret_alias);
        grew |= join(&mut self.syncs, &other.syncs);
        grew
    }
}

/// Convergence metrics for the recursive-summary experiment — the numbers
/// that answer "does the summary space stay small as programs grow?"
#[derive(Debug, Default)]
pub struct AnalysisStats {
    /// Strongly connected components in the call graph.
    pub scc_count: usize,
    /// Largest SCC (in functions).
    pub largest_scc: usize,
    /// SCCs that needed iteration (size > 1 or self-edge).
    pub recursive_sccs: usize,
    /// Worst-case passes to convergence over all SCCs.
    pub max_iterations: usize,
    /// Facts absorbed per fixpoint pass, across all SCCs.
    pub facts_per_pass: Vec<usize>,
    /// Cyclic functions whose `returns-alias` was seeded with the borrowed-
    /// arg bound (least-fixpoint {} would be unsound: the checker doesn't
    /// prove a return-cycle actually terminates).
    pub seeded_returns: usize,
    /// Total summary facts across the program.
    pub total_facts: usize,
    /// Largest single-function summary.
    pub max_facts: usize,
}

pub type SummaryMap = HashMap<String, Effect>;

/// What a live task may still do to a shared variable, weakest to strongest.
/// `Shared` reads may overlap; `Atomic` means synchronized ops only — other
/// atomic ops are legal, ordinary access is not; `Sync(domains)` means
/// lock-guarded writes — serialized ONLY where domains coincide, racing
/// every ordinary access and every write under a different domain;
/// `Write` requires exclusivity; `Transferred` means the child
/// moved/consumed the value — the parent lost authority at spawn, and it
/// never returns except through `join`'s own result.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Hold {
    Shared,
    Atomic,
    /// Runtime-serialized exclusivity: the task may write through a live
    /// guard of each named mutex domain. Two `Sync` holds are compatible
    /// only when both are the same singleton domain — a task that might
    /// write under EITHER of two locks can meet a sibling under the other.
    Sync(HashSet<String>),
    Write,
    Transferred,
}

/// Strength ordering for merging two holds on one var inside a single
/// env (`Sync` unions its domain set instead of taking a max).
fn hold_rank(h: &Hold) -> u8 {
    match h {
        Hold::Shared => 0,
        Hold::Atomic => 1,
        Hold::Sync(_) => 2,
        Hold::Write => 3,
        Hold::Transferred => 4,
    }
}

/// The access a live context performs against an outstanding task hold.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Acc {
    Read,
    Write,
    Atomic,
    /// A lock acquire or guard-mediated write under the named domain.
    Sync(String),
}

/// A suspended authority packet carried by a channel. Semantically the
/// message IS the graph delta `send` cut out of the sender's env: the
/// value's flags plus the alias/capability edges that travelled with it.
/// `sent_targets` are the alias-edge targets whose *authority* moved into
/// the channel (they enter `in_transit`); `caps` targets are identity
/// only — a handle carries no authority, so they never enter transit.
#[derive(Debug, Clone, PartialEq)]
struct Packet {
    is_handle: bool,
    is_ref: bool,
    is_atomic: bool,
    is_mutex: bool,
    ty_name: Option<String>,
    edges: Vec<(Option<String>, String)>,
    caps: Vec<(Option<String>, String)>,
    sent_targets: Vec<String>,
}

/// Builtin atomic operations — call names, not keywords. Each is ONE
/// synchronized access to the atomic storage named by its first argument.
fn is_atomic_op(name: &str) -> bool {
    matches!(
        name,
        "atomic_load" | "atomic_store" | "atomic_add" | "compare_exchange"
    )
}

/// Builtin lock operations — call names, not keywords. `lock(m)` derives a
/// guard binding; `unlock(g)` consumes one.
fn is_lock_op(name: &str) -> bool {
    matches!(name, "lock" | "unlock")
}

fn channel_ty(ty: &Type) -> bool {
    matches!(ty, Type::Channel(_))
}

/// Builtin channel operations — call names, not keywords. `send` cuts an
/// authority packet out of the sender's graph; `recv` installs it in the
/// receiver's; `close` consumes the endpoint.
fn is_chan_op(name: &str) -> bool {
    matches!(name, "send" | "recv" | "close")
}

fn atomic_ty(ty: &Type) -> bool {
    matches!(ty, Type::Atomic(_))
}

fn mutex_ty(ty: &Type) -> bool {
    matches!(ty, Type::Mutex(_))
}

fn named_ty(ty: &Type) -> Option<String> {
    match ty {
        Type::Named(n) => Some(n.clone()),
        _ => None,
    }
}

/// A spawned task's recorded outcome — the task-analogue of an `Effect`:
/// a name-keyed graph transform applied by `join`, learned by analysing
/// the spawn body once in a forked env.
#[derive(Debug, Default, Clone)]
struct TaskResult {
    /// Shared vars the body touched, strongest access observed.
    holds: HashMap<String, Hold>,
    /// Exit states for held vars — Consumed survives the join boundary.
    exit: HashMap<String, Temp>,
    /// Definite kills on shared roots observed at child exit.
    kills: HashSet<(String, String)>,
    /// Edges live at child exit connecting two shared vars.
    edges: HashSet<AliasEdge>,
    /// Capability edges on shared vars surviving at child exit.
    cap_edges: HashSet<AliasEdge>,
    /// The body's `return` value: alias edges, capability edges, and the
    /// shared var it moved out of (authority handed back through `join`).
    ret_edges: Vec<(Option<String>, String)>,
    ret_caps: Vec<(Option<String>, String)>,
    ret_is_handle: bool,
    ret_base: Option<String>,
}

struct Env<'a> {
    vars: HashMap<String, Info>,
    /// Live alias edges. Birth: taking `&x`, storing a ref, receiving a
    /// ref-returning call. Death: holder scope exit, holder rebind, field
    /// overwrite, target consumed/shadowed.
    edges: HashSet<AliasEdge>,
    /// Capability edges — `h = handle x` records `h -> x`: identity, not
    /// authority. Same birth/death rules as alias edges and they propagate
    /// task holds, but they never block `consume` (a freed target just
    /// leaves a stale ticket) — and their death lands in `dead_caps`.
    cap_edges: HashSet<AliasEdge>,
    /// (holder, path|"*") whose capability-edge target died — the
    /// generation-check failure made static where provable.
    dead_caps: HashSet<(String, String)>,
    scopes: Vec<Vec<String>>,
    sigs: &'a SigMap,
    /// Summaries of already-analyzed callees (topological order).
    summaries: &'a SummaryMap,
    /// Current function's params: name -> index. Used to capture effects.
    param_idx: HashMap<String, usize>,
    /// DEFINITE edge-kills on live bindings so far, name-keyed: a kill
    /// counts only if it happened on every path to this point (intersected
    /// at `if` joins, dropped from `while` bodies since the zero-iteration
    /// path keeps the edge). Function summaries convert names to param
    /// indices at capture; spawn bodies use the names directly.
    killed_paths: HashSet<(String, String)>,
    /// Vars read / written / dereferenced in this env since creation —
    /// the capability-partition bookkeeping for spawned-task capture.
    /// `reads` = plain touches; `reaches` = touches that may follow edges
    /// (field/deref bases); `writes` = exclusive modifications.
    /// `atomics` = synchronized-op touches on atomic storage (compatible
    /// with each other, incompatible with every ordinary access).
    reads: HashSet<String>,
    reaches: HashSet<String>,
    writes: HashSet<String>,
    atomics: HashSet<String>,
    /// (struct type, field name) pairs declared `atomic<T>` — used to
    /// resolve `s.cnt` arguments to atomic ops.
    atomic_fields: &'a HashSet<(String, String)>,
    /// (struct type, field name) pairs declared `mutex<T>` — `s.a` is a
    /// lockable domain.
    mutex_fields: &'a HashSet<(String, String)>,
    /// Synchronization domains with a live guard in THIS env — a second
    /// `lock` on one is a self-deadlock (non-recursive mutexes).
    live_domains: HashSet<String>,
    /// Vars written through a live guard: var -> set of domains covering
    /// the write. These become `Hold::Sync` at task capture — writes the
    /// runtime serializes, not compiler-proven exclusivity.
    sync_writes: HashMap<String, HashSet<String>>,
    /// The `(domain, mutex-root)` produced by the most recent `lock` eval —
    /// claimed by the binding the result lands in (`let g = lock(m)`), or
    /// released immediately if the guard is never bound.
    pending_guard: Option<(String, String)>,
    /// Vars whose authority is currently channel-held: a `send` moved
    /// their capability into a packet. Any access is `use-after-send` —
    /// the authority's LOCATION changed, not its state. This is graph
    /// topology, not a new programmer-visible temporal state.
    in_transit: HashSet<String>,
    /// In-flight authority packets per channel endpoint. `send` pushes
    /// the graph delta; `recv` pops and installs it on the receiver.
    chan_queues: HashMap<String, Vec<Packet>>,
    /// The packet popped by the most recent `recv`, waiting to be claimed
    /// by the enclosing `let` binding.
    pending_recv: Option<Packet>,
    /// Vars currently held by unjoined tasks spawned in this env:
    /// name -> (task id, access kind, span of the spawn that took it).
    task_holds: HashMap<String, (u32, Hold, Span)>,
    /// Spawned-not-yet-joined tasks by handle id.
    tasks: HashMap<u32, TaskResult>,
    next_task: u32,
    /// The value this env's `return` statements produced — alias edges,
    /// capability edges, the var it moved out, whether it's a handle
    /// (read by spawn-body capture; unused by fns).
    ret_out: Option<(
        Vec<(Option<String>, String)>,
        Vec<(Option<String>, String)>,
        Option<String>,
        bool,
    )>,
    /// Param indices the return value may alias (union over Return stmts).
    ret_alias: Vec<usize>,
    diags: Vec<Diagnostic>,
}

impl<'a> Env<'a> {
    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    /// Scope exit kills every edge whose holder died, plus any stale edge
    /// that pointed at a dead binding. A var a live task still holds — or a
    /// task handle whose task still holds vars — cannot die here: the
    /// task would outlive what it captured. The programmer fix is `join`
    /// inside the scope, which is the `lifetime(region) >= lifetime(task)`
    /// rule made lexical.
    fn pop_scope(&mut self) {
        if let Some(names) = self.scopes.pop() {
            for n in names {
                if let Some((_, _, sp)) = self.task_holds.get(&n).cloned() {
                    self.diags.push(Diagnostic::new(
                        sp,
                        "task-escapes-scope",
                        format!(
                            "`{n}` is held by a spawned task but goes out of scope before the task joins"
                        ),
                    ));
                }
                self.orphan_check(&n);
                // A guard dying returns its domain — authority derivation
                // is strictly scoped to the guard's lifetime.
                if let Some(d) = self
                    .vars
                    .get(&n)
                    .and_then(|i| i.guard_domain.clone())
                {
                    self.live_domains.remove(&d);
                }
                self.vars.remove(&n);
                self.edges.retain(|(h, _, t)| h != &n && t != &n);
                self.kill_cap_target(&n);
                self.cap_edges.retain(|(h, _, _)| h != &n);
                self.killed_paths.retain(|(r, _)| r != &n);
                self.in_transit.remove(&n);
                self.chan_queues.remove(&n);
            }
        }
    }

    /// `name` is dying/rebinding while it is a task handle whose task still
    /// holds shared vars — the join-right is being thrown away.
    fn orphan_check(&mut self, name: &str) {
        let Some(id) = self.vars.get(name).and_then(|i| i.task_id) else {
            return;
        };
        let live = self.tasks.get(&id).map(|r| !r.holds.is_empty()).unwrap_or(false);
        if live {
            self.diags.push(Diagnostic::new(
                Span { line: 0, col: 0 },
                "task-escapes-scope",
                format!("task `{name}` holds captured vars but is never joined"),
            ));
        }
    }

    /// Helper for escape diagnostics issued with a known spawn span.
    fn escape_diag(&mut self, name: &str, span: Span) {
        self.diags.push(Diagnostic::new(
            span,
            "task-escapes-scope",
            format!("`{name}` is held by a spawned task that outlives its binding"),
        ));
    }

    fn declare(&mut self, name: String, info: Info) {
        // Redeclaring an existing name replaces its identity: kill the old
        // binding's edges in both directions so the fresh value doesn't
        // inherit stale aliases. A shadowed parameter also stops counting as
        // a param for effect capture.
        if self.vars.contains_key(&name) {
            self.edges.retain(|(h, _, t)| h != &name && t != &name);
            self.kill_cap_target(&name);
            self.cap_edges.retain(|(h, _, _)| h != &name);
            self.killed_paths.retain(|(r, _)| r != &name);
            self.in_transit.remove(&name);
            self.chan_queues.remove(&name);
            self.param_idx.remove(&name);
            if let Some(d) = self
                .vars
                .get(&name)
                .and_then(|i| i.guard_domain.clone())
            {
                self.live_domains.remove(&d);
            }
        }
        // Shadowing a binding a live task holds — or a task handle whose
        // task still holds vars — is a lifetime violation: the child may
        // outlive the name it captured.
        if self.vars.contains_key(&name) {
            if let Some((_, _, sp)) = self.task_holds.get(&name).cloned() {
                self.escape_diag(&name, sp);
            }
            self.orphan_check(&name);
        }
        if let Some(s) = self.scopes.last_mut() {
            s.push(name.clone());
        }
        self.vars.insert(name, info);
    }

    /// Kill every edge whose holder is `name` (rebinding or consuming `name`
    /// destroys the references it stored) — alias AND capability edges. If
    /// `name` is a guard its domain is released with it.
    fn kill_holder(&mut self, name: &str) {
        self.edges.retain(|(h, _, _)| h != name);
        self.cap_edges.retain(|(h, _, _)| h != name);
        if let Some(d) = self.vars.get(name).and_then(|i| i.guard_domain.clone()) {
            self.live_domains.remove(&d);
        }
    }

    /// Kill the edge at `holder.path` and any deeper edge under it
    /// (`h = ...` kills `h.r`, `h.r.x`, ...).
    fn kill_field(&mut self, holder: &str, path: &str) {
        let prefix = format!("{path}.");
        self.edges
            .retain(|(h, f, _)| !(h == holder && f.as_deref().is_some_and(|p| p == path || p.starts_with(&prefix))));
        self.cap_edges
            .retain(|(h, f, _)| !(h == holder && f.as_deref().is_some_and(|p| p == path || p.starts_with(&prefix))));
    }

    /// `name` is dead as an IDENTITY: any capability edge that pointed at it
    /// now leaves a stale ticket. Record (holder, field|"*") in dead_caps so
    /// a later dereference reports `stale-handle` instead of failing the
    /// runtime generation check.
    fn kill_cap_target(&mut self, name: &str) {
        let dying: Vec<(String, String)> = self
            .cap_edges
            .iter()
            .filter(|(_, _, t)| t == name)
            .map(|(h, f, _)| (h.clone(), f.clone().unwrap_or_else(|| "*".to_string())))
            .collect();
        for (h, p) in dying {
            self.dead_caps.insert((h, p));
        }
        self.cap_edges.retain(|(_, _, t)| t != name);
    }

    /// How many live edges still point at `target`.
    fn alias_count(&self, target: &str) -> usize {
        self.edges.iter().filter(|e| e.2 == target).count()
    }
}

fn base_var(e: &Expr) -> Option<&str> {
    match e {
        Expr::Var(n, _) => Some(n),
        Expr::Field(b, _, _) | Expr::Deref(b, _) => base_var(b),
        _ => None,
    }
}

/// Every name `v` may reach by following its outgoing alias AND capability
/// edges, transitively (handles reach through aliases and vice versa).
fn reach_targets(env: &Env, name: &str) -> Vec<String> {
    let mut seen: HashSet<String> = HashSet::new();
    seen.insert(name.to_string());
    let mut stack = vec![name.to_string()];
    let mut out = Vec::new();
    while let Some(v) = stack.pop() {
        for (h, _f, t) in env.edges.iter().chain(env.cap_edges.iter()) {
            if h == &v && seen.insert(t.clone()) {
                stack.push(t.clone());
                out.push(t.clone());
            }
        }
    }
    out
}

/// Does `name` still have a live capability-edge target? If not, and the
/// binding carries a handle, dereferencing it would hit the runtime
/// generation check — reportable statically.
/// Is `root.path` atomic storage? `root`'s declared struct type gives the
/// field's `atomic<T>` annotation.
fn is_atomic_field(env: &Env, root: &str, path: &str) -> bool {
    let seg = path.split('.').next().unwrap_or(path);
    env.vars
        .get(root)
        .and_then(|i| i.ty_name.clone())
        .map(|t| env.atomic_fields.contains(&(t, seg.to_string())))
        .unwrap_or(false)
}

/// Is `root.path` a `mutex<T>` field? Same resolution as atomic fields.
fn is_mutex_field(env: &Env, root: &str, path: &str) -> bool {
    let seg = path.split('.').next().unwrap_or(path);
    env.vars
        .get(root)
        .and_then(|i| i.ty_name.clone())
        .map(|t| env.mutex_fields.contains(&(t, seg.to_string())))
        .unwrap_or(false)
}

/// Resolve a `lock(...)` argument to `(domain, protected-root)`:
///   `lock(m)`   — m IS the mutex: domain "m", guard edges into m's closure
///   `lock(s.a)` — a mutex FIELD: domain "s.a", writes land on root s
///   `lock(hm)`  — a handle/alias to a mutex: the reached mutex names both
/// Interior paths (`s.a.x` where `a` is the mutex) and non-mutex storage
/// resolve to nothing — the caller reports `non-mutex-op`.
fn mutex_place(env: &Env, e: &Expr) -> Option<(String, String)> {
    if let Some((root, path)) = field_path(e) {
        let mut acc = String::new();
        for seg in path.split('.') {
            if !acc.is_empty() {
                acc.push('.');
            }
            acc.push_str(seg);
            if is_mutex_field(env, &root, &acc) {
                return if acc == path {
                    Some((format!("{root}.{acc}"), root.clone()))
                } else {
                    None
                };
            }
        }
        return None;
    }
    let b = base_var(e)?;
    if env.vars.get(b).map(|i| i.is_mutex).unwrap_or(false) {
        return Some((b.to_string(), b.to_string()));
    }
    for t in reach_targets(env, b) {
        if env.vars.get(&t).map(|i| i.is_mutex).unwrap_or(false) {
            return Some((t.clone(), t));
        }
    }
    None
}

/// Resolve a channel op's endpoint argument to the endpoint's root var:
/// the var itself (`send(ch, ..)`) or a mutex/alias chain reaching it
/// (`send(hc, ..)` where `hc = handle ch`).
fn chan_root(env: &Env, e: &Expr) -> Option<String> {
    let b = base_var(e)?;
    if env.vars.get(b).map(|i| i.is_channel).unwrap_or(false) {
        return Some(b.to_string());
    }
    reach_targets(env, b)
        .into_iter()
        .find(|t| env.vars.get(t).map(|i| i.is_channel).unwrap_or(false))
}

/// Does field path `e` enter a mutex's interior without a guard? A root
/// that IS a mutex (`m.x`) or a path prefix that names a mutex field
/// (`s.a.x`) is protected storage; the whole path naming the mutex itself
/// (`s.a`) is the lock object — legal to touch.
fn unguarded_path(env: &Env, e: &Expr) -> Option<String> {
    let (root, path) = field_path(e)?;
    if env.vars.get(&root).map(|i| i.is_mutex).unwrap_or(false) {
        return Some(root);
    }
    let mut acc = String::new();
    for seg in path.split('.') {
        if !acc.is_empty() {
            acc.push('.');
        }
        acc.push_str(seg);
        if acc.len() < path.len() && is_mutex_field(env, &root, &acc) {
            return Some(format!("{root}.{acc}"));
        }
    }
    None
}

/// The atomic-storage roots an atomic op's place argument resolves to:
/// the place itself (`c`), a struct's atomic field (`s.cnt` -> `s`), or
/// targets reached through alias/capability edges (`atomic_add(h)` and
/// `atomic_add(*a)` reach what `h`/`a` point at).
fn atomic_targets(env: &Env, e: &Expr) -> Vec<String> {
    if let Some((root, path)) = field_path(e) {
        let mut out = Vec::new();
        if is_atomic_field(env, &root, &path) {
            out.push(root.clone());
        }
        for t in reach_targets(env, &root) {
            if is_atomic_field(env, &t, &path) && !out.contains(&t) {
                out.push(t);
            }
        }
        out
    } else if let Some(b) = base_var(e) {
        let mut out = Vec::new();
        if env.vars.get(b).map(|i| i.is_atomic).unwrap_or(false) {
            out.push(b.to_string());
        }
        for t in reach_targets(env, b) {
            if env.vars.get(&t).map(|i| i.is_atomic).unwrap_or(false) && !out.contains(&t) {
                out.push(t);
            }
        }
        out
    } else {
        Vec::new()
    }
}

fn is_stale_handle(env: &Env, root: &str, path: Option<&str>) -> bool {
    let key = path.unwrap_or("*");
    env.dead_caps.contains(&(root.to_string(), key.to_string()))
        || env.dead_caps.contains(&(root.to_string(), "*".to_string()))
}

/// For a field-store target like `h.r` or `g.inner.src`, the holder variable
/// and the dotted field path written (`("g", "inner.src")`).
fn field_path(e: &Expr) -> Option<(String, String)> {
    match e {
        Expr::Field(b, f, _) => {
            let (root, sub) = match &**b {
                Expr::Var(n, _) => Some((n.clone(), String::new())),
                _ => field_path(b),
            }?;
            let path = if sub.is_empty() { f.clone() } else { format!("{sub}.{f}") };
            Some((root, path))
        }
        Expr::Deref(b, _) => field_path(b),
        _ => None,
    }
}

fn is_prim(ty: &Type) -> bool {
    match ty {
        Type::Named(n) => matches!(n.as_str(),
            "u8"|"u16"|"u32"|"u64"|"i8"|"i16"|"i32"|"i64"|"bool"),
        // An atomic scalar is still a Copy value — atomicity is a
        // synchronization discipline, not a new ownership class.
        Type::Atomic(inner) => is_prim(inner),
        _ => false,
    }
}

/// The base variable a call argument refers to — `x`, `&x`, `&mut x.f` all
/// resolve to `x` (the caller-side name the callee's parameter binds to).
fn arg_base(e: &Expr) -> Option<String> {
    match e {
        Expr::Ref(p, _) | Expr::MutRef(p, _) => base_var(p).map(|b| b.to_string()),
        other => base_var(other).map(|b| b.to_string()),
    }
}

pub struct Affine;

impl Affine {
    /// Check a function body and capture its Effect summary. Param
    /// references carry a marker self-edge `(p, None, p)` — "this binding
    /// aliases param-slot p" — so copies like `let t = x` propagate
    /// alias-ness structurally and `return t` attributes the return to a
    /// parameter. Marker edges are filtered out of `creates`.
    fn check_fn(env: &mut Env, f: &FnDef) -> Effect {
        env.push_scope();
        for (i, p) in f.params.iter().enumerate() {
            let mut info = Info::live();
            info.copy = is_prim(&p.ty);
            info.is_ref = matches!(p.mode, Mode::Ref | Mode::Mut);
            info.is_atomic = atomic_ty(&p.ty);
            info.is_mutex = mutex_ty(&p.ty);
            info.is_channel = channel_ty(&p.ty);
            info.ty_name = named_ty(&p.ty);
            if info.is_ref {
                env.edges.insert((p.name.clone(), None, p.name.clone()));
            }
            env.param_idx.insert(p.name.clone(), i);
            env.declare(p.name.clone(), info);
        }
        Self::check_block(env, &f.body);
        // Capture the summary BEFORE popping the fn scope (pop would kill
        // every edge whose holder is a param).
        let mut eff = Effect::default();
        for (i, p) in f.params.iter().enumerate() {
            match env.vars.get(&p.name).map(|v| v.state) {
                Some(Temp::Consumed) => eff.consumes.push(i),
                Some(Temp::Maybe) => eff.maybe.push(i),
                _ => {}
            }
        }
        for (h, fld, t) in &env.edges {
            if h == t && fld.is_none() {
                continue; // marker self-edge, not a graph edge
            }
            if let (Some(&hi), Some(&ti)) = (env.param_idx.get(h), env.param_idx.get(t)) {
                eff.creates.push((hi, fld.clone(), ti));
            }
        }
        // Canonicalize: sorted, deduped facts — identical logic must produce
        // identical summaries (termination + stable output for `effects`).
        let mut kills: Vec<(usize, String)> = env
            .killed_paths
            .iter()
            .filter_map(|(n, p)| env.param_idx.get(n).map(|&i| (i, p.clone())))
            .collect();
        kills.sort();
        eff.kills = kills;
        eff.creates.sort();
        eff.creates.dedup();
        eff.ret_alias = env.ret_alias.clone();
        eff.ret_alias.sort();
        eff.ret_alias.dedup();
        // Synchronized writes on params become (param, domain-suffix)
        // facts — the caller replays them against the actual's mutex root.
        let mut syncs: Vec<(usize, String)> = Vec::new();
        for (root, doms) in &env.sync_writes {
            if let Some(&pi) = env.param_idx.get(root) {
                let pname = f.params[pi].name.as_str();
                for d in doms {
                    if let Some(suffix) = d.strip_prefix(pname) {
                        if suffix.is_empty() || suffix.starts_with('.') {
                            syncs.push((pi, suffix.to_string()));
                        }
                    }
                }
            }
        }
        syncs.sort();
        syncs.dedup();
        eff.syncs = syncs;
        env.pop_scope();
        eff
    }

    fn check_block(env: &mut Env, block: &Block) {
        env.push_scope();
        for s in &block.stmts {
            Self::check_stmt(env, s);
        }
        env.pop_scope();
    }

    /// Merge `entry` and `exit` variable states pointwise (control-flow join).
    /// `exit` is updated in place; `entry` is the pre-branch state (for a loop,
    /// the state at the loop head, i.e. the zero-iteration case). Alias edges
    /// are merged separately as a union — an edge live on ANY path is a
    /// possible alias after the join.
    fn merge_states(entry: &HashMap<String, Info>, exit: &mut HashMap<String, Info>) {
        for (name, info) in exit.iter_mut() {
            if let Some(prev) = entry.get(name) {
                info.state = merge_temp(prev.state, info.state);
                info.region_managed |= prev.region_managed;
                info.task_id = info.task_id.or(prev.task_id);
            }
        }
    }

    /// Check an `if` with state forking and a pointwise merge at the join.
    /// Post-join alias edges = union of the two paths' live edge sets.
    /// Kills merge by INTERSECTION: a kill is definite only if both paths
    /// (or the then-path and the implicit skip) performed it.
    fn check_if(env: &mut Env, then_block: &Block, else_block: Option<&Block>) {
        let entry = env.vars.clone();
        let entry_edges = env.edges.clone();
        let entry_kills = env.killed_paths.clone();
        let entry_holds = env.task_holds.clone();
        let entry_tasks = env.tasks.clone();
        let entry_caps = env.cap_edges.clone();
        let entry_dead = env.dead_caps.clone();
        let entry_domains = env.live_domains.clone();
        let entry_syncs = env.sync_writes.clone();
        let entry_transit = env.in_transit.clone();
        let entry_chanq = env.chan_queues.clone();
        Self::check_block(env, then_block);
        let then_out = env.vars.clone();
        let then_edges = env.edges.clone();
        let then_kills = env.killed_paths.clone();
        let then_holds = env.task_holds.clone();
        let then_tasks = env.tasks.clone();
        let then_caps = env.cap_edges.clone();
        let then_dead = env.dead_caps.clone();
        let then_domains = env.live_domains.clone();
        let then_syncs = env.sync_writes.clone();
        let then_transit = env.in_transit.clone();
        let then_chanq = env.chan_queues.clone();
        env.vars = entry.clone();
        env.edges = entry_edges;
        env.killed_paths = entry_kills;
        env.task_holds = entry_holds;
        env.tasks = entry_tasks;
        env.cap_edges = entry_caps;
        env.dead_caps = entry_dead;
        env.live_domains = entry_domains;
        env.sync_writes = entry_syncs;
        env.in_transit = entry_transit;
        env.chan_queues = entry_chanq;
        if let Some(eb) = else_block {
            Self::check_block(env, eb);
        }
        // else_out is env.vars now; merge then_out into it.
        Self::merge_states(&then_out, &mut env.vars);
        env.edges.extend(then_edges);
        env.cap_edges.extend(then_caps);
        // A target dead on either path leaves a maybe-stale ticket.
        env.dead_caps.extend(then_dead);
        env.killed_paths = then_kills
            .intersection(&env.killed_paths)
            .cloned()
            .collect();
        // A task hold live on EITHER path may still be live here: a join on
        // one branch doesn't mean the task finished on the other.
        env.task_holds.extend(then_holds);
        env.tasks.extend(then_tasks);
        // A guard live on either path may still hold its domain; a synced
        // write on either path is a possible synced write after the join.
        env.live_domains.extend(then_domains);
        for (v, ds) in then_syncs {
            env.sync_writes.entry(v).or_default().extend(ds);
        }
        // Channel state merges as a may-union: a packet sent on either
        // path may be in flight, and authority in transit on either path
        // stays unreachable here.
        env.in_transit.extend(then_transit);
        for (c, q) in then_chanq {
            let e = env.chan_queues.entry(c).or_default();
            for pk in q {
                if !e.contains(&pk) {
                    e.push(pk);
                }
            }
        }
    }

    /// Check a `while` loop: body once under entry state, merge with entry
    /// (the loop may run zero times), then re-check under the merged state so
    /// a consume that repeats on the next iteration is caught. Kills inside
    /// the body are never definite — the zero-iteration path keeps the edge.
    fn check_while(env: &mut Env, body: &Block) {
        let entry = env.vars.clone();
        let entry_edges = env.edges.clone();
        let entry_kills = env.killed_paths.clone();
        let entry_holds = env.task_holds.clone();
        let entry_tasks = env.tasks.clone();
        let entry_caps = env.cap_edges.clone();
        let entry_dead = env.dead_caps.clone();
        let entry_domains = env.live_domains.clone();
        let entry_syncs = env.sync_writes.clone();
        let entry_transit = env.in_transit.clone();
        let entry_chanq = env.chan_queues.clone();
        Self::check_block(env, body);
        let mut merged = env.vars.clone();
        Self::merge_states(&entry, &mut merged);
        env.vars = merged;
        env.edges.extend(entry_edges.iter().cloned());
        env.cap_edges.extend(entry_caps.iter().cloned());
        env.dead_caps.extend(entry_dead.iter().cloned());
        env.killed_paths = entry_kills.clone();
        env.task_holds.extend(entry_holds.clone());
        env.tasks.extend(entry_tasks.clone());
        // A domain may still be held on the zero-iteration path; synced
        // writes inside a loop body are possible synced writes after it.
        env.live_domains.extend(entry_domains.iter().cloned());
        for (v, ds) in entry_syncs.iter() {
            env.sync_writes
                .entry(v.clone())
                .or_default()
                .extend(ds.iter().cloned());
        }
        env.in_transit.extend(entry_transit.iter().cloned());
        for (c, q) in entry_chanq.iter() {
            let e = env.chan_queues.entry(c.clone()).or_default();
            for pk in q {
                if !e.contains(pk) {
                    e.push(pk.clone());
                }
            }
        }
        // Second pass: body under back-edge state (iteration >= 2).
        Self::check_block(env, body);
        let mut post = env.vars.clone();
        Self::merge_states(&entry, &mut post);
        env.vars = post;
        env.edges.extend(entry_edges);
        env.cap_edges.extend(entry_caps);
        env.dead_caps.extend(entry_dead);
        env.killed_paths = entry_kills;
        env.task_holds.extend(entry_holds);
        env.tasks.extend(entry_tasks);
        env.live_domains.extend(entry_domains);
        for (v, ds) in entry_syncs {
            env.sync_writes.entry(v).or_default().extend(ds);
        }
    }

    fn check_stmt(env: &mut Env, stmt: &Stmt) {
        match stmt {
            Stmt::Let { name, init, ty, .. } => {
                // Alias edges the new binding carries, computed BEFORE eval
                // (evaluating `init` may consume a source and kill its edges).
                let src_edges = Self::value_alias_edges(env, init);
                let src_caps = Self::value_cap_edges(env, init);
                match init {
                    Expr::Ref(place, span) | Expr::MutRef(place, span) => {
                        Self::use_place(env, place, *span);
                        let mut info = Info::live();
                        info.is_ref = true;
                        env.declare(name.clone(), info);
                    }
                    other => {
                        let region_managed =
                            matches!(other, Expr::Alloc { region: Some(_), .. });
                        // A call returning `&T`, a copy of a ref binding, or a
                        // join yielding an alias produces a reference binding.
                        let is_ref = (matches!(other, Expr::Call { name, .. }
                            if env.sigs.get(name).map(|s| s.1).unwrap_or(false)))
                            || matches!(other, Expr::Var(v, _)
                                if env.vars.get(v).map(|i| i.is_ref).unwrap_or(false))
                            || (matches!(other, Expr::Join { .. })
                                && !src_edges.is_empty());
                        // Moving a task handle transfers the join-right.
                        let moved_task = match other {
                            Expr::Var(v, _) => {
                                env.vars.get(v.as_str()).and_then(|i| i.task_id)
                            }
                            _ => None,
                        };
                        let is_handle = match other {
                            Expr::Persist { kind: PersistKind::Handle, .. } => true,
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .map(|i| i.is_handle)
                                .unwrap_or(false),
                            Expr::Join { inner, .. } => base_var(inner)
                                .and_then(|b| env.vars.get(b))
                                .and_then(|i| i.task_id)
                                .and_then(|id| env.tasks.get(&id))
                                .map(|r| r.ret_is_handle)
                                .unwrap_or(false),
                            _ => false,
                        };
                        // Copying a provably-stale ticket propagates staleness.
                        let was_stale = matches!(other, Expr::Var(v, _)
                            if is_stale_handle(env, v, None));
                        let task_id = match other {
                            Expr::Spawn { body, span } => {
                                Some(Self::spawn_task(env, body, *span))
                            }
                            _ => {
                                Self::eval_value(env, other);
                                moved_task
                            }
                        };
                        let mut info = Info::live();
                        info.copy = ty.as_ref().map(is_prim).unwrap_or(false)
                            || Self::is_copy_expr(env, other)
                            || is_handle;
                        info.is_atomic = ty.as_ref().map(atomic_ty).unwrap_or(false)
                            || matches!(other, Expr::Alloc { ty: t, .. } if atomic_ty(t))
                            || matches!(other, Expr::Var(v, _)
                                if env.vars.get(v.as_str()).map(|i| i.is_atomic).unwrap_or(false));
                        info.is_mutex = ty.as_ref().map(mutex_ty).unwrap_or(false)
                            || matches!(other, Expr::Persist { kind: PersistKind::Mutex, .. })
                            || matches!(other, Expr::Var(v, _)
                                if env.vars.get(v.as_str()).map(|i| i.is_mutex).unwrap_or(false));
                        info.is_channel = ty.as_ref().map(channel_ty).unwrap_or(false)
                            || matches!(other, Expr::Call { name: n, .. } if n == "channel")
                            || matches!(other, Expr::Var(v, _)
                                if env.vars.get(v.as_str()).map(|i| i.is_channel).unwrap_or(false));
                        info.ty_name = ty.as_ref().and_then(named_ty).or_else(|| match other {
                            Expr::StructLit { name: n, .. } => Some(n.clone()),
                            Expr::Alloc { ty: t, .. } => named_ty(t),
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .and_then(|i| i.ty_name.clone()),
                            _ => None,
                        });
                        info.region_managed = region_managed;
                        info.is_ref = is_ref;
                        info.task_id = task_id;
                        info.is_handle = is_handle;
                        env.declare(name.clone(), info);
                        // `let g = lock(m)` — the fresh binding claims the
                        // pending guard: it becomes an authority token and
                        // edges INTO the mutex's closure (the lock domain).
                        if let Some((dom, mroot)) = env.pending_guard.take() {
                            if let Some(i) = env.vars.get_mut(name.as_str()) {
                                i.is_guard = true;
                                i.guard_domain = Some(dom);
                            }
                            env.cap_edges.insert((name.clone(), None, mroot));
                        }
                        // `let ch2 = ch` moves the endpoint — its queue
                        // of in-flight packets follows the binding.
                        if let Expr::Var(v, _) = other {
                            if let Some(q) = env.chan_queues.remove(v.as_str()) {
                                env.chan_queues.insert(name.clone(), q);
                            }
                        }
                        // `let g = recv(ch)` claims the popped packet: the
                        // transported authority installs on the receiver.
                        Self::claim_packet(env, name);
                        if was_stale {
                            env.dead_caps.insert((name.clone(), "*".to_string()));
                        }
                    }
                }
                for (fld, tgt) in src_edges {
                    env.edges.insert((name.clone(), fld, tgt));
                }
                for (fld, tgt) in src_caps {
                    env.cap_edges.insert((name.clone(), fld, tgt));
                }
            }
            Stmt::Assign { target, value, .. } => {
                // Snapshot the value's edges BEFORE evaluating it: `g.h = h`
                // consumes `h` (killing its edges), but the stored value must
                // carry those edges into `g`.
                let src_edges = Self::value_alias_edges(env, value);
                let src_caps = Self::value_cap_edges(env, value);
                let value_is_handle = match value {
                    Expr::Persist { kind: PersistKind::Handle, .. } => true,
                    Expr::Var(v, _) => env
                        .vars
                        .get(v.as_str())
                        .map(|i| i.is_handle)
                        .unwrap_or(false),
                    Expr::Join { inner, .. } => base_var(inner)
                        .and_then(|b| env.vars.get(b))
                        .and_then(|i| i.task_id)
                        .and_then(|id| env.tasks.get(&id))
                        .map(|r| r.ret_is_handle)
                        .unwrap_or(false),
                    _ => false,
                };
                // A spawn in value position registers the task now; its id
                // lands on the target binding below. Moving a task handle
                // carries the join-right to the new binding.
                let value_task = match value {
                    Expr::Spawn { body, span } => {
                        Some(Self::spawn_task(env, body, *span))
                    }
                    Expr::Ref(place, s) | Expr::MutRef(place, s) => {
                        Self::use_place(env, place, *s);
                        None
                    }
                    Expr::Var(v, _) => {
                        let id = env.vars.get(v.as_str()).and_then(|i| i.task_id);
                        Self::eval_value(env, value);
                        id
                    }
                    _ => {
                        Self::eval_value(env, value);
                        None
                    }
                };
                match target {
                    Expr::Var(tname, ts) => {
                        // Rebinding kills the old value's outgoing edges AND
                        // incoming edges (the new value is a different object),
                        // then the new value's edges move in. Reassignment also
                        // re-acquires: a consumed slot can be reopened.
                        Self::check_hold(env, tname, *ts, Acc::Write);
                        env.orphan_check(tname);
                        env.kill_holder(tname);
                        env.edges.retain(|(_, _, t)| t != tname);
                        // Rebinding gives `tname` a new identity: tickets
                        // that pointed at the old one go stale.
                        env.kill_cap_target(tname);
                        env.cap_edges.retain(|(h, _, _)| h != tname);
                        env.dead_caps.retain(|(r, _)| r != tname);
                        env.killed_paths.insert((tname.clone(), "*".to_string()));
                        env.writes.insert(tname.clone());
                        let region_managed = match value {
                            Expr::Alloc { region: Some(_), .. } => true,
                            Expr::Persist { .. } => false,
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .map(|i| i.region_managed)
                                .unwrap_or(false),
                            _ => false,
                        };
                        let is_ref = match value {
                            Expr::Ref(..) | Expr::MutRef(..) => true,
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .map(|i| i.is_ref)
                                .unwrap_or(false),
                            Expr::Call { name, .. } => env
                                .sigs
                                .get(name)
                                .map(|s| s.1)
                                .unwrap_or(false),
                            Expr::Join { .. } => !src_edges.is_empty(),
                            _ => false,
                        };
                        let value_ty = match value {
                            Expr::StructLit { name: n, .. } => Some(n.clone()),
                            Expr::Alloc { ty: t, .. } => named_ty(t),
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .and_then(|i| i.ty_name.clone()),
                            _ => None,
                        };
                        let value_is_mutex = match value {
                            Expr::Persist { kind: PersistKind::Mutex, .. } => true,
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .map(|i| i.is_mutex)
                                .unwrap_or(false),
                            _ => false,
                        };
                        let value_is_channel = match value {
                            Expr::Call { name: n, .. } => n == "channel",
                            Expr::Var(v, _) => env
                                .vars
                                .get(v.as_str())
                                .map(|i| i.is_channel)
                                .unwrap_or(false),
                            _ => false,
                        };
                        if let Some(i) = env.vars.get_mut(tname) {
                            i.state = Temp::Live;
                            i.region_managed = region_managed;
                            i.is_ref = is_ref;
                            i.task_id = value_task;
                            i.is_handle = value_is_handle;
                            i.copy |= value_is_handle;
                            i.is_mutex = value_is_mutex;
                            i.is_channel = value_is_channel;
                            i.is_guard = false;
                            i.guard_domain = None;
                            if value_ty.is_some() {
                                i.ty_name = value_ty;
                            }
                        }
                        // `x = lock(m)` rebinds the slot to a fresh guard —
                        // the old binding's domain was released above.
                        if let Some((dom, mroot)) = env.pending_guard.take() {
                            if let Some(i) = env.vars.get_mut(tname) {
                                i.is_guard = true;
                                i.guard_domain = Some(dom);
                            }
                            env.cap_edges.insert((tname.clone(), None, mroot));
                        }
                        if let Expr::Var(v, _) = value {
                            if let Some(q) = env.chan_queues.remove(v.as_str()) {
                                env.chan_queues.insert(tname.clone(), q);
                            }
                        }
                        // `x = recv(ch)` claims the packet onto the
                        // rebound slot.
                        Self::claim_packet(env, tname);
                        for (fld, tgt) in src_edges {
                            env.edges.insert((tname.clone(), fld, tgt));
                        }
                        for (fld, tgt) in src_caps {
                            env.cap_edges.insert((tname.clone(), fld, tgt));
                        }
                    }
                    _ => {
                        // Field store. `h.r = v` where h is an alias/handle
                        // writes THROUGH to the edge targets; `b.slot = v`
                        // where b.slot carries an edge overwrites the slot.
                        // Otherwise it writes `root`'s own memory — though a
                        // write deeper than a live edge still crosses it.
                        if let Some((root, path)) = field_path(target) {
                            Self::check_hold(env, &root, target.span(), Acc::Read);
                            env.reaches.insert(root.clone());
                            // A write into a mutex's interior without a
                            // guard bypasses the lock entirely.
                            if let Some(mp) = unguarded_path(env, target) {
                                env.diags.push(Diagnostic::new(
                                    target.span(),
                                    "unguarded-access",
                                    format!("`{mp}` is `mutex<T>` storage — its interior is only reachable through a live guard"),
                                ));
                            }
                            let info_root = env.vars.get(&root);
                            let through_root = info_root
                                .map(|i| i.is_ref || i.is_handle || i.is_guard)
                                .unwrap_or(false);
                            // A write through a guard is synchronized —
                            // recorded under the guard's domain, not as an
                            // ordinary exclusive write.
                            let guard_dom = info_root.and_then(|i| {
                                if i.is_guard { i.guard_domain.clone() } else { None }
                            });
                            let mut wts: Vec<String> = Vec::new();
                            if through_root {
                                wts = reach_targets(env, &root);
                            } else {
                                for (h, f, t) in env
                                    .edges
                                    .iter()
                                    .chain(env.cap_edges.iter())
                                {
                                    if h == &root {
                                        if let Some(fp) = f {
                                            if path.starts_with(&format!("{fp}.")) {
                                                wts.push(t.clone());
                                            }
                                        }
                                    }
                                }
                            }
                            if wts.is_empty() {
                                wts.push(root.clone());
                            }
                            // Using a guard means it must still be alive.
                            if guard_dom.is_some() {
                                Self::use_var(env, &root, target.span());
                            }
                            for w in wts {
                                if let Some(dom) = &guard_dom {
                                    env.sync_writes
                                        .entry(w.clone())
                                        .or_default()
                                        .insert(dom.clone());
                                    Self::check_hold(
                                        env,
                                        &w,
                                        target.span(),
                                        Acc::Sync(dom.clone()),
                                    );
                                } else {
                                    env.writes.insert(w.clone());
                                    Self::check_hold(env, &w, target.span(), Acc::Write);
                                }
                            }
                            env.kill_field(&root, &path);
                            env.killed_paths.insert((root.clone(), path.clone()));
                            env.dead_caps.retain(|(r, p)| !(r == &root && p == &path));
                            for (_fld, tgt) in src_edges {
                                env.edges.insert((root.clone(), Some(path.clone()), tgt));
                            }
                            for (_fld, tgt) in src_caps {
                                env.cap_edges.insert((root.clone(), Some(path.clone()), tgt));
                            }
                        }
                    }
                }
            }
            Stmt::Return { value: Some(v), .. } => {
                // Record which params the returned value may alias BEFORE
                // evaluating it (eval may consume a source and kill edges).
                let ret_edges = Self::value_alias_edges(env, v);
                for (_fld, t) in &ret_edges {
                    if let Some(&pi) = env.param_idx.get(t) {
                        env.ret_alias.push(pi);
                    }
                }
                // For spawn bodies this is the value `join` hands back:
                // union of edges across all returns + the var moved out.
                let ret_caps = Self::value_cap_edges(env, v);
                let base = base_var(v).map(|b| b.to_string());
                let is_handle = base
                    .as_deref()
                    .and_then(|b| env.vars.get(b))
                    .map(|i| i.is_handle)
                    .unwrap_or(false);
                let base_is_guard = base
                    .as_deref()
                    .and_then(|b| env.vars.get(b))
                    .map(|i| i.is_guard)
                    .unwrap_or(false);
                let out = env.ret_out.get_or_insert_with(Default::default);
                out.0.extend(ret_edges);
                out.1.extend(ret_caps);
                if out.2.is_none() {
                    out.2 = base;
                }
                if is_handle {
                    out.3 = true;
                }
                Self::eval_value(env, v);
                // Guards are scope-bound authority: returning one (or an
                // unclaimed `lock` result) escapes the locking scope.
                if env.pending_guard.take().is_some() || base_is_guard {
                    env.diags.push(Diagnostic::new(
                        v.span(),
                        "guard-escape",
                        "a lock guard cannot escape its locking scope".to_string(),
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
                Self::eval_value(env, cond);
                Self::check_if(env, then_block, else_block.as_ref());
            }
            Stmt::While { cond, body, .. } => {
                Self::eval_value(env, cond);
                Self::check_while(env, body);
            }
            Stmt::Region { body, .. } => Self::check_block(env, body),
            Stmt::Free { value, span } => Self::consume(env, value, *span, Op::Free),
            Stmt::Expr { expr, .. } => {
                Self::eval_value(env, expr);
                // A bare `lock(m)` binds no guard — the domain releases
                // immediately (the lock protects nothing).
                if let Some((dom, _)) = env.pending_guard.take() {
                    env.live_domains.remove(&dom);
                }
                // A bare `recv(ch)` discards its packet — the released
                // authority is dropped, not claimed.
                env.pending_recv = None;
            }
        }
    }

    /// Reading a variable's value requires it be provably Live — and no
    /// running task may hold it Exclusively/Transferred.
    fn use_var(env: &mut Env, name: &str, span: Span) {
        env.reads.insert(name.to_string());
        Self::check_hold(env, name, span, Acc::Read);
        if is_stale_handle(env, name, None) {
            env.diags.push(Diagnostic::new(
                span,
                "stale-handle",
                format!("`{name}` is a handle whose target is already dead"),
            ));
        }
        let info = match env.vars.get(name) {
            Some(i) => i,
            None => return,
        };
        match info.state {
            Temp::Live => {}
            Temp::Consumed => env.diags.push(Diagnostic::new(
                span,
                "use-after-consume",
                format!("`{name}` was already consumed"),
            )),
            Temp::Maybe => env.diags.push(Diagnostic::new(
                span,
                "use-after-consume",
                format!("`{name}` may already have been consumed on another path"),
            )),
        }
    }

    fn use_place(env: &mut Env, place: &Expr, span: Span) {
        if let Some(b) = base_var(place) {
            Self::use_var(env, b, span);
        } else {
            Self::eval_value(env, place);
        }
    }

    /// Extract a base name and consume it; non-variable targets are just read.
    fn consume(env: &mut Env, e: &Expr, span: Span, op: Op) {
        match base_var(e) {
            Some(name) => {
                let name = name.to_string();
                Self::consume_named(env, &name, span, op)
            }
            None => Self::eval_value(env, e),
        }
    }

    /// `free`/consume/`own`-transfer of a named binding: requires Live and
    /// un-aliased, requires an owning (non-reference) binding, and must not
    /// target region-managed memory. A `free` additionally requires a real
    /// resource, while a `move`/`promote` of a Copy value is a plain read.
    fn consume_named(env: &mut Env, name: &str, span: Span, op: Op) {
        Self::check_hold(env, name, span, Acc::Write);
        let info = match env.vars.get(name) {
            Some(i) => i.clone(),
            None => return,
        };
        if info.copy {
            // Moves/copies of plain values are free; `free` of one is an error.
            if op == Op::Free {
                env.diags.push(Diagnostic::new(
                    span,
                    "invalid-consume",
                    format!("cannot free `{name}`: it is a plain value, not a resource"),
                ));
            }
            return;
        }
        if info.is_ref {
            env.diags.push(Diagnostic::new(
                span,
                "consume-through-alias",
                format!("cannot {} `{name}` through a borrowed alias", op.name()),
            ));
            return;
        }
        // A guard is released by `unlock` or scope exit — `free g` is a
        // category error (the lock isn't a heap resource).
        if op == Op::Free && info.is_guard {
            env.diags.push(Diagnostic::new(
                span,
                "invalid-consume",
                format!("cannot free `{name}`: a lock guard is released by `unlock` or scope exit"),
            ));
            return;
        }
        // Only `free` is forbidden on region-managed memory. A *move* of a
        // region value (`keeper = t`) is the region model's problem to police
        // as an escape — the affine layer stays orthogonal.
        if op == Op::Free && info.region_managed {
            env.diags.push(Diagnostic::new(
                span,
                "free-region-memory",
                format!(
                    "cannot free `{name}`: it is region-managed and freed with its region"
                ),
            ));
            return;
        }
        if env.alias_count(name) > 0 {
            env.diags.push(Diagnostic::new(
                span,
                "consume-while-aliased",
                format!("cannot {} `{name}`: a live alias still references it", op.name()),
            ));
            return;
        }
        match info.state {
            Temp::Live => {
                env.vars.get_mut(name).unwrap().state = Temp::Consumed;
                // The consumed binding's own outgoing references die with
                // it — and tickets pointing AT it go stale (recorded in
                // dead_caps so a later deref is a compile error).
                env.kill_holder(name);
                env.kill_cap_target(name);
            }
            Temp::Consumed | Temp::Maybe => env.diags.push(Diagnostic::new(
                span,
                "double-consume",
                format!(
                    "`{name}` {} already have been consumed",
                    if info.state == Temp::Maybe { "may" } else { "was" }
                ),
            )),
        }
    }

    fn is_copy_expr(env: &Env, e: &Expr) -> bool {
        match e {
            Expr::Int(..) | Expr::Bool(..) | Expr::Binary { .. } => true,
            Expr::Var(n, _) => env
                .vars
                .get(n)
                .map(|i| i.copy || i.is_handle)
                .unwrap_or(false),
            Expr::Call { name, .. } => is_atomic_op(name),
            _ => false,
        }
    }

    /// The alias edges a value would carry into a new holder, computed BEFORE
    /// the expression is evaluated — `g.h = h` consumes `h` (killing its
    /// edges), but the stored value must bring those edges with it.
    ///   `&place`/`&mut place` -> a direct edge to the borrowed base
    ///   `v`                   -> copies every edge `v` holds
    ///   `f(..)` returning `&T` -> edges to its borrowed argument bases
    fn value_alias_edges(env: &Env, e: &Expr) -> Vec<(Option<String>, String)> {
        match e {
            Expr::Ref(p, _) | Expr::MutRef(p, _) => base_var(p)
                .into_iter()
                .map(|b| (None, b.to_string()))
                .collect(),
            Expr::Var(v, _) => env
                .edges
                .iter()
                .filter(|e| &e.0 == v)
                .map(|e| (e.1.clone(), e.2.clone()))
                .collect(),
            // `mutex e` moves e's whole reachable world under the lock —
            // the wrapped binding's aliases come with it.
            Expr::Persist { kind: PersistKind::Mutex, inner, .. } => {
                Self::value_alias_edges(env, inner)
            }
            Expr::Call { .. } => Self::call_alias_targets(env, e)
                .into_iter()
                .map(|t| (None, t))
                .collect(),
            // `join t` yields the child's return — which may carry the
            // edges the task's `return` produced.
            Expr::Join { inner, .. } => base_var(inner)
                .and_then(|b| env.vars.get(b))
                .and_then(|i| i.task_id)
                .and_then(|id| env.tasks.get(&id))
                .map(|r| r.ret_edges.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// The capability edges a value carries — `handle x` makes one, a copied
    /// ticket keeps it, `join t` brings back the task's returned tickets.
    /// These are identity edges: they never block `consume`.
    fn value_cap_edges(env: &Env, e: &Expr) -> Vec<(Option<String>, String)> {
        match e {
            Expr::Persist {
                kind: PersistKind::Handle,
                inner,
                ..
            } => base_var(inner)
                .into_iter()
                .map(|b| (None, b.to_string()))
                .collect(),
            // `mutex h` puts `h`'s targets under the lock's protection —
            // the guard's write-through then covers the closure.
            Expr::Persist {
                kind: PersistKind::Mutex,
                inner,
                ..
            } => Self::value_cap_edges(env, inner),
            Expr::Var(v, _) => env
                .cap_edges
                .iter()
                .filter(|e| &e.0 == v)
                .map(|e| (e.1.clone(), e.2.clone()))
                .collect(),
            Expr::Join { inner, .. } => base_var(inner)
                .and_then(|b| env.vars.get(b))
                .and_then(|i| i.task_id)
                .and_then(|id| env.tasks.get(&id))
                .map(|r| r.ret_caps.clone())
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// If `e` is a call returning a reference, the caller-side bases the
    /// result may alias. Prefer the callee's computed `ret_alias` summary
    /// (precise); fall back to "any borrowed arg" when the callee hasn't been
    /// summarized yet (shouldn't happen post-SCC-ordering — kept as a safe
    /// default for calls into unknown functions).
    fn call_alias_targets(env: &Env, e: &Expr) -> Vec<String> {
        let Expr::Call { name, args, .. } = e else {
            return Vec::new();
        };
        if let Some(eff) = env.summaries.get(name) {
            return eff
                .ret_alias
                .iter()
                .filter_map(|&pi| args.get(pi).and_then(arg_base))
                .collect();
        }
        let Some(sig) = env.sigs.get(name) else {
            return Vec::new();
        };
        if !sig.1 {
            return Vec::new();
        }
        args.iter()
            .enumerate()
            .filter_map(|(i, arg)| {
                let mode = sig.0.get(i).copied();
                if matches!(mode, Some(Mode::Ref | Mode::Mut)) {
                    let place = match arg {
                        Expr::Ref(p, _) | Expr::MutRef(p, _) => &**p,
                        other => other,
                    };
                    base_var(place).map(|b| b.to_string())
                } else {
                    None
                }
            })
            .collect()
    }

    /// Apply a callee's Effect to the caller's alias graph. Kills first, then
    /// creates — the net effect of the callee's body. Temporal effects only
    /// fire on ref/mut-passed args: `own` args were already consumed at the
    /// boundary (a moved value is gone whether or not the callee consumed it).
    fn apply_effect(
        env: &mut Env,
        eff: &Effect,
        actuals: &[Option<String>],
        sig: &Option<(Vec<Mode>, bool)>,
        span: Span,
    ) {
        for (pi, path) in &eff.kills {
            if let Some(Some(a)) = actuals.get(*pi) {
                let a = a.clone();
                if path == "*" {
                    env.kill_holder(&a);
                    env.edges.retain(|(_, _, t)| t != &a);
                } else {
                    env.kill_field(&a, path);
                }
                // The kill propagates into this env's own definite-kill set
                // (name-keyed — captured per-param or per-capture later) and
                // counts as a write for the task-capability partition.
                env.killed_paths.insert((a.clone(), path.clone()));
                env.writes.insert(a.clone());
            }
        }
        for &pi in &eff.consumes {
            if matches!(sig.as_ref().and_then(|s| s.0.get(pi)), Some(Mode::Own)) {
                continue; // boundary move already handled
            }
            if let Some(Some(a)) = actuals.get(pi) {
                let a = a.clone();
                if env.vars.get(&a).map(|i| i.state) != Some(Temp::Consumed) {
                    Self::consume_named(env, &a, span, Op::Move);
                }
            }
        }
        for &pi in &eff.maybe {
            if matches!(sig.as_ref().and_then(|s| s.0.get(pi)), Some(Mode::Own)) {
                continue;
            }
            if let Some(Some(a)) = actuals.get(pi) {
                if let Some(i) = env.vars.get_mut(a) {
                    if i.state == Temp::Live {
                        i.state = Temp::Maybe;
                    }
                }
            }
        }
        for (ph, fld, pt) in &eff.creates {
            if let (Some(Some(h)), Some(Some(t))) =
                (actuals.get(*ph), actuals.get(*pt))
            {
                env.edges.insert((h.clone(), fld.clone(), t.clone()));
                env.writes.insert(h.clone());
            }
        }
        // Synchronized writes: the callee's lock-mediated writes replay on
        // the caller's mutex root. `update(m)` and `update(hm)` where
        // `hm -> m` both resolve to domain "m" — the mutex's identity, not
        // the argument spelling.
        for (pi, suffix) in &eff.syncs {
            if let Some(Some(a)) = actuals.get(*pi) {
                let a = a.clone();
                let mut roots = Vec::new();
                if env.vars.get(&a).map(|i| i.is_mutex).unwrap_or(false) {
                    roots.push(a.clone());
                }
                for t in reach_targets(env, &a) {
                    if env.vars.get(&t).map(|i| i.is_mutex).unwrap_or(false)
                        && !roots.contains(&t)
                    {
                        roots.push(t);
                    }
                }
                if roots.is_empty() {
                    roots.push(a.clone());
                }
                for r in roots {
                    env.sync_writes
                        .entry(r.clone())
                        .or_default()
                        .insert(format!("{r}{suffix}"));
                }
            }
        }
    }

    /// A task-hold violation in this env: `name` is still reachable by an
    /// unjoined task. Compatibility: Shared reads overlap, Atomic ops
    /// overlap Atomic ops, and a Sync(domain) access is legal only against
    /// a Sync hold naming exactly that one domain — runtime serialization
    /// is mutual exclusion only within one synchronization domain.
    fn check_hold(env: &mut Env, name: &str, span: Span, acc: Acc) {
        // Authority parked in a channel: the binding is live but its
        // capability is in transit — any access is `use-after-send`.
        // Topology, not a temporal state: the authority's location moved.
        if env.in_transit.contains(name) {
            env.diags.push(Diagnostic::new(
                span,
                "use-after-send",
                format!(
                    "`{name}`'s authority was sent through a channel — `recv` must claim it first"
                ),
            ));
            return;
        }
        let Some((_, hold, _)) = env.task_holds.get(name).cloned() else {
            return;
        };
        match &hold {
            Hold::Transferred => env.diags.push(Diagnostic::new(
                span,
                "use-after-send",
                format!("`{name}` was transferred to a spawned task"),
            )),
            Hold::Write => env.diags.push(Diagnostic::new(
                span,
                "data-race",
                format!("`{name}` may be written by a spawned task that is still running"),
            )),
            // Atomic holds synchronize only with other atomic ops — any
            // ordinary access is unsynchronized and races them.
            Hold::Atomic if acc != Acc::Atomic => env.diags.push(Diagnostic::new(
                span,
                "data-race",
                format!("`{name}` may be atomically accessed by a spawned task that is still running"),
            )),
            // A synchronized hold tolerates only same-domain synchronized
            // access: any ordinary access — or a write under a different
            // mutex — races it.
            Hold::Sync(doms) => {
                let compat = matches!(&acc, Acc::Sync(d) if doms.len() == 1 && doms.contains(d));
                if !compat {
                    env.diags.push(Diagnostic::new(
                        span,
                        "data-race",
                        format!("`{name}` may be written under a lock by a spawned task that is still running"),
                    ));
                }
            }
            Hold::Shared if acc != Acc::Read => env.diags.push(Diagnostic::new(
                span,
                "data-race",
                format!("`{name}` may be read by a spawned task that is still running"),
            )),
            Hold::Shared | Hold::Atomic => {}
        }
    }

    /// Analyse a spawn body in a forked env and return the new task's id.
    /// The child sees a snapshot of the parent's vars/edges; its outcome is
    /// recorded as a `TaskResult` — nothing merges into the parent until
    /// `join`. The child's touches become holds on the shared vars it could
    /// reach, transitively through alias edges.
    fn spawn_task(env: &mut Env, body: &Block, span: Span) -> u32 {
        let id = env.next_task;
        env.next_task += 1;
        let shared: HashSet<String> = env.vars.keys().cloned().collect();
        let mut child = Env {
            vars: env.vars.clone(),
            edges: env.edges.clone(),
            cap_edges: env.cap_edges.clone(),
            dead_caps: env.dead_caps.clone(),
            scopes: Vec::new(),
            sigs: env.sigs,
            summaries: env.summaries,
            param_idx: HashMap::new(),
            killed_paths: HashSet::new(),
            reads: HashSet::new(),
            reaches: HashSet::new(),
            writes: HashSet::new(),
            atomics: HashSet::new(),
            atomic_fields: env.atomic_fields,
            mutex_fields: env.mutex_fields,
            // The child's own held domains start empty: a task acquiring
            // `m` while the parent holds it is legal serialized concurrency
            // (deadlock-freedom is a separate, unmodeled property).
            live_domains: HashSet::new(),
            sync_writes: HashMap::new(),
            pending_guard: None,
            // The channel world is shared memory, not a per-task
            // capability: the child sees the parent's in-flight packets
            // and transit set; its ops merge back at spawn.
            chan_queues: env.chan_queues.clone(),
            in_transit: env.in_transit.clone(),
            pending_recv: None,
            task_holds: HashMap::new(),
            tasks: HashMap::new(),
            next_task: 1,
            ret_out: None,
            ret_alias: Vec::new(),
            diags: Vec::new(),
        };
        Self::check_block(&mut child, body);
        env.diags.extend(child.diags.iter().cloned());
        // Channel effects apply immediately at spawn (the channel is
        // shared state): the child's final queue supersedes the snapshot
        // it started from, and its in-transit map is the net authority
        // location. Only shared vars merge — a child-local name can
        // collide with a later sibling's fresh binding.
        for (c, q) in child.chan_queues.iter() {
            if shared.contains(c) {
                env.chan_queues.insert(c.clone(), q.clone());
            }
        }
        env.in_transit = child
            .in_transit
            .iter()
            .filter(|v| shared.contains(*v))
            .cloned()
            .collect();
        // Capability partition: strongest observed access per var. A child
        // that consumed/moved a shared var took Transferred authority.
        // Touches propagate along the child's edges — alias AND capability
        // (a write through `h.fd` reaches `x`), but only `reaches`/`writes`
        // propagate: copying a ticket doesn't touch what it names.
        let mut holds: HashMap<String, Hold> = HashMap::new();
        let raise = |holds: &mut HashMap<String, Hold>, v: &str, k: Hold| {
            match (holds.get_mut(v), k) {
                // Sync holds UNION their domain sets: the child may write
                // under any of them.
                (Some(Hold::Sync(ds)), Hold::Sync(new)) => {
                    ds.extend(new);
                }
                (Some(e), k) => {
                    if hold_rank(&k) > hold_rank(e) {
                        *e = k;
                    }
                }
                (None, k) => {
                    holds.insert(v.to_string(), k);
                }
            }
        };
        for v in &child.reads {
            raise(&mut holds, v, Hold::Shared);
        }
        for v in child.reaches.iter().cloned().collect::<Vec<_>>() {
            raise(&mut holds, &v, Hold::Shared);
            for t in reach_targets(&child, &v) {
                raise(&mut holds, &t, Hold::Shared);
            }
        }
        for v in child.writes.iter().cloned().collect::<Vec<_>>() {
            raise(&mut holds, &v, Hold::Write);
            for t in reach_targets(&child, &v) {
                raise(&mut holds, &t, Hold::Write);
            }
        }
        for v in &child.atomics {
            raise(&mut holds, v, Hold::Atomic);
        }
        // Guard-mediated writes become domain-tagged synchronized holds —
        // legal against the same domain, racing everything else.
        for (v, doms) in &child.sync_writes {
            raise(&mut holds, v, Hold::Sync(doms.clone()));
        }
        // A var touched BOTH atomically and plainly inside the child has an
        // unsynchronized access in its window: the hold must reject every
        // concurrent access — Write's compatibility profile does that.
        for v in child.atomics.intersection(&child.reads).cloned().collect::<Vec<_>>() {
            raise(&mut holds, &v, Hold::Write);
        }
        for v in child.atomics.intersection(&child.reaches).cloned().collect::<Vec<_>>() {
            raise(&mut holds, &v, Hold::Write);
        }
        // Same promotion for mutex-mediated writes: a target the child also
        // touched plainly has an unguarded access in its window.
        let sync_roots: HashSet<String> = child.sync_writes.keys().cloned().collect();
        for v in sync_roots
            .intersection(&child.reads)
            .chain(sync_roots.intersection(&child.reaches))
            .chain(sync_roots.intersection(&child.writes))
            .chain(sync_roots.intersection(&child.atomics))
            .cloned()
            .collect::<Vec<_>>()
        {
            raise(&mut holds, &v, Hold::Write);
        }
        for (v, info) in &child.vars {
            if shared.contains(v) && info.state != Temp::Live {
                raise(&mut holds, v, Hold::Transferred);
            }
        }
        // A guard is a unique authority token — a child that TOUCHED a
        // captured guard (any access kind) claimed it; the parent loses it.
        let child_touched: HashSet<&String> = child
            .reads
            .iter()
            .chain(child.reaches.iter())
            .chain(child.writes.iter())
            .chain(child.atomics.iter())
            .collect();
        for v in &child_touched {
            if shared.contains(*v)
                && child
                    .vars
                    .get(*v)
                    .map(|i| i.is_guard)
                    .unwrap_or(false)
            {
                raise(&mut holds, v, Hold::Transferred);
            }
        }
        // Only shared vars become parent-visible holds. If the new task's
        // holds conflict with a sibling task's, that's a race between them.
        holds.retain(|v, _| shared.contains(v));
        for (v, k) in &holds {
            // A spawned task's touches are the SPAWNING context's transitive
            // touches for its whole window — nested holds propagate upward
            // through reads/writes even if the child joins its subtask first.
            match k {
                Hold::Shared => {
                    env.reads.insert(v.clone());
                }
                Hold::Atomic => {
                    env.atomics.insert(v.clone());
                }
                // A synchronized hold propagates as synchronized: nested
                // task windows keep the domain tag.
                Hold::Sync(doms) => {
                    env.sync_writes
                        .entry(v.clone())
                        .or_default()
                        .extend(doms.iter().cloned());
                }
                _ => {
                    env.writes.insert(v.clone());
                }
            }
            if let Some((_, other, sp)) = env.task_holds.get(v).cloned() {
                // Two Sync holds are compatible ONLY when both are the same
                // singleton domain — a task that may write under either of
                // two locks can meet the sibling under the other one.
                let compatible = match (k, &other) {
                    (Hold::Shared, Hold::Shared) | (Hold::Atomic, Hold::Atomic) => true,
                    (Hold::Sync(a), Hold::Sync(b)) => a.len() == 1 && a == b,
                    _ => false,
                };
                if !compatible {
                    env.diags.push(Diagnostic::new(
                        sp,
                        "data-race",
                        format!("two spawned tasks may access `{v}` concurrently"),
                    ));
                }
            }
            env.task_holds.insert(v.clone(), (id, k.clone(), span));
        }
        // The child's definite kills / live edges on shared vars, its exit
        // states, and its return value all apply at join.
        let kills: HashSet<(String, String)> = child
            .killed_paths
            .iter()
            .filter(|(r, _)| shared.contains(r))
            .cloned()
            .collect();
        let edges: HashSet<AliasEdge> = child
            .edges
            .iter()
            .filter(|(h, _, t)| shared.contains(h) && shared.contains(t))
            .cloned()
            .collect();
        let cap_edges: HashSet<AliasEdge> = child
            .cap_edges
            .iter()
            .filter(|(h, _, t)| shared.contains(h) && shared.contains(t))
            .cloned()
            .collect();
        let mut exit = HashMap::new();
        for v in holds.keys() {
            if let Some(i) = child.vars.get(v) {
                exit.insert(v.clone(), i.state);
            }
        }
        let (ret_edges, ret_caps, ret_base, ret_is_handle) =
            child.ret_out.unwrap_or_default();
        // A returned ref to a task-local var dangles past the task.
        for (_f, t) in &ret_edges {
            if !shared.contains(t) {
                env.diags.push(Diagnostic::new(
                    span,
                    "task-escapes-scope",
                    format!(
                        "task returns a reference to `{t}`, which does not outlive the task"
                    ),
                ));
            }
        }
        let ret_edges: Vec<(Option<String>, String)> = ret_edges
            .into_iter()
            .filter(|(_, t)| shared.contains(t))
            .collect();
        let ret_caps: Vec<(Option<String>, String)> = ret_caps
            .into_iter()
            .filter(|(_, t)| shared.contains(t))
            .collect();
        let ret_base = ret_base.filter(|b| shared.contains(b));
        if let Some(b) = &ret_base {
            // The returned var's authority moves out through `join`.
            exit.insert(b.clone(), Temp::Consumed);
        }
        env.tasks.insert(
            id,
            TaskResult {
                holds,
                exit,
                kills,
                edges,
                cap_edges,
                ret_edges,
                ret_caps,
                ret_is_handle,
                ret_base,
            },
        );
        id
    }

    /// An atomic builtin: `atomic_op(place, extra...)`. The first argument
    /// names atomic storage — resolved through alias AND capability edges.
    /// Each resolved target records an `atomics` touch (raised to a
    /// `Hold::Atomic` at capture); a stale ticket reports `stale-handle`;
    /// nothing resolvable reports `non-atomic-op`. The operation carries
    /// the synchronization — no read/write is recorded on the target, so a
    /// child's atomic op never masquerades as a plain read.
    fn atomic_op(env: &mut Env, args: &[Expr], span: Span) {
        let Some(place) = args.first() else { return };
        let stale = match base_var(place) {
            Some(b) => {
                let p = field_path(place).map(|(_, p)| p);
                let dead = is_stale_handle(env, b, p.as_deref());
                if dead {
                    env.diags.push(Diagnostic::new(
                        span,
                        "stale-handle",
                        format!("`{b}` is a handle whose target is already dead"),
                    ));
                }
                dead
            }
            None => false,
        };
        let targets = atomic_targets(env, place);
        if targets.is_empty() && !stale {
            env.diags.push(Diagnostic::new(
                span,
                "non-atomic-op",
                "atomic operation requires `atomic<T>` storage".to_string(),
            ));
        }
        for t in &targets {
            match env.vars.get(t.as_str()).map(|i| i.state) {
                Some(Temp::Consumed) => env.diags.push(Diagnostic::new(
                    span,
                    "use-after-consume",
                    format!("`{t}` was already consumed"),
                )),
                Some(Temp::Maybe) => env.diags.push(Diagnostic::new(
                    span,
                    "use-after-consume",
                    format!("`{t}` may already have been consumed on another path"),
                )),
                _ => {}
            }
            env.atomics.insert(t.clone());
            Self::check_hold(env, t, span, Acc::Atomic);
        }
        // The named holder is read (a ticket/alias is a plain read); skip
        // it when it IS the atomic target — the op itself is the access.
        if let Some(b) = base_var(place) {
            let b = b.to_string();
            if !targets.contains(&b) && !stale {
                Self::use_var(env, &b, span);
            }
        }
        for a in &args[1..] {
            Self::eval_value(env, a);
        }
    }

    /// A lock builtin: `lock(m)` derives a guard, `unlock(g)` releases one.
    /// `lock` resolves its argument to a `(domain, root)` mutex place —
    /// through a field (`s.a`) or a ticket (`lock(hm)` reaches `m`) — and
    /// requires the domain not be held in THIS env (non-recursive v1).
    /// The acquisition itself is a synchronized access on the domain: it
    /// contends with ordinary holds but not with same-domain guards. The
    /// guard's authority lands when the result is bound (`pending_guard`).
    fn lock_op(env: &mut Env, name: &str, args: &[Expr], span: Span) {
        let Some(arg) = args.first() else { return };
        if name == "unlock" {
            // `unlock(g)` consumes the guard — affine resource semantics.
            let Some(b) = base_var(arg) else {
                Self::eval_value(env, arg);
                return;
            };
            let b = b.to_string();
            if !env.vars.get(&b).map(|i| i.is_guard).unwrap_or(false) {
                env.diags.push(Diagnostic::new(
                    span,
                    "non-mutex-op",
                    format!("`{b}` is not a live lock guard"),
                ));
                return;
            }
            Self::consume_named(env, &b, span, Op::Unlock);
            return;
        }
        // `lock(...)`: stale-ticket check, then domain resolution.
        let mut stale = false;
        if let Some(b) = base_var(arg) {
            let p = field_path(arg).map(|(_, p)| p);
            if is_stale_handle(env, b, p.as_deref()) {
                stale = true;
                env.diags.push(Diagnostic::new(
                    span,
                    "stale-handle",
                    format!("`{b}` is a handle whose target is already dead"),
                ));
            }
        }
        match mutex_place(env, arg) {
            Some((domain, root)) => {
                if env.live_domains.contains(&domain) {
                    env.diags.push(Diagnostic::new(
                        span,
                        "recursive-lock",
                        format!("`{domain}` is already locked by a live guard in this scope"),
                    ));
                }
                match env.vars.get(&root).map(|i| i.state) {
                    Some(Temp::Consumed) => env.diags.push(Diagnostic::new(
                        span,
                        "use-after-consume",
                        format!("`{root}` was already consumed"),
                    )),
                    Some(Temp::Maybe) => env.diags.push(Diagnostic::new(
                        span,
                        "use-after-consume",
                        format!("`{root}` may already have been consumed on another path"),
                    )),
                    _ => {}
                }
                env.live_domains.insert(domain.clone());
                env.pending_guard = Some((domain.clone(), root.clone()));
                Self::check_hold(env, &root, span, Acc::Sync(domain));
            }
            None => {
                if !stale {
                    env.diags.push(Diagnostic::new(
                        span,
                        "non-mutex-op",
                        "lock requires `mutex<T>` storage".to_string(),
                    ));
                }
            }
        }
        // Locking THROUGH a ticket reads the ticket, not the mutex.
        if let Some(b) = base_var(arg) {
            let b = b.to_string();
            let resolved_root = mutex_place(env, arg).map(|(_, r)| r);
            if resolved_root.as_deref() != Some(b.as_str()) && !stale {
                Self::use_var(env, &b, span);
            }
        }
    }

    /// Install the most recently popped `recv` packet on `name`: flags,
    /// and the alias/capability edges the message carried. This is the
    /// receive-side graph transform `G_channel -> G_receiver + authority`.
    fn claim_packet(env: &mut Env, name: &str) {
        let Some(pkt) = env.pending_recv.take() else {
            return;
        };
        if let Some(i) = env.vars.get_mut(name) {
            i.is_handle |= pkt.is_handle;
            i.is_ref |= pkt.is_ref;
            i.is_atomic |= pkt.is_atomic;
            i.is_mutex |= pkt.is_mutex;
            if pkt.is_handle {
                i.copy = true;
            }
            if pkt.ty_name.is_some() {
                i.ty_name = pkt.ty_name.clone();
            }
        }
        for (fld, t) in &pkt.edges {
            env.edges.insert((name.to_string(), fld.clone(), t.clone()));
        }
        for (fld, t) in &pkt.caps {
            env.cap_edges.insert((name.to_string(), fld.clone(), t.clone()));
        }
    }

    /// A `send`/`recv`/`close` builtin. The endpoint is shared state:
    /// every op is a synchronized access on the endpoint itself
    /// (`sync_writes[ch] = {ch}`), so sibling channel ops are legal and
    /// ordinary access to the endpoint races them.
    ///
    ///   `send(ch, v)` — cut v's authority packet out of this env, push it
    ///                   onto ch's queue (`G_s -> G_s - auth + G_ch`)
    ///   `recv(ch)`    — pop one packet; the claimed edges/targets land in
    ///                   `pending_recv` for the binding to claim, and the
    ///                   sent targets leave `in_transit` in THIS env
    ///                   (`G_ch -> G_r + auth`)
    ///   `close(ch)`   — affine consume of the endpoint
    fn chan_op(env: &mut Env, name: &str, args: &[Expr], span: Span) {
        let Some(ch_e) = args.first() else { return };
        let mut stale = false;
        if let Some(b) = base_var(ch_e) {
            let p = field_path(ch_e).map(|(_, p)| p);
            if is_stale_handle(env, b, p.as_deref()) {
                stale = true;
                env.diags.push(Diagnostic::new(
                    span,
                    "stale-handle",
                    format!("`{b}` is a handle whose target is already dead"),
                ));
            }
        }
        let Some(ch) = chan_root(env, ch_e) else {
            if !stale {
                let b = base_var(ch_e)
                    .map(|b| b.to_string())
                    .unwrap_or_else(|| "?".to_string());
                env.diags.push(Diagnostic::new(
                    span,
                    "non-channel-op",
                    format!("`{b}` is not a `channel<T>` endpoint"),
                ));
            }
            for a in args {
                Self::eval_value(env, a);
            }
            return;
        };
        // Operating through a ticket reads the ticket, not the endpoint.
        if let Some(b) = base_var(ch_e) {
            if b != ch.as_str() && !stale {
                Self::use_var(env, b, span);
            }
        }
        // The endpoint itself must be live; then the op is a synchronized
        // access on it.
        match env.vars.get(&ch).map(|i| i.state) {
            Some(Temp::Consumed) => {
                env.diags.push(Diagnostic::new(
                    span,
                    "use-after-consume",
                    format!("channel `{ch}` was already consumed"),
                ));
                return;
            }
            Some(Temp::Maybe) => {
                env.diags.push(Diagnostic::new(
                    span,
                    "use-after-consume",
                    format!("channel `{ch}` may already have been consumed on another path"),
                ));
                return;
            }
            _ => {}
        }
        env.sync_writes
            .entry(ch.clone())
            .or_default()
            .insert(ch.clone());
        Self::check_hold(env, &ch, span, Acc::Sync(ch.clone()));
        match name {
            "close" => Self::consume_named(env, &ch, span, Op::Close),
            "send" => {
                let pkt = Self::packet_for(env, args.get(1), span);
                env.chan_queues.entry(ch).or_default().push(pkt);
            }
            _ => {
                // `recv`: pop one packet. Its sent targets' authority
                // arrives in THIS env; a packet whose targets already died
                // delivers a provably-stale ticket (the generation check,
                // made static across the transport boundary).
                let pkt = env
                    .chan_queues
                    .get_mut(&ch)
                    .and_then(|q| if q.is_empty() { None } else { Some(q.remove(0)) });
                if let Some(pkt) = pkt {
                    for t in &pkt.sent_targets {
                        env.in_transit.remove(t);
                    }
                    for (_, t) in pkt.edges.iter().chain(pkt.caps.iter()) {
                        let alive = env
                            .vars
                            .get(t)
                            .map(|i| i.state == Temp::Live)
                            .unwrap_or(false);
                        if !alive {
                            env.diags.push(Diagnostic::new(
                                span,
                                "stale-handle",
                                format!(
                                    "received a ticket to `{t}`, which is already dead"
                                ),
                            ));
                        }
                    }
                    env.pending_recv = Some(pkt);
                }
            }
        }
        if args.len() > 2 {
            for a in &args[2..] {
                Self::eval_value(env, a);
            }
        }
    }

    /// Cut `send`'s payload out of the sender's graph into a `Packet`.
    ///   `&x`/`&mut x` — the capability travels; `&mut x` also moves the
    ///                   target's exclusive authority into the channel
    ///                   (`in_transit`)
    ///   `handle e`    — a ticket travels: identity, no transit
    ///   `v` (is_ref)  — the capability binding travels; its edge targets
    ///                   enter `in_transit`
    ///   `v` (handle)  — the ticket copies across; sender keeps it
    ///   `v` (value)   — affine transfer Live -> Consumed; a value still
    ///                   carrying field alias edges is `send-escape`
    ///   `v` (guard)   — `guard-escape`: scope-bound authority
    ///   other exprs   — evaluated normally; opaque payload
    fn packet_for(env: &mut Env, e: Option<&Expr>, span: Span) -> Packet {
        let mut pkt = Packet {
            is_handle: false,
            is_ref: false,
            is_atomic: false,
            is_mutex: false,
            ty_name: None,
            edges: Vec::new(),
            caps: Vec::new(),
            sent_targets: Vec::new(),
        };
        let Some(e) = e else { return pkt };
        match e {
            Expr::Persist {
                kind: PersistKind::Handle,
                ..
            } => {
                pkt.is_handle = true;
                pkt.caps = Self::value_cap_edges(env, e);
                Self::eval_value(env, e);
            }
            Expr::Ref(p, pspan) => {
                pkt.is_ref = true;
                pkt.edges = base_var(p)
                    .into_iter()
                    .map(|b| (None, b.to_string()))
                    .collect();
                Self::use_place(env, p, *pspan);
            }
            Expr::MutRef(p, _pspan) => {
                // The exclusive capability transfers wholesale: the send
                // itself is the only sender-side touch — no residual read
                // mark (the authority leaves; `in_transit` is the trace).
                pkt.is_ref = true;
                pkt.edges = base_var(p)
                    .into_iter()
                    .map(|b| (None, b.to_string()))
                    .collect();
                pkt.sent_targets = pkt.edges.iter().map(|(_, t)| t.clone()).collect();
                for t in pkt.sent_targets.clone() {
                    Self::check_hold(env, &t, span, Acc::Write);
                    Self::transit_insert(env, &t, span);
                }
            }
            Expr::Var(v, vspan) => {
                let info = env
                    .vars
                    .get(v.as_str())
                    .cloned()
                    .unwrap_or_else(Info::live);
                pkt.is_handle = info.is_handle;
                pkt.is_ref = info.is_ref;
                pkt.is_atomic = info.is_atomic;
                pkt.is_mutex = info.is_mutex;
                pkt.ty_name = info.ty_name.clone();
                pkt.edges = Self::value_alias_edges(env, e);
                pkt.caps = Self::value_cap_edges(env, e);
                if info.is_guard {
                    env.diags.push(Diagnostic::new(
                        *vspan,
                        "guard-escape",
                        "a lock guard cannot be sent through a channel".to_string(),
                    ));
                } else if info.copy || info.is_handle {
                    // Copy payloads and tickets copy — sender keeps them.
                    Self::use_var(env, v, *vspan);
                } else if info.is_ref {
                    // A capability binding transfers wholesale: consume the
                    // binding, move its edge targets' authority into the
                    // channel.
                    Self::check_hold(env, v, *vspan, Acc::Write);
                    match info.state {
                        Temp::Live => {
                            if let Some(i) = env.vars.get_mut(v.as_str()) {
                                i.state = Temp::Consumed;
                            }
                            env.kill_holder(v);
                            pkt.sent_targets =
                                pkt.edges.iter().map(|(_, t)| t.clone()).collect();
                            for t in pkt.sent_targets.clone() {
                                Self::check_hold(env, &t, *vspan, Acc::Write);
                                Self::transit_insert(env, &t, span);
                            }
                        }
                        Temp::Consumed | Temp::Maybe => env.diags.push(Diagnostic::new(
                            *vspan,
                            "double-consume",
                            format!(
                                "`{v}` {} already have been consumed",
                                if info.state == Temp::Maybe { "may" } else { "was" }
                            ),
                        )),
                    }
                } else {
                    // An owned value transfers — but not while it still
                    // carries references into sender memory (edge closure:
                    // the promote rule restated for transport).
                    if pkt.edges.iter().any(|(f, _)| f.is_some()) {
                        env.diags.push(Diagnostic::new(
                            *vspan,
                            "send-escape",
                            format!(
                                "`{v}` carries a live reference field — it cannot cross a channel boundary"
                            ),
                        ));
                    }
                    pkt.sent_targets =
                        pkt.edges.iter().map(|(_, t)| t.clone()).collect();
                    Self::consume_named(env, v, *vspan, Op::Move);
                    for t in pkt.sent_targets.clone() {
                        Self::transit_insert(env, &t, span);
                    }
                }
            }
            other => Self::eval_value(env, other),
        }
        pkt
    }

    /// Move `t`'s authority into the channel: a target already consumed or
    /// already in transit reports; otherwise the target joins the
    /// in-transit set (authority location: channel).
    fn transit_insert(env: &mut Env, t: &str, span: Span) {
        match env.vars.get(t).map(|i| i.state) {
            Some(Temp::Consumed) => env.diags.push(Diagnostic::new(
                span,
                "use-after-consume",
                format!("`{t}` was already consumed"),
            )),
            Some(Temp::Maybe) => env.diags.push(Diagnostic::new(
                span,
                "use-after-consume",
                format!("`{t}` may already have been consumed on another path"),
            )),
            _ => {}
        }
        if env.in_transit.contains(t) {
            env.diags.push(Diagnostic::new(
                span,
                "use-after-send",
                format!("`{t}`'s authority is already channel-held"),
            ));
            return;
        }
        env.in_transit.insert(t.to_string());
    }

    /// `join t` merges the task's recorded outcome into this env: the holds
    /// it took are released, exit states apply (consumed authority stays
    /// consumed), definite kills fire, the child's surviving edges may-alias
    /// — the child's world becomes the parent's. Sequential again after.
    /// On a Maybe handle (a conditional spawn) the effects downgrade:
    /// Consumed becomes Maybe, kills don't fire, edges still union.
    fn join_task(env: &mut Env, inner: &Expr, span: Span) {
        let Some(name) = base_var(inner).map(|b| b.to_string()) else {
            Self::eval_value(env, inner);
            return;
        };
        let Some(info) = env.vars.get(&name).cloned() else {
            return;
        };
        if info.task_id.is_none() {
            env.diags.push(Diagnostic::new(
                span,
                "invalid-join",
                format!("`{name}` is not a spawned task"),
            ));
            return;
        }
        if info.state == Temp::Consumed {
            env.diags.push(Diagnostic::new(
                span,
                "use-after-consume",
                format!("task `{name}` was already joined"),
            ));
            return;
        }
        let maybe = info.state == Temp::Maybe;
        let Some(id) = info.task_id else { return };
        let Some(res) = env.tasks.remove(&id) else { return };
        // This task's holds end here.
        env.task_holds.retain(|_, (tid, _, _)| *tid != id);
        for (v, st) in &res.exit {
            let target = if maybe && *st == Temp::Consumed {
                Temp::Maybe
            } else {
                *st
            };
            let cur = env.vars.get(v).map(|i| i.state);
            match (cur, target) {
                (Some(Temp::Live), Temp::Consumed) => {
                    env.vars.get_mut(v).unwrap().state = Temp::Consumed;
                    env.kill_holder(v);
                    env.kill_cap_target(v);
                }
                (Some(Temp::Live), Temp::Maybe) => {
                    env.vars.get_mut(v).unwrap().state = Temp::Maybe;
                }
                _ => {}
            }
        }
        if !maybe {
            for (root, path) in &res.kills {
                if path == "*" {
                    env.kill_holder(root);
                    env.edges.retain(|(_, _, t)| t != root);
                } else {
                    env.kill_field(root, path);
                }
                env.killed_paths.insert((root.clone(), path.clone()));
            }
        }
        env.edges.extend(res.edges.iter().cloned());
        env.cap_edges.extend(res.cap_edges.iter().cloned());
        if let Some(i) = env.vars.get_mut(&name) {
            i.state = Temp::Consumed;
            i.task_id = None;
        }
    }

    fn eval_value(env: &mut Env, e: &Expr) {
        match e {
            Expr::Int(..) | Expr::Bool(..) | Expr::Alloc { .. } => {}
            Expr::Var(name, span) => {
                let (copy, is_ref, is_handle) = env
                    .vars
                    .get(name.as_str())
                    .map(|i| (i.copy, i.is_ref, i.is_handle))
                    .unwrap_or((false, false, false));
                if !copy && !is_ref && !is_handle {
                    // Reading a non-Copy owned value transfers it (a move).
                    Self::consume_named(env, name, *span, Op::Move);
                } else {
                    Self::use_var(env, name, *span);
                }
            }
            Expr::Ref(place, span) | Expr::MutRef(place, span) => {
                Self::use_place(env, place, *span)
            }
            Expr::Deref(inner, span) => {
                if let Some(b) = base_var(inner) {
                    env.reaches.insert(b.to_string());
                    for t in reach_targets(env, b) {
                        Self::check_hold(env, &t, *span, Acc::Read);
                    }
                    Self::use_var(env, b, *span);
                    if env.vars.get(b).map(|i| i.is_mutex).unwrap_or(false) {
                        env.diags.push(Diagnostic::new(
                            *span,
                            "unguarded-access",
                            format!("`{b}` is `mutex<T>` storage — its interior is only reachable through a live guard"),
                        ));
                    }
                } else {
                    Self::eval_value(env, inner);
                }
            }
            Expr::Field(base, _, span) => {
                if let Some(b) = base_var(base) {
                    env.reaches.insert(b.to_string());
                    // A field access may reach through the base's edges —
                    // `h.fd` reads the handle's target, not just `h`.
                    for t in reach_targets(env, b) {
                        Self::check_hold(env, &t, *span, Acc::Read);
                    }
                    Self::use_var(env, b, *span);
                    if let Some((r, p)) = field_path(e) {
                        if is_stale_handle(env, &r, Some(&p)) {
                            env.diags.push(Diagnostic::new(
                                *span,
                                "stale-handle",
                                format!("`{r}.{p}` is a handle whose target is already dead"),
                            ));
                        }
                    }
                    if let Some(mp) = unguarded_path(env, e) {
                        env.diags.push(Diagnostic::new(
                            *span,
                            "unguarded-access",
                            format!("`{mp}` is `mutex<T>` storage — its interior is only reachable through a live guard"),
                        ));
                    }
                } else {
                    Self::eval_value(env, base);
                }
            }
            // `promote` moves the value; `handle` merely reads it — a handle is
            // generation-checked and does not extend the referent's lifetime.
            Expr::Persist { kind: PersistKind::Promote, inner, span } => {
                if let Some(b) = base_var(inner) {
                    let b = b.to_string();
                    Self::consume_named(env, &b, *span, Op::Move);
                } else {
                    Self::eval_value(env, inner);
                }
            }
            Expr::Persist { kind: PersistKind::Handle, inner, span } => {
                if let Some(b) = base_var(inner) {
                    Self::use_var(env, b, *span);
                } else {
                    Self::eval_value(env, inner);
                }
            }
            // `mutex e` wraps `e` — moves it in if non-Copy (like `promote`).
            Expr::Persist { kind: PersistKind::Mutex, inner, span } => {
                if let Some(b) = base_var(inner) {
                    Self::consume_named(env, &b.to_string(), *span, Op::Move);
                } else {
                    Self::eval_value(env, inner);
                }
            }
            Expr::StructLit { fields, .. } => {
                for (_, v) in fields {
                    Self::eval_value(env, v);
                }
            }
            // A spawn whose handle is dropped: the task still runs — its
            // holds persist and report when a captured var dies unjoined.
            Expr::Spawn { body, span } => {
                Self::spawn_task(env, body, *span);
            }
            Expr::Join { inner, span } => Self::join_task(env, inner, *span),
            Expr::Call { name, args, span } if is_atomic_op(name) => {
                Self::atomic_op(env, args, *span);
            }
            Expr::Call { name, args, span } if is_lock_op(name) => {
                Self::lock_op(env, name, args, *span);
            }
            Expr::Call { name, args, span } if is_chan_op(name) => {
                Self::chan_op(env, name, args, *span);
            }
            Expr::Call { name, args, span } => {
                let sig = env.sigs.get(name).cloned();
                let eff = env.summaries.get(name).cloned();
                let actuals: Vec<Option<String>> =
                    args.iter().map(|a| arg_base(a)).collect();
                for (i, arg) in args.iter().enumerate() {
                    let mode = sig.as_ref().and_then(|s| s.0.get(i).copied());
                    match mode {
                        // own: consumes the argument outright.
                        Some(Mode::Own) => Self::consume(env, arg, *span, Op::Move),
                        // mut: borrowed exclusively — a write capability,
                        // and writes through it may reach edge targets.
                        // UNLESS the callee's summary says those writes are
                        // lock-mediated: then `apply_effect` records them as
                        // synchronized and no ordinary-access mark lands here.
                        Some(Mode::Mut) => {
                            let synced = eff
                                .as_ref()
                                .map(|e| e.syncs.iter().any(|(pi, _)| *pi == i))
                                .unwrap_or(false);
                            if synced {
                                // Still requires a live argument.
                                if let Some(Some(a)) = actuals.get(i) {
                                    let a = a.clone();
                                    match env.vars.get(&a).map(|i| i.state) {
                                        Some(Temp::Consumed) => env.diags.push(
                                            Diagnostic::new(
                                                *span,
                                                "use-after-consume",
                                                format!("`{a}` was already consumed"),
                                            ),
                                        ),
                                        Some(Temp::Maybe) => env.diags.push(
                                            Diagnostic::new(
                                                *span,
                                                "use-after-consume",
                                                format!("`{a}` may already have been consumed on another path"),
                                            ),
                                        ),
                                        _ => {}
                                    }
                                } else {
                                    Self::use_place(env, arg, *span);
                                }
                            } else {
                                Self::use_place(env, arg, *span);
                                if let Some(Some(a)) = actuals.get(i) {
                                    env.writes.insert(a.clone());
                                    for t in reach_targets(env, a) {
                                        env.writes.insert(t);
                                    }
                                }
                            }
                        }
                        // ref: borrowed — the callee may dereference it.
                        Some(Mode::Ref) => {
                            Self::use_place(env, arg, *span);
                            if let Some(Some(a)) = actuals.get(i) {
                                env.reaches.insert(a.clone());
                            }
                        }
                        // unknown callee: conservative read.
                        None => Self::eval_value(env, arg),
                    }
                }
                // Apply the callee's summary: a call is a graph
                // transformation G -> G', not a re-analysis.
                if let Some(eff) = eff {
                    Self::apply_effect(env, &eff, &actuals, &sig, *span);
                }
                // A `lock` result passed positionally is never bound —
                // the domain releases when the call returns.
                if let Some((dom, _)) = env.pending_guard.take() {
                    env.live_domains.remove(&dom);
                }
            }
            Expr::Binary { lhs, rhs, .. } => {
                Self::eval_value(env, lhs);
                Self::eval_value(env, rhs);
            }
        }
    }

    /// Check a whole program. The call graph is decomposed into strongly
    /// connected components processed callee-first: acyclic components are
    /// analyzed once; recursive components iterate to a fixed point where
    /// each pass union-absorbs newly observed summary facts (monotone — the
    /// fact space is finite, so this terminates). Diagnostics are collected
    /// only from each component's final, converged pass.
    pub fn analyze(program: &Program) -> (Vec<Diagnostic>, SummaryMap, AnalysisStats) {
        let sigs = signatures(program);
        // (struct, field) pairs declared `atomic<T>` — field-level atomic
        // storage for `s.cnt`-style atomic-op arguments.
        let atomic_fields: HashSet<(String, String)> = program
            .structs()
            .flat_map(|st| {
                st.fields
                    .iter()
                    .filter(|f| atomic_ty(&f.ty))
                    .map(|f| (st.name.clone(), f.name.clone()))
            })
            .collect();
        // (struct, field) pairs declared `mutex<T>` — field-level lockable
        // domains for `lock(s.a)`.
        let mutex_fields: HashSet<(String, String)> = program
            .structs()
            .flat_map(|st| {
                st.fields
                    .iter()
                    .filter(|f| mutex_ty(&f.ty))
                    .map(|f| (st.name.clone(), f.name.clone()))
            })
            .collect();
        let fns: HashMap<&str, &FnDef> =
            program.functions().map(|f| (f.name.as_str(), f)).collect();
        let mut summaries = SummaryMap::new();
        let mut diags = Vec::new();
        let mut stats = AnalysisStats::default();

        for scc in Self::sccs(program) {
            stats.scc_count += 1;
            stats.largest_scc = stats.largest_scc.max(scc.len());
            let recursive = scc.len() > 1
                || scc.iter().any(|n| {
                    fns.get(n.as_str())
                        .map(|f| Self::callees(&f.body).iter().any(|c| c == n))
                        .unwrap_or(false)
                });

            if !recursive {
                for name in &scc {
                    let Some(f) = fns.get(name.as_str()) else { continue };
                    let mut env = Env {
                        vars: HashMap::new(),
                        edges: HashSet::new(),
                        cap_edges: HashSet::new(),
                        dead_caps: HashSet::new(),
                        scopes: Vec::new(),
                        sigs: &sigs,
                        summaries: &summaries,
                        param_idx: HashMap::new(),
                        killed_paths: HashSet::new(),
                        reads: HashSet::new(),
                        reaches: HashSet::new(),
                        writes: HashSet::new(),
                        atomics: HashSet::new(),
                        atomic_fields: &atomic_fields,
                        mutex_fields: &mutex_fields,
                        live_domains: HashSet::new(),
                        sync_writes: HashMap::new(),
                        pending_guard: None,
                        chan_queues: HashMap::new(),
                        in_transit: HashSet::new(),
                        pending_recv: None,
                        task_holds: HashMap::new(),
                        tasks: HashMap::new(),
                        next_task: 1,
                        ret_out: None,
                        ret_alias: Vec::new(),
                        diags: Vec::new(),
                    };
                    let eff = Self::check_fn(&mut env, f);
                    diags.extend(env.diags);
                    summaries.insert(name.clone(), eff);
                }
                continue;
            }

            stats.recursive_sccs += 1;
            // Seed the SCC's summaries. kills/creates/consumes/maybe start at
            // bottom and grow to their true fixed point, but `ret_alias` is
            // seeded with the borrowed-arg bound: a cycle whose only return
            // path loops through itself has least fixpoint {} — and since the
            // checker doesn't prove such a function never returns, {} would
            // under-approximate may-alias (unsound). The seed is the
            // conservative top for exactly that one fact class.
            for name in &scc {
                let mut eff = Effect::default();
                if let Some(sig) = sigs.get(name.as_str()) {
                    if sig.1 {
                        eff.ret_alias = sig
                            .0
                            .iter()
                            .enumerate()
                            .filter(|(_, m)| matches!(m, Mode::Ref | Mode::Mut))
                            .map(|(i, _)| i)
                            .collect();
                        if !eff.ret_alias.is_empty() {
                            stats.seeded_returns += 1;
                        }
                    }
                }
                summaries.insert(name.clone(), eff);
            }

            let mut passes = 0;
            loop {
                passes += 1;
                let mut changed = false;
                let mut pass_diags = Vec::new();
                let mut pass_added = 0usize;
                for name in &scc {
                    let Some(f) = fns.get(name.as_str()) else { continue };
                    let (eff, ds) = {
                        let mut env = Env {
                            vars: HashMap::new(),
                            edges: HashSet::new(),
                            cap_edges: HashSet::new(),
                            dead_caps: HashSet::new(),
                            scopes: Vec::new(),
                            sigs: &sigs,
                            summaries: &summaries,
                            param_idx: HashMap::new(),
                            killed_paths: HashSet::new(),
                        reads: HashSet::new(),
                        reaches: HashSet::new(),
                        writes: HashSet::new(),
                        atomics: HashSet::new(),
                        atomic_fields: &atomic_fields,
                        mutex_fields: &mutex_fields,
                        live_domains: HashSet::new(),
                        sync_writes: HashMap::new(),
                        pending_guard: None,
                        chan_queues: HashMap::new(),
                        in_transit: HashSet::new(),
                        pending_recv: None,
                        task_holds: HashMap::new(),
                        tasks: HashMap::new(),
                        next_task: 1,
                        ret_out: None,
                            ret_alias: Vec::new(),
                            diags: Vec::new(),
                        };
                        let eff = Self::check_fn(&mut env, f);
                        (eff, std::mem::take(&mut env.diags))
                    };
                    let slot = summaries.get_mut(name).unwrap();
                    let before = slot.fact_count();
                    changed |= slot.absorb(&eff);
                    pass_added += slot.fact_count() - before;
                    pass_diags.extend(ds);
                }
                stats.facts_per_pass.push(pass_added);
                stats.max_iterations = stats.max_iterations.max(passes);
                if !changed {
                    diags.extend(pass_diags);
                    break;
                }
            }
        }
        stats.total_facts = summaries.values().map(|e| e.fact_count()).sum();
        stats.max_facts = summaries.values().map(|e| e.fact_count()).max().unwrap_or(0);
        // Loop bodies are checked twice (entry + back-edge), which can emit the
        // same diagnostic at a span twice; deduplicate.
        let mut seen = std::collections::HashSet::new();
        diags.retain(|d| seen.insert((d.span.line, d.span.col, d.kind)));
        (diags, summaries, stats)
    }

    /// Tarjan's strongly-connected-components over the call graph, emitted in
    /// callee-first order: a component pops only after every component it
    /// calls. Recursive component = size > 1 or a self-edge.
    fn sccs(program: &Program) -> Vec<Vec<String>> {
        struct T<'a> {
            fns: HashMap<&'a str, &'a FnDef>,
            index: HashMap<String, usize>,
            low: HashMap<String, usize>,
            stack: Vec<String>,
            on_stack: HashSet<String>,
            next: usize,
            out: Vec<Vec<String>>,
        }
        impl<'a> T<'a> {
            fn connect(&mut self, name: &str) {
                self.index.insert(name.to_string(), self.next);
                self.low.insert(name.to_string(), self.next);
                self.next += 1;
                self.stack.push(name.to_string());
                self.on_stack.insert(name.to_string());
                if let Some(f) = self.fns.get(name) {
                    for callee in Affine::callees(&f.body) {
                        if !self.fns.contains_key(callee.as_str()) {
                            continue;
                        }
                        if !self.index.contains_key(&callee) {
                            self.connect(&callee);
                            let nl = self.low[name].min(self.low[&callee]);
                            self.low.insert(name.to_string(), nl);
                        } else if self.on_stack.contains(&callee) {
                            let nl = self.low[name].min(self.index[&callee]);
                            self.low.insert(name.to_string(), nl);
                        }
                    }
                }
                if self.low[name] == self.index[name] {
                    let mut scc = Vec::new();
                    while let Some(n) = self.stack.pop() {
                        self.on_stack.remove(&n);
                        let done = n == name;
                        scc.push(n);
                        if done {
                            break;
                        }
                    }
                    self.out.push(scc);
                }
            }
        }
        let fns: HashMap<&str, &FnDef> =
            program.functions().map(|f| (f.name.as_str(), f)).collect();
        let mut t = T {
            fns,
            index: HashMap::new(),
            low: HashMap::new(),
            stack: Vec::new(),
            on_stack: HashSet::new(),
            next: 0,
            out: Vec::new(),
        };
        for f in program.functions() {
            if !t.index.contains_key(&f.name) {
                t.connect(&f.name);
            }
        }
        t.out
    }

    /// All function names called inside a block (transitive expr walk).
    fn callees(block: &Block) -> Vec<String> {
        let mut out = Vec::new();
        fn expr(e: &Expr, out: &mut Vec<String>) {
            match e {
                Expr::Call { name, args, .. } => {
                    out.push(name.clone());
                    for a in args {
                        expr(a, out);
                    }
                }
                Expr::Binary { lhs, rhs, .. } => {
                    expr(lhs, out);
                    expr(rhs, out);
                }
                Expr::Ref(p, _) | Expr::MutRef(p, _) | Expr::Deref(p, _) => expr(p, out),
                Expr::Field(b, _, _) => expr(b, out),
                Expr::Persist { inner, .. } | Expr::Join { inner, .. } => expr(inner, out),
                Expr::Spawn { body, .. } => walk(body, out),
                Expr::StructLit { fields, .. } => {
                    for (_, v) in fields {
                        expr(v, out);
                    }
                }
                _ => {}
            }
        }
        fn walk(b: &Block, out: &mut Vec<String>) {
            for s in &b.stmts {
                match s {
                    Stmt::Let { init, .. } => expr(init, out),
                    Stmt::Assign { value, .. } => expr(value, out),
                    Stmt::Return { value: Some(v), .. } => expr(v, out),
                    Stmt::If {
                        cond,
                        then_block,
                        else_block,
                        ..
                    } => {
                        expr(cond, out);
                        walk(then_block, out);
                        if let Some(eb) = else_block {
                            walk(eb, out);
                        }
                    }
                    Stmt::While { cond, body, .. } => {
                        expr(cond, out);
                        walk(body, out);
                    }
                    Stmt::Region { body, .. } => walk(body, out),
                    Stmt::Free { value, .. } => expr(value, out),
                    Stmt::Expr { expr: e, .. } => expr(e, out),
                    _ => {}
                }
            }
        }
        walk(block, &mut out);
        out
    }
}

impl MemoryModel for Affine {
    fn name(&self) -> &'static str {
        "affine"
    }

    fn tagline(&self) -> &'static str {
        "minimal temporal state: Live/Consumed/Maybe on consumables only"
    }

    fn profile(&self) -> crate::check::Profile {
        crate::check::Profile {
            states: &["res-live", "res-consumed", "res-maybe"],
            resource_states: &[],
            temporal_states: 3,
            lifetime_concepts: 0,
            move_rules: 1,
            // 6 consume-guards + concurrency: data-race, use-after-send,
            // task-escapes-scope, invalid-join + stale-handle
            // + non-atomic-op + mutex: unguarded-access, guard-escape,
            //   recursive-lock, non-mutex-op.
            rules: 18,
            path_sensitive: true,
            machinery: &[
                "path-merge",
                "alias-edge-graph",
                "definite-kills(scope|rebind|field|consume|call|join)",
                "effect-summaries",
                "scc-fixpoint",
                "capability-partition(holds)",
                "join-merge",
                "task-escape-check",
                "capability-edges(identity)",
                "stale-handle-tracking",
                "atomic-op-resolution",
                "synchronized-domains(guards)",
                "channel-packets(in-transit)",
            ],
        }
    }

    fn check(&self, program: &Program) -> Vec<Diagnostic> {
        Self::analyze(program).0
    }
}
