//! The typed-AST pass. Scope: machine ints + bool + aggregates +
//! references, lexical scopes, function signatures, literal fitting,
//! constant-index bounds — and the first imported lab semantic:
//! REGION PROVENANCE.
//!
//! Every expression yields `(Ty, Prov)`. A region is a lexical frame:
//! the function body is the root; each `region name { .. }` nests one
//! deeper. Locals (and params) belong to the region that binds them;
//! `&x` produces a reference whose provenance is x's owner region.
//!
//! The one rule: a reference may only flow *outward* — storing prov F
//! into a location owned by region D requires F to be an
//! ancestor-or-equal of D (the referent outlives the location).
//! Returning `&T` requires `Param` provenance — abstract caller
//! lifetime that `&T` parameters carry — so `return &x` can never
//! typecheck for local x, and `fn id(r: &i32) -> &i32 { return r }` can.
//!
//! Deliberately NOT here (documented gaps, later milestones):
//!   - borrow conflicts: `&mut x` twice, `&mut x` + read of x — needs
//!     the alias graph (N4)
//!   - dynamic index bounds
//!   - per-element array provenance (all `a[i]` refs share `a[]`)

use crate::frontend::ast::*;
use crate::index::{self, DefLoc};
use crate::{Diag, Label, Span};
use std::collections::{BTreeSet, HashMap, HashSet};

/// `struct name -> field table`, shared by every function check.
type Structs<'a> = HashMap<String, &'a Vec<(String, Ty)>>;
/// name -> variant list (decl order = tag order). Variant payload is
/// `Option<Ty>` — `None` is a nullary case. An enum also registers a
/// pseudo field-list in `structs` so type/layout/field lookup treat
/// `x.Variant` as an ordinary place (N22).
type Enums<'a> = HashMap<String, &'a Vec<(String, Option<Ty>)>>;

/// Where a reference's referent lives. Order by lifetime:
/// `Param` (the caller's frame) outlives every `Region`; `Region(a)`
/// outlives `Region(b)` iff a is b's ancestor-or-equal in the tree.
/// `Unknown` = uninitialized — any use is an error.
#[derive(Debug, Clone, PartialEq)]
enum Prov {
    Unknown,
    Region(u32),
    /// Abstract caller lifetime — carried by `&T`/`&mut T` params.
    Param,
}

impl Prov {
    fn describe(&self, regs: &[RegionFrame]) -> String {
        match self {
            Prov::Unknown => "uninitialized".into(),
            Prov::Param => "the caller's frame".into(),
            Prov::Region(id) => format!("region `{}`", regs[*id as usize].name),
        }
    }
}

/// Join on branches: the weaker guarantee (shorter lifetime) wins —
/// LCA on the region tree. `Param` is the top lifetime, so
/// join(Param, r) = r. Unknown stays Unknown (maybe-uninitialized).
fn join_prov(a: &Prov, b: &Prov, regs: &[RegionFrame]) -> Prov {
    match (a, b) {
        (Prov::Unknown, p) | (p, Prov::Unknown) => p.clone(),
        (Prov::Param, Prov::Param) => Prov::Param,
        (Prov::Param, p) | (p, Prov::Param) => p.clone(),
        (Prov::Region(x), Prov::Region(y)) => Prov::Region(lca(*x, *y, regs)),
    }
}

#[derive(Debug, Clone)]
struct RegionFrame {
    name: String,
    parent: Option<u32>,
}

fn is_ancestor_or_eq(a: u32, b: u32, regs: &[RegionFrame]) -> bool {
    let mut cur = Some(b);
    while let Some(id) = cur {
        if id == a {
            return true;
        }
        cur = regs[id as usize].parent;
    }
    false
}

fn lca(a: u32, b: u32, regs: &[RegionFrame]) -> u32 {
    let mut chain = HashSet::new();
    let mut cur = Some(a);
    while let Some(id) = cur {
        chain.insert(id);
        cur = regs[id as usize].parent;
    }
    let mut cur = b;
    loop {
        if chain.contains(&cur) {
            return cur;
        }
        cur = regs[cur as usize].parent.expect("root has a parent?");
    }
}

/// A lexical binding: its type, the provenance of the reference value
/// it currently holds (for `&T` bindings), the region that owns its
/// storage, and an epoch distinguishing shadowed re-`let`s — edges
/// keyed `name#epoch` die with their binding instance.
#[derive(Debug, Clone)]
struct Bind {
    ty: Ty,
    /// Declaration site — N36B's go-to-definition for locals/params.
    decl: Span,
    prov: Prov,
    owner: u32,
    epoch: u32,
    /// Capability params (`read`/`write`/`own n: T`) are place
    /// aliases, not locals — `n` IS the caller's storage, reached
    /// through the param's own edge (`n#e -> $argN`).
    auth: Option<Auth>,
    /// N11: a live `lock` token — (domain key, protected root key,
    /// locker keys). `g.x` resolves to `root.x`; the bind's death or
    /// consume releases the domain. `lockers` are the holder keys the
    /// `lock` call borrowed THROUGH (`m#e` for `lock(m)` on a cap
    /// param) — their authority edges onto the domain are the lock's
    /// own derivation, not foreign claims.
    guard: Option<(String, String, Vec<String>)>,
}

// ---------- alias topology (N4) ----------
//
// Edges say who currently points where. Birth: `&x`, ref copies,
// call args, params. Death: scope exit, rebind, overwrite, move —
// structural only, never annotated. Both endpoints are place ids
// (`x#3`, `s#1.r`, `a#0[]`, `$arg2` for synthetic call/param targets).

/// Edge kinds ARE capability authorities (N5): topology and authority
/// ride one edge — `&x` writes Read, `&mut x` writes Write, and `own`
/// params hold Own. The matrix: Read+Read coexist; Write/Own are
/// exclusive against everything. Own adds the right to transfer.
type EdgeKind = Auth;

/// Certainty at a join: definite = the edge exists on every path;
/// possible = it exists on some path. Conflicts fire on both
/// (conservative) — a maybe-alias must be treated as an alias.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Cert {
    Def,
    Maybe,
}

/// The affine temporal state — the lab's Live/Consumed/MaybeConsumed,
/// minus Live (a key absent from `consumed` is Live; ordinary region
/// memory simply never enters the map).
#[derive(Debug, Clone, Copy, PartialEq, PartialOrd)]
enum ConsState {
    Maybe,
    Cons,
}

/// N35B — a consumed place plus WHERE it died. `at` keeps every
/// branch origin a Maybe fact carries (the paths it can be dead on) —
/// usually one span; a join of two different consume sites keeps both
/// so the diagnostic can name each path instead of lying.
#[derive(Debug, Clone)]
struct ConsFact {
    st: ConsState,
    at: Vec<Span>,
}

impl ConsFact {
    fn cons(at: Span) -> Self {
        ConsFact {
            st: ConsState::Cons,
            at: vec![at],
        }
    }
}

/// One edge's birth site — N35B "borrow created here" labels. `None`
/// for edges with no user-facing origin (parameter frames at function
/// entry, internal seeds).
#[derive(Debug, Clone, Copy, PartialEq)]
struct EdgeFact {
    kind: EdgeKind,
    cert: Cert,
    born: Option<Span>,
}

impl EdgeFact {
    fn new(kind: EdgeKind, cert: Cert, born: Option<Span>) -> Self {
        EdgeFact { kind, cert, born }
    }
}

// ---------- N72 — canonical check event log ----------
//
// Every semantic decision the checker commits — bindings, edges,
// caps, consumes, init marks, tags, provs, holds, kills, tasks,
// channels, defers, joins, summaries, diagnostics — appends a record
// in scan order. `dump_check` drains the log per module; the
// self-hosted checker must reproduce it byte-for-byte. Records are
// events, not map dumps: the commit point is the semantics.
thread_local! {
    static CHECK_LOG: std::cell::RefCell<Vec<String>> =
        std::cell::RefCell::new(Vec::new());
}

/// Drain the checker's event log — `dump_check` calls this after
/// each `check_module` so per-module records flush in module order.
pub fn take_check_log() -> Vec<String> {
    CHECK_LOG.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

/// `(check "file")` — the dump's head record, emitted by the driver.
pub fn log_head(file: &str) {
    rec(format!("(check {}", qs(file)));
}

/// `(module "name" "file")` / `(end)` — the per-module bracket.
/// `frontend_source` emits these around each `check_module` so the
/// log segments by compilation unit, dependency-order like the pass.
pub fn module_begin(name: &str, file: &str) {
    rec(format!("(module {} {}", qs(name), qs(file)));
}
pub fn module_end() {
    rec("(end)".to_string());
}

// Dry-run / speculative checking mutes the log: a cloned Ctx's
// events are NOT commits (the defer registration dry-run checks the
// call to surface its diagnostics early; the real effects land at
// exit-time replay, which emits its own events).
thread_local! {
    static CHECK_MUTE: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

fn rec(s: String) {
    if CHECK_MUTE.with(|m| m.get() == 0) {
        CHECK_LOG.with(|v| v.borrow_mut().push(s));
    }
}

/// `rec` under a name no parameter shadows (`replay_defer` binds a
/// `rec: &DeferRec`).
fn emit(s: String) {
    rec(s);
}

fn muted<R>(f: impl FnOnce() -> R) -> R {
    CHECK_MUTE.with(|m| m.set(m.get() + 1));
    let r = f();
    CHECK_MUTE.with(|m| m.set(m.get() - 1));
    r
}

fn auth_s(a: Auth) -> &'static str {
    match a {
        Auth::Read => "read",
        Auth::Write => "write",
        Auth::Own => "own",
        Auth::Atomic => "atomic",
        Auth::Sync => "sync",
    }
}

fn cert_s(c: Cert) -> &'static str {
    match c {
        Cert::Def => "def",
        Cert::Maybe => "maybe",
    }
}

fn prov_s(p: &Prov, regs: &[RegionFrame]) -> String {
    match p {
        Prov::Unknown => "?".into(),
        Prov::Param => "param".into(),
        Prov::Region(id) => format!("\"{}\"", regs[*id as usize].name),
    }
}

/// `@L:C` for a span, empty when the fact has no user-facing origin.
fn at_s(sp: Option<Span>) -> String {
    match sp {
        Some(s) => format!(" @{}:{}", s.line, s.col),
        None => String::new(),
    }
}

/// `"…"` quoting — escapes backslash, quote, and control chars.
fn qs(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `ctx.edges.insert` + the `(edge …)` event.
fn edge_set(ctx: &mut Ctx, h: String, t: String, fact: EdgeFact) {
    rec(format!(
        "(edge {} -> {} {} {}{})",
        qs(&h),
        qs(&t),
        auth_s(fact.kind),
        cert_s(fact.cert),
        at_s(fact.born)
    ));
    ctx.edges.insert((h, t), fact);
}

/// `ctx.caps.insert` + the `(cap …)` event.
fn cap_set(ctx: &mut Ctx, h: String, t: String, cert: Cert) {
    rec(format!("(cap {} -> {} {})", qs(&h), qs(&t), cert_s(cert)));
    ctx.caps.insert((h, t), cert);
}

/// `ctx.consumed.insert` + the `(cons …)` event — `at` sites print
/// in order.
fn cons_set(ctx: &mut Ctx, k: String, fact: ConsFact) {
    let st = match fact.st {
        ConsState::Cons => "cons",
        ConsState::Maybe => "maybe",
    };
    let mut s = format!("(cons {} {}", qs(&k), st);
    for at in &fact.at {
        s.push_str(&format!(" @{}:{}", at.line, at.col));
    }
    s.push(')');
    rec(s);
    ctx.consumed.insert(k, fact);
}

/// `ctx.init.insert` + the `(init …)` event.
fn init_set(ctx: &mut Ctx, k: String, cert: Cert) {
    rec(format!("(init {} {})", qs(&k), cert_s(cert)));
    ctx.init.insert(k, cert);
}

/// `ctx.tag.insert` + the `(tag …)` event — `?` = known-enum,
/// unknown-variant.
fn tag_set(ctx: &mut Ctx, k: String, t: Option<String>) {
    let v = match &t {
        Some(v) => qs(v),
        None => "?".into(),
    };
    rec(format!("(tag {} {})", qs(&k), v));
    ctx.tag.insert(k, t);
}

/// `ctx.path_provs.insert` + the `(prov …)` event.
fn prov_set(ctx: &mut Ctx, k: String, p: Prov) {
    rec(format!(
        "(prov {} {})",
        qs(&k),
        prov_s(&p, &ctx.regs)
    ));
    ctx.path_provs.insert(k, p);
}

/// `ctx.errs.push` + the `(err …)` event — labels and notes emit
/// inline in their own order.
fn push_diag(ctx: &mut Ctx, d: Diag) {
    let mut s = format!(
        "(err @{}:{} {}",
        d.span.line,
        d.span.col,
        qs(&d.msg)
    );
    for l in &d.labels {
        s.push_str(&format!(
            " (label @{}:{} {})",
            l.span.line,
            l.span.col,
            qs(&l.msg)
        ));
    }
    for n in &d.notes {
        s.push_str(&format!(" (note {})", qs(n)));
    }
    s.push(')');
    rec(s);
    ctx.errs.push(d);
}

/// `check_module`'s decl-level errs — same record shape, no Ctx.
fn push_decl_diag(errs: &mut Vec<Diag>, d: Diag) {
    let mut s = format!(
        "(err @{}:{} {}",
        d.span.line,
        d.span.col,
        qs(&d.msg)
    );
    for l in &d.labels {
        s.push_str(&format!(
            " (label @{}:{} {})",
            l.span.line,
            l.span.col,
            qs(&l.msg)
        ));
    }
    for n in &d.notes {
        s.push_str(&format!(" (note {})", qs(n)));
    }
    s.push(')');
    rec(s);
    errs.push(d);
}

/// `ctx.edge_kills.insert` + the `(kill …)` event — the edge graph's
/// structural deletion fact (the `retain` that applies it is
/// mechanical, not a fact).
fn kill_set(ctx: &mut Ctx, k: String, cert: Cert) {
    rec(format!("(kill {} {})", qs(&k), cert_s(cert)));
    ctx.edge_kills.insert(k, cert);
}

/// `ctx.cap_kills.insert` + the `(capkill …)` event.
fn capkill_set(ctx: &mut Ctx, k: String, cert: Cert) {
    rec(format!("(capkill {} {})", qs(&k), cert_s(cert)));
    ctx.cap_kills.insert(k, cert);
}

/// `ctx.transit.insert` + the `(transit …)` event — channel-held
/// authority.
fn transit_set(ctx: &mut Ctx, k: String) {
    rec(format!("(transit {})", qs(&k)));
    ctx.transit.insert(k);
}

/// `ctx.doms.insert` + the `(dom …)` event — a mutex domain claims a
/// protected root.
fn dom_set(ctx: &mut Ctx, d: String, r: String) {
    rec(format!("(dom {} {})", qs(&d), qs(&r)));
    ctx.doms.insert(d, r);
}

/// `ctx.slice_lens.insert` + the `(slen …)` event.
fn slen_set(ctx: &mut Ctx, k: String, n: i64) {
    rec(format!("(slen {} {})", qs(&k), n));
    ctx.slice_lens.insert(k, n);
}

/// `ctx.cstr_safe.insert` + the `(csafe …)` event.
fn csafe_set(ctx: &mut Ctx, k: String) {
    rec(format!("(csafe {})", qs(&k)));
    ctx.cstr_safe.insert(k);
}

/// `ctx.foreign.insert` + the `(foreign …)` event — a slice-capable
/// place with no graph edges to claim.
fn foreign_set(ctx: &mut Ctx, k: String) {
    rec(format!("(foreign {})", qs(&k)));
    ctx.foreign.insert(k);
}

/// `ctx.tag.remove` + the `(untag …)` event.
fn tag_unset(ctx: &mut Ctx, k: &str) {
    rec(format!("(untag {})", qs(k)));
    ctx.tag.remove(k);
}

/// `ctx.edges.retain` + an `(unedge …)` per removed fact. A removal
/// is a structural kill — observable state, so it logs.
fn edges_retain(ctx: &mut Ctx, mut f: impl FnMut(&(String, String), &EdgeFact) -> bool) {
    let mut dead: Vec<(String, String)> = ctx
        .edges
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(unedge {} {})", qs(&k.0), qs(&k.1)));
        ctx.edges.remove(&k);
    }
}

/// `ctx.caps.retain` + an `(uncap …)` per removed fact.
fn caps_retain(ctx: &mut Ctx, mut f: impl FnMut(&(String, String), &Cert) -> bool) {
    let mut dead: Vec<(String, String)> = ctx
        .caps
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(uncap {} {})", qs(&k.0), qs(&k.1)));
        ctx.caps.remove(&k);
    }
}

/// `ctx.consumed.retain` + an `(uncons …)` per removed fact.
fn cons_retain(ctx: &mut Ctx, mut f: impl FnMut(&String, &ConsFact) -> bool) {
    let mut dead: Vec<String> = ctx
        .consumed
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(uncons {})", qs(&k)));
        ctx.consumed.remove(&k);
    }
}

/// `ctx.init.retain` + an `(uninit …)` per removed fact.
fn init_retain(ctx: &mut Ctx, mut f: impl FnMut(&String, &Cert) -> bool) {
    let mut dead: Vec<String> = ctx
        .init
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(uninit {})", qs(&k)));
        ctx.init.remove(&k);
    }
}

/// `ctx.tag.retain` + an `(untag …)` per removed fact.
fn tag_retain(ctx: &mut Ctx, mut f: impl FnMut(&String, &Option<String>) -> bool) {
    let mut dead: Vec<String> = ctx
        .tag
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(untag {})", qs(&k)));
        ctx.tag.remove(&k);
    }
}

/// `ctx.slice_lens.retain` + an `(unslen …)` per removed fact.
fn slen_retain(ctx: &mut Ctx, mut f: impl FnMut(&String, &i64) -> bool) {
    let mut dead: Vec<String> = ctx
        .slice_lens
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(unslen {})", qs(&k)));
        ctx.slice_lens.remove(&k);
    }
}

/// `ctx.cstr_safe.retain` + an `(uncsafe …)` per removed fact.
fn csafe_retain(ctx: &mut Ctx, mut f: impl FnMut(&String) -> bool) {
    let mut dead: Vec<String> = ctx
        .cstr_safe
        .iter()
        .filter(|k| !f(k))
        .cloned()
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(uncsafe {})", qs(&k)));
        ctx.cstr_safe.remove(&k);
    }
}

/// `ctx.foreign.retain` + an `(unforeign …)` per removed fact.
fn foreign_retain(ctx: &mut Ctx, mut f: impl FnMut(&String) -> bool) {
    let mut dead: Vec<String> = ctx
        .foreign
        .iter()
        .filter(|k| !f(k))
        .cloned()
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(unforeign {})", qs(&k)));
        ctx.foreign.remove(&k);
    }
}

/// `ctx.path_provs.retain` + an `(unprov …)` per removed fact.
fn provs_retain(ctx: &mut Ctx, mut f: impl FnMut(&String, &Prov) -> bool) {
    let mut dead: Vec<String> = ctx
        .path_provs
        .iter()
        .filter(|(k, v)| !f(k, v))
        .map(|(k, _)| k.clone())
        .collect();
    dead.sort();
    for k in dead {
        rec(format!("(unprov {})", qs(&k)));
        ctx.path_provs.remove(&k);
    }
}

fn unkill(ctx: &mut Ctx, k: &str) {
    rec(format!("(unkill {})", qs(k)));
    ctx.edge_kills.remove(k);
}
fn uncapkill(ctx: &mut Ctx, k: &str) {
    rec(format!("(uncapkill {})", qs(k)));
    ctx.cap_kills.remove(k);
}
fn unhold(ctx: &mut Ctx, k: &str) {
    rec(format!("(unhold {})", qs(k)));
    ctx.holds.remove(k);
}
fn untransit(ctx: &mut Ctx, k: &str) {
    rec(format!("(untransit {})", qs(k)));
    ctx.transit.remove(k);
}
fn unslen(ctx: &mut Ctx, k: &str) {
    rec(format!("(unslen {})", qs(k)));
    ctx.slice_lens.remove(k);
}
fn unforeign(ctx: &mut Ctx, k: &str) {
    rec(format!("(unforeign {})", qs(k)));
    ctx.foreign.remove(k);
}
fn uncsafe(ctx: &mut Ctx, k: &str) {
    rec(format!("(uncsafe {})", qs(k)));
    ctx.cstr_safe.remove(k);
}
/// `ctx.defer_res.insert` + the `(dres …)` event — a deferred call's
/// freeze reservation on a place.
fn dres_set(ctx: &mut Ctx, k: String, span: Span) {
    rec(format!("(dres {} @{}:{})", qs(&k), span.line, span.col));
    ctx.defer_res.insert(k, span);
}

fn undres(ctx: &mut Ctx, k: &str) {
    rec(format!("(undres {})", qs(k)));
    ctx.defer_res.remove(k);
}

/// `(flags dead|live term|cont)` — divergence/termination commits.
fn rec_flags(ctx: &Ctx) {
    rec(format!(
        "(flags {} {})",
        if ctx.diverged { "dead" } else { "live" },
        if ctx.terminated { "term" } else { "cont" }
    ));
}

/// `(joined …)` — the post-merge snapshot at a branch/loop join.
/// Every channel prints sorted: the merge output IS the semantics,
/// so this is where certainty loss and fact unions are observed.
fn rec_joined(ctx: &Ctx, span: Option<Span>) {
    let mut s = format!("(joined{}", at_s(span));
    let mut es: Vec<_> = ctx.edges.iter().collect();
    es.sort_by(|a, b| a.0.cmp(b.0));
    for ((h, t), f) in es {
        s.push_str(&format!(
            " (e {} {} {} {}{})",
            qs(h),
            qs(t),
            auth_s(f.kind),
            cert_s(f.cert),
            at_s(f.born)
        ));
    }
    let mut cs: Vec<_> = ctx.caps.iter().collect();
    cs.sort_by(|a, b| a.0.cmp(b.0));
    for ((h, t), c) in cs {
        s.push_str(&format!(" (c {} {} {})", qs(h), qs(t), cert_s(*c)));
    }
    let mut cons: Vec<_> = ctx.consumed.iter().collect();
    cons.sort_by(|a, b| a.0.cmp(b.0));
    for (k, f) in cons {
        let st = match f.st {
            ConsState::Cons => "cons",
            ConsState::Maybe => "maybe",
        };
        s.push_str(&format!(" (m {} {})", qs(k), st));
    }
    let mut ins: Vec<_> = ctx.init.iter().collect();
    ins.sort_by(|a, b| a.0.cmp(b.0));
    for (k, c) in ins {
        s.push_str(&format!(" (i {} {})", qs(k), cert_s(*c)));
    }
    let mut ts: Vec<_> = ctx.tag.iter().collect();
    ts.sort_by(|a, b| a.0.cmp(b.0));
    for (k, t) in ts {
        let v = match t {
            Some(v) => qs(v),
            None => "?".into(),
        };
        s.push_str(&format!(" (g {} {})", qs(k), v));
    }
    let mut ks: Vec<_> = ctx.edge_kills.iter().collect();
    ks.sort_by(|a, b| a.0.cmp(b.0));
    for (k, c) in ks {
        s.push_str(&format!(" (k {} {})", qs(k), cert_s(*c)));
    }
    let mut cks: Vec<_> = ctx.cap_kills.iter().collect();
    cks.sort_by(|a, b| a.0.cmp(b.0));
    for (k, c) in cks {
        s.push_str(&format!(" (ck {} {})", qs(k), cert_s(*c)));
    }
    let mut pv: Vec<_> = ctx.path_provs.iter().collect();
    pv.sort_by(|a, b| a.0.cmp(b.0));
    for (k, p) in pv {
        s.push_str(&format!(" (p {} {})", qs(k), prov_s(p, &ctx.regs)));
    }
    let mut tr: Vec<_> = ctx.transit.iter().collect();
    tr.sort();
    for k in tr {
        s.push_str(&format!(" (t {})", qs(k)));
    }
    let mut ch: Vec<_> = ctx.chan_queues.iter().collect();
    ch.sort_by(|a, b| a.0.cmp(b.0));
    for (c, q) in ch {
        s.push_str(&format!(" (q {}", qs(c)));
        for p in q {
            s.push_str(&format!(" (pk {}", qs(&p.ty.to_string())));
            let mut pe: Vec<_> = p.edges.iter().collect();
            pe.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            for (h, t, f) in pe {
                s.push_str(&format!(
                    " (pe {} {} {} {}{})",
                    qs(h),
                    qs(t),
                    auth_s(f.kind),
                    cert_s(f.cert),
                    at_s(f.born)
                ));
            }
            let mut pc: Vec<_> = p.caps.iter().collect();
            pc.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
            for (h, t, c) in pc {
                s.push_str(&format!(" (pc {} {} {})", qs(h), qs(t), cert_s(*c)));
            }
            let mut ps: Vec<_> = p.sent.iter().collect();
            ps.sort();
            for k in ps {
                s.push_str(&format!(" (ps {})", qs(k)));
            }
            s.push(')');
        }
        s.push(')');
    }
    let mut dr: Vec<_> = ctx.defer_res.iter().collect();
    dr.sort_by(|a, b| a.0.cmp(b.0));
    for (k, sp) in dr {
        s.push_str(&format!(" (dr {} @{}:{})", qs(k), sp.line, sp.col));
    }
    for frame in &ctx.defers {
        if !frame.is_empty() {
            s.push_str(" (df");
            for r in frame {
                let sp = r.expr.span();
                s.push_str(&format!(" @{}:{}", sp.line, sp.col));
            }
            s.push(')');
        }
    }
    s.push(')');
    rec(s);
}


/// How a `V(b)` match arm binds its payload — the checker decides
/// from authority at `scrut.V` (`place_auth`); IR gen binds the same
/// slot by value (load+copy) or `&` (payload address).
/// Recorded into `ARM_BINDS` since check sees `&Program` — gen
/// can't re-derive borrow facts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmBind {
    Value,
    Ref,
}

thread_local! {
    static ARM_BINDS: std::cell::RefCell<Vec<(Span, String, ArmBind)>> =
        std::cell::RefCell::new(Vec::new());
}

/// `(match-span, variant) -> mode` tuples recorded during this
/// module's check — the pipeline drains them per module exactly the
/// way `index::drain` works, then hands the folded map to codegen.
pub fn take_arm_binds() -> Vec<(Span, String, ArmBind)> {
    ARM_BINDS.with(|v| std::mem::take(&mut *v.borrow_mut()))
}

fn record_arm_bind(span: Span, variant: &str, mode: ArmBind) {
    rec(format!(
        "(armbind @{}:{} {} {})",
        span.line,
        span.col,
        qs(variant),
        match mode {
            ArmBind::Value => "value",
            ArmBind::Ref => "ref",
        }
    ));
    ARM_BINDS.with(|v| {
        v.borrow_mut().push((span, variant.to_string(), mode))
    });
}

/// What a reference-typed *value* points at when it flows somewhere.
/// `move_from` marks `&mut` sources: binding them transfers the edge.
/// `foreign` marks `slice(p, n)`-born views: no edges to claim, so
/// the moved-reference check must not read empty edges as dead.
#[derive(Debug, Clone)]
struct RefInfo {
    targets: Vec<String>,
    mutable: bool,
    move_from: Option<String>,
    foreign: bool,
}

/// N28 — what a live match arm contributes to the expression join,
/// captured while the arm's scope is still live (payload binders and
/// arm-locals are dead by the time `refs_of`/`caps_of`/`slice_len_of`
/// inspect the `Expr::Match` from outside). The match's own prov is
/// the per-arm prov join; refs/caps union the arm tails the way the
/// branch join unions parallel-path bindings.
#[derive(Debug, Clone)]
struct ArmYieldInfo {
    ty: Ty,
    prov: Prov,
    refs: Option<RefInfo>,
    caps: Option<Vec<(String, Cert)>>,
    len: Option<i64>,
    /// N40 — the yielded place's subtree facts, rel-keyed; unioned
    /// across arms, then drained into the lit channels when the
    /// match result binds.
    lit_edges: Vec<(String, RefInfo)>,
    lit_caps: Vec<(String, Vec<String>)>,
    lit_provs: Vec<(String, Prov)>,
    lit_dead: Vec<(String, ConsFact)>,
    lit_lens: Vec<(String, i64)>,
    lit_tags: Vec<(String, Option<String>)>,
}

/// `(holder, target) -> fact` — kind, certainty, and (N35B) where
/// the edge was born.
type Edges = HashMap<(String, String), EdgeFact>;

/// N9 — a spawned task's suspended transform, in PARENT-context keys
/// (the child ran on a clone of the parent's key space — no $arg
/// translation needed). Computed at `spawn`, applied at `join`.
#[derive(Debug, Clone)]
struct TaskEffects {
    /// Child-born edges — `born` spans travel so "borrow created
    /// here" labels point INSIDE the spawned body.
    creates: Vec<(String, String, EdgeFact)>,
    kills: HashMap<String, Cert>,
    creates_caps: Vec<(String, String, Cert)>,
    cap_kills: HashMap<String, Cert>,
    consumed: HashMap<String, ConsFact>,
    path_provs: Vec<(String, Prov)>,
    ret_edges: Vec<(String, bool)>,
    ret_caps: Vec<String>,
    ret: Ty,
}

impl Default for TaskEffects {
    fn default() -> Self {
        TaskEffects {
            creates: Vec::new(),
            kills: HashMap::new(),
            creates_caps: Vec::new(),
            cap_kills: HashMap::new(),
            consumed: HashMap::new(),
            path_provs: Vec::new(),
            ret_edges: Vec::new(),
            ret_caps: Vec::new(),
            ret: Ty::Void,
        }
    }
}

/// What `spawn` leaves in the parent ctx via a side channel: the
/// authority claims (become `task_key -> k` edges) plus the suspended
/// diff (land on `join`).
#[derive(Clone)]
struct SpawnResult {
    /// Where the `spawn` sits — escape/leak diagnostics point at it.
    span: Span,
    /// External places the child touched, strongest auth recorded.
    /// Becomes real `t -> place` edges while the task is outstanding.
    holds: HashMap<String, Auth>,
    /// CHAN-004 S3b — bounded endpoints the child `close`d: their
    /// `Own` edge is the close's claim, mutex-serialized at runtime
    /// against in-flight sends — not an ownership steal, so it must
    /// not read as an unsynchronized access at the parent's ops.
    closed_eps: HashSet<String>,
    /// The suspended transform, applied at `join`.
    eff: TaskEffects,
}

/// Per-task-body checker state: the accumulated result type, the
/// region watermark (ids >= base_regs are task-internal), and the
/// binding keys visible at spawn (what may legitimately be named
/// by the task's effects and returned refs).
#[derive(Clone)]
struct TaskScope {
    ret: Ty,
    /// Binding keys present at spawn — the only places a returned
    /// reference may legitimately name.
    ext: HashSet<String>,
    /// CHAN-004 S3b — endpoints this task `close`d (see SpawnResult).
    closed_eps: HashSet<String>,
}

/// A suspended authority packet (N12) — the graph delta `send` cut out
/// of the sender's graph. `edges`/`caps` hold (rel-suffix, target, cert)
/// tuples re-rooted at the receiver's binding key; `sent` names targets
/// whose exclusive authority is channel-held (`transit`) until `recv`.
/// `PartialEq` powers the S3a join's queue-divergence test: two paths
/// holding equal-length queues of DIFFERENT packets is still may-order.
#[derive(Clone, PartialEq)]
struct Packet {
    /// The payload's full type — what `recv` hands the binding.
    ty: Ty,
    /// Alias/authority edges that travelled (suffix, target, fact).
    /// For capability packets the suffix is "" — the binding IS the
    /// ref. `born` = the send site: a receiver's "borrow created
    /// here" label lands on the `send` that minted the view.
    edges: Vec<(String, String, EdgeFact)>,
    /// Identity (handle) edges that travelled — never authority.
    caps: Vec<(String, String, Cert)>,
    /// Targets whose authority moved into the channel (transit set).
    sent: Vec<String>,
    /// Capability packets install as `auth`-kind bindings on recv.
    auth: Option<Auth>,
    /// Handle packets install a cap edge — identity only.
    is_handle: bool,
}

/// CHAN-004 S3a — per-endpoint uncertainty bookkeeping.
/// `chan_queues` is the live may-bag; this records WHY membership
/// is may (branch joins, spawn merges) and how many `recv`s
/// provably departed on this path. A pop cannot identify which
/// candidate message left the physical queue, so queue
/// membership never shrinks per-pop — only `pops` advances, and
/// `pops >= pushes` is the sole drain proof.
#[derive(Clone, Default)]
struct ChanMeta {
    /// Sticky: set when a branch join merged differing queues or
    /// a task's endpoint effects interleaved with the parent's.
    unordered: bool,
    /// Packets ever pushed on this path-set (may-count: every
    /// physical enqueue is a push, but a push may be a ghost).
    pushes: u32,
    /// `Some`-recv count on this path — joins take the min: a
    /// pop credited on one arm is not a pop on the other.
    pops: u32,
}

/// CHAN-004 S3a — a pending bounded-send fact: the packet stays
/// latent while the Result place is unresolved (the physical
/// message may already be queued). `Vec` = path alternatives —
/// a join merges them as a disjunction, never co-owners.
#[derive(Clone)]
struct PendingPkt {
    /// The endpoint key this send targeted.
    ep: String,
    /// The latent packet — claimed by the Err arm, delivered by Ok.
    pkt: Packet,
}

/// CHAN-004 S3a — an unknown-identity edge target minted by a
/// claim that can't prove which pushed packet departed. The key
/// sorts after `#`/`$` so it can't collide with a place key;
/// `bot_cands[key]` is its candidate set — every named source the
/// physical capability may alias (never distinct identity).
fn bot_key(n: u64) -> String {
    format!("${{bot{n}}}")
}

/// Is `k` an unknown-identity key minted by `bot_key`?
fn is_bot_key(k: &str) -> bool {
    k.starts_with("${bot") && k.ends_with('}')
}

/// CHAN-004 S3a — the outstanding-capability world: every fact
/// `MayOutstanding(t)` draws on, bundled for snapshots and joins.
#[derive(Clone, Default)]
struct Outstanding {
    /// Per-endpoint uncertainty meta (unordered/pushes/pops).
    chan_meta: HashMap<String, ChanMeta>,
    /// Per-endpoint candidate-name union: every named source a
    /// pushed packet could still carry — a pop can't identify
    /// which candidate departed, so this NEVER shrinks except
    /// when the endpoint itself dies (caps die with the queue).
    chan_names: HashMap<String, BTreeSet<String>>,
    /// Latent bounded-send packets on unresolved Result places
    /// (inert until S3b). Vec = path alternatives at a join.
    pending_send: HashMap<String, Vec<PendingPkt>>,
    /// `${botN}` edge target -> the named sources it may alias.
    bot_cands: HashMap<String, BTreeSet<String>>,
    /// Mint counter — joins take max so fresh keys never collide.
    next_bot: u64,
}

/// Rank for hold union: a task's claim over a place is its strongest
/// recorded access (a maybe-write is still a write-hold).
fn auth_rank(a: Auth) -> u8 {
    match a {
        Auth::Read => 0,
        Auth::Atomic => 1,
        Auth::Sync => 2,
        Auth::Write => 3,
        Auth::Own => 4,
    }
}

/// Record an access. Only meaningful inside a task body — at spawn
/// end the parent filters these to external keys and turns them into
/// hold edges. Everywhere else the map is inert scratch space.
fn record_hold(ctx: &mut Ctx, key: &str, auth: Auth) {
    let e = ctx.holds.entry(key.to_string()).or_insert(auth);
    if auth_rank(auth) > auth_rank(*e) {
        *e = auth;
    }
    rec(format!("(hold {} {})", qs(key), auth_s(*e)));
}

/// N15 — the statically-known element count of a slice place, if any:
/// `let s = &a[2..5]` records `s#e -> 3`; `slice(p, k)` records k.
/// `None` = dynamic — the runtime bounds check still applies.
fn slice_len_at(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Option<i64> {
    // `s.bytes` is the str's own span — its length is the str's.
    if let Expr::Field(b, name, _) = e {
        if name == "bytes" && matches!(expr_ty_at(b, scopes, ctx).0, Ty::Str) {
            return slice_len_at(b, scopes, ctx);
        }
    }
    place_id(e, scopes).and_then(|k| ctx.slice_lens.get(&k).copied())
}

/// The length an init expression gives its new binding, when it is
/// provable at check time.
fn slice_len_of(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> Option<i64> {
    match e {
        Expr::AddrOf { place, .. } => match &**place {
            Expr::IndexRange { base, lo, hi, .. } => {
                let limit = match expr_ty_at(base, scopes, ctx).0 {
                    Ty::Array(_, n) => Some(n as i64),
                    _ => slice_len_at(base, scopes, ctx),
                }?;
                let lov = match lo.as_deref() {
                    Some(Expr::Int(k, _)) => Some(*k),
                    None => Some(0),
                    _ => None,
                }?;
                let hiv = match hi.as_deref() {
                    Some(Expr::Int(k, _)) => Some(*k),
                    None => Some(limit),
                    _ => None,
                }?;
                if hiv >= lov {
                    Some(hiv - lov)
                } else {
                    None
                }
            }
            _ => None,
        },
        Expr::Call(n, _tg, args, _) if n == "slice" && !ctx.local_fns.contains(n) => {
            match args.get(1) {
                Some(e2) => match e2 {
                    Expr::Int(k, _) => Some(*k),
                    _ => None,
                },
                None => None,
            }
        }
        // `str_from_utf8(b)` — the string's length is the slice's.
        Expr::Call(n, _tg, args, _) if n == "str_from_utf8" && !ctx.local_fns.contains(n) => {
            args.first().and_then(|b| slice_len_at(b, scopes, ctx))
        }
        // `alloc_slice<T>(a, n)` — the span's length is the literal
        // count.
        Expr::Alloc { count: Some(n), .. } => match &**n {
            Expr::Int(k, _) => Some(*k),
            _ => None,
        },
        // A literal under `str` — the byte count is known outright.
        Expr::Str(bytes, _) => Some(bytes.len() as i64),
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) => {
            slice_len_at(e, scopes, ctx)
        }
        // N28 — survives only if every live arm yielded the same
        // static length (recorded while arm scopes lived).
        Expr::Match { span, .. } => ctx
            .match_yields
            .get(&(span.line, span.col))
            .and_then(|y| y.len),
        _ => None,
    }
}

/// Does this place key belong to the spawner's world? A root bind
/// present at spawn time, or a `$arg` param-storage key.
fn is_external(key: &str, extern_roots: &HashSet<String>) -> bool {
    let r = path_root(key);
    r.starts_with('$') || extern_roots.contains(r)
}

/// A `spawn` result meets its binding. `allowed` is false when the
/// spawn wasn't the direct init/RHS (e.g. `f(spawn {..})`) — an
/// unbound task is rejected outright, since nobody can join it and
/// dropping holds would lose the child's claimed authority silently.
fn bind_task(key: &str, allowed: bool, ctx: &mut Ctx) {
    let Some(sr) = ctx.last_spawn.take() else { return };
    if !allowed {
        push_diag(ctx, Diag::new(
            sr.span,
            "a `spawn` must be bound directly: `let t = spawn { .. }`".to_string(),
        ));
        return;
    }
    // Rebinding a task place abandons the old task: its hold edges die
    // with the binding (the suspended transform never lands).
    edges_retain(ctx, |(h, _), _| !key_under(h, key));
    let mut holds: Vec<(&String, &Auth)> = sr.holds.iter().collect();
    holds.sort_by(|a, b| a.0.cmp(b.0));
    for (k, a) in holds {
        edge_set(ctx, key.to_string(), k.clone(), EdgeFact::new(*a, Cert::Def, Some(sr.span)));
        // Propagate the hold upward: inside a task body this feeds the
        // outer spawn's hold set (nested spawn shares the window).
        record_hold(ctx, k, *a);
    }
    rec(format!("(task {} @{}:{})", qs(key), sr.span.line, sr.span.col));
    ctx.tasks.insert(key.to_string(), sr);
}


/// The cap-graph half of `apply_kill`: a must-kill removes the cap
/// edges under `key`, a may-kill weakens certainty.
fn apply_cap_kill(ctx: &mut Ctx, key: &str, cert: Cert) {
    match cert {
        Cert::Def => caps_retain(ctx, |(h, _), _| !key_under(h, key)),
        Cert::Maybe => {
            let mut under: Vec<(String, String)> = ctx
                .caps
                .keys()
                .filter(|(h, _)| key_under(h, key))
                .cloned()
                .collect();
            under.sort();
            for k in under {
                if let Some(c) = ctx.caps.get_mut(&k) {
                    rec(format!("(weakcap {} {})", qs(&k.0), qs(&k.1)));
                    *c = Cert::Maybe;
                }
            }
        }
    }
}

/// `h` under `key` (exact, `.field`, `[i]` subtree)?
fn key_under(h: &str, key: &str) -> bool {
    h == key || h.starts_with(&format!("{key}.")) || h.starts_with(&format!("{key}["))
}

/// N8 — a function's effect on its parameters' places, expressed over
/// the callee-side `$argN[.path]` vocabulary the param edges already
/// use. At a call site every `$argI.rest` translates through arg I's
/// resolved caller keys. `kills`/`cap_kills` carry Cert: a kill inside
/// a conditional is may-only — it weakens, never removes, the caller's
/// edge. `ret_edges`/`ret_caps` translate the returned alias targets.
///
/// N17 — the summary is key-shaped only (no types, no bindings), so it
/// crosses a module boundary unchanged: it's the semantic contract an
/// importer replays without ever reopening the callee's body.
#[derive(Debug, Clone, Default)]
pub struct FnSummary {
    creates: Vec<(String, String, Auth, Cert)>,
    kills: HashMap<String, Cert>,
    creates_caps: Vec<(String, String, Cert)>,
    cap_kills: HashMap<String, Cert>,
    /// N18 — `$argN.*` places the callee definitely initialized: a
    /// `write` param's stores export so the caller's `h#e.f` proves
    /// itself without the caller ever seeing the body.
    init_paths: Vec<String>,
    ret_edges: Vec<(String, bool)>,
    ret_caps: Vec<String>,
    /// Enum-typed returns: ref payloads ride per-variant
    /// (`Ok`/`Ok.f` rel keys) — a caller's `let r = f()` replays
    /// them through `lit_edges` so `r.Ok` binds live edges.
    ret_lit_edges: Vec<(String, RefInfo)>,
    ret_lit_caps: Vec<(String, Vec<String>)>,
    ret_lit_provs: Vec<(String, Prov)>,
    /// N11 — `$argN` domains the callee claimed via `lock` — a caller
    /// inside a task replays them as Sync holds so two tasks calling
    /// `update(m)` share the serialization domain.
    syncs: Vec<String>,
    /// N32 — the body ends terminated: every path out is `unreachable`
    /// (or a call that is itself diverges). Call sites replay this as
    /// diverged+terminated — `process.exit` is an ordinary library fn
    /// whose no-return property reaches callers through this bit, not
    /// through a builtin registry.
    diverges: bool,
}

/// Translate a `$argN.rest` summary key through the resolved caller
/// keys for each arg position. Empty when the key isn't param-rooted
/// or the arg index is out of range.
fn translate_key(k: &str, bases: &[Vec<String>]) -> Vec<String> {
    let Some(rest) = k.strip_prefix("$arg") else {
        return Vec::new();
    };
    let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
    let Ok(i) = digits.parse::<usize>() else {
        return Vec::new();
    };
    let suffix = &rest[digits.len()..];
    bases
        .get(i)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .map(|b| format!("{b}{suffix}"))
        .collect()
}

type Scopes = Vec<HashMap<String, Bind>>;

fn lookup<'a>(scopes: &'a Scopes, name: &str) -> Option<&'a Bind> {
    scopes.iter().rev().find_map(|s| s.get(name))
}

fn lookup_mut<'a>(scopes: &'a mut Scopes, name: &str) -> Option<&'a mut Bind> {
    scopes.iter_mut().rev().find_map(|s| s.get_mut(name))
}

/// The provenance/edge key of a place-path: `x#3`, `s#1.f`, `a#0[]`
/// (index-insensitive). The epoch suffix pins a path to one binding
/// instance — a shadowed `x`'s edges can't leak into the outer `x`'s.
/// Deref roots get `*<key>` — opaque; two derefs of different refs may
/// alias without us seeing it (documented N4 gap, like real borrowck's
/// dependence on lifetime structure).
fn place_id(e: &Expr, scopes: &mut Scopes) -> Option<String> {
    match e {
        Expr::Var(n, _) => lookup(scopes, n).map(|b| format!("{n}#{}", b.epoch)),
        Expr::Field(b, f, _) => guard_base(b, scopes)
            .or_else(|| place_id(b, scopes))
            .map(|k| format!("{k}.{f}")),
        Expr::Index(b, _, _) => guard_base(b, scopes)
            .or_else(|| place_id(b, scopes))
            .map(|k| format!("{k}[]")),
        Expr::IndexRange { base, .. } => place_id(base, scopes)
            .map(|k| format!("{k}[..]")),
        Expr::Deref(inner, _) => match &**inner {
            // `*g` names the whole protected payload — the domain's root.
            Expr::Var(n, _) => lookup(scopes, n).and_then(|b| {
                b.guard.as_ref().map(|(_, r, _)| r.clone())
            }).or_else(|| place_id(inner, scopes).map(|k| format!("*{k}"))),
            _ => place_id(inner, scopes).map(|k| format!("*{k}")),
        },
        _ => None,
    }
}

/// `g` where the guard's domain protects `root`: `g.f` means `root.f`.
fn guard_base(e: &Expr, scopes: &mut Scopes) -> Option<String> {
    if let Expr::Var(n, _) = e {
        if let Some(b) = lookup(scopes, n) {
            return b.guard.as_ref().map(|(_, r, _)| r.clone());
        }
    }
    None
}

/// The holder keys traversed to reach a place — the refs/cap params
/// whose own authority we borrow THROUGH: `(*m).f` → {m#e},
/// `n.f` (cap param n) → {n#e}, `s[i]` → {s#e} (the slice's edge IS
/// its span authority), plain `x`/`p.f`/`a[i]` → {}.
fn path_holders(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> HashSet<String> {
    match e {
        Expr::Var(n, _) => match lookup(scopes, n) {
            Some(b) if b.auth.is_some() || b.guard.is_some() => {
                HashSet::from([format!("{n}#{}", b.epoch)])
            }
            _ => HashSet::new(),
        },
        Expr::Field(b, ..) => path_holders(b, scopes, ctx),
        // `s[i]`/`s[lo..hi]` on a slice base reaches its span THROUGH
        // the slice binding's edge — the base's resolved keys are the
        // holders (`s#e`, `p#e.sv`, a `slice` param's `s#e`).
        Expr::Index(b, ..) | Expr::IndexRange { base: b, .. } => {
            let mut h = path_holders(b, scopes, ctx);
            if matches!(expr_ty_at(b, scopes, ctx).0, Ty::Slice { .. }) {
                h.extend(resolve_place(b, scopes, ctx));
            }
            h
        }
        Expr::Deref(inner, _) => {
            let mut h = path_holders(inner, scopes, ctx);
            if let Some(k) = place_id(inner, scopes) {
                h.insert(k);
            }
            h
        }
        _ => HashSet::new(),
    }
}

/// The semantic targets a place names, following edges: `x` → [x#e];
/// `*r` → r's live edge targets; `(*r).f` → [t.f for t in *r];
/// cap-param `n` → [$argN]. Opaque derefs fall back to `*<key>`.
fn resolve_place(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Vec<String> {
    // Target lists come from edge-map iteration — sort so downstream
    // replay order (and the event log) is canonical.
    let mut out = resolve_place_inner(e, scopes, ctx);
    out.sort();
    out.dedup();
    out
}

fn resolve_place_inner(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Vec<String> {
    match e {
        Expr::Var(n, _) => match lookup(scopes, n) {
            Some(b) if b.auth.is_some() => ctx
                .edges
                .iter()
                .filter(|((h, _), _)| *h == format!("{n}#{}", b.epoch))
                .map(|((_, t), _)| t.clone())
                .collect(),
            Some(b) => vec![format!("{n}#{}", b.epoch)],
            None => Vec::new(),
        },
        Expr::Field(b, f, _) => {
            // `s.bytes` names the str's whole span — resolve through
            // the str's own edge, not a `.bytes` sub-path.
            if f == "bytes" && matches!(expr_ty_at(b, scopes, ctx).0, Ty::Str) {
                return resolve_place(b, scopes, ctx);
            }
            guard_base(b, scopes)
                .map(|r| vec![r])
                .unwrap_or_else(|| resolve_place(b, scopes, ctx))
                .into_iter()
                .map(|k| format!("{k}.{f}"))
                .collect()
        }
        Expr::Index(b, _, _) => {
            if let Ty::Slice { .. } = expr_ty_at(b, scopes, ctx).0 {
                // `s[i]` names an element of the slice's claimed span:
                // follow the slice's edges like a deref, then tag the
                // element path — `s -> a#e` resolves `s[i]` to `a#e[]`.
                let mut ts = Vec::new();
                for h in resolve_place(b, scopes, ctx) {
                    let mut any = false;
                    for ((eh, t), _) in ctx.edges.iter() {
                        if *eh == h && !ts.contains(t) {
                            ts.push(t.clone());
                            any = true;
                        }
                    }
                    for ((eh, t), _) in ctx.caps.iter() {
                        if *eh == h && !ts.contains(t) {
                            ts.push(t.clone());
                            any = true;
                        }
                    }
                    if !any {
                        ts.push(format!("*{h}"));
                    }
                }
                return ts.into_iter().map(|k| format!("{k}[]")).collect();
            }
            guard_base(b, scopes)
                .map(|r| vec![r])
                .unwrap_or_else(|| resolve_place(b, scopes, ctx))
                .into_iter()
                .map(|k| format!("{k}[]"))
                .collect()
        }
        Expr::IndexRange { base, .. } => {
            if let Ty::Slice { .. } = expr_ty_at(base, scopes, ctx).0 {
                // `&s[lo..hi]` claims the span through s's edges —
                // the whole target (v1 slices claim their full span).
                let mut ts = Vec::new();
                for h in resolve_place(base, scopes, ctx) {
                    let mut any = false;
                    for ((eh, t), _) in ctx.edges.iter() {
                        if *eh == h && !ts.contains(t) {
                            ts.push(t.clone());
                            any = true;
                        }
                    }
                    for ((eh, t), _) in ctx.caps.iter() {
                        if *eh == h && !ts.contains(t) {
                            ts.push(t.clone());
                            any = true;
                        }
                    }
                    if !any {
                        ts.push(format!("*{h}"));
                    }
                }
                return ts;
            }
            resolve_place(base, scopes, ctx)
        }
        Expr::Deref(inner, _) => {
            // `*g` for a guard names the domain's whole protected root.
            if let Some(r) = guard_base(inner, scopes) {
                return vec![r];
            }
            // `*p` on a `box` names the heap cell — whose key IS the
            // box binding's own. No edge hop: the box IS the root, so
            // `&*p` borrows `p#e` and `free(p)` sees it.
            if matches!(expr_ty_at(inner, scopes, ctx).0, Ty::Box(_)) {
                return resolve_place(inner, scopes, ctx);
            }
            let mut ts = Vec::new();
            for h in resolve_place(inner, scopes, ctx) {
                let mut any = false;
                for ((eh, t), _) in ctx.edges.iter() {
                    if *eh == h && !ts.contains(t) {
                        ts.push(t.clone());
                        any = true;
                    }
                }
                for ((eh, t), _) in ctx.caps.iter() {
                    if *eh == h && !ts.contains(t) {
                        ts.push(t.clone());
                        any = true;
                    }
                }
                if !any {
                    ts.push(format!("*{h}"));
                }
            }
            ts
        }
        _ => Vec::new(),
    }
}

/// The key a derivation-move kills: `&mut *m` consumes m's authority,
/// `&mut n` consumes the write param's edge — the outermost borrowed-
/// through holder on the path.
fn derivation_source(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Option<String> {
    match e {
        Expr::Var(n, _) => match lookup(scopes, n) {
            Some(b) if b.auth.is_some() => Some(format!("{n}#{}", b.epoch)),
            _ => None,
        },
        Expr::Field(b, ..) => derivation_source(b, scopes, ctx),
        // `&mut s[i]` / `&mut s[lo..hi]` through a slice reborrow-moves
        // the slice's span authority — same rule as `&mut *r`.
        Expr::Index(b, ..) | Expr::IndexRange { base: b, .. } => {
            if matches!(expr_ty_at(b, scopes, ctx).0, Ty::Slice { .. }) {
                place_id(b, scopes)
            } else {
                derivation_source(b, scopes, ctx)
            }
        }
        Expr::Deref(inner, _) => place_id(inner, scopes),
        _ => None,
    }
}

/// The hold a capability derivation leaves for task windows: a `write`
/// contract on mutex storage claims the lock DOMAIN, not plain write —
/// `f(write m)` in two tasks serializes through `m` instead of racing.
fn cap_hold(t: &str, required: Auth, scopes: &mut Scopes, ctx: &Ctx) -> Auth {
    if required == Auth::Write && mutex_inner(t, scopes, ctx).is_some() {
        Auth::Sync
    } else {
        required
    }
}

/// The capability law, enforced where authority is *required*:
///   required Read  — own Read/Write/Own authority may derive it
///   required Write — own Write/Own may derive it (reborrow-MOVE:
///                     the parent edge dies); a Read parent = escalation
///   required Own   — only an Own parent transfers; a Write/Read
///                     parent cannot fabricate Own
/// Other holders' edges follow the matrix: Read tolerates Reads;
/// Write/Own tolerate nothing. Returns resolved targets.
fn cap_check(e: &Expr, required: Auth, scopes: &mut Scopes, ctx: &mut Ctx) -> Vec<String> {
    let ts = resolve_place(e, scopes, ctx);
    let holders = path_holders(e, scopes, ctx);
    for t in &ts {
        record_hold(ctx, t, cap_hold(t, required, scopes, ctx));
    }
    if ts.is_empty() && !holders.is_empty() {
        // Every authority on the path was consumed — reborrow-move or
        // own-transfer already claimed it.
        push_diag(ctx, Diag::new(
            e.span(),
            format!("use of moved authority `{}`", target_name(e)),
        ));
    }
    // Reaching a target THROUGH a handle must prove liveness first.
    let via_handle = holders
        .iter()
        .any(|h| ctx.caps.keys().any(|(ch, _)| ch == h));
    if via_handle {
        for t in &ts {
            let root = path_root(t);
            if root.starts_with('$') {
                continue;
            }
            let name = root.split('#').next().unwrap_or(root);
            let alive = match lookup(scopes, name) {
                Some(b) => {
                    format!("{name}#{}", b.epoch) == root
                        && consumed_state(ctx, t).is_none()
                }
                None => false,
            };
            if !alive {
                push_diag(ctx, Diag::new(
                    e.span(),
                    format!(
                        "use of stale handle `{}`: its target no longer exists",
                        target_name(e)
                    ),
                ));
                return ts;
            }
        }
    }
    for t in &ts {
        unguarded_check(e, t, scopes, ctx);
        transit_check(e, t, ctx);
        for (h, f) in edges_to(ctx, t) {
            if holders.contains(&h) {
                // Deriving through our own authority — the parent edge.
                let ok = match (required, f.kind) {
                    (Auth::Read, _) => true,
                    (Auth::Write, Auth::Write) | (Auth::Write, Auth::Own) => true,
                    (Auth::Write, Auth::Read)
                    | (Auth::Write, Auth::Atomic)
                    | (Auth::Write, Auth::Sync) => false,
                    (Auth::Own, Auth::Own) => true,
                    (Auth::Own, _) => false,
                    (Auth::Atomic, _) | (Auth::Sync, _) => false, // never param contracts
                };
                if !ok {
                    let msg = match required {
                        Auth::Write => format!(
                            "cannot derive write authority from read on `{}`",
                            target_name(e)
                        ),
                        _ => format!(
                            "cannot transfer own authority through a borrow of `{}`",
                            target_name(e)
                        ),
                    };
                    push_diag(ctx, Diag::new(e.span(), msg));
                }
            } else {
                // An edge whose holder lives INSIDE the consumed target
                // dies with the root — `p.next = &*p` is internal, not
                // an outstanding borrow that could dangle.
                if required == Auth::Own && key_under(&h, t) {
                    continue;
                }
                let conflict = match cap_hold(t, required, scopes, ctx) {
                    Auth::Read => f.kind != Auth::Read,
                    // Two tasks' claims on the same mutex domain
                    // serialize — only same-target Sync edges reach
                    // this comparison, so they share the domain.
                    Auth::Sync => f.kind != Auth::Sync,
                    _ => true,
                };
                if conflict {
                    let msg = if ctx.tasks.contains_key(&h) {
                        let tn = h.split('#').next().unwrap_or(&h);
                        format!(
                            "cannot touch `{}`: held by unjoined task `{tn}`",
                            target_name(e)
                        )
                    } else {
                        match required {
                            Auth::Read => format!(
                                "cannot borrow `{}` as shared: it is mutably borrowed",
                                target_name(e)
                            ),
                            Auth::Own => format!(
                                "cannot consume `{}`: it is borrowed",
                                target_name(e)
                            ),
                            _ => format!(
                                "cannot mutably borrow `{}`: already borrowed",
                                target_name(e)
                            ),
                        }
                    };
                    let mut d = Diag::new(e.span(), msg);
                    if let Some(l) = born_label(&h, &f, ctx) {
                        d.labels.push(l);
                    }
                    push_diag(ctx, d);
                }
            }
        }
    }
    ts
}

/// The strongest authority derivable at a place right now, computed
/// without recording holds or creating edges — the match payload
/// binder reads it to pick a mode: `Own` means the payload could
/// move (value-bind), `Write` means mutate-in-place (`&mut`-bind),
/// and anything less means observe-only (`&`-bind). Mirrors
/// `cap_check`'s derivation matrix: a holder edge degrades to its
/// kind, any foreign claim degrades to Read (internal edges — a
/// holder inside the target — exempt for Own exactly as there).
fn place_auth(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> Auth {
    let ts = resolve_place(e, scopes, ctx);
    if ts.is_empty() {
        return Auth::Read;
    }
    let holders = path_holders(e, scopes, ctx);
    let mut own = true;
    let mut write = true;
    for t in &ts {
        for (h, f) in edges_to(ctx, t) {
            if holders.contains(&h) {
                match f.kind {
                    Auth::Own => {}
                    Auth::Write => own = false,
                    _ => {
                        own = false;
                        write = false;
                    }
                }
            } else {
                if key_under(&h, t) {
                    continue;
                }
                own = false;
                write = false;
            }
        }
    }
    if own {
        Auth::Own
    } else if write {
        Auth::Write
    } else {
        Auth::Read
    }
}

/// N14 — the C-ABI boundary. An `extern fn` call is an opaque graph
/// transform: no body, no summary. Authority still flows through the
/// graph: a `*T` parameter claims Write on whatever the argument names
/// for the call's duration (`*const T` claims Read), recorded as
/// transient `$tmp{i}` edges — so `f(&mut x, &mut x)` self-conflicts,
/// an FFI write races a live borrow or an unjoined task, and a `&x`
/// arg can't fill a writable `*T` slot. Raw pointers never come back
/// alive: a `*T` return is an opaque value with no authority.
fn extern_call(
    n: &str,
    params: &[(Option<Auth>, Ty)],
    ret: &Ty,
    args: &[Expr],
    s: Span,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    if params.len() != args.len() {
        push_diag(ctx, Diag::new(
            s,
            format!(
                "`{n}` takes {} argument(s), got {}",
                params.len(),
                args.len()
            ),
        ));
    }
    let mut arg_holders = Vec::new();
    for (i, a) in args.iter().enumerate() {
        let Some((_, want)) = params.get(i) else { break };
        let Ty::Ptr { mutable, elem } = want else {
            // Scalar param — plain value argument.
            let (got, _) = expr_tv_w(a, Some(want), scopes, ctx);
            if let Ty::Guard(_) = got {
                push_diag(ctx, Diag::new(
                    a.span(),
                    "a lock guard cannot cross a call boundary".to_string(),
                ));
            }
            coerce(want, a, &got, &mut ctx.errs);
            continue;
        };
        let req = if *mutable { Auth::Write } else { Auth::Read };
        let mut claim: Vec<String> = Vec::new();
        match a {
            // `&x` / `&mut x` — the referent IS the argument. The
            // referent must name elem (or the param is `*void`), and a
            // writable `*T` requires `&mut`.
            Expr::AddrOf {
                mutable: am,
                place,
                ..
            } => {
                if *mutable && !am {
                    push_diag(ctx, Diag::new(
                        a.span(),
                        format!("`{n}` wants a writable `*T` — pass `&mut`"),
                    ));
                }
                let (got, _) = place_tv(place, scopes, ctx);
                if **elem != Ty::Void {
                    coerce(elem, place, &got, &mut ctx.errs);
                }
                claim = cap_check(place, req, scopes, ctx);
            }
            // `handle <place>` — identity crosses; the referent still
            // takes the authority claim (C can touch it).
            Expr::Handle(place, _) => {
                let (got, _) = place_tv(place, scopes, ctx);
                if **elem != Ty::Void && !matches!(got, Ty::Handle(_)) {
                    coerce(elem, place, &got, &mut ctx.errs);
                }
                claim = cap_check(place, req, scopes, ctx);
            }
            // `"lit"` — a read-only global; `*T` (writable) rejects.
            Expr::Str(..) => {
                if *mutable {
                    push_diag(ctx, Diag::new(
                        a.span(),
                        format!("`{n}` wants a writable `*T` — a string literal is `*const`"),
                    ));
                }
                if **elem != Ty::Void && **elem != Ty::U8 {
                    push_diag(ctx, Diag::new(
                        a.span(),
                        format!("string literal is `*const u8`, not `{want}`"),
                    ));
                }
            }
            // `0` — C's null pointer constant; no storage, no claim.
            Expr::Int(0, _) => {}
            // `s.ptr` — the slice's span rides along: a `*T` callee
            // can write through it, so the FFI contract claims the
            // whole span the slice's edges name. The slice's own edge
            // is the authority (a holder); every other claim conflicts.
            Expr::Field(b, name, _)
                if name == "ptr"
                    && matches!(
                        expr_ty_at(b, scopes, ctx).0,
                        Ty::Slice { .. } | Ty::BoxSlice(_)
                    ) =>
            {
                let (got, _) = expr_ty_at(a, scopes, ctx);
                if let Ty::Ptr { mutable: gm, elem: ge } = &got {
                    if *mutable && !gm {
                        push_diag(ctx, Diag::new(
                            a.span(),
                            format!("`{n}` wants a writable `*T`, found `{got}`"),
                        ));
                    }
                    if **elem != Ty::Void && **ge != Ty::Void && **ge != **elem {
                        push_diag(ctx, Diag::new(
                            a.span(),
                            format!("type mismatch: expected `{want}`, found `{got}`"),
                        ));
                    }
                }
                let mut holders: HashSet<String> =
                    resolve_place(b, scopes, ctx).into_iter().collect();
                // Under a cap param (`own/write/read v`), `v.buf`
                // resolves to `$argN.buf` — and `v#e -> $argN`, the
                // param's own derivation edge, OVERLAPS that span in
                // `edges_to`. It is our authority, not a borrow:
                // exclude the holders the path borrows through.
                holders.extend(path_holders(b, scopes, ctx));
                let mut ts = Vec::new();
                if matches!(expr_ty_at(b, scopes, ctx).0, Ty::BoxSlice(_)) {
                    // A `box slice`'s span IS its root — no edge hop.
                    ts = resolve_place(b, scopes, ctx);
                } else {
                    for h in &holders {
                        for ((eh, t), _) in ctx.edges.iter() {
                            if *eh == *h && !ts.contains(t) {
                                ts.push(t.clone());
                            }
                        }
                    }
                }
                for t in &ts {
                    for (h, ..) in edges_to(ctx, t) {
                        if holders.contains(&h) {
                            continue;
                        }
                        push_diag(ctx, Diag::new(
                            a.span(),
                            format!(
                                "`{n}`'s pointer reaches `{}`: it is borrowed",
                                target_name(b)
                            ),
                        ));
                    }
                }
                claim = ts;
            }
            // Existing pointer-ish values: `*T`, `&T`, `handle T` —
            // ref/handle vars carry real authority to claim; foreign
            // `*T`s carry none (already opaque).
            _ => {
                let (got, _) = expr_tv_w(a, Some(want), scopes, ctx);
                match &got {
                    Ty::Ptr {
                        mutable: gm,
                        elem: ge,
                    } => {
                        if *mutable && !gm {
                            push_diag(ctx, Diag::new(
                                a.span(),
                                format!("`{n}` wants a writable `*T`, found `{got}`"),
                            ));
                        }
                        if **elem != Ty::Void && **ge != Ty::Void && **ge != **elem {
                            push_diag(ctx, Diag::new(
                                a.span(),
                                format!("type mismatch: expected `{want}`, found `{got}`"),
                            ));
                        }
                    }
                    Ty::Ref { mutable: rm, elem: re } => {
                        if *mutable && !rm {
                            push_diag(ctx, Diag::new(
                                a.span(),
                                format!("`{n}` wants a writable `*T`, found `{got}`"),
                            ));
                        }
                        if **elem != Ty::Void && **re != Ty::Void && **re != **elem {
                            push_diag(ctx, Diag::new(
                                a.span(),
                                format!("type mismatch: expected `{want}`, found `{got}`"),
                            ));
                        }
                        claim = refs_of(a, scopes, ctx)
                            .map(|ri| ri.targets)
                            .unwrap_or_default();
                    }
                    Ty::Handle(_) => {
                        claim = caps_of(a, scopes, ctx).unwrap_or_default();
                    }
                    _ => {
                        coerce(want, a, &got, &mut ctx.errs);
                    }
                }
            }
        }
        if !claim.is_empty() {
            let h = format!("$tmp{i}");
            for t in claim {
                edge_set(ctx, h.clone(), t, EdgeFact::new(req, Cert::Def, Some(s)));
            }
            arg_holders.push(h);
        }
    }
    for h in arg_holders {
        edges_retain(ctx, |(eh, _), _| eh != &h);
    }
    (ret.clone(), Prov::Unknown)
}

/// The affine transfer — `consume p`, `consume p.f`, `f(own x)` all
/// land here. Rules:
///   - the place must carry Own-compatible authority (cap_check Own):
///     no other live edge, and a path through a borrow can't fabricate
///     Own (`consume *m` through write authority rejects).
///   - a path reaching its target only through a HANDLE names identity,
///     not authority — `consume *h` rejects outright.
///   - the consumed key may not already be dead (double consume) or
///     overlap a dead subtree (moving `p` can't revive `p.f`).
///   - marking: the resolved key -> Cons and its outgoing edges die
///     with it (container death).
fn consume_place(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    // N18 — consuming is a read+move: there must be a value to take.
    // `let x: box T; free(x)` dies here, as does `f(own x)` on uninit.
    require_init(e, scopes, ctx);
    for h in path_holders(e, scopes, ctx) {
        let via_cap = ctx.caps.keys().any(|(ch, _)| ch == &h);
        let via_edge = ctx.edges.keys().any(|(eh, _)| eh == &h);
        if via_cap && !via_edge {
            push_diag(ctx, Diag::new(
                e.span(),
                format!(
                    "cannot transfer ownership through a handle `{}`",
                    target_name(e)
                ),
            ));
            return;
        }
    }
    let ts = cap_check(e, Auth::Own, scopes, ctx);
    for t in ts {
        // N54 — a deferred call reserved this place: it must still be
        // live (and identical) when the scope exits.
        if let Some(sp) = defer_conflict(ctx, &t) {
            push_diag(ctx, Diag::new(
                e.span(),
                format!(
                    "cannot consume `{}`: it is reserved by the `defer` at line {}",
                    target_name(e),
                    sp.line
                ),
            ));
            return;
        }
        match consumed_overlap(ctx, &t) {
            Some(ConsState::Cons) => push_diag(ctx, consume_labels(
                ctx,
                &t,
                Diag::new(e.span(), format!("double consume `{}`", target_name(e))),
            )),
            Some(ConsState::Maybe) => push_diag(ctx, consume_labels(
                ctx,
                &t,
                Diag::new(
                    e.span(),
                    format!(
                        "cannot consume `{}`: it may be dead on another path",
                        target_name(e)
                    ),
                ),
            )),
            None => {}
        }
        cons_set(ctx, t.clone(), ConsFact::cons(e.span()));
        kill_set(ctx, t.clone(), Cert::Def);
        edges_retain(ctx, |(eh, _), _| {
            eh != &t
                && !eh.starts_with(&format!("{t}."))
                && !eh.starts_with(&format!("{t}["))
        });
    }
}

/// An owning value moving to a new binding/argument/return consumes
/// its source place: `let q = p` transfers p's heap root to q and
/// marks p spent — the same temporal transition `free` performs,
/// minus the byte release. `box`/`box slice` move directly; an
/// aggregate whose subtree holds live own-authority or edges/caps
/// moves whole — `let old = w` may never alias `w.n` with `old.n`
/// nor `w.r` with `old.r`. `alloc` tokens are shareable, and a
/// pure-data aggregate still copies freely.
fn consume_owned_src(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    match e {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {
            let t = expr_ty_at(e, scopes, ctx).0;
            let owns = match t {
                Ty::Box(_) | Ty::BoxSlice(_) => true,
                Ty::Struct(_) | Ty::Array(..) => subtree_owned(e, scopes, ctx),
                _ => false,
            };
            if owns {
                consume_place(e, scopes, ctx);
            }
        }
        _ => {}
    }
}

/// Does any place `e` resolves to carry something only a move may
/// hand over — a live `box` subtree (`live_own_at`) or a claimed
/// edge/cap? Pure-data subtrees answer false: they copy.
fn subtree_owned(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> bool {
    let t = expr_ty_at(e, scopes, ctx).0;
    for k in resolve_place(e, scopes, ctx) {
        if live_own_at(&k, &t, ctx).is_some() {
            return true;
        }
        let dot = format!("{k}.");
        if ctx.edges.keys().any(|(h, _)| h.starts_with(&dot))
            || ctx.caps.keys().any(|(h, _)| h.starts_with(&dot))
        {
            return true;
        }
    }
    false
}

/// The rel-keyed fact bundle a place's subtree carries — what a move,
/// `replace`, or `swap` hands to the destination key. `""` rels are
/// the place's own level (a `&mut`/`handle` slot's edge lives at the
/// key itself).
#[derive(Default)]
struct LitPkt {
    edges: Vec<(String, RefInfo)>,
    caps: Vec<(String, Vec<String>)>,
    provs: Vec<(String, Prov)>,
    dead: Vec<(String, ConsFact)>,
    lens: Vec<(String, i64)>,
    tags: Vec<(String, Option<String>)>,
    csafe: Vec<String>,
}

/// Collect every fact rooted at `k` or below, rel-keyed for re-root.
fn collect_lit(k: &str, ctx: &Ctx) -> LitPkt {
    let mut pkt = LitPkt {
        edges: payload_edges(k, ctx),
        caps: payload_caps(k, ctx),
        provs: payload_provs(k, ctx),
        ..LitPkt::default()
    };
    // The holder's own level — `payload_*` only sees `k.` paths.
    let mut ri = RefInfo {
        targets: Vec::new(),
        mutable: false,
        move_from: None,
        foreign: ctx.foreign.contains(k),
    };
    for ((h, t), f) in &ctx.edges {
        if h == k {
            ri.targets.push(t.clone());
            ri.mutable |= f.kind == Auth::Write;
        }
    }
    ri.targets.sort();
    if !ri.targets.is_empty() || ri.foreign {
        pkt.edges.insert(0, (String::new(), ri));
    }
    let mut cts: Vec<String> = ctx
        .caps
        .iter()
        .filter(|((h, _), _)| h == k)
        .map(|((_, t), _)| t.clone())
        .collect();
    cts.sort();
    if !cts.is_empty() {
        pkt.caps.insert(0, (String::new(), cts));
    }
    let dot = format!("{k}.");
    for (c, f) in &ctx.consumed {
        if let Some(rel) = c.strip_prefix(&dot) {
            pkt.dead.push((rel.to_string(), f.clone()));
        }
    }
    pkt.dead.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(l) = ctx.slice_lens.get(k) {
        pkt.lens.push((String::new(), *l));
    }
    for (c, l) in &ctx.slice_lens {
        if let Some(rel) = c.strip_prefix(&dot) {
            pkt.lens.push((rel.to_string(), *l));
        }
    }
    pkt.lens.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(t) = ctx.tag.get(k) {
        pkt.tags.push((String::new(), t.clone()));
    }
    for (c, t) in &ctx.tag {
        if let Some(rel) = c.strip_prefix(&dot) {
            pkt.tags.push((rel.to_string(), t.clone()));
        }
    }
    pkt.tags.sort_by(|a, b| a.0.cmp(&b.0));
    for c in &ctx.cstr_safe {
        if c == k {
            pkt.csafe.push(String::new());
        } else if let Some(rel) = c.strip_prefix(&dot) {
            pkt.csafe.push(rel.to_string());
        }
    }
    pkt.csafe.sort();
    pkt
}

/// Feed a collected packet into the lit channels so the next
/// `record_lit_fields` re-roots it under the destination key.
fn push_lit(pkt: LitPkt, ctx: &mut Ctx) {
    ctx.lit_edges.extend(pkt.edges);
    ctx.lit_caps.extend(pkt.caps);
    ctx.lit_dead.extend(pkt.dead);
    ctx.lit_lens.extend(pkt.lens);
    ctx.lit_tags.extend(pkt.tags);
    ctx.lit_csafe.extend(pkt.csafe);
    if !pkt.provs.is_empty() {
        let mut cur = ctx.last_lit.take().unwrap_or_default();
        cur.extend(pkt.provs);
        ctx.last_lit = Some(cur);
    }
}

/// An aggregate place expr moving to a new key pushes its subtree
/// facts into the lit channels — same rel-keyed shape a literal or
/// callee export produces, so `record_lit_fields` re-roots them under
/// the destination (`w#e.r -> t` becomes `old#e.r -> t`). Call BEFORE
/// `consume_owned_src`, which kills the source edges. Sites without a
/// `record_lit_fields` install downstream (call args, statement exprs)
/// must not push — the channel would leak into the next `let`.
fn push_place_lit(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    // `=> w`-style yields: a match result's already-joined packet
    // drains back into the channels, so `let r = match ...`,
    // `s.f = match ...`, `return match ...` all re-root it.
    if let Expr::Match { span, .. } = e {
        if let Some(yi) = ctx.match_yields.get(&(span.line, span.col)).cloned() {
            ctx.lit_edges.extend(yi.lit_edges);
            ctx.lit_caps.extend(yi.lit_caps);
            ctx.lit_dead.extend(yi.lit_dead);
            ctx.lit_lens.extend(yi.lit_lens);
            ctx.lit_tags.extend(yi.lit_tags);
            if !yi.lit_provs.is_empty() {
                let mut cur = ctx.last_lit.take().unwrap_or_default();
                cur.extend(yi.lit_provs);
                ctx.last_lit = Some(cur);
            }
        }
        return;
    }
    let t = expr_ty_at(e, scopes, ctx).0;
    if !matches!(t, Ty::Struct(_) | Ty::Array(..)) {
        return;
    }
    for k in resolve_place(e, scopes, ctx) {
        push_lit(collect_lit(&k, ctx), ctx);
    }
}

/// N23 — `Result<T,E>` is a SHAPE, not a blessed name: an enum with
/// exactly `Ok(payload)` + `Err(payload)` variants. Returns (T, E).
/// `std.result.Result` is an ordinary generic enum satisfying this.
fn result_parts(t: &Ty, ctx: &Ctx) -> Option<(Ty, Ty)> {
    let n = match t {
        Ty::Struct(n) => n,
        _ => return None,
    };
    let vars = ctx.enums.get(n)?;
    if vars.len() != 2 {
        return None;
    }
    let ok = vars.iter().find(|(vn, _)| vn == "Ok")?.1.clone()?;
    let er = vars.iter().find(|(vn, _)| vn == "Err")?.1.clone()?;
    Some((ok, er))
}

/// Join two affine maps at a control-flow merge: `Cons` survives only
/// where both sides agree; anything else is `Maybe` — the lab's third
/// state, use of which is rejected until reassignment restores `Live`.
fn join_consumed(a: &HashMap<String, ConsFact>, b: &HashMap<String, ConsFact>) -> HashMap<String, ConsFact> {
    let mut out = HashMap::new();
    for k in a.keys().chain(b.keys()) {
        let (fa, fb) = (a.get(k), b.get(k));
        let (st, at) = match (fa, fb) {
            (Some(x), Some(y)) if x.st == ConsState::Cons && y.st == ConsState::Cons => {
                (ConsState::Cons, merge_origins(&x.at, &y.at))
            }
            _ => (
                ConsState::Maybe,
                merge_origins(
                    &fa.map(|f| f.at.clone()).unwrap_or_default(),
                    &fb.map(|f| f.at.clone()).unwrap_or_default(),
                ),
            ),
        };
        out.insert(k.clone(), ConsFact { st, at });
    }
    out
}

/// Union two origin lists, deduped, capped — a Maybe fact names its
/// dead paths, but a loop can't grow the list without bound.
fn merge_origins(a: &[Span], b: &[Span]) -> Vec<Span> {
    let mut out = a.to_vec();
    for s in b {
        if !out.contains(s) && out.len() < 4 {
            out.push(*s);
        }
    }
    out
}

/// Cert-keyed map join: Def only where both maps agree Def; anything
/// else (one side absent, or a Maybe anywhere) is Maybe.
fn join_cert_map(
    a: &HashMap<String, Cert>,
    b: &HashMap<String, Cert>,
) -> HashMap<String, Cert> {
    let mut out = HashMap::new();
    for k in a.keys().chain(b.keys()) {
        let c = match (a.get(k), b.get(k)) {
            (Some(Cert::Def), Some(Cert::Def)) => Cert::Def,
            _ => Cert::Maybe,
        };
        out.insert(k.clone(), c);
    }
    out
}

/// N18 — definite initialization, the third temporal map. Distinct
/// from `consumed`: `x#e` marked here means storage holds a valid
/// value; `x#e` in `consumed` means authority over it was used up.
/// `free(p)` moves `p#e` Init+Live -> Init+Consumed; `p = alloc` moves
/// it back to Init+Live. Neither map encodes the other's fact.
#[derive(Clone, Copy, PartialEq)]
enum InitState {
    Init,
    Maybe,
    Uninit,
}

/// Canonical element family — `a#e[..]` and `a#e[]` are the same
/// collapsed element key (per-index precision isn't tracked, matching
/// the borrow model's `[]` convention).
fn init_canon(k: &str) -> String {
    match k.strip_suffix("[..]") {
        Some(stripped) => format!("{stripped}[]"),
        None => k.to_string(),
    }
}

/// The parent path of a place key: `p#e.f` -> `p#e`, `a#e[]` -> `a#e`,
/// `*k` -> `k` (an opaque deref's init follows its holder — a live
/// view's span is as initialized as the view's source allows).
fn init_parent(k: &str) -> Option<&str> {
    if let Some(stripped) = k.strip_suffix("[..]") {
        Some(stripped)
    } else if let Some(stripped) = k.strip_suffix("[]") {
        Some(stripped)
    } else if let Some(inner) = k.strip_prefix('*') {
        Some(inner)
    } else {
        k.rsplit_once('.').map(|(p, _)| p)
    }
}

/// `Def` at `k` or any ancestor — `p#e` covers `p#e.f.g`.
fn init_covered(init: &HashMap<String, Cert>, k: &str) -> bool {
    let canon = init_canon(k);
    let mut cur = Some(canon.as_str());
    while let Some(key) = cur {
        if matches!(init.get(key), Some(Cert::Def)) {
            return true;
        }
        cur = init_parent(key);
    }
    false
}

/// Does `t` embed own authority inline — a `box`/`box slice` reached
/// through struct fields, enum payloads, arrays, or atoms? Views and
/// pointers don't embed (`slice`/`str`/`&`/`handle`/`*`), and
/// capabilities (`mutex`/`guard`/`task`/`chan`) have their own rules.
/// Same domain as `consume_owned_src`'s transfer predicate.
fn reaches_own(t: &Ty, ctx: &Ctx, seen: &mut HashSet<String>) -> bool {
    match t {
        Ty::Box(_) | Ty::BoxSlice(_) => true,
        Ty::Atomic(e) | Ty::Array(e, _) => reaches_own(e, ctx, seen),
        Ty::Struct(n) => {
            if !seen.insert(n.clone()) {
                return false;
            }
            if let Some(vars) = ctx.enums.get(n) {
                return vars.iter().any(|(_, pt)| {
                    pt.as_ref()
                        .map_or(false, |pt| reaches_own(pt, ctx, seen))
                });
            }
            ctx.structs.get(n).map_or(false, |fs| {
                fs.iter().any(|(_, ft)| reaches_own(ft, ctx, seen))
            })
        }
        _ => false,
    }
}

/// `Def` OR `Maybe` at `k` or any ancestor — for the overwrite law a
/// maybe-initialized place counts as present: possibly-live authority
/// still needs explicit handling.
fn init_present(init: &HashMap<String, Cert>, k: &str) -> bool {
    let canon = init_canon(k);
    let mut cur = Some(canon.as_str());
    while let Some(key) = cur {
        if matches!(init.get(key), Some(Cert::Def | Cert::Maybe)) {
            return true;
        }
        cur = init_parent(key);
    }
    false
}

/// The init state of a whole read of `k` (typed `ty`): Init when the
/// spine proves it, Uninit when nothing reached it, Maybe otherwise.
/// For a struct the whole read demands every leaf path — `p` with only
/// `p#e.a` stored is not a complete `Pair`.
fn init_state(ctx: &Ctx, k: &str, ty: &Ty) -> InitState {
    let canon = init_canon(k);
    let k = canon.as_str();
    let mut cur = Some(k);
    let mut spine_maybe = false;
    while let Some(key) = cur {
        match ctx.init.get(key) {
            Some(Cert::Def) => return InitState::Init,
            Some(Cert::Maybe) => spine_maybe = true,
            None => {}
        }
        cur = init_parent(key);
    }
    // Leaves decide before a bare spine-Maybe does: `p#e` may be Maybe
    // at a join (a whole store covered one arm) while every field is
    // still Def through its own path — a complete value either way.
    if let Ty::Struct(n) = ty {
        if let Some(fields) = ctx.structs.get(n) {
            let mut any = false;
            let mut all = true;
            for (fname, fty) in fields.iter() {
                match init_state(ctx, &format!("{k}.{fname}"), fty) {
                    InitState::Init => any = true,
                    InitState::Maybe => {
                        any = true;
                        all = false;
                    }
                    InitState::Uninit => all = false,
                }
            }
            return match (all, any) {
                (true, _) => InitState::Init,
                (false, false) if !spine_maybe => InitState::Uninit,
                _ => InitState::Maybe,
            };
        }
    }
    if let Ty::Array(elem, _) = ty {
        // Elements live in the collapsed `[]` family — the whole read
        // asks the element key, not per-index precision we don't have.
        return match init_state(ctx, &format!("{k}[]"), elem) {
            InitState::Init => InitState::Init,
            InitState::Uninit if !spine_maybe => InitState::Uninit,
            _ => InitState::Maybe,
        };
    }
    if spine_maybe {
        return InitState::Maybe;
    }
    InitState::Uninit
}

/// Branch join for init: a key is `Def` after the merge only where a
/// `Def` covers it on BOTH paths (coverage, not raw presence —
/// `p = Pair{..}` then-side covers `p#e.a` else-side wrote directly).
fn join_init(
    a: &HashMap<String, Cert>,
    b: &HashMap<String, Cert>,
) -> HashMap<String, Cert> {
    let mut out = HashMap::new();
    for k in a.keys().chain(b.keys()) {
        match (init_covered(a, k), init_covered(b, k)) {
            (true, true) => {
                out.insert(k.clone(), Cert::Def);
            }
            (true, false) | (false, true) => {
                out.insert(k.clone(), Cert::Maybe);
            }
            (false, false) => {}
        }
    }
    out
}

/// A value read requires definite initialization of every place it
/// resolves to. `.len`/`.ptr` are computed members — the storage they
/// name is the (ptr, len) pair itself, so the base carries the proof.
fn require_init(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    if let Expr::Field(b, f, _) = e {
        if matches!(f.as_str(), "len" | "ptr")
            && matches!(
                expr_ty_at(b, scopes, ctx).0,
                Ty::Slice { .. } | Ty::BoxSlice(_) | Ty::Str
            )
        {
            return require_init(b, scopes, ctx);
        }
    }
    let (ty, _) = expr_ty_at(e, scopes, ctx);
    for k in resolve_place(e, scopes, ctx) {
        // `a[lo..hi]` names the span's ELEMENTS — resolve returns the
        // root (`a#e`), but the range place lives in the `[]` family.
        let k = if let Expr::IndexRange { .. } = e {
            format!("{k}[]")
        } else {
            k
        };
        let msg = match init_state(ctx, &k, &ty) {
            InitState::Init => continue,
            InitState::Maybe => {
                format!("use of possibly-uninitialized `{}`", target_name(e))
            }
            InitState::Uninit => format!("use of uninitialized `{}`", target_name(e)),
        };
        // Several sites check the same read (place_tv, check_store,
        // require_place_bases) — one diagnostic per real failure.
        if !ctx.errs.iter().any(|d| d.span == e.span() && d.msg == msg) {
            push_diag(ctx, Diag::new(e.span(), msg));
        }
    }
}

/// Bases whose VALUE computes a path's address: `*p` reads p's
/// pointer, `b[i]`/`b[lo..hi]` on a slice reads its (ptr, len). Struct
/// fields and array elements are direct storage — the slot exists
/// whether or not a value ever landed in it. Applies to write targets
/// and `&`/`&mut` places alike — neither may ride a wild pointer.
fn require_place_bases(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    match e {
        Expr::Deref(inner, _) => {
            require_init(inner, scopes, ctx);
            require_place_bases(inner, scopes, ctx);
        }
        Expr::Index(b, ..) | Expr::IndexRange { base: b, .. } => {
            let bt = expr_ty_at(b, scopes, ctx).0;
            if matches!(bt, Ty::Slice { .. } | Ty::BoxSlice(_)) {
                require_init(b, scopes, ctx);
            }
            // N52 — `v.buf[i] = x` / `&v.buf[..]` ride the slice's
            // own claim; `let s = v.buf` moved that edge out, so a
            // dead, non-foreign, previously-moved field must not keep
            // serving as a view base. `box slice` is exempt: owning
            // authority is intrinsic (no edge), and its moves are
            // already covered by consume tracking.
            if matches!(bt, Ty::Slice { .. }) {
                if let Some(k) = place_id(b, scopes) {
                    if ctx.edge_kills.contains_key(&k)
                        && !ctx.foreign.contains(&k)
                        && !live_out_edge(ctx, &k)
                    {
                        push_diag(ctx, Diag::new(
                            b.span(),
                            format!("use of moved reference `{}`", target_name(b)),
                        ));
                    }
                }
            }
            require_place_bases(b, scopes, ctx);
        }
        Expr::Field(b, ..) => require_place_bases(b, scopes, ctx),
        _ => {}
    }
}

/// Is `key` inside a consumed place? A consumed `p#e` blocks every
/// path under it; a consumed `p#e.f` blocks only `.f` paths — field-
/// level consumption without consuming the whole.
fn consumed_state(ctx: &Ctx, key: &str) -> Option<ConsState> {
    let mut worst = None;
    for (c, f) in ctx.consumed.iter() {
        let covers = key == c
            || key.starts_with(&format!("{c}."))
            || key.starts_with(&format!("{c}["));
        if covers {
            worst = match worst {
                Some(ConsState::Cons) => Some(ConsState::Cons),
                _ => Some(f.st),
            };
        }
    }
    worst
}

/// Does `key` overlap a consumed place in EITHER direction? Used by
/// transfer ops — moving `p` when `p.f` is dead revives nothing.
fn consumed_overlap(ctx: &Ctx, key: &str) -> Option<ConsState> {
    let mut worst = consumed_state(ctx, key);
    for (c, f) in ctx.consumed.iter() {
        // `[]`-collapsed descendants are per-element approximations,
        // not whole-place facts: `slots[].Full` marked dead in one
        // match arm can't prove the shell is partially dead. Exact
        // `[]` keys still count via consumed_state above — the same
        // rule `live_own_at` uses (per-element liveness is a
        // container contract, not a checkable place).
        if c != key && c.contains("[]") {
            continue;
        }
        let covered = c == key
            || c.starts_with(&format!("{key}."))
            || c.starts_with(&format!("{key}["));
        if covered {
            worst = match worst {
                Some(ConsState::Cons) => Some(ConsState::Cons),
                _ => Some(f.st),
            };
        }
    }
    worst
}

/// N35B — attach "consumed here" labels for every recorded death
/// covering `key` (the consumed_overlap scan, kept in lockstep).
/// A Maybe fact can carry several origins — each dead path gets its
/// own label rather than the renderer pretending there's one site.
fn consume_labels(ctx: &Ctx, key: &str, mut d: Diag) -> Diag {
    let mut seen: Vec<Span> = Vec::new();
    for (c, f) in ctx.consumed.iter() {
        let covers = key == c
            || key.starts_with(&format!("{c}."))
            || key.starts_with(&format!("{c}["))
            || c.starts_with(&format!("{key}."))
            || c.starts_with(&format!("{key}["));
        if !covers || c.contains("[]") {
            continue;
        }
        for at in &f.at {
            if !seen.contains(at) { seen.push(*at);
                d.labels.push(Label {
                    span: *at,
                    msg: "consumed here".to_string(),
                    file: None,
                });
            }
        }
    }
    d
}

/// N35B — the "X created here" label for an edge's birth site.
/// `holder` names the borrowing binding in user terms (`v#3` -> `v`).
/// `None` when the edge has no user-facing origin (param frames).
fn born_label(h: &str, f: &EdgeFact, ctx: &Ctx) -> Option<Label> {
    let span = f.born?;
    let name = h.split('#').next().unwrap_or(h);
    // `$rv`/`$tmp`/`$arg` are compiler-internal holders — the span is
    // still the user's expression, but the name must never surface.
    let internal = name.starts_with('$');
    let msg = if ctx.tasks.contains_key(h) {
        format!("task `{name}` claimed it here")
    } else if internal {
        match f.kind {
            Auth::Write => "mutably borrowed here".to_string(),
            _ => "borrowed here".to_string(),
        }
    } else {
        match f.kind {
            Auth::Write => format!("`{name}` mutably borrows it here"),
            Auth::Sync => format!("`{name}` locked it here"),
            Auth::Atomic => format!("`{name}` claimed it atomically here"),
            _ => format!("`{name}` borrows it here"),
        }
    };
    Some(Label {
        span,
        msg,
        file: None,
    })
}

/// Does `k` (typed `ty`) still hold live own authority — a `box` or
/// `box slice` root reachable through struct fields or the active
/// enum variant's payload? A store over it would silently drop that
/// authority: the language has no hidden `free` on overwrite, so the
/// write must extract (`let old = s.p` / `Full(e) => ..`) or `free`
/// the previous value first. Returns the offending place key.
///
/// Heap-element keys (`[]`) opt out: every index collapses to one
/// path, so per-element liveness is unknowable — `slots[i] = Tomb`
/// over `Full(box)` is a container CONTRACT (extract-then-write,
/// which `StrMap::remove` honors), not a checkable place.
fn live_own_at(k: &str, ty: &Ty, ctx: &Ctx) -> Option<String> {
    // A place that isn't present, or whose subtree was consumed,
    // holds nothing live below — prune before descending.
    if k.contains("[]")
        || !init_present(&ctx.init, k)
        || matches!(consumed_state(ctx, k), Some(ConsState::Cons))
    {
        return None;
    }
    match ty {
        // `box`/`box slice` ARE the authority — same predicate as
        // `consume_owned_src`. Views (`slice`/`str`/`&`/`handle`) and
        // capabilities (`mutex`/`guard`/`task`/`chan`) carry their own
        // rules, not this one.
        Ty::Box(_) | Ty::BoxSlice(_) => Some(k.to_string()),
        Ty::Atomic(e) => live_own_at(k, e, ctx),
        Ty::Struct(n) => {
            if let Some(vars) = ctx.enums.get(n) {
                for (vn, pt) in vars.iter() {
                    let Some(pt) = pt else { continue };
                    // The payload is live only while the tag can still
                    // name this variant.
                    let possible = match ctx.tag.get(k) {
                        Some(Some(v)) => v == vn,
                        _ => true,
                    };
                    if possible {
                        if let Some(hit) = live_own_at(&format!("{k}.{vn}"), pt, ctx) {
                            return Some(hit);
                        }
                    }
                }
                return None;
            }
            if let Some(fields) = ctx.structs.get(n) {
                for (fn_, ft) in fields.iter() {
                    if let Some(hit) = live_own_at(&format!("{k}.{fn_}"), ft, ctx) {
                        return Some(hit);
                    }
                }
            }
            None
        }
        _ => None,
    }
}

/// Diagnose a read of a maybe/dead place — Var and path arms share it.
fn use_check(key: &str, e: &Expr, ctx: &mut Ctx) {
    if transit_check(e, key, ctx) {
        return;
    }
    match consumed_state(ctx, key) {
        Some(ConsState::Cons) => push_diag(ctx, consume_labels(
            ctx,
            key,
            Diag::new(e.span(), format!("use after consume `{}`", target_name(e))),
        )),
        Some(ConsState::Maybe) => push_diag(ctx, consume_labels(
            ctx,
            key,
            Diag::new(
                e.span(),
                format!(
                    "use of maybe-consumed `{}`",
                    target_name(e)
                ),
            ),
        )),
        None => {}
    }
}

/// Root variable segment of a path id (`s#1.f` → `s#1`).
fn path_root(k: &str) -> &str {
    k.split(['.', '[']).next().unwrap_or(k)
}

/// Do two path ids share storage? Same root, and one is a prefix of the
/// other (writing `p` touches `p.x`; a ref to `p.x` sees writes to `p`).
fn overlap(a: &str, b: &str) -> bool {
    if path_root(a) != path_root(b) {
        return false;
    }
    if a == b {
        return true;
    }
    let (long, short) = if a.len() > b.len() { (a, b) } else { (b, a) };
    long.starts_with(short)
        && long
            .as_bytes()
            .get(short.len())
            .map(|c| *c == b'.' || *c == b'[' || *c == b'*')
            .unwrap_or(false)
}

/// The context a checker function needs but can't hold mutably:
/// signature tables, struct tables, the region tree under construction,
/// and the field-path provenance map.
#[derive(Clone)]
struct Ctx<'a> {
    /// (params, ret, is_extern) — extern fns carry signatures only;
    /// their calls dispatch to `extern_call`, not summary replay.
    fns: &'a HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
    /// Names declared by THIS module — the builtin-shadowing set:
    /// a local `fn close`/`extern fn close` takes the name over
    /// `close`/`send`/`lock`-family builtins inside this module only;
    /// imported names never shadow builtins (N21B).
    local_fns: &'a HashSet<String>,
    structs: &'a Structs<'a>,
    enums: &'a Enums<'a>,
    regs: Vec<RegionFrame>,
    /// Active-variant state (N22) — a fact orthogonal to init and
    /// affine state: `Some("Some")` = that variant is definitely
    /// active, `None` = a valid enum value whose case is unknown
    /// (branch-joined). Absent key = no enum knowledge.
    tag: HashMap<String, Option<String>>,
    /// EnumLit / enum-place side-channel, `last_lit`-style: the
    /// Let/Assign site installs it under the target's key.
    last_tag: Option<Option<String>>,
    /// Provenance of references stored at field/index paths of locals.
    path_provs: HashMap<String, Prov>,
    /// Field provs of the most recently evaluated struct literal —
    /// Lets/Assigns lift them into path_provs for the target.
    last_lit: Option<Vec<(String, Prov)>>,
    /// Ditto for alias edges born inside a struct literal.
    lit_edges: Vec<(String, RefInfo)>,
    /// Moving-aggregate side channels (N40) — `push_place_lit` fills
    /// them alongside `lit_edges`/`lit_caps`/`last_lit` so a moved
    /// value's interior facts arrive intact, not resurrected:
    /// interior consumed marks, known slice lengths, enum tags.
    lit_dead: Vec<(String, ConsFact)>,
    lit_lens: Vec<(String, i64)>,
    lit_tags: Vec<(String, Option<String>)>,
    /// NUL-provenance re-roots with the value too — a `str` moved by
    /// `replace`/`swap` must not inherit the OTHER place's `cstr`
    /// safety, and a provably-safe one shouldn't lose it.
    lit_csafe: Vec<String>,
    /// The alias graph — see the N4 block above.
    edges: Edges,
    /// The IDENTITY graph (N6): `h --cap--> x` from `handle x`.
    /// Cap edges never constrain the target — no liveness keeping, no
    /// borrow conflicts. A use of `*h` checks whether the target is
    /// still alive; the edge surviving a dead target is the point.
    caps: HashMap<(String, String), Cert>,
    /// Cap edges born inside a struct literal (parallel to lit_edges).
    lit_caps: Vec<(String, Vec<String>)>,
    /// Unique id per binding, so shadowed names keep separate edges.
    next_epoch: u32,
    /// The affine temporal map (N7): place keys whose authority was
    /// consumed — `x#e`, `p#e.resource`. `Cons` = used up on every
    /// path; `Maybe` = used up on some path (join-produced). Live =
    /// absent. Reassignment resurrects by removing the key.
    consumed: HashMap<String, ConsFact>,
    /// N8 bookkeeping: holder-prefix edge kills (`h.ptr = v` kills
    /// edges under `h.ptr`), joined must/may like the edge map. Only
    /// `$arg`-rooted entries survive into a FnSummary.
    edge_kills: HashMap<String, Cert>,
    cap_kills: HashMap<String, Cert>,
    /// Returned alias/identity targets, callee-side `$arg`-keyed.
    ret_edges: Vec<(String, bool)>,
    ret_caps: Vec<String>,
    /// Enum-typed returns: per-variant payload channels
    /// (lit_edges/lit_caps/last_lit shape) exported to the summary.
    ret_lit_edges: Vec<(String, RefInfo)>,
    ret_lit_caps: Vec<(String, Vec<String>)>,
    ret_lit_provs: Vec<(String, Prov)>,
    /// N9: every authority-touching access this ctx performed
    /// (key -> strongest kind). For a task-body ctx the spawner reads
    /// it; for fns it's inert.
    holds: HashMap<String, Auth>,
    /// Tasks ever bound here: place-key -> spawn result (holds +
    /// suspended effects). Entries persist after join so `refs_of` can
    /// still read the task's return edges while binding the result.
    tasks: HashMap<String, SpawnResult>,
    /// Inside a task body: `return` binds the task's result, not the
    /// fn's. `ext` bounds what may escape the body.
    task: Option<TaskScope>,
    /// The enclosing fn's declared return type — `expr?` (N23) checks
    /// its Err propagation against this (or the task's `ret` inside a
    /// spawn body, mirroring `return`).
    fn_ret: Ty,
    /// Spawn side-channel, `last_lit`-style: Let/Assign pick it up.
    last_spawn: Option<SpawnResult>,
    /// Resolved caller keys per call site (span -> per-arg targets),
    /// captured DURING arg-contract checking — before authority moves
    /// (a write-cap arg reborrow-moves its edge, so post-hoc
    /// resolution sees a dead edge and loses the translation).
    call_bases: HashMap<(u32, u32), Vec<Vec<String>>>,
    /// `replace(place, v)` result facts (N40) — the OLD value's
    /// root-level edges/caps, keyed by the call's span. `refs_of`/
    /// `caps_of` read them so `let r = replace(w.r, &mut y)` attaches
    /// the old borrow's target to `r`, not arg1's — and a `&mut`
    /// result in arg position keeps its referent frozen for the call.
    replace_outs: HashMap<(u32, u32), (RefInfo, Vec<String>)>,
    /// `$argN` -> payload ty for params on atomic storage (`write c:
    /// atomic u64`, `r: &mut atomic u64`) — atomic ops through cap
    /// params resolve here.
    arg_atomic: HashMap<String, Ty>,
    /// Same for `mutex T` params — `$argN` -> payload ty.
    arg_mutex: HashMap<String, Ty>,
    /// N11 — every mutex domain: domain key -> protected root key.
    /// Self-payload mutexes map to themselves; `mutex h` maps the
    /// domain to the referent's root (protects the whole closure).
    doms: HashMap<String, String>,
    /// `lock` side-channel (last_lit-style): (domain, root, payload
    /// ty) — the Let/Assign site turns it into a guard bind.
    /// Pending `lock(m)` result for the enclosing `let`: (domain,
    /// protected root, payload ty, locker holder keys).
    last_lock: Option<(String, String, Ty, Vec<String>)>,
    /// `mutex h`'s foreign roots — the Let site records doms[key]=root.
    last_mutex: Option<Vec<String>>,
    /// N12 — in-flight authority packets per endpoint key (`ch#e`).
    /// The channel world is SHARED state: it clones into task bodies
    /// and the child's result merges back at spawn-end, so sibling
    /// send/recv pairs see each other through the parent.
    chan_queues: HashMap<String, Vec<Packet>>,
    /// Place keys whose authority is channel-held — any access is
    /// `use-after-send`. Topology (a set), not a temporal state.
    transit: HashSet<String>,
    /// `recv` side-channel (last_lit-style): the Let/Assign site
    /// claims the popped packet and installs its graph delta.
    last_recv: Option<Packet>,
    /// CHAN-004 S3b — bounded `send` side-channel: the Result's
    /// latent packets ride to the binding site, where they key into
    /// `out.pending_send`. Leftover at a statement's end = the Result
    /// was never bound anywhere it could resolve — an error.
    last_send: Option<Vec<PendingPkt>>,
    /// CHAN-004 S3a — outstanding-capability tracking: endpoint
    /// uncertainty, push-history name unions, pending sends, and
    /// unknown-identity candidate sets. Bundled so every snapshot
    /// and merge site carries the uncertainty as one field.
    out: Outstanding,
    /// `$argN` -> payload ty for params on channel storage.
    arg_chan: HashMap<String, Ty>,
    /// N15 — place keys whose slice value was asserted over foreign
    /// storage (`slice(p, n)`): no graph edges to be live or dead, so
    /// edge-liveness checks must not read empty edges as moved.
    foreign: HashSet<String>,
    /// N15 — statically-known slice lengths (place key -> element
    /// count): `&a[2..5]` records 3 under the new binding's key.
    /// Drives compile-time rejection of constant OOB indexes; absent
    /// means dynamic-only (the runtime check still applies).
    slice_lens: HashMap<String, i64>,
    /// `str` bindings whose storage provably ends in a NUL byte —
    /// literal-derived only. `cstr` consults this; nothing else may
    /// claim C-string compatibility (N15B).
    cstr_safe: HashSet<String>,
    /// The definite-initialization map (N18): place keys whose storage
    /// provably holds a value — `x#e`, `p#e.f`, `a#e[]`. `Def` = a
    /// store reached it on every path; `Maybe` = join-produced
    /// (initialized on some path). Uninit = absent. This is NOT the
    /// affine map: `free(p)` leaves `p#e` initialized-but-consumed —
    /// "a valid value existed" and "authority remains" are different
    /// facts, tracked by different maps.
    init: HashMap<String, Cert>,
    /// `$argN.*` keys a STORE actually wrote — distinguished from the
    /// params' presumed-live seed marks, so a `write` param's definite
    /// callee-side initialization can export through the FnSummary.
    init_writes: HashSet<String>,
    /// Control left this point (`return` ran): branch joins use it to
    /// merge only the paths that actually reach the join — a diverged
    /// arm's state does not weaken its sibling's.
    diverged: bool,
    /// N32 — the path ended WITHOUT unwinding (`unreachable()` /
    /// `process.exit` — the process died): scope-exit obligations
    /// (unjoined task, task-escape) are suppressed on terminated
    /// paths — the process, not the continuation, owns cleanup. A
    /// join is terminated only when every arm that reached it
    /// terminated; a `return` arm unwinds, it doesn't terminate.
    terminated: bool,
    /// N25D — loop stack: `break`/`continue` snapshot into the
    /// innermost entry; the `while` merge joins every recorded exit
    /// against the pre-loop state. A task body gets its own ctx, so a
    /// break can never tunnel out of a spawned frame.
    loops: Vec<LoopCtx>,
    /// Already-analyzed callees (dependency order: callee first).
    sums: &'a HashMap<String, FnSummary>,
    /// N17 — qualified names that exist in imported modules but are
    /// module-private: turns a call miss into "private" instead of
    /// "undefined".
    private_syms: &'a HashSet<String>,
    /// N28 — `(line, col)` of an `Expr::Match` -> what its live arms
    /// yielded. Read by `expr_tv` (result prov), `refs_of`, `caps_of`,
    /// `slice_len_of` — all of which run AFTER arm scopes expired.
    match_yields: HashMap<(u32, u32), ArmYieldInfo>,
    /// N45R — place keys already diagnosed as abandoned on a diverged
    /// path (`return`/`?`): the same place's later `}`-pop must not
    /// double-report it.
    abandoned: HashSet<String>,
    /// N54 — deferred calls, stacked one list per scope like `scopes`
    /// (the two push and pop together); replays run LIFO at exit.
    defers: Vec<Vec<DeferRec>>,
    /// Place keys frozen by a live defer → its registration span.
    /// Moves, consumes, rebinding stores, and `promote` touching a
    /// reserved key reject — the deferred call must see the operand
    /// it registered.
    defer_res: HashMap<String, Span>,
    errs: Vec<Diag>,
}

/// N54 — one registered `defer`: the operand place keys frozen at
/// registration (any place the call reads), the keys its exit replay
/// consumes (discharging R19), the call itself, and its site.
#[derive(Clone)]
struct DeferRec {
    operands: Vec<String>,
    consumes: Vec<String>,
    expr: Expr,
}

impl DeferRec {
    fn keys(&self) -> impl Iterator<Item = &String> {
        self.operands.iter().chain(self.consumes.iter())
    }
}

/// Kill edges held under `key`: a must-kill removes them, a may-kill
/// only weakens certainty — the edge is still a possible alias.
fn apply_kill(edges: &mut Edges, key: &str, cert: Cert) {
    match cert {
        Cert::Def => {
            let mut dead: Vec<(String, String)> = edges
                .keys()
                .filter(|(h, _)| {
                    !(h != key
                        && !h.starts_with(&format!("{key}."))
                        && !h.starts_with(&format!("{key}[")))
                })
                .cloned()
                .collect();
            dead.sort();
            for k in dead {
                rec(format!("(unedge {} {})", qs(&k.0), qs(&k.1)));
                edges.remove(&k);
            }
        }
        Cert::Maybe => {
            let under: Vec<(String, String)> = edges
                .keys()
                .filter(|(h, _)| {
                    h == key
                        || h.starts_with(&format!("{key}."))
                        || h.starts_with(&format!("{key}["))
                })
                .cloned()
                .collect();
            let mut under = under;
            under.sort();
            for k in under {
                if let Some(v) = edges.get_mut(&k) {
                    rec(format!("(weakedge {} {})", qs(&k.0), qs(&k.1)));
                    v.cert = Cert::Maybe;
                }
            }
        }
    }
}

/// Replay a callee's summary onto the caller's graph. Kills land
/// before creates (an overwrite inside the callee removed the old
/// edge before adding the new); both record into the caller's own
/// kill maps so the effect percolates up the call chain.
fn replay_summary(
    sum: &FnSummary,
    bases: &[Vec<String>],
    call_span: Span,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    let mut kills: Vec<(&String, &Cert)> = sum.kills.iter().collect();
    kills.sort_by(|a, b| a.0.cmp(b.0));
    for (hk, cert) in kills {
        for ck in translate_key(hk, bases) {
            kill_set(ctx, ck.clone(), *cert);
            apply_kill(&mut ctx.edges, &ck, *cert);
        }
    }
    let mut ckills: Vec<(&String, &Cert)> = sum.cap_kills.iter().collect();
    ckills.sort_by(|a, b| a.0.cmp(b.0));
    for (hk, cert) in ckills {
        for ck in translate_key(hk, bases) {
            capkill_set(ctx, ck.clone(), *cert);
            let kill = ck.clone();
            apply_cap_kill(ctx, &kill, *cert);
        }
    }
    let mut creates: Vec<&(String, String, Auth, Cert)> = sum.creates.iter().collect();
    creates.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (h, t, kind, cert) in creates {
        for ch in translate_key(h, bases) {
            for ct in translate_key(t, bases) {
                // The summary is key-shaped (no spans) — the borrow's
                // user-facing origin is this call.
                edge_set(ctx, ch.clone(), ct.clone(), EdgeFact::new(*kind, *cert, Some(call_span)));
                // The stored ref's prov at the holder path = its
                // target's owner region — what a direct store does.
                let root = path_root(&ct);
                let name = root.split('#').next().unwrap_or(root);
                if let Some(b) = lookup(scopes, name) {
                    if format!("{name}#{}", b.epoch) == root {
                        prov_set(ctx, ch.clone(), Prov::Region(b.owner));
                    }
                }
            }
        }
    }
    let mut ccaps: Vec<&(String, String, Cert)> = sum.creates_caps.iter().collect();
    ccaps.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (h, t, cert) in ccaps {
        for ch in translate_key(h, bases) {
            for ct in translate_key(t, bases) {
                cap_set(ctx, ch.clone(), ct.clone(), *cert);
            }
        }
    }
    // Synchronized domain claims replay as holds: inside a task they
    // become `t -> caller-domain` Sync edges — task-window compat.
    let mut syncs: Vec<&String> = sum.syncs.iter().collect();
    syncs.sort();
    for dk in syncs {
        for cd in translate_key(dk, bases) {
            record_hold(ctx, &cd, Auth::Sync);
        }
    }
    // N18 — definite callee writes replay as definite caller-side
    // initialization: `link(write h)` that stores `h.h` inside marks
    // `h#e.h` Init here. `$arg`-rooted targets re-export — the fact
    // flows through nested write-param chains unchanged.
    let mut inits: Vec<&String> = sum.init_paths.iter().collect();
    inits.sort();
    for k in inits {
        for ck in translate_key(k, bases) {
            init_set(ctx, ck.clone(), Cert::Def);
            if ck.starts_with("$arg") {
                ctx.init_writes.insert(ck);
            }
        }
    }
}

/// N17 — the semantic contract a module hands its importers. Only
/// `pub` symbols appear here; `private` holds the qualified names that
/// exist but are module-internal, so importers get "private" rather
/// than "undefined" diagnostics. In-memory for now — nothing about the
/// shape prevents serializing it later (`vec.mod`).
#[derive(Default)]
pub struct ModuleInterface {
    /// Qualified name -> (params, ret, is_extern) — the same map the
    /// checker builds for local fns, so imported calls replay
    /// identically.
    pub fns: HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
    /// Qualified name -> field list — layouts for `m.T` references.
    pub structs: HashMap<String, Vec<(String, Ty)>>,
    /// Qualified name -> variant list (decl order), `m.E` layouts (N22).
    pub enums: HashMap<String, Vec<(String, Option<Ty>)>>,
    /// Qualified name -> FnSummary — the effect contract replayed at
    /// call sites without ever opening the callee's body.
    pub sums: HashMap<String, FnSummary>,
    /// Qualified names that exist but aren't `pub`.
    pub private: HashSet<String>,
    /// Qualified name -> declaration site + signature — N36B's
    /// go-to-definition for imported symbols. `file` stays `None`
    /// inside the checker; the pipeline stamps it when the module's
    /// path is known.
    pub locs: HashMap<String, DefLoc>,
}

/// `read`/`write`/`own` rendered for a signature hover — `None` is a
/// plain local param, rendered bare.
fn auth_str(a: Option<Auth>) -> &'static str {
    match a {
        Some(Auth::Read) => "read ",
        Some(Auth::Write) => "write ",
        Some(Auth::Own) => "own ",
        _ => "",
    }
}

/// `fn f<T>(own x: i32, y: bool) -> bool` — hover text built from the
/// declared signature (param names live only on `FnDef`).
fn fn_sig(f: &FnDef) -> String {
    let tp = if f.tparams.is_empty() {
        String::new()
    } else {
        format!("<{}>", f.tparams.join(","))
    };
    let params: Vec<String> = f
        .params
        .iter()
        .map(|(n, a, t)| format!("{}{n}: {t}", auth_str(*a)))
        .collect();
    format!(
        "{} {}{tp}({}) -> {}",
        if f.is_extern { "extern fn" } else { "fn" },
        f.name,
        params.join(", "),
        f.ret
    )
}

/// Check a single-file program — `check_module` with no imports.
pub fn check(p: &Program) -> Result<(), Vec<Diag>> {
    check_module(p, &ModuleInterface::default(), &mut ModuleInterface::default())
}

/// Check one module whose dependencies' interfaces are already built.
/// `imports` supplies every symbol the module's `m.f`/`m.T` references
/// resolve against; `out` receives this module's own `pub` slice.
pub fn check_module(
    p: &Program,
    imports: &ModuleInterface,
    out: &mut ModuleInterface,
) -> Result<(), Vec<Diag>> {
    let mut errs = Vec::new();

    // Struct registry first — field types may name other structs.
    // Imported defs are collected once, then referenced — the map
    // stays a borrow-map over this frame's owned copies.
    let imported: Vec<(String, Vec<(String, Ty)>)> = imports
        .structs
        .iter()
        .map(|(n, f)| (n.clone(), f.clone()))
        .collect();
    let mut structs: Structs = HashMap::new();
    for (n, fields) in &imported {
        structs.insert(n.clone(), fields);
    }
    for s in &p.structs {
        if !s.tparams.is_empty() {
            continue; // N19 — a template, not a type: only its
                      // instantiated clones register.
        }
        // Instantiated clones (`m.T<..>`) may legitimately appear twice —
        // once through an imported interface and once instantiated
        // locally; N19 makes the layouts byte-identical, so only a
        // collision with a user-written name is an error.
        if structs.insert(s.name.clone(), &s.fields).is_some()
            && !s.name.contains('<')
        {
            push_decl_diag(&mut errs, Diag::new(s.span, format!("duplicate struct `{}`", s.name)));
        }
        let mut seen = HashSet::new();
        for (fn_, _) in &s.fields {
            if !seen.insert(fn_) {
                push_decl_diag(&mut errs, Diag::new(
                    s.span,
                    format!("duplicate field `{fn_}` in struct `{}`", s.name),
                ));
            }
        }
    }
    // N22 — enum registry: variants in decl order (tag index). Each
    // enum ALSO pseudo-registers in `structs` — `(variant, payload)`
    // pairs, `void` for nullary — so `check_ty`/`reaches_struct`/
    // `field_ty` need no enum cases: `x.Some` is an ordinary place.
    // The tag check (N22) is the only enum-specific access rule.
    let imported_enums: Vec<(String, Vec<(String, Option<Ty>)>)> = imports
        .enums
        .iter()
        .map(|(n, v)| (n.clone(), v.clone()))
        .collect();
    let mut enum_storage: Vec<(String, Vec<(String, Option<Ty>)>)> = imported_enums;
    for e in &p.enums {
        if !e.tparams.is_empty() {
            continue; // N19-style template — only clones register.
        }
        enum_storage.push((e.name.clone(), e.variants.clone()));
    }
    // CHAN-004 S3b — bounded-send results: for every `bchannel T`
    // the program mentions, register the `Result<void,T>` instance
    // `send` returns so `match`/`?` resolve `Ok`/`Err` whether or
    // not `std.result` is imported. An imported real instantiation
    // (identical variants) wins; the synthetic entry is a no-op.
    for payload in bchan_payloads(p) {
        let n = send_result_name(&payload);
        if !enum_storage.iter().any(|(en, _)| *en == n) {
            enum_storage.push((
                n,
                vec![
                    ("Ok".to_string(), Some(Ty::Void)),
                    ("Err".to_string(), Some(payload)),
                ],
            ));
        }
    }
    let mut enums: Enums = HashMap::new();
    for (n, vs) in &enum_storage {
        // Clone names may collide between an imported interface and a
        // local instantiation — same layout, no error (see structs).
        if enums.insert(n.clone(), vs).is_some() && !n.contains('<') {
            push_decl_diag(&mut errs, Diag::new(
                Span { line: 0, col: 0 },
                format!("duplicate enum `{n}`"),
            ));
        }
    }
    let mut enum_pseudo: Vec<(String, Vec<(String, Ty)>)> = Vec::new();
    for (n, vs) in &enum_storage {
        enum_pseudo.push((
            n.clone(),
            vs.iter()
                .map(|(vn, vt)| (vn.clone(), vt.clone().unwrap_or(Ty::Void)))
                .collect(),
        ));
    }
    // Register layouts BEFORE checking payload types — a payload may
    // name another enum (`Maybe<Maybe<i32>>`), and its name must
    // already resolve.
    for (n, fields) in &enum_pseudo {
        structs.insert(n.clone(), fields);
    }
    for e in &p.enums {
        if !e.tparams.is_empty() {
            continue;
        }
        let mut seen = HashSet::new();
        for (vn, vt) in &e.variants {
            if !seen.insert(vn) {
                push_decl_diag(&mut errs, Diag::new(
                    e.span,
                    format!("duplicate variant `{vn}` in enum `{}`", e.name),
                ));
            }
            if let Some(t) = vt {
                check_ty(t, &structs, e.span, &mut errs);
            }
        }
    }
    // Enum payload cycles are infinite layout too — `enum E { A(E) }`.
    for e in &p.enums {
        if !e.tparams.is_empty() {
            continue;
        }
        for (_, vt) in &e.variants {
            if let Some(t) = vt {
                if reaches_struct(t, &e.name, &structs, &mut HashSet::new()) {
                    push_decl_diag(&mut errs, Diag::new(
                        e.span,
                        format!(
                            "enum `{}` contains itself by value — wrap in `box`",
                            e.name
                        ),
                    ));
                }
            }
        }
    }

    // Field/param/annotation types must resolve; structs may not
    // contain themselves by value (no pointers yet — infinite layout).
    for s in &p.structs {
        if !s.tparams.is_empty() {
            continue;
        }
        for (fn_, ft) in &s.fields {
            check_ty(ft, &structs, s.span, &mut errs);
            if reaches_struct(ft, &s.name, &structs, &mut HashSet::new()) {
                push_decl_diag(&mut errs, Diag::new(
                    s.span,
                    format!("field `{fn_}` makes struct `{}` recursive", s.name),
                ));
            }
        }
    }

    // Function signatures — calls may precede definitions. Imported
    // signatures seed the same map, so `m.f` calls look up exactly
    // like local ones.
    let mut fns: HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)> = imports.fns.clone();
    // Names this module declares — the set a builtin name defers to.
    // Externs count: `extern fn close` means the module really does
    // want libc close, not the channel builtin (N21B).
    let local: HashSet<String> = p.fns.iter().map(|f| f.name.clone()).collect();
    for f in &p.fns {
        if !f.tparams.is_empty() {
            continue; // N19 — template: instantiated clones are the fns.
        }
        for (_, _, t) in &f.params {
            check_ty(t, &structs, f.span, &mut errs);
        }
        check_ty(&f.ret, &structs, f.span, &mut errs);
        if f.is_extern {
            // A C-ABI boundary carries plain values only — pointers
            // and scalars. Capabilities and language-only types can't
            // cross: authority is claimed by `*T`/`*const T` instead.
            for (pn, auth, t) in &f.params {
                if auth.is_some() {
                    push_decl_diag(&mut errs, Diag::new(
                        f.span,
                        format!(
                            "extern `{n}` parameter `{pn}` takes no capability — \
                             use `*T` (writable) or `*const T`",
                            n = f.name
                        ),
                    ));
                }
                if !is_ffi_ty(t) {
                    push_decl_diag(&mut errs, Diag::new(
                        f.span,
                        format!(
                            "extern `{n}` parameter `{pn}` has non-FFI type `{t}`",
                            n = f.name
                        ),
                    ));
                }
            }
            if f.ret != Ty::Void && !is_ffi_ty(&f.ret) {
                push_decl_diag(&mut errs, Diag::new(
                    f.span,
                    format!("extern `{}` returns non-FFI type `{}`", f.name, f.ret),
                ));
            }
        }
        let sig = (
            f.params.iter().map(|(_, a, t)| (*a, t.clone())).collect(),
            f.ret.clone(),
            f.is_extern,
        );
        // Two modules may declare the same extern (`mml_errno` in
        // fs.lang AND net.lang) — identical sigs dedupe like C's
        // redeclarations; a sig mismatch stays a hard duplicate.
        match fns.get(&f.name) {
            Some(prev) if f.is_extern && *prev == sig => {}
            _ => {
                if fns.insert(f.name.clone(), sig).is_some() {
                    push_decl_diag(&mut errs, Diag::new(f.span, format!("duplicate function `{}`", f.name)));
                }
            }
        }
    }
    // N36B — the declaration table: local decls + the direct imports'
    // exported sites. `index::enter_module` makes it the table every
    // use-site record resolves through; decl occs land in the same
    // batch the pipeline stamps with this module's file.
    {
        let mut locs: HashMap<String, DefLoc> = imports.locs.clone();
        for f in &p.fns {
            locs.insert(
                f.name.clone(),
                DefLoc {
                    span: f.span,
                    file: None,
                    sig: fn_sig(f),
                    kind: "fn",
                    fields: Vec::new(),
                    variants: Vec::new(),
                },
            );
            index::record(index::Occ {
                span: f.span,
                name: f.name.clone(),
                kind: "fn",
                hover: fn_sig(f),
                def: None,
                file: None,
            });
        }
        for st in &p.structs {
            locs.entry(st.name.clone()).or_insert(DefLoc {
                span: st.span,
                file: None,
                sig: format!("struct `{}`", st.name),
                kind: "struct",
                fields: st
                    .fields
                    .iter()
                    .map(|(n, t)| (n.clone(), format!("{t}")))
                    .collect(),
                variants: Vec::new(),
            });
            index::record(index::Occ {
                span: st.span,
                name: st.name.clone(),
                kind: "struct",
                hover: format!("struct `{}`", st.name),
                def: None,
                file: None,
            });
        }
        for e in &p.enums {
            locs.insert(
                e.name.clone(),
                DefLoc {
                    span: e.span,
                    file: None,
                    sig: format!("enum `{}`", e.name),
                    kind: "enum",
                    fields: Vec::new(),
                    variants: e.variants.iter().map(|(n, _)| n.clone()).collect(),
                },
            );
            index::record(index::Occ {
                span: e.span,
                name: e.name.clone(),
                kind: "enum",
                hover: format!("enum `{}`", e.name),
                def: None,
                file: None,
            });
        }
        index::enter_module(locs);
    }

    // N17 — a `pub` signature may only name types importers can also
    // see. A `pub fn` returning a private `m.Hidden` would hand out a
    // value callers can't name, inspect, or free correctly.
    let private_structs: HashSet<String> = p
        .structs
        .iter()
        .filter(|s| !s.is_pub && s.tparams.is_empty())
        .map(|s| s.name.clone())
        .chain(
            p.enums
                .iter()
                .filter(|e| !e.is_pub && e.tparams.is_empty())
                .map(|e| e.name.clone()),
        )
        .collect();
    for f in &p.fns {
        if !f.is_pub || f.is_extern || !f.tparams.is_empty() {
            continue;
        }
        for t in f
            .params
            .iter()
            .map(|(_, _, t)| t)
            .chain(std::iter::once(&f.ret))
        {
            if let Some(n) = ty_mentions(t, &private_structs) {
                push_decl_diag(&mut errs, Diag::new(
                    f.span,
                    format!(
                        "`pub fn {}` exposes private struct `{n}` in its signature",
                        f.name
                    ),
                ));
            }
        }
    }
    for s in &p.structs {
        if !s.is_pub || !s.tparams.is_empty() {
            continue;
        }
        for (fn_, ft) in &s.fields {
            if let Some(n) = ty_mentions(ft, &private_structs) {
                push_decl_diag(&mut errs, Diag::new(
                    s.span,
                    format!("`pub struct {}` field `{fn_}` has private type `{n}`", s.name),
                ));
            }
        }
    }
    // N8 ordering: callee-first so every non-cyclic callee contributes
    // a summary. A fn on a call cycle gets checked but its summary is
    // withheld — its callers fall back to arg-union approximation.
    let calls: HashMap<String, HashSet<String>> = p
        .fns
        .iter()
        .filter(|f| f.tparams.is_empty())
        .map(|f| (f.name.clone(), calls_in(&f.body)))
        .collect();
    let mut order: Vec<&FnDef> = Vec::new();
    let mut cyclic: HashSet<String> = HashSet::new();
    {
        // postorder DFS; a back-edge marks the SCC cyclic.
        let mut state: HashMap<String, u8> = HashMap::new();
        let fn_names: HashSet<String> = p
            .fns
            .iter()
            .filter(|f| f.tparams.is_empty())
            .map(|f| f.name.clone())
            .collect();
        fn dfs(
            n: &str,
            calls: &HashMap<String, HashSet<String>>,
            fns: &HashSet<String>,
            state: &mut HashMap<String, u8>,
            cyclic: &mut HashSet<String>,
            order_names: &mut Vec<String>,
        ) {
            state.insert(n.to_string(), 1);
            if let Some(cs) = calls.get(n) {
                for c in cs {
                    if !fns.contains(c) {
                        continue;
                    }
                    match state.get(c).copied().unwrap_or(0) {
                        0 => dfs(c, calls, fns, state, cyclic, order_names),
                        1 => {
                            cyclic.insert(n.to_string());
                            cyclic.insert(c.clone());
                        }
                        _ => {}
                    }
                }
            }
            state.insert(n.to_string(), 2);
            order_names.push(n.to_string());
        }
        let mut order_names = Vec::new();
        for f in &p.fns {
            if !f.tparams.is_empty() {
                continue;
            }
            if state.get(&f.name).copied().unwrap_or(0) == 0 {
                dfs(
                    &f.name,
                    &calls,
                    &fn_names,
                    &mut state,
                    &mut cyclic,
                    &mut order_names,
                );
            }
        }
        for n in order_names {
            if let Some(f) = p.fns.iter().find(|f| f.name == n) {
                order.push(f);
            }
        }
    }
    // Imported summaries seed the same map — `m.f` call sites replay
    // them through the ordinary arg-translation path (N17).
    let mut sums: HashMap<String, FnSummary> = imports.sums.clone();
    for f in order {
        if f.is_extern {
            continue; // declarations have no body to check.
        }
        let sum = check_fn(
            f,
            &fns,
            &local,
            &structs,
            &enums,
            &sums,
            &imports.private,
            &mut errs,
        );
        if !cyclic.contains(&f.name) {
            sums.insert(f.name.clone(), sum);
        }
    }
    // N17 — build the interface: the `pub` slice of this module's
    // signatures, summaries, and layouts. Private names are recorded
    // so importers report "private", not "undefined".
    for s in &p.structs {
        if !s.tparams.is_empty() {
            continue; // templates aren't interface members — their
                      // instantiated clones export like any decl.
        }
        if s.is_pub || s.name.contains('<') {
            // Instantiated clones aren't `pub`, but their canonical
            // name makes the layout a global fact — without this, an
            // importer couldn't resolve `m.f<T>`-typed fields.
            out.structs.insert(s.name.clone(), s.fields.clone());
            if !s.is_pub {
                continue;
            }
            out.locs.insert(
                s.name.clone(),
                DefLoc {
                    span: s.span,
                    file: None,
                    sig: format!("struct `{}`", s.name),
                    kind: "struct",
                    fields: s
                        .fields
                        .iter()
                        .map(|(n, t)| (n.clone(), format!("{t}")))
                        .collect(),
                    variants: Vec::new(),
                },
            );
        } else {
            out.private.insert(s.name.clone());
        }
    }
    for e in &p.enums {
        if !e.tparams.is_empty() {
            continue;
        }
        if e.is_pub || e.name.contains('<') {
            // Clone layouts are global facts — see the struct arm.
            out.enums.insert(e.name.clone(), e.variants.clone());
            if !e.is_pub {
                continue;
            }
            out.locs.insert(
                e.name.clone(),
                DefLoc {
                    span: e.span,
                    file: None,
                    sig: format!("enum `{}`", e.name),
                    kind: "enum",
                    fields: Vec::new(),
                    variants: e.variants.iter().map(|(n, _)| n.clone()).collect(),
                },
            );
        } else {
            out.private.insert(e.name.clone());
        }
    }
    for f in &p.fns {
        if !f.tparams.is_empty() {
            continue;
        }
        if f.is_extern {
            // Externs name global C symbols, not module members —
            // every importer sees them (and instantiated clones
            // legitimately call the owner's externs, N19).
            out.fns
                .insert(f.name.clone(), fns.get(&f.name).unwrap().clone());
            out.locs.insert(
                f.name.clone(),
                DefLoc {
                    span: f.span,
                    file: None,
                    sig: fn_sig(f),
                    kind: "fn",
                    fields: Vec::new(),
                    variants: Vec::new(),
                },
            );
            continue;
        }
        if f.is_pub {
            out.fns.insert(f.name.clone(), fns.get(&f.name).unwrap().clone());
            if let Some(sum) = sums.get(&f.name) {
                out.sums.insert(f.name.clone(), sum.clone());
            }
            out.locs.insert(
                f.name.clone(),
                DefLoc {
                    span: f.span,
                    file: None,
                    sig: fn_sig(f),
                    kind: "fn",
                    fields: Vec::new(),
                    variants: Vec::new(),
                },
            );
        } else {
            out.private.insert(f.name.clone());
        }
    }
    if errs.is_empty() {
        Ok(())
    } else {
        Err(errs)
    }
}

/// Function names called (transitively inside the body), for the
/// callee-first summary order.
fn calls_in(stmts: &[Stmt]) -> HashSet<String> {
    fn expr(e: &Expr, out: &mut HashSet<String>) {
        match e {
            Expr::Call(n, _tg, args, _) => {
                out.insert(n.clone());
                for a in args {
                    expr(a, out);
                }
            }
            Expr::Unary(_, i, _) | Expr::Deref(i, _) | Expr::Handle(i, _) => expr(i, out),
            Expr::Bin(_, a, b, _) => {
                expr(a, out);
                expr(b, out);
            }
            Expr::Field(b, ..) => expr(b, out),
            Expr::Index(b, i, _) => {
                expr(b, out);
                expr(i, out);
            }
            Expr::IndexRange { base, lo, hi, .. } => {
                expr(base, out);
                for e in lo.iter().chain(hi.iter()) {
                    expr(e, out);
                }
            }
            Expr::AddrOf { place, .. } => expr(place, out),
            Expr::Alloc { alloc, count, .. } => {
                expr(alloc, out);
                if let Some(c) = count {
                    expr(c, out);
                }
            }
            Expr::StructLit(_, _tg, fields, _) => {
                for (_, fe) in fields {
                    expr(fe, out);
                }
            }
            Expr::EnumLit { payload, .. } => {
                if let Some(p) = payload {
                    expr(p, out);
                }
            }
            Expr::Try { inner, .. } => expr(inner, out),
            _ => {}
        }
    }
    let mut out = HashSet::new();
    fn go(stmts: &[Stmt], out: &mut HashSet<String>) {
        for s in stmts {
            match s {
                Stmt::Let { init, .. } => {
                    if let Some(i) = init {
                        expr(i, out);
                    }
                }
                Stmt::Assign { target, value, .. } => {
                    expr(target, out);
                    expr(value, out);
                }
                Stmt::Expr { expr: e, .. }
                | Stmt::Consume { target: e, .. }
                | Stmt::Promote { target: e, .. }
                | Stmt::Defer { expr: e, .. } => expr(e, out),
                Stmt::Return { value, .. } => {
                    if let Some(v) = value {
                        expr(v, out);
                    }
                }
                Stmt::If {
                    cond,
                    then_body,
                    else_body,
                    ..
                } => {
                    expr(cond, out);
                    go(then_body, out);
                    if let Some(eb) = else_body {
                        go(eb, out);
                    }
                }
                Stmt::While { cond, body, .. } => {
                    expr(cond, out);
                    go(body, out);
                }
                Stmt::Match { scrut, arms, .. } => {
                    expr(scrut, out);
                    for a in arms {
                        go(&a.body, out);
                    }
                }
                Stmt::Region { body, .. } => go(body, out),
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
        }
    }
    go(stmts, &mut out);
    out
}

/// Types that may cross the C ABI — scalars and raw pointers only.
/// `*T`'s element may be anything (it lives behind the pointer);
/// language carriers (`&T`, `handle`, `task`, `mutex`, `channel`,
/// aggregates) are refused at the declaration.
pub(crate) fn is_ffi_ty(t: &Ty) -> bool {
    matches!(
        t,
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
            | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
            | Ty::Bool
            | Ty::Ptr { .. }
    )
}

fn check_ty(t: &Ty, structs: &Structs, span: Span, errs: &mut Vec<Diag>) {
    match t {
        Ty::Struct(n) if !structs.contains_key(n) => {
            push_decl_diag(errs, Diag::new(span, format!("unknown struct `{n}`")));
        }
        Ty::Array(e, _) => check_ty(e, structs, span, errs),
        Ty::Ref { elem, .. } => check_ty(elem, structs, span, errs),
        Ty::Handle(e) | Ty::Task(e) | Ty::Atomic(e) | Ty::Mutex(e) | Ty::Channel(e)
        | Ty::BChannel(e) => check_ty(e, structs, span, errs),
        Ty::Ptr { elem, .. } => check_ty(elem, structs, span, errs),
        Ty::Slice { elem, .. } => check_ty(elem, structs, span, errs),
        // `box T`/`box slice T` — the indirection is what makes
        // recursive structures legal: `struct Node { next: box Node }`
        // embeds a pointer, not a value.
        Ty::Box(e) | Ty::BoxSlice(e) => check_ty(e, structs, span, errs),
        // Guards are scope-bound authority — lock-produced, never a
        // declared field/param/return type.
        Ty::Guard(_) => push_decl_diag(errs, Diag::new(
            span,
            "`guard` is not a declarable type — it comes from `lock`".to_string(),
        )),
        _ => {}
    }
}

/// N17 — the first private-struct name reachable through `t`, if any:
/// used to keep `pub` signatures from exposing module-private types.
fn ty_mentions(t: &Ty, private: &HashSet<String>) -> Option<String> {
    match t {
        // Instantiated names (`m.Vec<i32>`) are exempt: visibility was
        // enforced at instantiation — if the clone exists, the
        // requester was already allowed to name it (N19).
        Ty::Struct(n) if private.contains(n) && !n.contains('<') => Some(n.clone()),
        Ty::Array(e, _)
        | Ty::Ref { elem: e, .. }
        | Ty::Handle(e)
        | Ty::Task(e)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::BChannel(e)
        | Ty::Ptr { elem: e, .. }
        | Ty::Slice { elem: e, .. }
        | Ty::Box(e)
        | Ty::BoxSlice(e) => ty_mentions(e, private),
        _ => None,
    }
}

/// Does `t` (transitively, through fields/elems) embed `name`?
/// A `&T` edge does NOT embed — references break the value cycle.
fn reaches_struct(t: &Ty, name: &str, structs: &Structs, seen: &mut HashSet<String>) -> bool {
    match t {
        Ty::Array(e, _) => reaches_struct(e, name, structs, seen),
        // Refs, handles and task tokens are pointers — none embeds a
        // value cycle. `atomic T` stores T inline — it CAN embed.
        Ty::Ref { .. }
        | Ty::Handle(_)
        | Ty::Task(_)
        | Ty::Ptr { .. }
        | Ty::Slice { .. }
        | Ty::Str => false,
        Ty::Atomic(e) => reaches_struct(e, name, structs, seen),
        Ty::Struct(n) => {
            if n == name {
                return true;
            }
            if !seen.insert(n.clone()) {
                return false;
            }
            match structs.get(n) {
                Some(fields) => fields
                    .iter()
                    .any(|(_, ft)| reaches_struct(ft, name, structs, seen)),
                None => false,
            }
        }
        _ => false,
    }
}

/// A literal can adopt any int type it fits; everything else must match
/// exactly. `want == None` means "just infer".
fn coerce(want: &Ty, e: &Expr, got: &Ty, errs: &mut Vec<Diag>) -> Ty {
    if *want == *got {
        return got.clone();
    }
    // `slice mut T` weakens to `slice T` — the edge keeps Write;
    // the type is a view restriction. Never the other way.
    if let (
        Ty::Slice {
            mutable: wm,
            elem: we,
        },
        Ty::Slice {
            mutable: gm,
            elem: ge,
        },
    ) = (want, got)
    {
        if we == ge && (!*wm || *gm) {
            return want.clone();
        }
    }
    // A fresh `channel()`/`bchannel()` is unbound — `channel void`
    // takes whatever payload the annotation declares.
    if let (Ty::Channel(w), Ty::Channel(g)) = (want, got) {
        if **g == Ty::Void || w == g {
            return want.clone();
        }
    }
    if let (Ty::BChannel(w), Ty::BChannel(g)) = (want, got) {
        if **g == Ty::Void || w == g {
            return want.clone();
        }
    }
    // `*T` weakens to `*void`/`*const void` — C's rule: any object
    // pointer is a void pointer. Adding const is free (`*mut T` →
    // `*const void`); dropping it is never allowed.
    if let (
        Ty::Ptr {
            mutable: wm,
            elem: we,
        },
        Ty::Ptr { mutable: gm, .. },
    ) = (want, got)
    {
        if **we == Ty::Void && (!*wm || *gm) {
            return want.clone();
        }
    }
    // `*mut T` weakens to `*const T` — same as `slice mut` → `slice`:
    // the capability narrows, the bits don't change.
    if let (
        Ty::Ptr {
            mutable: false,
            elem: we,
        },
        Ty::Ptr {
            mutable: true,
            elem: ge,
        },
    ) = (want, got)
    {
        if we == ge {
            return want.clone();
        }
    }
    // Atomicity is a storage discipline: at the value level
    // `atomic T` and `T` are the same bits.
    if want.unatomic() == got.unatomic() {
        return want.clone();
    }
    // A literal can BE the string view where `str` is wanted —
    // the global's (ptr, len) pair, not just the pointer.
    if let (Ty::Str, Expr::Str(..)) = (want, e) {
        return want.clone();
    }
    // A string literal is a `*const u8` global — `*const void` takes
    // it too; a mutable `*T` never does (literal storage is read-only).
    if let (Ty::Ptr { mutable: false, elem }, Expr::Str(..)) = (want, e) {
        if **elem == Ty::U8 || **elem == Ty::Void {
            return want.clone();
        }
    }
    // A literal `0` is C's null pointer constant.
    if let (Ty::Ptr { .. }, Expr::Int(0, _)) = (want, e) {
        return want.clone();
    }
    if let Expr::Int(n, _) = e {
        if let Some((lo, hi)) = want.range() {
            if *n >= lo && *n <= hi {
                return want.clone();
            }
        }
    }
    // N29 — a pure-literal TREE adopts `want` like a bare literal:
    // `x != -128` peer-types the Neg, `f(100 + 28)` folds at the
    // param type. The i128 fold keeps u64's top half honest.
    if let Some(v) = const_int(e, want) {
        if let Some((lo, hi)) = const_range(want) {
            if v >= lo && v <= hi {
                return want.clone();
            }
            push_decl_diag(errs, Diag::new(
                e.span(),
                format!("constant expression folds to {v}, which does not fit {want}"),
            ));
            return want.clone();
        }
    }
    push_decl_diag(errs, Diag::new(
        e.span(),
        format!("type mismatch: expected `{want}`, found `{got}`"),
    ));
    want.clone()
}

/// N29 — the mathematical value of a pure-literal int tree computed
/// at `ty`'s semantics, in i128 so intermediates can't themselves
/// wrap. `None` = a non-literal or an op the folder doesn't model —
/// the emitted code decides those.
///
/// `~` and `>>` are width/sign-sensitive, so `ty` is threaded: `~v`
/// is `hi - v` for unsigned (complement = subtract from all-ones)
/// and `-v - 1` for signed; `>>` is an i128 arithmetic shift, which
/// matches both logical shifts on nonnegatives and arithmetic
/// shifts on negatives at any width.
/// The true bit-range of an int type in i128 — `Ty::range` caps u64
/// at i64::MAX because that's all a *spelled* literal can be, but a
/// folded value (`1 << 63`) legitimately fills the full u64 domain.
fn const_range(ty: &Ty) -> Option<(i128, i128)> {
    let (lo, hi) = ty.unatomic().range()?;
    Some(match ty.unatomic() {
        Ty::U64 => (0, u64::MAX as i128),
        _ => (lo as i128, hi as i128),
    })
}

fn const_int(e: &Expr, ty: &Ty) -> Option<i128> {
    let unsigned = const_range(ty).map_or(false, |(lo, _)| lo >= 0);
    match e {
        Expr::Int(n, _) => Some(*n as i128),
        Expr::Unary(UnOp::Neg, i, _) => const_int(i, ty)?.checked_neg(),
        Expr::Unary(UnOp::BitNot, i, _) => {
            let v = const_int(i, ty)?;
            if unsigned {
                // hi - v: the complement at ty's width. v ⊆ [0, hi]
                // already (literals fit-check before the fold).
                Some(const_range(ty)?.1 - v)
            } else {
                v.checked_neg()?.checked_sub(1)
            }
        }
        Expr::Bin(op, a, b, _) => {
            let x = const_int(a, ty)?;
            let y = const_int(b, ty)?;
            match op {
                BinOp::Add => x.checked_add(y),
                BinOp::Sub => x.checked_sub(y),
                BinOp::Mul => x.checked_mul(y),
                // Division by zero is diagnosed at the Bin site; the
                // folder just declines to model it.
                BinOp::Div | BinOp::Mod => {
                    if y == 0 {
                        None
                    } else if matches!(op, BinOp::Div) {
                        x.checked_div(y)
                    } else {
                        x.checked_rem(y)
                    }
                }
                BinOp::Shl => u32::try_from(y).ok().and_then(|k| x.checked_shl(k)),
                BinOp::Shr => u32::try_from(y).ok().and_then(|k| x.checked_shr(k)),
                BinOp::BitAnd => Some(x & y),
                BinOp::BitOr => Some(x | y),
                BinOp::BitXor => Some(x ^ y),
                _ => None,
            }
        }
        _ => None,
    }
}

/// The single region rule: may a reference with provenance `f` be
/// stored where a lifetime of `d` is required? Yes iff f's region is
/// d's ancestor-or-equal (the referent outlives the location), or f is
/// caller provenance, which outlives everything inside the call.
fn outlives(f: &Prov, d: &Prov, regs: &[RegionFrame]) -> bool {
    match (f, d) {
        (Prov::Unknown, _) => true, // uninitialized is diagnosed separately
        (_, Prov::Param) => matches!(f, Prov::Param),
        (Prov::Param, _) => true,
        (Prov::Region(a), Prov::Region(b)) => is_ancestor_or_eq(*a, *b, regs),
        (Prov::Region(_), Prov::Unknown) => true,
    }
}

/// Region of the store location an expression addresses — what a value
/// must outlive to be stored there. `x`, `p.f`, `a[i]` share the root
/// local's owner; `*r` requires the pointee's own provenance.
fn place_req(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> Prov {
    match e {
        Expr::Var(n, _) => match lookup(scopes, n) {
            // A capability param aliases caller storage — references
            // derived from it legitimately escape this frame.
            Some(b) if b.auth.is_some() => Prov::Param,
            Some(b) => Prov::Region(b.owner),
            None => Prov::Unknown,
        },
        Expr::Field(b, ..) => place_req(b, scopes, ctx),
        Expr::Index(b, ..) => {
            // `s[i]` names storage the SLICE points at — the required
            // lifetime is the slice's own provenance (like `*r`), not
            // the slot's owner.
            if matches!(expr_ty_at(b, scopes, ctx).0, Ty::Slice { .. }) {
                expr_tv(b, scopes, ctx).1
            } else {
                place_req(b, scopes, ctx)
            }
        }
        Expr::IndexRange { base, .. } => place_req(base, scopes, ctx),
        Expr::Deref(inner, _) => expr_tv(inner, scopes, ctx).1,
        _ => Prov::Unknown,
    }
}

/// Type + provenance of `p.f` / `a[i]` / `*r` path tails.
fn path_ty_prov(
    base: (Ty, Prov),
    e: &Expr,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    match e {
        Expr::Field(b, name, s) => {
            let t = field_ty(&base.0, name, ctx.structs, *s, &mut ctx.errs);
            index::record(index::Occ {
                span: *s,
                name: name.clone(),
                kind: "field",
                hover: format!("`{name}`: {t}"),
                def: if let Ty::Struct(en) = &base.0 {
                    index::def(en).map(|d| (d.span, d.file.clone()))
                } else {
                    None
                },
                file: None,
            });
            // N22 — a variant payload is a place only while the tag
            // statically holds it; `match` is how you learn it.
            if let Ty::Struct(en) = &base.0 {
                if let Some(vars) = ctx.enums.get(en).cloned() {
                    if let Some((_, pt)) = vars.iter().find(|(vn, _)| vn == name) {
                        if pt.is_none() {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("variant `{name}` of `{en}` carries no payload"),
                            ));
                        } else {
                            for k in resolve_place(b, scopes, ctx) {
                                match ctx.tag.get(&k) {
                                    Some(Some(v)) if v == name => {}
                                    _ => push_diag(ctx, Diag::new(
                                        *s,
                                        format!(
                                            "cannot access `{name}` payload: active variant is not statically `{name}` — `match` on it first"
                                        ),
                                    )),
                                }
                            }
                        }
                    }
                }
            }
            let prov = match place_id(e, scopes) {
                Some(k) => {
                    use_check(&k, e, ctx);
                    match root_guard(e, scopes) {
                        Some((gk, dom, _, srcs)) => {
                            guarded_check(e, &k, false, &gk, &dom, &srcs, scopes, ctx)
                        }
                        None => {
                            unguarded_check(e, &k, scopes, ctx);
                            transit_check(e, &k, ctx);
                            record_hold(ctx, &k, Auth::Read);
                        }
                    }
                    let moved_view = match &t {
                        Ty::Ref { .. } => {
                            prov_of(&ctx.path_provs, &k) != Prov::Unknown
                                && !live_out_edge(ctx, &k)
                        }
                        // N52 — a `slice` field's claim moves out on
                        // `let s = v.buf` (edge_kills marks the move;
                        // capability-param fields never carry their
                        // own edge, so the kill marker distinguishes
                        // moved from param-structural dead).
                        Ty::Slice { .. } => {
                            ctx.edge_kills.contains_key(&k)
                                && !ctx.foreign.contains(&k)
                                && !live_out_edge(ctx, &k)
                        }
                        _ => false,
                    };
                    if moved_view {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("use of moved reference `{}`", target_name(e)),
                        ));
                    }
                    prov_of(&ctx.path_provs, &k)
                }
                None => Prov::Unknown,
            };
            if matches!(&t, Ty::Struct(en) if ctx.enums.contains_key(en)) {
                    ctx.last_tag = Some(
                    place_id(e, scopes)
                        .and_then(|k| ctx.tag.get(&k).cloned().flatten()),
                );
            }
            (t, prov)
        }
        Expr::Index(base_e, idx, s) => {
            let it = expr_tv(idx, scopes, ctx).0;
            if !it.is_int() {
                push_diag(ctx, Diag::new(
                    idx.span(),
                    format!("array index must be an integer, found {it}"),
                ));
            }
            match &base.0 {
                Ty::Array(elem, n) => {
                    if let Expr::Int(k, s) = &**idx {
                        if *k < 0 || *k >= *n as i64 {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("index {k} out of bounds for [{n}]"),
                            ));
                        }
                    }
                    let prov = match place_id(e, scopes) {
                        Some(k) => {
                            use_check(&k, e, ctx);
                            match root_guard(e, scopes) {
                                Some((gk, dom, _, srcs)) => {
                                    guarded_check(e, &k, false, &gk, &dom, &srcs, scopes, ctx)
                                }
                                None => {
                                    unguarded_check(e, &k, scopes, ctx);
                                    transit_check(e, &k, ctx);
                                    record_hold(ctx, &k, Auth::Read);
                                }
                            }
                            ctx.path_provs.get(&k).cloned().unwrap_or(Prov::Unknown)
                        }
                        None => Prov::Unknown,
                    };
                    ((**elem).clone(), prov)
                }
                Ty::Slice { elem, .. } | Ty::BoxSlice(elem) => {
                    // `s[i]`/`p[i]` — the element read reaches the
                    // claimed span: constant indexes die here when the
                    // length is statically known; the read scans
                    // foreign edges on the resolved span keys with
                    // the view's own edge excluded (it IS authority).
                    if let Expr::Int(k, s) = &**idx {
                        if *k < 0 {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("negative index {k} into a slice"),
                            ));
                        } else if let Some(len) = slice_len_at(base_e, scopes, ctx) {
                            if *k >= len {
                                push_diag(ctx, Diag::new(
                                    *s,
                                    format!("index {k} out of bounds for slice of length {len}"),
                                ));
                            }
                        }
                    }
                    if let Some(k) = place_id(e, scopes) {
                        use_check(&k, e, ctx);
                    }
                    let holders = path_holders(e, scopes, ctx);
                    for t in resolve_place(e, scopes, ctx) {
                        unguarded_check(e, &t, scopes, ctx);
                        transit_check(e, &t, ctx);
                        record_hold(ctx, &t, Auth::Read);
                        for (h, f) in edges_to(ctx, &t) {
                            if holders.contains(&h) {
                                continue;
                            }
                            if f.kind != Auth::Read {
                                let mut d = Diag::new(
                                    *s,
                                    format!(
                                        "cannot read `{}`: it is mutably borrowed",
                                        target_name(e)
                                    ),
                                );
                                if let Some(l) = born_label(&h, &f, ctx) {
                                    d.labels.push(l);
                                }
                                push_diag(ctx, d);
                                break;
                            }
                        }
                    }
                    ((**elem).clone(), Prov::Param)
                }
                t => {
                    push_diag(ctx, Diag::new(*s, format!("indexing non-array {t}")));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        _ => unreachable!(),
    }
}

fn field_ty(
    base_ty: &Ty,
    name: &str,
    structs: &Structs,
    span: Span,
    errs: &mut Vec<Diag>,
) -> Ty {
    match base_ty {
        // Storage classes are transparent to field lookup — `g.x`
        // sees the payload's fields; protection is checked elsewhere.
        Ty::Guard(inner) | Ty::Mutex(inner) => {
            field_ty(inner, name, structs, span, errs)
        }
        Ty::Slice { .. } => {
            push_decl_diag(errs, Diag::new(
                span,
                format!("slice member `.{name}` is a computed view, not a place"),
            ));
            Ty::Void
        }
        Ty::Str => {
            push_decl_diag(errs, Diag::new(
                span,
                format!("str member `.{name}` is a computed view, not a place"),
            ));
            Ty::Void
        }
        Ty::BoxSlice(_) => {
            push_decl_diag(errs, Diag::new(
                span,
                format!("box slice member `.{name}` is computed, not a place"),
            ));
            Ty::Void
        }
        Ty::Struct(sn) => match structs.get(sn) {
            Some(fields) => match fields.iter().find(|(fn_, _)| fn_ == name) {
                Some((_, t)) => t.clone(),
                None => {
                    push_decl_diag(errs, Diag::new(span, format!("no field `{name}` on struct `{sn}`")));
                    Ty::Void
                }
            },
            None => Ty::Void, // unknown struct — already reported
        },
        t => {
            push_decl_diag(errs, Diag::new(span, format!("field access on non-struct {t}")));
            Ty::Void
        }
    }
}

/// Every expression yields (type, provenance). Provenance is meaningful
/// only for reference-typed results; elsewhere it's Unknown and unused.
fn prov_of(m: &HashMap<String, Prov>, k: &str) -> Prov {
    m.get(k).cloned().unwrap_or(Prov::Unknown)
}

fn expr_tv(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    expr_tv_w(e, None, scopes, ctx)
}

/// `handle x`/`join x`/`mutex x` are builtin FORMS — the parser binds
/// them before call syntax, so `handle(c)` can never call a fn named
/// `handle` declared in this file (qualified `m.handle(..)` still
/// can). A silent bind is exactly the hidden-behavior class this
/// language forbids, so the use is an error when such a fn exists —
/// except inside that fn's own body, where `mutex x` is how
/// `sync.mutex` reaches the builtin it wraps.
fn builtin_shadow_diag(name: &str, span: Span, ctx: &mut Ctx) {
    if ctx.local_fns.contains(name) && ctx.regs[0].name != name {
        push_diag(ctx, Diag::new(
            span,
            format!(
                "`{name}` is a builtin form here, not a call to the fn `{name}` \
                 declared in this module — rename the fn (it stays callable \
                 qualified as `mod.{name}(..)`)"
            ),
        ));
    }
}

/// `want` is the bidirectional literal hint (N26): an expected int
/// type flows into literal-driven subexpressions, so `10 & 12`
/// under `i64` types both literals `i64`. Range-checked at the
/// `Expr::Int` leaf — `want` never coerces concrete operands.
fn expr_tv_w(e: &Expr, want: Option<&Ty>, scopes: &mut Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    expr_tv_go(e, want, scopes, ctx, true)
}

/// `init_ok` gates the N18 read-requirement: a place evaluated as a
/// PATH PREFIX (`p` in `p.f`, `s` in `s[i]`) isn't a value read — the
/// leaf carries the requirement, so prefix evals pass `false`.
fn expr_tv_go(
    e: &Expr,
    want: Option<&Ty>,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
    init_ok: bool,
) -> (Ty, Prov) {
    if init_ok
        && matches!(
            e,
            Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..)
        )
    {
        require_init(e, scopes, ctx);
    }
    if let Expr::Call(n, _, _, s) = e {
        rec(format!("(call {} @{}:{})", qs(n), s.line, s.col));
    }
    match e {
        Expr::Int(n, s) => {
            // A literal IS whatever int type the context wants —
            // provided it fits. `atomic T` context means plain `T`
            // here; the atomic-ness lives on the storage side.
            let w = want.map(|t| t.unatomic());
            match w {
                Some(t) if t.is_int() => {
                    if let Some((lo, hi)) = t.range() {
                        if *n < lo || *n > hi {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("integer literal {n} does not fit {t}"),
                            ));
                        }
                    }
                    (t, Prov::Unknown)
                }
                _ => (Ty::INT_DEFAULT, Prov::Unknown),
            }
        }
        Expr::Bool(..) => (Ty::Bool, Prov::Unknown),
        // `sizeof<T>` — a compile-time layout fact, not a read.
        Expr::Sizeof(..) => (Ty::U64, Prov::Unknown),
        // N20 — sugar that mono desugars; a survivor means its
        // desugar already diagnosed (unresolved method/assoc fn).
        Expr::MethodCall { .. } | Expr::AssocFn { .. } => (Ty::Void, Prov::Unknown),
        // `"..."` — a `*const u8` backed by a private global; opaque,
        // unborrowed, no authority over anything.
        Expr::Str(..) => (
            Ty::Ptr {
                mutable: false,
                elem: Box::new(Ty::U8),
            },
            Prov::Unknown,
        ),
        Expr::Var(n, s) => match lookup(scopes, n).cloned() {
            Some(b) => {
                index::record(index::Occ {
                    span: *s,
                    name: n.clone(),
                    kind: if b.auth.is_some() { "param" } else { "local" },
                    hover: format!("`{n}`: {}", b.ty),
                    def: Some((b.decl, None)),
                    file: None,
                });
                let key = format!("{n}#{}", b.epoch);
                if consumed_state(ctx, &key).is_some() {
                    use_check(&key, e, ctx);
                } else if init_state(ctx, &key, &b.ty) == InitState::Init {
                    // A live, initialized value — the authority checks
                    // below (require_init already flagged the rest).
                    if matches!(b.ty, Ty::Ref { .. } | Ty::Slice { .. })
                        && b.auth.is_none()
                        && !ctx.foreign.contains(&key)
                    {
                        if !live_out_edge(ctx, &key) {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("use of moved reference `{n}`"),
                            ));
                        }
                    } else if b.auth.is_some() {
                        // Capability param: `n` reads through its edge
                        // to the caller's place. Dead edge = moved
                        // authority; live edge still conflicts with
                        // OTHER holders' Write.
                        let key = format!("{n}#{}", b.epoch);
                        if !live_out_edge(ctx, &key) {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("use of moved authority `{n}`"),
                            ));
                        } else {
                            for t in resolve_place(e, scopes, ctx) {
                                access_check_excl(e, &t, false, &key, scopes, ctx);
                            }
                        }
                    } else {
                        // Reading a plain place conflicts with a live
                        // `&mut` edge into it (exclusivity).
                        if let Some(key) = place_id(e, scopes) {
                            access_check(e, &key, false, scopes, ctx);
                        }
                    }
                }
                if matches!(&b.ty, Ty::Struct(en) if ctx.enums.contains_key(en)) {
                    ctx.last_tag = Some(ctx.tag.get(&key).cloned().unwrap_or(None));
                }
                (b.ty.clone(), b.prov.clone())
            }
            None => {
                push_diag(ctx, Diag::new(*s, format!("undefined variable `{n}`")));
                (Ty::Void, Prov::Unknown)
            }
        },
        Expr::Unary(op, inner, s) => {
            // `-1`/`~0xff` adopt the context type. `-` propagates
            // only to a SIGNED want — `let x: u8 = -1` must keep
            // rejecting, not silently wrap to 255.
            let uw = want.filter(|t| {
                let t = t.unatomic();
                match op {
                    UnOp::Neg => t.is_int() && t.range().map_or(false, |(lo, _)| lo < 0),
                    UnOp::BitNot => t.is_int(),
                    _ => false,
                }
            });
            // `-LIT` under signed `T` range-checks the MAGNITUDE —
            // -128 is a valid i8 even though +128 isn't, so the
            // literal's bound is `-min`, not `max`.
            if matches!(op, UnOp::Neg) {
                if let (Some(t), Expr::Int(n, ns)) = (uw, &**inner) {
                    // `-min` overflows on i64 — its magnitude bound is
                    // 2^63, which no positive i64 literal can reach.
                    if let Some(mag) = t.unatomic().range().unwrap().0.checked_neg() {
                        if *n > mag {
                            push_diag(ctx, Diag::new(
                                *ns,
                                format!("integer literal -{n} does not fit {t}"),
                            ));
                        }
                    }
                    return (t.unatomic(), Prov::Unknown);
                }
            }
            let t = expr_tv_w(inner, uw, scopes, ctx).0;
            match op {
                UnOp::Neg => {
                    if !t.is_int() {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("`-` requires an integer, found {t}"),
                        ));
                    }
                    (t, Prov::Unknown)
                }
                UnOp::Not => {
                    if t != Ty::Bool {
                        push_diag(ctx, Diag::new(*s, format!("`!` requires bool, found {t}")));
                    }
                    (Ty::Bool, Prov::Unknown)
                }
                UnOp::BitNot => {
                    if !t.is_int() {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("`~` requires an integer, found {t}"),
                        ));
                    }
                    (t, Prov::Unknown)
                }
            }
        }
        Expr::Bin(op @ (BinOp::LAnd | BinOp::LOr), a, b, s) => {
            // N25C — `a && b` evaluates b only when a held (`||`:
            // when it didn't). b's effects join against the state
            // where it never ran — the same cert discipline as an
            // `if` arm with an empty else.
            let sym = if *op == BinOp::LAnd { "&&" } else { "||" };
            let ta = expr_tv(a, scopes, ctx).0;
            if ta != Ty::Bool {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("`{sym}` requires bool operands, found {ta}"),
                ));
            }
            let saved_edges = ctx.edges.clone();
            let saved_caps = ctx.caps.clone();
            let saved_cons = ctx.consumed.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_ck = ctx.cap_kills.clone();
            let saved_paths = ctx.path_provs.clone();
            let saved_transit = ctx.transit.clone();
            let saved_chans = ctx.chan_queues.clone();
            let saved_out = ctx.out.clone();
            let tb = expr_tv(b, scopes, ctx).0;
            if tb != Ty::Bool {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("`{sym}` requires bool operands, found {tb}"),
                ));
            }
            let after_edges = std::mem::take(&mut ctx.edges);
            let after_caps = std::mem::take(&mut ctx.caps);
            let after_cons = std::mem::take(&mut ctx.consumed);
            let after_kills = std::mem::take(&mut ctx.edge_kills);
            let after_ck = std::mem::take(&mut ctx.cap_kills);
            let after_paths = std::mem::take(&mut ctx.path_provs);
            let after_transit = std::mem::take(&mut ctx.transit);
            let after_chans = std::mem::take(&mut ctx.chan_queues);
            let after_out = std::mem::take(&mut ctx.out);
            let mut merged_edges = saved_edges.clone();
            for (k, f) in after_edges.iter().chain(saved_edges.iter()) {
                let cert = match (after_edges.get(k), saved_edges.get(k)) {
                    (Some(t), Some(e)) if t.kind == e.kind && t.cert == Cert::Def && e.cert == Cert::Def => Cert::Def,
                    _ => Cert::Maybe,
                };
                merged_edges.insert(k.clone(), EdgeFact {
                    kind: f.kind,
                    cert,
                    born: after_edges.get(k).and_then(|x| x.born).or_else(|| saved_edges.get(k).and_then(|x| x.born)),
                });
            }
            ctx.edges = merged_edges;
            let mut merged_caps = saved_caps.clone();
            for (k, _) in after_caps.iter().chain(saved_caps.iter()) {
                let cert = match (after_caps.get(k), saved_caps.get(k)) {
                    (Some(Cert::Def), Some(Cert::Def)) => Cert::Def,
                    _ => Cert::Maybe,
                };
                merged_caps.insert(k.clone(), cert);
            }
            ctx.caps = merged_caps;
            ctx.consumed = join_consumed(&after_cons, &saved_cons);
            ctx.edge_kills = join_cert_map(&after_kills, &saved_kills);
            ctx.cap_kills = join_cert_map(&after_ck, &saved_ck);
            // Channel/transit may-state is a union on either path.
            ctx.transit = saved_transit.union(&after_transit).cloned().collect();
            // ⊥ arm views BEFORE `saved_out` moves into ctx — each
            // side's ⊥ membership lives in its own `out` snapshot.
            let saved_bots =
                holder_bots(&saved_edges, &saved_caps, &saved_out);
            let after_bots =
                holder_bots(&after_edges, &after_caps, &after_out);
            ctx.chan_queues = saved_chans;
            pending_symmetry(&[pend_keys(&saved_out), pend_keys(&after_out)], *s, ctx);
            ctx.out = saved_out;
            join_chan_may(ctx, &after_chans, &after_out);
            // ⊥ coalescing on arm-level evidence: ⊥s arriving from
            // DIFFERENT arms are mutually exclusive alternatives;
            // ⊥s co-resident on one arm are independent positions.
            coalesce_join_bots(ctx, &[saved_bots, after_bots]);
            ctx.path_provs = saved_paths.clone();
            for (k, v) in after_paths.iter() {
                let j = join_prov(v, saved_paths.get(k).unwrap_or(v), &ctx.regs);
                ctx.path_provs.insert(k.clone(), j);
            }
            rec_joined(ctx, Some(*s));
            (Ty::Bool, Prov::Unknown)
        }
        Expr::Bin(op, a, b, s) => {
            // Result-context propagation: int-preserving ops forward
            // `want` to literal operands (`0xff << 32` under `u64`).
            // A shift forwards it to the LHS only — the amount is its
            // own integer. Cmp/logical yield bool: nothing to forward
            // (peer-operand typing below covers literals there).
            let iw = want.filter(|t| {
                !op.is_cmp()
                    && !matches!(op, BinOp::LAnd | BinOp::LOr)
                    && t.unatomic().is_int()
            });
            let bw = if matches!(op, BinOp::Shl | BinOp::Shr) {
                None
            } else {
                iw
            };
            let mut ta = expr_tv_w(a, iw, scopes, ctx).0;
            let mut tb = expr_tv_w(b, bw, scopes, ctx).0;
            // `str == str` — value equality over the bytes (O(n),
            // not a pointer compare). A literal operand adopts `str`
            // through coerce; any other non-str operand rejects —
            // there is no implicit bytes→str conversion.
            if matches!(op, BinOp::Eq | BinOp::Ne) && (ta == Ty::Str || tb == Ty::Str) {
                if ta != Ty::Str {
                    coerce(&tb, a, &ta, &mut ctx.errs);
                } else if tb != Ty::Str {
                    coerce(&ta, b, &tb, &mut ctx.errs);
                }
                return (Ty::Bool, Prov::Unknown);
            }
            // Peer-operand propagation: an unadopted literal takes
            // the concrete sibling's type — and N29 makes that any
            // pure-literal tree (`x != -128` peer-types the Neg,
            // `x > 100 + 28` folds at the peer). `== INT_DEFAULT`
            // guards this — a literal that already adopted `want`
            // is typed, not default, and a shift amount stays `i32`
            // by design.
            if tb.is_int() && ta == Ty::INT_DEFAULT && const_int(a, &tb).is_some() {
                ta = coerce(&tb, a, &ta, &mut ctx.errs);
            }
            if ta.is_int() && tb == Ty::INT_DEFAULT && const_int(b, &ta).is_some() {
                tb = coerce(&ta, b, &tb, &mut ctx.errs);
            }
            if !ta.is_int() || !tb.is_int() {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("arithmetic on non-integers: {ta} and {tb}"),
                ));
            } else if ta != tb {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("integer width/sign mismatch: {ta} vs {tb}"),
                ));
            }
            // N25A — a constant shift amount must fit the operand's
            // width; a dynamic amount bounds-checks at runtime.
            if matches!(op, BinOp::Shl | BinOp::Shr) {
                if let Expr::Int(k, _) = &**b {
                    let w = ta.bits() as i64;
                    if *k < 0 || *k >= w {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("shift amount {k} out of range for {ta} (0..{w})"),
                        ));
                    }
                }
            }
            // N29 — a fully-literal arithmetic tree folds at the
            // operand type and faces the same bar a bare literal
            // does: `let x: u32 = 0 - 1` and `let x: u8 = 255 + 1`
            // reject instead of silently wrapping in emitted code.
            if !op.is_cmp() && ta.is_int() && ta == tb {
                if matches!(op, BinOp::Div | BinOp::Mod) {
                    if let Some(0) = const_int(b, &ta) {
                        push_diag(ctx, Diag::new(
                            *s,
                            "division by zero in constant expression".to_string(),
                        ));
                    }
                }
                if let Some(v) = const_int(e, &ta) {
                    if let Some((lo, hi)) = const_range(&ta) {
                        if v < lo || v > hi {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!(
                                    "constant expression folds to {v}, which does not fit {ta}"
                                ),
                            ));
                        }
                    }
                }
            }
            if op.is_cmp() {
                (Ty::Bool, Prov::Unknown)
            } else {
                (ta, Prov::Unknown)
            }
        }
        Expr::Call(n, _tg, args, s) if n == "slice" && !ctx.local_fns.contains(n) => {
            // `slice(p, n)` — assert foreign storage as a span. The
            // compiler cannot verify the buffer's length or lifetime:
            // this is `handle`-level trust — a declaration, not a
            // derivation. `*void` becomes a byte view.
            if args.len() != 2 {
                push_diag(ctx, Diag::new(
                    *s,
                    "slice() takes a pointer and a length".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (pt, _) = expr_tv(&args[0], scopes, ctx);
            let (lt, _) = expr_tv(&args[1], scopes, ctx);
            if !lt.is_int() {
                push_diag(ctx, Diag::new(
                    args[1].span(),
                    format!("slice() length must be an integer, found {lt}"),
                ));
            }
            match &pt {
                Ty::Ptr { mutable, elem } => {
                    let elem = if **elem == Ty::Void {
                        Ty::U8
                    } else {
                        (**elem).clone()
                    };
                    (
                        Ty::Slice {
                            mutable: *mutable,
                            elem: Box::new(elem),
                        },
                        Prov::Unknown,
                    )
                }
                t => {
                    push_diag(ctx, Diag::new(
                        *s,
                        format!("slice() requires a `*T`/`*const T` pointer, found {t}"),
                    ));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        Expr::Call(n, _tg, args, s) if n == "cstr" && !ctx.local_fns.contains(n) => {
            // `cstr(s)` — the explicit str → `*const u8` adapter for
            // C's NUL-terminated APIs. Only storage PROVABLY carrying
            // a terminator crosses: literals (`"x"`'s global has a
            // physical trailing NUL) and str bindings made from one.
            // Nothing allocates; anything else must go through an
            // allocator-backed conversion (not yet built).
            if args.len() != 1 {
                ctx.errs
                    .push(Diag::new(*s, "`cstr` takes one string".to_string()));
                return (Ty::Void, Prov::Unknown);
            }
            let (at, _) = expr_tv(&args[0], scopes, ctx);
            let safe = match &args[0] {
                Expr::Str(..) => true,
                Expr::Var(n, _) => lookup(scopes, n)
                    .map(|b| ctx.cstr_safe.contains(&format!("{n}#{}", b.epoch)))
                    .unwrap_or(false),
                _ => false,
            };
            if at != Ty::Str && !matches!(args[0], Expr::Str(..)) {
                push_diag(ctx, Diag::new(
                    args[0].span(),
                    format!("`cstr` takes a `str`, found {at}"),
                ));
            } else if !safe {
                push_diag(ctx, Diag::new(
                    args[0].span(),
                    "`cstr` requires storage known to be NUL-terminated — only literals and views bound from one qualify"
                        .to_string(),
                ));
            }
            (
                Ty::Ptr {
                    mutable: false,
                    elem: Box::new(Ty::U8),
                },
                Prov::Unknown,
            )
        }
        Expr::Call(n, _tg, args, s) if n == "str_from_utf8" && !ctx.local_fns.contains(n) => {
            // `str_from_utf8(b)` — the ONLY bytes→str door, and it is
            // explicit: runtime validates UTF-8 (traps like a bounds
            // check — no error channel yet). The result holds the
            // slice's Read claim transitively, so the viewed bytes
            // stay frozen while the string lives.
            if args.len() != 1 {
                push_diag(ctx, Diag::new(
                    *s,
                    "`str_from_utf8` takes a `slice u8`".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (at, _) = expr_tv(&args[0], scopes, ctx);
            match &at {
                Ty::Slice { elem, .. } if **elem == Ty::U8 => (Ty::Str, Prov::Unknown),
                t => {
                    push_diag(ctx, Diag::new(
                        args[0].span(),
                        format!("`str_from_utf8` takes a `slice u8`, found {t}"),
                    ));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        Expr::Call(n, _tg, args, s) if n == "system_allocator" && !ctx.local_fns.contains(n) => {
            // The system heap token. Carries no authority — `alloc`
            // names it only to keep the allocation site explicit.
            if !args.is_empty() {
                push_diag(ctx, Diag::new(
                    *s,
                    "`system_allocator` takes no arguments".to_string(),
                ));
            }
            (Ty::Alloc, Prov::Unknown)
        }
        // `free(x)` IS a consume: the box's root dies exactly once,
        // the runtime releases the bytes. The alias graph does the
        // rest — a live borrow into `p#e` blocks it (cannot consume:
        // it is borrowed), a second `free` is double consume, `*p`
        // after is use of moved authority, and `handle *p` goes
        // stale. A user-declared `free` (e.g. `extern fn free(*void)`
        // for malloc'd C memory) shadows the builtin outright.
        Expr::Call(n, _tg, args, s) if n == "free" && !ctx.local_fns.contains(n) => {
            if args.len() != 1 {
                ctx.errs
                    .push(Diag::new(*s, "`free` takes one box".to_string()));
                return (Ty::Void, Prov::Unknown);
            }
            let (at, _) = expr_tv(&args[0], scopes, ctx);
            match &at {
                Ty::Box(_) | Ty::BoxSlice(_) => {
                    consume_place(&args[0], scopes, ctx);
                }
                t => push_diag(ctx, Diag::new(
                    args[0].span(),
                    format!("`free` takes a `box`/`box slice`, found {t}"),
                )),
            }
            (Ty::Void, Prov::Unknown)
        }
        // `replace(place, v)` — the atomic exchange (N40): the old
        // value moves OUT to the result while `v` moves IN, so the
        // place never holds a hole. That makes it legal through a
        // `write` borrow where `let old = place` would strand the
        // referent, and it needs no overwrite-law exception — the old
        // authority isn't lost, it is returned.
        Expr::Call(n, _tg, args, s)
            if n == "replace" && !ctx.local_fns.contains(n) =>
        {
            if args.len() != 2 {
                push_diag(ctx, Diag::new(
                    *s,
                    "`replace` takes a place and a value".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            if !matches!(
                args[0],
                Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..)
            ) {
                push_diag(ctx, Diag::new(
                    args[0].span(),
                    "`replace` target is not a place".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (pt, pprov) = place_tv(&args[0], scopes, ctx);
            let vtv = expr_tv_w(&args[1], Some(&pt), scopes, ctx);
            coerce(&pt, &args[1], &vtv.0, &mut ctx.errs);
            check_store(&args[0], &args[1], &vtv, scopes, ctx);
            require_place_bases(&args[0], scopes, ctx);
            require_init(&args[0], scopes, ctx);
            let mut ks = resolve_place(&args[0], scopes, ctx);
            if ks.is_empty() {
                ks.extend(place_id(&args[0], scopes));
            }
            for k in &ks {
                use_check(k, &args[0], ctx);
            }
            // `replace(w, w.f)` would move the value into its own
            // subtree — reject any shared storage.
            for k in &ks {
                for vk in resolve_place(&args[1], scopes, ctx) {
                    if overlap(k, &vk) {
                        push_diag(ctx, Diag::new(
                            args[1].span(),
                            "`replace` value overlaps its target place".to_string(),
                        ));
                    }
                }
            }
            // The OLD packet leaves before either side rewrites k's
            // facts: it is what the result binding re-roots.
            let pkts: Vec<LitPkt> = ks.iter().map(|k| collect_lit(k, ctx)).collect();
            // The result's root-level refs/caps ride a span-keyed
            // channel — `refs_of`/`caps_of` answer from it so the
            // generic arg-union fallback can't misattribute arg1.
            let mut out_ri = RefInfo {
                targets: Vec::new(),
                mutable: false,
                move_from: None,
                foreign: false,
            };
            let mut out_caps: Vec<String> = Vec::new();
            for p in &pkts {
                for (rel, ri) in &p.edges {
                    if rel.is_empty() {
                        out_ri.foreign |= ri.foreign;
                        out_ri.mutable |= ri.mutable;
                        for t in &ri.targets {
                            if !out_ri.targets.contains(t) {
                                out_ri.targets.push(t.clone());
                            }
                        }
                    }
                }
                for (rel, ts) in &p.caps {
                    if rel.is_empty() {
                        for t in ts {
                            if !out_caps.contains(t) {
                                out_caps.push(t.clone());
                            }
                        }
                    }
                }
            }
            ctx.replace_outs
                .insert((s.line, s.col), (out_ri, out_caps));
            push_place_lit(&args[1], scopes, ctx);
            consume_owned_src(&args[1], scopes, ctx);
            for k in &ks {
                install_store(k, &args[1], &pt, &vtv, scopes, ctx);
            }
            // install_store drained the channels (the value's packet);
            // the old packet now re-enters for the result's let site.
            for p in pkts {
                push_lit(p, ctx);
            }
            (pt, pprov)
        }
        // `swap(a, b)` — two places exchange values atomically: each
        // one's whole packet (own authority, edges, caps, dead marks,
        // tags, lens, cstr-safety) moves to the other root, so no
        // intermediate state ever duplicates or strands ownership.
        Expr::Call(n, _tg, args, s)
            if n == "swap" && !ctx.local_fns.contains(n) =>
        {
            if args.len() != 2 {
                ctx.errs
                    .push(Diag::new(*s, "`swap` takes two places".to_string()));
                return (Ty::Void, Prov::Unknown);
            }
            let mut bad = false;
            for a in &args[..2] {
                if !matches!(
                    a,
                    Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..)
                ) {
                    push_diag(ctx, Diag::new(
                        a.span(),
                        "`swap` operand is not a place".to_string(),
                    ));
                    bad = true;
                }
            }
            if bad {
                return (Ty::Void, Prov::Unknown);
            }
            let (ta, pa) = place_tv(&args[0], scopes, ctx);
            let (tb, pb) = place_tv(&args[1], scopes, ctx);
            if ta != tb {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("`swap` needs two places of the same type — {ta} vs {tb}"),
                ));
            }
            check_store(&args[0], &args[1], &(tb.clone(), pb.clone()), scopes, ctx);
            check_store(&args[1], &args[0], &(ta.clone(), pa.clone()), scopes, ctx);
            require_place_bases(&args[0], scopes, ctx);
            require_place_bases(&args[1], scopes, ctx);
            require_init(&args[0], scopes, ctx);
            require_init(&args[1], scopes, ctx);
            let mut kas = resolve_place(&args[0], scopes, ctx);
            if kas.is_empty() {
                kas.extend(place_id(&args[0], scopes));
            }
            let mut kbs = resolve_place(&args[1], scopes, ctx);
            if kbs.is_empty() {
                kbs.extend(place_id(&args[1], scopes));
            }
            for k in &kas {
                use_check(k, &args[0], ctx);
            }
            for k in &kbs {
                use_check(k, &args[1], ctx);
            }
            // `swap(a, a)` / `swap(w, w.f)` — the exchange is only
            // meaningful between disjoint storage. One exception: a
            // `[]`-collapsed self-pair (`swap(v[i], v[j])`) is the
            // index-FAMILY swap — the family's fact set is the union
            // over all elements, which the exchange leaves invariant,
            // so nothing re-roots (the memory still swaps in codegen).
            let mut overlap_hit = false;
            for k in &kas {
                for k2 in &kbs {
                    if k == k2 && k.ends_with("[]") {
                        continue;
                    }
                    if overlap(k, k2) {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!(
                                "cannot swap `{}` with `{}` — the places overlap",
                                target_name(&args[0]),
                                target_name(&args[1])
                            ),
                        ));
                        overlap_hit = true;
                    }
                }
            }
            if overlap_hit {
                return (Ty::Void, Prov::Unknown);
            }
            // Both packets leave BEFORE either place rewrites — each
            // then installs under the other's key via the same
            // re-root machinery `let` uses.
            let pkts_a: Vec<LitPkt> =
                kas.iter().map(|k| collect_lit(k, ctx)).collect();
            let pkts_b: Vec<LitPkt> =
                kbs.iter().map(|k| collect_lit(k, ctx)).collect();
            let clear = |k: &str, ctx: &mut Ctx| {
                edges_retain(ctx, |(h, _), _| {
                    h != k
                        && !h.starts_with(&format!("{k}."))
                        && !h.starts_with(&format!("{k}["))
                });
                caps_retain(ctx, |(h, _), _| {
                    h != k
                        && !h.starts_with(&format!("{k}."))
                        && !h.starts_with(&format!("{k}["))
                });
                cons_retain(ctx, |c, _| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
                tag_retain(ctx, |c, _| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
                slen_retain(ctx, |c, _| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
                csafe_retain(ctx, |c| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
                foreign_retain(ctx, |c| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
                provs_retain(ctx, |c, _| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
            };
            for (k, p) in kas.iter().zip(pkts_b.into_iter()) {
                if kbs.contains(k) {
                    continue; // `[]` self-pair — facts already right
                }
                clear(k, ctx);
                init_set(ctx, k.clone(), Cert::Def);
                if k.starts_with("$arg") {
                    ctx.init_writes.insert(k.clone());
                }
                push_lit(p, ctx);
                record_lit_fields(k, Some(*s), ctx);
            }
            for (k, p) in kbs.iter().zip(pkts_a.into_iter()) {
                if kas.contains(k) {
                    continue;
                }
                clear(k, ctx);
                init_set(ctx, k.clone(), Cert::Def);
                if k.starts_with("$arg") {
                    ctx.init_writes.insert(k.clone());
                }
                push_lit(p, ctx);
                record_lit_fields(k, Some(*s), ctx);
            }
            (Ty::Void, Prov::Unknown)
        }
        Expr::Alloc {
            ty, alloc, count, ..
        } => {
            // `alloc<T>(a)` / `alloc_slice<T>(a, n)` — create a heap
            // storage root whose lifetime ends at `free`, not a scope.
            // The allocator arg must be an `alloc` token: the site
            // states WHO allocates, matching the no-hidden-allocation
            // principle.
            let (at, _) = expr_tv(alloc, scopes, ctx);
            if at != Ty::Alloc {
                push_diag(ctx, Diag::new(
                    alloc.span(),
                    format!("`alloc` takes an `alloc` token (e.g. system_allocator()), found {at}"),
                ));
            }
            match count {
                None => (Ty::Box(ty.clone()), Prov::Region(cur_region(ctx))),
                Some(n) => {
                    let (nt, _) = expr_tv(n, scopes, ctx);
                    if !nt.is_int() {
                        push_diag(ctx, Diag::new(
                            n.span(),
                            format!("`alloc_slice` count must be an integer, found {nt}"),
                        ));
                    }
                    (Ty::BoxSlice(ty.clone()), Prov::Region(cur_region(ctx)))
                }
            }
        }
        Expr::Call(n, _tg, args, s) if is_atomic_op(n) && !ctx.local_fns.contains(n) => {
            atomic_op(n, args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s) if n == "lock" && !ctx.local_fns.contains(n) => {
            lock_op(args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s) if n == "unlock" && !ctx.local_fns.contains(n) => {
            unlock_op(args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s) if n == "channel" && !ctx.local_fns.contains(n) => {
            // `channel()` — a fresh endpoint. Its payload type arrives
            // via the `let` annotation (`channel void` = opaque until
            // then); sends enqueue authority packets.
            if !args.is_empty() {
                ctx.errs
                    .push(Diag::new(*s, "`channel` takes no arguments".to_string()));
            }
            (Ty::Channel(Box::new(Ty::Void)), Prov::Unknown)
        }
        Expr::Call(n, _tg, args, s) if n == "bchannel" && !ctx.local_fns.contains(n) => {
            // CHAN-004 S3b — `bchannel(cap)` — a fresh bounded
            // endpoint. Like `channel()`, the payload type arrives via
            // the `let` annotation (`bchannel T`); `cap` is a positive
            // integer bound the runtime enforces.
            match args.len() {
                1 => {
                    let (ct, _) = expr_tv(&args[0], scopes, ctx);
                    if !ct.is_int() {
                        push_diag(ctx, Diag::new(
                            args[0].span(),
                            "`bchannel` takes a positive integer bound".to_string(),
                        ));
                    }
                    if let Expr::Int(n, _) = &args[0] {
                        if *n < 1 {
                            push_diag(ctx, Diag::new(
                                args[0].span(),
                                "`bchannel` takes a positive integer bound".to_string(),
                            ));
                        }
                    }
                }
                _ => {
                    ctx.errs.push(Diag::new(
                        *s,
                        "`bchannel` takes one argument (the bound)".to_string(),
                    ));
                }
            }
            (Ty::BChannel(Box::new(Ty::Void)), Prov::Unknown)
        }
        Expr::Call(n, _tg, args, s) if is_chan_op(n) && !ctx.local_fns.contains(n) => {
            chan_op(n, args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s)
            if n == "unreachable" && !ctx.local_fns.contains(n) =>
        {
            // `unreachable()` — the one termination primitive (N32):
            // control never continues past this point and the path
            // carried no unwind, so scope-exit obligations don't apply.
            // `process.exit` is `mml_exit(code); unreachable()` — an
            // ordinary fn wrapping it; its `diverges` summary bit does
            // the same two sets at every call site.
            if !args.is_empty() {
                push_diag(ctx, Diag::new(
                    *s,
                    "`unreachable` takes no arguments".to_string(),
                ));
            }
            ctx.diverged = true;
            ctx.terminated = true;
            rec_flags(ctx);
            (Ty::Void, Prov::Unknown)
        }
        Expr::Call(n, _tg, args, s)
            if int_cast_ty(n).is_some() && !ctx.local_fns.contains(n) =>
        {
            // `u64(x)`/`i32(x)` — explicit numeric conversion; the
            // gen side emits Instr::Cast (sext/zext/trunc).
            let ty = int_cast_ty(n).unwrap();
            if args.len() != 1 {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("`{n}` cast takes exactly one argument"),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (at, _) = expr_tv_w(&args[0], Some(&ty), scopes, ctx);
            if at.is_int() {
                (ty, Prov::Unknown)
            } else {
                push_diag(ctx, Diag::new(
                    args[0].span(),
                    format!("`{n}` cast takes an int, found {at}"),
                ));
                (Ty::Void, Prov::Unknown)
            }
        }
        Expr::Mutex(inner, s) => {
            builtin_shadow_diag("mutex", *s, ctx);
            let (t, _) = expr_tv(inner, scopes, ctx);
            let ty = match t {
                // `mutex h` — the domain protects the referent's closure.
                Ty::Handle(e) => {
                    let ts = caps_of(inner, scopes, ctx).unwrap_or_default();
                    stale_targets(inner, &ts, scopes, ctx);
                    ctx.last_mutex = Some(ts);
                    Ty::Mutex(e)
                }
                // `mutex <value>` — the domain protects its own payload.
                t => {
                    ctx.last_mutex = Some(Vec::new());
                    Ty::Mutex(Box::new(t))
                }
            };
            (ty, Prov::Unknown)
        }
        // N14 — the C-ABI boundary: opaque transform, no summary.
        // `*T` args claim Write on the referent for the call's
        // duration; `*const T` claims Read — same transient-edge
        // mechanics the capability contracts use.
        Expr::Call(n, _tg, args, s)
            if matches!(ctx.fns.get(n), Some((_, _, true))) =>
        {
            index::record(index::Occ {
                span: *s,
                name: n.clone(),
                kind: "fn",
                hover: index::def(n).map(|d| d.sig).unwrap_or_else(|| format!("extern fn `{n}`")),
                def: index::def(n).map(|d| (d.span, d.file)),
                file: None,
            });
            let (params, ret, _) = ctx.fns.get(n).cloned().unwrap();
            extern_call(n, &params, &ret, args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s) => {
            match ctx.fns.get(n) {
            Some((params, ret, _)) => {
                index::record(index::Occ {
                    span: *s,
                    name: n.clone(),
                    kind: "fn",
                    hover: index::def(n).map(|d| d.sig).unwrap_or_else(|| format!("fn `{n}`")),
                    def: index::def(n).map(|d| (d.span, d.file)),
                    file: None,
                });
                if params.len() != args.len() {
                    push_diag(ctx, Diag::new(
                        *s,
                        format!(
                            "`{n}` takes {} argument(s), got {}",
                            params.len(),
                            args.len()
                        ),
                    ));
                }
                // A `&T` return's provenance is the join of the
                // reference args it could have been derived from —
                // the callee's own return-check guarantees it can't
                // fabricate one from nothing.
                let mut rprov: Option<Prov> = None;
                let mut arg_holders = Vec::new();
                // N24C — `write` reborrows parked for the call's
                // duration: the callee holds the source's Write but
                // does not retain it, so the edges/caps come back
                // unless the summary says it was consumed.
                let mut reborrows: Vec<(
                    String,
                    Vec<((String, String), EdgeFact)>,
                    Vec<((String, String), Cert)>,
                )> = Vec::new();
                let mut bases: Vec<Vec<String>> = vec![Vec::new(); params.len()];
                for (i, (a, (auth, want))) in args.iter().zip(params.iter()).enumerate() {
                    match auth {
                        // Capability param: the arg is a PLACE, and the
                        // contract is enforced here — read args need no
                        // live Write into them, write args need nothing
                        // live at all. The edge is transient per call.
                        Some(req @ (Auth::Read | Auth::Write)) => {
                            match a {
                                Expr::Var(..)
                                | Expr::Field(..)
                                | Expr::Index(..)
                                | Expr::Deref(..) => {}
                                _ => push_diag(ctx, Diag::new(
                                    a.span(),
                                    format!(
                                        "`{}` parameter takes a place (a variable or path)",
                                        match req {
                                            Auth::Read => "read",
                                            _ => "write",
                                        }
                                    ),
                                )),
                            }
                            let (got, _) = place_tv(a, scopes, ctx);
                            coerce(want, a, &got, &mut ctx.errs);
                            // N18 — `read`/`own` args name live
                            // storage: the callee will read (or take)
                            // a value, so one must exist. `write` args
                            // may be callee-initialized — the callee's
                            // `$arg`-rooted stores export through its
                            // summary instead.
                            if *req != Auth::Write {
                                require_init(a, scopes, ctx);
                            }
                            let ts = cap_check(a, *req, scopes, ctx);
                            // Resolved BEFORE authority moves — this
                            // is the caller-side name for $arg{i}.
                            bases[i] = ts.clone();
                            rprov = Some(match rprov {
                                None => place_req(a, scopes, ctx),
                                Some(p) => join_prov(&p, &place_req(a, scopes, ctx), &ctx.regs),
                            });
                            let h = format!("$tmp{i}");
                            // N24C — park the derivation source's
                            // edges AND caps for the call's duration:
                            // `f(write m, write m)` still self-conflicts
                            // (the source is gone during arg checking),
                            // but the borrow is not permanent. Killing
                            // only `edges` here left `m`'s `caps` claim
                            // alive, so `a` stayed borrowed forever (B1).
                            if let Some(src) = match req {
                                Auth::Write => derivation_source(a, scopes, ctx),
                                _ => None,
                            } {
                                let mut pe = Vec::new();
                                for ((eh, et), k) in ctx.edges.iter() {
                                    if eh == &src
                                        || eh.starts_with(&format!("{src}."))
                                        || eh.starts_with(&format!("{src}["))
                                    {
                                        pe.push(((eh.clone(), et.clone()), *k));
                                    }
                                }
                                let mut pc = Vec::new();
                                for ((eh, et), c) in ctx.caps.iter() {
                                    if eh == &src
                                        || eh.starts_with(&format!("{src}."))
                                        || eh.starts_with(&format!("{src}["))
                                    {
                                        pc.push(((eh.clone(), et.clone()), *c));
                                    }
                                }
                                kill_set(ctx, src.clone(), Cert::Def);
                                capkill_set(ctx, src.clone(), Cert::Def);
                                edges_retain(ctx, |(eh, _), _| {
                                    eh != &src
                                        && !eh.starts_with(&format!("{src}."))
                                        && !eh.starts_with(&format!("{src}["))
                                });
                                caps_retain(ctx, |(eh, _), _| {
                                    eh != &src
                                        && !eh.starts_with(&format!("{src}."))
                                        && !eh.starts_with(&format!("{src}["))
                                });
                                reborrows.push((src, pe, pc));
                            }
                            for t in ts {
                                edge_set(ctx, h.clone(), t, EdgeFact::new(*req, Cert::Def, Some(*s)));
                            }
                            arg_holders.push(h);
                        }
                        // `atomic`/`sync` are task-hold kinds, not
                        // param contracts — the parser can't produce them.
                        Some(Auth::Atomic) | Some(Auth::Sync) => push_diag(ctx, Diag::new(
                            *s,
                            "`atomic`/`sync` are not parameter capabilities".to_string(),
                        )),
                        // `own x: T` — the caller transfers the place
                        // outright. Any place transfers: `f(own p.f)`
                        // consumes just the field's authority.
                        Some(Auth::Own) => {
                            bases[i] = resolve_place(a, scopes, ctx);
                            let (got, _) = expr_tv_w(a, Some(want), scopes, ctx);
                            if let Ty::Guard(_) = got {
                                push_diag(ctx, Diag::new(
                                    a.span(),
                                    "a lock guard cannot cross a call boundary".to_string(),
                                ));
                            }
                            coerce(want, a, &got, &mut ctx.errs);
                            consume_place(a, scopes, ctx);
                        }
                        None => {
                            let (got, prov) = expr_tv_w(a, Some(want), scopes, ctx);
                            if let Ty::Guard(_) = got {
                                push_diag(ctx, Diag::new(
                                    a.span(),
                                    "a lock guard cannot cross a call boundary".to_string(),
                                ));
                            }
                            coerce(want, a, &got, &mut ctx.errs);
                            // A box arg MOVES regardless of param
                            // mode — owning values are affine; there
                            // is no borrowing a box into a call.
                            consume_owned_src(a, scopes, ctx);
                            if matches!(want, Ty::Handle(_)) {
                                bases[i] =
                                    caps_of(a, scopes, ctx).unwrap_or_default();
                            }
                            if matches!(want, Ty::Ref { .. } | Ty::Slice { .. } | Ty::Str) {
                                // The $arg{i} name inside the callee =
                                // the view's targets — a slice's span
                                // claim travels exactly like a ref's
                                // pointee; a `str`'s Read claim on the
                                // viewed bytes travels the same way.
                                bases[i] = refs_of(a, scopes, ctx)
                                    .map(|ri| ri.targets)
                                    .unwrap_or_default();
                                rprov = Some(match rprov {
                                    None => prov.clone(),
                                    Some(p) => join_prov(&p, &prov, &ctx.regs),
                                });
                                // Arg edges live for the call's duration:
                                // `f(&mut x, &mut x)` self-conflicts,
                                // `f(&x)` conflicts with an outstanding
                                // `&mut x`, and `f(m)` moves m's edge
                                // permanently.
                                match a {
                                    Expr::AddrOf { place, mutable, .. } => {
                                        // N18 — `&x` crossing a call
                                        // boundary: the callee may
                                        // read the referent, so it
                                        // must hold a value. `&mut x`
                                        // is write authority — the
                                        // callee may initialize it,
                                        // and its stores export.
                                        if !mutable {
                                            require_init(place, scopes, ctx);
                                        }
                                        // `$tmp` (not `$arg`): the
                                        // caller's transient holders
                                        // must never look like the
                                        // callee's param-path keys.
                                        let h = format!("$tmp{i}");
                                        if let Some(ri) = refs_of(a, scopes, ctx) {
                                            record_edges(&h, &ri, Some(a.span()), ctx);
                                            arg_holders.push(h);
                                        }
                                    }
                                    Expr::Var(n, _) => {
                                        if let Some(b) = lookup(scopes, n) {
                                            if matches!(
                                                b.ty,
                                                Ty::Ref { mutable: true, .. }
                                                    | Ty::Slice { mutable: true, .. }
                                            ) && b.auth.is_none()
                                            {
                                                let key = format!("{n}#{}", b.epoch);
                                                kill_set(ctx, key.clone(), Cert::Def);
                                                edges_retain(ctx, |(h, _), _| h != &key);
                                            }
                                        }
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }
                }
                for h in arg_holders {
                    edges_retain(ctx, |(eh, _), _| eh != &h);
                }
                // N8: apply the callee's derived effects — param-path
                // edges/kills replay through each arg's caller keys.
                // The body is never reopened.
                ctx.call_bases.insert((s.line, s.col), bases.clone());
                let sum = ctx.sums.get(n).cloned();
                if let Some(sum) = &sum {
                    replay_summary(sum, &bases, *s, scopes, ctx);
                    // Enum-typed returns carry per-variant payload
                    // refs/caps/provs — replay them through the lit
                    // channels so `let r = f()` records `r.Ok` edges.
                    for (rel, ri) in &sum.ret_lit_edges {
                        let mut ri = ri.clone();
                        let mut ts = Vec::new();
                        for t in &ri.targets {
                            let tr = translate_key(t, &bases);
                            if tr.is_empty() {
                                if !ts.contains(t) {
                                    ts.push(t.clone());
                                }
                            } else {
                                for u in tr {
                                    if !ts.contains(&u) {
                                        ts.push(u);
                                    }
                                }
                            }
                        }
                        ri.targets = ts;
                        ri.move_from = None;
                        ctx.lit_edges.push((rel.clone(), ri));
                    }
                    for (rel, caps) in &sum.ret_lit_caps {
                        // Cap targets are callee-keyed like edge
                        // targets — `$arg`/param paths translate
                        // through the call's arg bases.
                        let mut ts = Vec::new();
                        for t in caps {
                            let tr = translate_key(t, &bases);
                            if tr.is_empty() {
                                if !ts.contains(t) {
                                    ts.push(t.clone());
                                }
                            } else {
                                for u in tr {
                                    if !ts.contains(&u) {
                                        ts.push(u);
                                    }
                                }
                            }
                        }
                        ctx.lit_caps.push((rel.clone(), ts));
                    }
                    if !sum.ret_lit_provs.is_empty() {
                        let mut lps = ctx.last_lit.take().unwrap_or_default();
                        for (rel, p) in &sum.ret_lit_provs {
                            lps.push((rel.clone(), p.clone()));
                        }
                        ctx.last_lit = Some(lps);
                    }
                }
                // N24C — unpark each write-reborrow. A `write` callee
                // can mutate the place but cannot consume or retain its
                // authority (that takes `own`), so the parked edges and
                // caps always return — unless there is no summary to
                // prove what the callee did (cyclic call: conservative).
                for (src, pe, pc) in reborrows {
                    if sum.is_none() {
                        continue;
                    }
                    unkill(ctx, &src);
                    uncapkill(ctx, &src);
                    for ((eh, et), k) in pe {
                        edge_set(ctx, eh, et, k);
                    }
                    for ((eh, et), c) in pc {
                        cap_set(ctx, eh, et, c);
                    }
                }
                // N32 — a `diverges` callee never returns: this path
                // ends terminated, so scope-exit obligations don't
                // apply past the call.
                if sum.as_ref().map(|s| s.diverges).unwrap_or(false) {
                    ctx.diverged = true;
            ctx.terminated = true;
                    rec_flags(ctx);
                }
                let prov = rprov.unwrap_or(Prov::Region(cur_region(ctx)));
                (ret.clone(), prov)
            }
            None => {
                let msg = if ctx.private_syms.contains(n.as_str()) {
                    format!("`{n}` is private to its module")
                } else {
                    format!("undefined function `{n}`")
                };
                push_diag(ctx, Diag::new(*s, msg));
                (Ty::Void, Prov::Unknown)
            }
            }
        }
        Expr::Field(..) | Expr::Index(..) => {
            let base_e = match e {
                Expr::Field(b, ..) | Expr::Index(b, ..) => b,
                _ => unreachable!(),
            };
            // The base is a path prefix, not a value read — `p.f`
            // requires `p#e.f`, not all of `p` (init-by-parts, N18).
            let base = expr_tv_go(base_e, None, scopes, ctx, false);
            // `s.len` / `s.ptr` — computed members of the (ptr, len)
            // pair: `.ptr` hands the span to FFI (`*T`/`*const T`),
            // `.len` is a u64. Neither is an assignable place.
            if let (Expr::Field(_, name, s), Ty::Slice { mutable, elem }) =
                (e, &base.0)
            {
                return match name.as_str() {
                    "len" => (Ty::U64, Prov::Unknown),
                    "ptr" => (
                        Ty::Ptr {
                            mutable: *mutable,
                            elem: elem.clone(),
                        },
                        Prov::Unknown,
                    ),
                    _ => {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("no member `.{name}` on a slice (has `.len`, `.ptr`)"),
                        ));
                        (Ty::Void, Prov::Unknown)
                    }
                };
            }
            // `box slice` members — `.len`/`.ptr` mirror slice; the
            // owning span's `.ptr` is the writable `*T`.
            if let (Expr::Field(_, name, s), Ty::BoxSlice(elem)) = (e, &base.0) {
                return match name.as_str() {
                    "len" => (Ty::U64, Prov::Unknown),
                    "ptr" => (
                        Ty::Ptr {
                            mutable: true,
                            elem: elem.clone(),
                        },
                        Prov::Unknown,
                    ),
                    _ => {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("no member `.{name}` on a box slice (has `.len`, `.ptr`)"),
                        ));
                        (Ty::Void, Prov::Unknown)
                    }
                };
            }
            // `str` pseudo-members: `.len` is the byte count, `.bytes`
            // re-reads the same {ptr, len} as `slice u8`. `.ptr` is
            // deliberately absent — C interop is `s.bytes.ptr` for
            // length-delimited APIs or `cstr(s)` for NUL-terminated.
            if let (Expr::Field(_, name, s), Ty::Str) = (e, &base.0) {
                return match name.as_str() {
                    "len" => (Ty::U64, Prov::Unknown),
                    "bytes" => (
                        Ty::Slice {
                            mutable: false,
                            elem: Box::new(Ty::U8),
                        },
                        Prov::Unknown,
                    ),
                    "ptr" => {
                        push_diag(ctx, Diag::new(
                            *s,
                            "no `.ptr` on `str` — use `s.bytes.ptr` (len-delimited) or `cstr(s)` (NUL-terminated)"
                                .to_string(),
                        ));
                        (Ty::Void, Prov::Unknown)
                    }
                    _ => {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("no member `.{name}` on `str` (has `.len`, `.bytes`)"),
                        ));
                        (Ty::Void, Prov::Unknown)
                    }
                };
            }
            // `s[i]` on a `str` is rejected: byte-vs-character
            // indexing is ambiguous, so bytes go through `.bytes`.
            if let (Expr::Index(..), Ty::Str) = (e, &base.0) {
                push_diag(ctx, Diag::new(
                    e.span(),
                    "`str` is not element-indexed — byte access is explicit: `s.bytes[i]`"
                        .to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            path_ty_prov(base, e, scopes, ctx)
        }
        Expr::IndexRange { span, .. } => {
            push_diag(ctx, Diag::new(
                *span,
                "a range `a[lo..hi]` only names storage under `&`/`&mut` — a slice is a view, not a value"
                    .to_string(),
            ));
            (Ty::Void, Prov::Unknown)
        }
        Expr::AddrOf {
            mutable,
            place,
            span,
        } => {
            match &**place {
                Expr::Var(..)
                | Expr::Field(..)
                | Expr::Index(..)
                | Expr::IndexRange { .. }
                | Expr::Deref(..) => {}
                _ => {
                    ctx.errs
                        .push(Diag::new(*span, "`&` requires a place (a variable or path)".to_string()));
                    return (Ty::Void, Prov::Unknown);
                }
            }
            // N18 — `&*p`/`&b[i]` mint a view THROUGH a pointer: the
            // base's value is read to compute the address.
            require_place_bases(place, scopes, ctx);
            // `&a[lo..hi]` / `&mut a[lo..hi]` — a `slice` view: the
            // binding's edge onto the base's keys IS the span's
            // authority, so `s[i]` rides the ordinary borrow rules.
            if let Expr::IndexRange { base, lo, hi, .. } = &**place {
                let (bt, _) = place_tv(base, scopes, ctx);
                let (elem, n) = match &bt {
                    Ty::Array(e, n) => ((*e.clone()), Some(*n as i64)),
                    // A `box slice` borrows like `slice mut` — the
                    // owner always holds Write authority over its
                    // own heap span.
                    Ty::BoxSlice(elem) => ((**elem).clone(), None),
                    Ty::Slice { elem, mutable: bm } => {
                        // `&mut` needs a mutable view: a read-only
                        // slice (or a str's `.bytes`) cannot mint
                        // `slice mut` — edgeless bases have no graph
                        // claim to refuse it otherwise.
                        if *mutable && !*bm {
                            push_diag(ctx, Diag::new(
                                *span,
                                "cannot derive `slice mut` from a read-only view"
                                    .to_string(),
                            ));
                        }
                        ((**elem).clone(), None)
                    }
                    t => {
                        push_diag(ctx, Diag::new(
                            *span,
                            format!("cannot slice `{t}`: want an array or slice"),
                        ));
                        return (Ty::Void, Prov::Unknown);
                    }
                };
                for bound in lo.iter().chain(hi.iter()) {
                    let it = expr_tv(bound, scopes, ctx).0;
                    if !it.is_int() {
                        push_diag(ctx, Diag::new(
                            bound.span(),
                            format!("slice bound must be an integer, found {it}"),
                        ));
                    }
                }
                // Provably-invalid ranges die here; dynamic bounds get
                // a runtime check at slice birth.
                let limit = n.or_else(|| slice_len_at(base, scopes, ctx));
                if let Some(lim) = limit {
                    if let (Some(l), Some(h)) = (lo.as_deref(), hi.as_deref()) {
                        if let (Expr::Int(a, _), Expr::Int(b, bs)) = (l, h) {
                            if *a < 0 || *b < *a || *b > lim {
                                push_diag(ctx, Diag::new(
                                    *bs,
                                    format!("range {a}..{b} out of bounds for length {lim}"),
                                ));
                            }
                        }
                    }
                    if let (Some(l), None) = (lo.as_deref(), hi.as_deref()) {
                        if let Expr::Int(a, s) = l {
                            if *a < 0 || *a > lim {
                                push_diag(ctx, Diag::new(
                                    *s,
                                    format!("range {a}.. out of bounds for length {lim}"),
                                ));
                            }
                        }
                    }
                    if let (None, Some(h)) = (lo.as_deref(), hi.as_deref()) {
                        if let Expr::Int(b, s) = h {
                            if *b < 0 || *b > lim {
                                push_diag(ctx, Diag::new(
                                    *s,
                                    format!("range ..{b} out of bounds for length {lim}"),
                                ));
                            }
                        }
                    }
                }
                let prov = place_req(place, scopes, ctx);
                cap_check(
                    place,
                    if *mutable { Auth::Write } else { Auth::Read },
                    scopes,
                    ctx,
                );
                return (
                    Ty::Slice {
                        mutable: *mutable,
                        elem: Box::new(elem),
                    },
                    prov,
                );
            }
            let (pt, _) = place_tv(place, scopes, ctx);
            let prov = place_req(place, scopes, ctx);
            // `&mut` through a read-only view can never be valid —
            // foreign slices and `str` bytes carry no edge to refuse
            // the derivation, so the TYPE gates it here.
            if *mutable && read_only_slice_hop(place, scopes, ctx) {
                push_diag(ctx, Diag::new(
                    *span,
                    "cannot take `&mut` through a read-only `slice` view".to_string(),
                ));
            }
            // Capability law: `&x` derives Read; `&mut x` derives Write.
            // Both check against OTHER holders; deriving through our
            // own edge is a reborrow — Write derivation reborrow-moves.
            cap_check(
                place,
                if *mutable { Auth::Write } else { Auth::Read },
                scopes,
                ctx,
            );
            (
                Ty::Ref {
                    mutable: *mutable,
                    elem: Box::new(pt),
                },
                prov,
            )
        }
        Expr::Handle(place, s) => {
            builtin_shadow_diag("handle", *s, ctx);
            match &**place {
                Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
                _ => {
                    push_diag(ctx, Diag::new(
                        *s,
                        "`handle` requires a place (a variable or path)".to_string(),
                    ));
                    return (Ty::Void, Prov::Unknown);
                }
            }
            let (pt, _) = place_tv(place, scopes, ctx);
            // No conflict check, no liveness: a handle is identity —
            // borrowing rules attach only at `*h` use.
            (Ty::Handle(Box::new(pt)), Prov::Unknown)
        }
        Expr::Spawn(body, _s) => {
            rec(format!("(spawn @{}:{})", _s.line, _s.col));
            // Fork the world: the child sees the parent's graph but
            // runs on a clone — its effects suspend until `join`.
            let mut ext: HashSet<String> = HashSet::new();
            for scope in scopes.iter() {
                for (n, b) in scope {
                    ext.insert(format!("{n}#{}", b.epoch));
                }
            }
            // S3a — the counters the child inherits; the merge adds
            // the child's DELTA (spawn-merge composes effects, so
            // pushes/pops sum, never max — under-counting pushes
            // would make the drain proof easier, the unsafe way).
            let spawn_meta = ctx.out.chan_meta.clone();
            let mut child = Ctx {
                fns: ctx.fns,
                local_fns: ctx.local_fns,
                structs: ctx.structs,
                enums: ctx.enums,
                tag: ctx.tag.clone(),
                last_tag: None,
                regs: ctx.regs.clone(),
                path_provs: ctx.path_provs.clone(),
                last_lit: None,
                lit_edges: Vec::new(),
                lit_dead: Vec::new(),
                lit_lens: Vec::new(),
                lit_tags: Vec::new(),
                lit_csafe: Vec::new(),
                edges: ctx.edges.clone(),
                caps: ctx.caps.clone(),
                lit_caps: Vec::new(),
                next_epoch: ctx.next_epoch,
                consumed: ctx.consumed.clone(),
                init: ctx.init.clone(),
                init_writes: HashSet::new(),
                diverged: false,
                terminated: false,
                edge_kills: HashMap::new(),
                cap_kills: HashMap::new(),
                ret_edges: Vec::new(),
                ret_caps: Vec::new(),
                ret_lit_edges: Vec::new(),
                ret_lit_caps: Vec::new(),
                ret_lit_provs: Vec::new(),
                call_bases: HashMap::new(),
                replace_outs: HashMap::new(),
                arg_atomic: ctx.arg_atomic.clone(),
                arg_mutex: ctx.arg_mutex.clone(),
                doms: ctx.doms.clone(),
                last_lock: None,
                last_mutex: None,
                // The channel world is shared memory, not a per-task
                // capability — the child sees the parent's queues and
                // transit set; its changes merge back at spawn-end.
                chan_queues: ctx.chan_queues.clone(),
                transit: ctx.transit.clone(),
                last_recv: None,
                last_send: None,
                out: ctx.out.clone(),
                arg_chan: ctx.arg_chan.clone(),
                foreign: ctx.foreign.clone(),
                slice_lens: ctx.slice_lens.clone(),
                cstr_safe: ctx.cstr_safe.clone(),
                holds: HashMap::new(),
                tasks: HashMap::new(),
                task: Some(TaskScope {
                    ret: Ty::Void,
                    ext: ext.clone(),
                    closed_eps: HashSet::new(),
                }),
                fn_ret: ctx.fn_ret.clone(),
                last_spawn: None,
                loops: Vec::new(),
                sums: ctx.sums,
                private_syms: ctx.private_syms,
                match_yields: ctx.match_yields.clone(),
                abandoned: HashSet::new(),
                // A task body's defers are task-local: the inherited
                // scope frames carry no deferred calls.
                defers: vec![Vec::new(); scopes.len()],
                defer_res: HashMap::new(),
                errs: Vec::new(),
            };
            let mut child_scopes = scopes.clone();
            rec("(task-begin)".to_string());
            check_block(body, &mut child_scopes, &mut child, &Ty::Void);
            rec("(task-end)".to_string());
            ctx.next_epoch = child.next_epoch;
            ctx.errs.extend(child.errs);

            // Channel effects land at spawn — the endpoint is shared
            // state, not a per-task capability. External endpoints take
            // the child's final queue; external transit takes the
            // child's set (both supersede the parent's view).
            let mut cq: Vec<(&String, &Vec<Packet>)> = child.chan_queues.iter().collect();
            cq.sort_by(|a, b| a.0.cmp(b.0));
            for (c, q) in cq {
                if is_external(c, &ext) {
                    rec(format!("(chan {} {})", qs(c), q.len()));
                    ctx.chan_queues.insert(c.clone(), q.clone());
                }
            }
            // CHAN-004 S3a — a task boundary is a physical merge, not
            // a path join: the child's endpoint effects interleave
            // with the parent's.
            merge_task_out(ctx, &child.out, &spawn_meta, &ext);
            ctx.transit = child
                .transit
                .iter()
                .filter(|k| is_external(k, &ext))
                .cloned()
                .collect();

            // Holds: the child's accesses to PARENT places become
            // `t -> k` edges — the whole conflict matrix applies for
            // free during [spawn, join).
            let mut holds: HashMap<String, Auth> = HashMap::new();
            for (k, a) in child.holds.iter() {
                if is_external(k, &ext) {
                    let e = holds.entry(k.clone()).or_insert(*a);
                    if auth_rank(*a) > auth_rank(*e) {
                        *e = *a;
                    }
                }
            }
            let base_regs = ctx.regs.len();
            let eff = TaskEffects {
                // Diff vs the parent's graph — the child inherited our
                // edges, so only NEW external-holder edges are creates.
                creates: child
                    .edges
                    .iter()
                    .filter(|((h, t), f)| {
                        is_external(h, &ext)
                            && is_external(t, &ext)
                            && ctx
                                .edges
                                .get(&(h.clone(), t.clone()))
                                .map(|p| (p.kind, p.cert))
                                != Some((f.kind, f.cert))
                    })
                    .map(|((h, t), f)| (h.clone(), t.clone(), *f))
                    .collect(),
                kills: child
                    .edge_kills
                    .iter()
                    .filter(|(k, _)| is_external(k, &ext))
                    .map(|(k, c)| (k.clone(), *c))
                    .collect(),
                creates_caps: child
                    .caps
                    .iter()
                    .filter(|((h, t), c)| {
                        is_external(h, &ext)
                            && is_external(t, &ext)
                            && ctx.caps.get(&(h.clone(), t.clone())) != Some(c)
                    })
                    .map(|((h, t), c)| (h.clone(), t.clone(), *c))
                    .collect(),
                cap_kills: child
                    .cap_kills
                    .iter()
                    .filter(|(k, _)| is_external(k, &ext))
                    .map(|(k, c)| (k.clone(), *c))
                    .collect(),
                consumed: child
                    .consumed
                    .iter()
                    .filter(|(k, v)| {
                        is_external(k, &ext)
                            && ctx.consumed.get(*k).map(|f| f.st) != Some(v.st)
                    })
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect(),
                // Prov ids >= base_regs name child-internal regions —
                // meaningless to the parent, filtered.
                path_provs: child
                    .path_provs
                    .iter()
                    .filter(|(k, pv)| {
                        is_external(k, &ext)
                            && !matches!(pv, Prov::Region(id)
                                if *id as usize >= base_regs)
                    })
                    .map(|(k, pv)| (k.clone(), pv.clone()))
                    .collect(),
                ret_edges: child.ret_edges.clone(),
                ret_caps: child.ret_caps.clone(),
                ret: child.task.as_ref().map(|t| t.ret.clone()).unwrap_or(Ty::Void),
            };
            let ret = eff.ret.clone();
            ctx.last_spawn = Some(SpawnResult {
                span: *_s,
                holds,
                closed_eps: child
                    .task
                    .as_ref()
                    .map(|t| {
                        t.closed_eps
                            .iter()
                            .filter(|ep| is_external(ep, &ext))
                            .cloned()
                            .collect()
                    })
                    .unwrap_or_default(),
                eff,
            });
            (Ty::Task(Box::new(ret)), Prov::Unknown)
        }
        Expr::Join(inner, s) => {
            builtin_shadow_diag("join", *s, ctx);
            let (ty, _) = expr_tv(inner, scopes, ctx);
            let Ty::Task(ret) = ty.clone() else {
                push_diag(ctx, Diag::new(
                    *s,
                    format!("`join` requires a task, found {ty}"),
                ));
                return (Ty::Void, Prov::Unknown);
            };
            let key = place_id(inner, scopes);
            match key.and_then(|k| ctx.tasks.get(&k).map(|sr| (k, sr.eff.clone()))) {
                Some((key, eff)) => {
                    rec(format!("(task-join {} @{}:{})", qs(&key), s.line, s.col));
                    if consumed_state(ctx, &key) == Some(ConsState::Cons) {
                        // `join t` twice: the Var read above already
                        // reported use-after-consume; don't re-apply.
                        return (*ret, Prov::Unknown);
                    }
                    // The suspended transform lands: kills then creates
                    // then caps — same order as summary replay.
                    let mut kills: Vec<(&String, &Cert)> = eff.kills.iter().collect();
                    kills.sort_by(|a, b| a.0.cmp(b.0));
                    for (k, cert) in kills {
                        kill_set(ctx, k.clone(), *cert);
                        let kk = k.clone();
                        apply_kill(&mut ctx.edges, &kk, *cert);
                    }
                    let mut ckills: Vec<(&String, &Cert)> = eff.cap_kills.iter().collect();
                    ckills.sort_by(|a, b| a.0.cmp(b.0));
                    for (k, cert) in ckills {
                        capkill_set(ctx, k.clone(), *cert);
                        let kk = k.clone();
                        apply_cap_kill(ctx, &kk, *cert);
                    }
                    let mut creates: Vec<&(String, String, EdgeFact)> = eff.creates.iter().collect();
                    creates.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
                    for (h, t, f) in creates {
                        edge_set(ctx, h.clone(), t.clone(), *f);
                        let root = path_root(t);
                        let name = root.split('#').next().unwrap_or(root);
                        if let Some(b) = lookup(scopes, name) {
                            if format!("{name}#{}", b.epoch) == root {
                                prov_set(ctx, h.clone(), Prov::Region(b.owner));
                            }
                        }
                    }
                    let mut ccaps: Vec<&(String, String, Cert)> = eff.creates_caps.iter().collect();
                    ccaps.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
                    for (h, t, cert) in ccaps {
                        cap_set(ctx, h.clone(), t.clone(), *cert);
                    }
                    let mut cons: Vec<(&String, &ConsFact)> = eff.consumed.iter().collect();
                    cons.sort_by(|a, b| a.0.cmp(b.0));
                    for (k, f) in cons {
                        cons_set(ctx, k.clone(), f.clone());
                    }
                    let mut pvs: Vec<&(String, Prov)> = eff.path_provs.iter().collect();
                    pvs.sort_by(|a, b| a.0.cmp(&b.0));
                    for kv in pvs {
                        let (k, pv) = (&kv.0, &kv.1);
                        prov_set(ctx, k.clone(), pv.clone());
                    }
                    // The window ends: hold edges die, token is spent.
                    edges_retain(ctx, |(h, _), _| !key_under(h, &key));
                    cons_set(ctx, key, ConsFact::cons(*s));
                }
                None => {
                    push_diag(ctx, Diag::new(
                        *s,
                        format!("join of unknown or already-joined task `{}`", target_name(inner)),
                    ));
                }
            }
            (*ret, Prov::Param)
        }
        Expr::Deref(inner, s) => {
            // `*r` liveness rides on the inner eval: Var/path reads of a
            // moved or uninit reference are diagnosed there already.
            let (t, prov) = expr_tv(inner, scopes, ctx);
            match &t {
                Ty::Ref { elem, .. } => ((**elem).clone(), prov),
                Ty::Box(elem) => {
                    // `*p` reaches the heap object — the box's own
                    // binding key IS the root, so the region check
                    // on `inner` (its key is `p#e`) already proved the
                    // root live; reads/writes then ride the same
                    // access_check the next layer down performs.
                    ((**elem).clone(), prov)
                }
                Ty::Guard(elem) => {
                    // `*g` — the whole protected payload: a guarded
                    // access under the guard's domain.
                    if let Some((gk, dom, _, srcs)) = root_guard(e, scopes) {
                        for t in resolve_place(e, scopes, ctx) {
                            guarded_check(e, &t, false, &gk, &dom, &srcs, scopes, ctx);
                        }
                    }
                    ((**elem).clone(), prov)
                }
                Ty::Handle(elem) => {
                    // Identity use: the target must still be alive —
                    // and a handle grants no authority over a mutably
                    // borrowed place either (`m=&mut x; *h` can't read
                    // x through h while m lives).
                    stale_check(inner, e, scopes, ctx);
                    for t in resolve_place(e, scopes, ctx) {
                        access_check(e, &t, false, scopes, ctx);
                    }
                    ((**elem).clone(), Prov::Param)
                }
                _ => {
                    ctx.errs
                        .push(Diag::new(*s, format!("`*` requires a reference, found {t}")));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        Expr::StructLit(n, _, _, s) if ctx.enums.contains_key(n) => {
            push_diag(ctx, Diag::new(
                *s,
                format!("`{n}` is an enum — construct a variant: `{n}::V(..)`"),
            ));
            (Ty::Void, Prov::Unknown)
        }
        Expr::StructLit(n, _tg, fields, s) => {
            index::record(index::Occ {
                span: *s,
                name: n.clone(),
                kind: "struct",
                hover: index::def(n).map(|d| d.sig).unwrap_or_else(|| format!("struct `{n}`")),
                def: index::def(n).map(|d| (d.span, d.file)),
                file: None,
            });
            match ctx.structs.get(n) {
            Some(defs) => {
                let mut seen = HashSet::new();
                let mut field_provs: HashMap<String, Prov> = HashMap::new();
                let mut lit_prov = Prov::Param; // top; join pulls it down
                let mut any_ref = false;
                for (fn_, fe) in fields {
                    if !seen.insert(fn_) {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("field `{fn_}` initialized twice in `{n}`"),
                        ));
                        continue;
                    }
                    match defs.iter().find(|(dn, _)| dn == fn_) {
                        Some((_, want)) => {
                            let (got, prov) = expr_tv_w(fe, Some(want), scopes, ctx);
                            field_provs.insert(fn_.clone(), prov.clone());
                            coerce(want, fe, &got, &mut ctx.errs);
                            if matches!(want, Ty::Ref { .. } | Ty::Slice { .. }) {
                                any_ref = true;
                                lit_prov = join_prov(&lit_prov, &prov, &ctx.regs);
                            }
                        }
                        None => push_diag(ctx, Diag::new(
                            *s,
                            format!("no field `{fn_}` on struct `{n}`"),
                        )),
                    }
                }
                for (dn, _) in defs.iter() {
                    if !seen.contains(dn) {
                        push_diag(ctx, Diag::new(
                            *s,
                            format!("missing field `{dn}` in `{n}` literal"),
                        ));
                    }
                }
                // Field provs AND ref-edge infos ride one side-channel;
                // a nested literal's entries get prefixed (inner.r).
                let mut lit_provs = Vec::new();
                let mut lit_edges = Vec::new();
                let mut lit_caps = Vec::new();
                for (fn_, fe) in fields {
                    lit_provs.push((
                        fn_.clone(),
                        field_provs.get(fn_).cloned().unwrap_or(Prov::Unknown),
                    ));
                    // `W { b: p }` — a box field moves the box.
                    push_place_lit(fe, scopes, ctx);
                    consume_owned_src(fe, scopes, ctx);
                    if let Some(mut inner) = ctx.lit_edges.clone().into_iter().next() {
                        let _ = &mut inner;
                    }
                    if let Some(ri) = refs_of(fe, scopes, ctx) {
                        lit_edges.push((fn_.clone(), ri));
                    }
                    for (rel, ri) in std::mem::take(&mut ctx.lit_edges) {
                        lit_edges.push((format!("{fn_}.{rel}"), ri));
                    }
                    if let Some(ts) = caps_of(fe, scopes, ctx) {
                        lit_caps.push((fn_.clone(), ts));
                    }
                    for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
                        lit_caps.push((format!("{fn_}.{rel}"), ts));
                    }
                }
                ctx.lit_edges = lit_edges;
                ctx.lit_caps = lit_caps;
                ctx.last_lit = if any_ref {
                    Some(lit_provs)
                } else {
                    None
                };
                (
                    Ty::Struct(n.clone()),
                    if any_ref { lit_prov } else { Prov::Unknown },
                )
            }
            None => {
                push_diag(ctx, Diag::new(*s, format!("unknown struct `{n}`")));
                (Ty::Void, Prov::Unknown)
            }
            }
        },
        Expr::EnumLit {
            ty,
            variant,
            payload,
            span,
        } => {
            let name = match &**ty {
                Ty::Struct(n) => n.clone(),
                t => format!("{t}"),
            };
            let vars = ctx.enums.get(&name).cloned();
            if vars.is_some() {
                index::record(index::Occ {
                    span: *span,
                    name: format!("{name}::{variant}"),
                    kind: "variant",
                    hover: format!("variant `{variant}` of `{name}`"),
                    def: index::def(&name).map(|d| (d.span, d.file)),
                    file: None,
                });
            }
            match vars {
                Some(vars) => match vars.iter().find(|(vn, _)| vn == variant) {
                    Some((_, want)) => {
                        let mut lit_prov = Prov::Param;
                        let mut any_ref = false;
                        match (want, payload) {
                            (Some(w), Some(pe)) => {
                                let (got, prov) = expr_tv_w(pe, Some(w), scopes, ctx);
                                coerce(w, pe, &got, &mut ctx.errs);
                                // Payload moves like a struct field:
                                // `Some(p)` for `box` consumes `p`.
                                push_place_lit(pe, scopes, ctx);
                                consume_owned_src(pe, scopes, ctx);
                                let mut lit_edges = Vec::new();
                                let mut lit_caps = Vec::new();
                                if let Some(ri) = refs_of(pe, scopes, ctx) {
                                    lit_edges.push((variant.clone(), ri));
                                }
                                for (rel, ri) in std::mem::take(&mut ctx.lit_edges) {
                                    lit_edges.push((format!("{variant}.{rel}"), ri));
                                }
                                if let Some(ts) = caps_of(pe, scopes, ctx) {
                                    lit_caps.push((variant.clone(), ts));
                                }
                                for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
                                    lit_caps.push((format!("{variant}.{rel}"), ts));
                                }
                                ctx.lit_edges = lit_edges;
                                ctx.lit_caps = lit_caps;
                                if matches!(w, Ty::Ref { .. } | Ty::Slice { .. }) {
                                    any_ref = true;
                                    lit_prov = join_prov(&lit_prov, &prov, &ctx.regs);
                                    ctx.last_lit =
                                        Some(vec![(variant.clone(), prov.clone())]);
                                }
                            }
                            (Some(_), None) => push_diag(ctx, Diag::new(
                                *span,
                                format!("variant `{variant}` of `{name}` takes a payload"),
                            )),
                            (None, Some(_)) => push_diag(ctx, Diag::new(
                                *span,
                                format!("variant `{variant}` of `{name}` takes no payload"),
                            )),
                            (None, None) => {}
                        }
                        ctx.last_tag = Some(Some(variant.clone()));
                        (
                            Ty::Struct(name),
                            if any_ref { lit_prov } else { Prov::Unknown },
                        )
                    }
                    None => {
                        push_diag(ctx, Diag::new(
                            *span,
                            format!("enum `{name}` has no variant `{variant}`"),
                        ));
                        (Ty::Void, Prov::Unknown)
                    }
                },
                None => {
                    let msg = if ctx.private_syms.contains(name.as_str()) {
                        format!("`{name}` is private to its module")
                    } else {
                        format!("unknown enum `{name}`")
                    };
                    push_diag(ctx, Diag::new(*span, msg));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        Expr::Match { scrut, arms, span } => {
            // `match` as a value (N28) — `check_match` runs the same
            // scrutinee checks and N-arm effect join as the statement
            // form; the arms' yields unify into the expression's type.
            // `ret` is the enclosing fn's — a `=> return e` arm leaves
            // through it (inside `spawn`, ctx.task intercepts first).
            let fret = ctx.fn_ret.clone();
            let ty = check_match(scrut, arms, span, scopes, ctx, &fret, want);
            let prov = ctx
                .match_yields
                .get(&(span.line, span.col))
                .map(|y| y.prov.clone())
                .unwrap_or(Prov::Unknown);
            (
                ty,
                prov,
            )
        }
        Expr::Try { inner, span } => {
            rec(format!("(try @{}:{})", span.line, span.col));
            // `expr?` = `match expr { Ok(v) => v, Err(e) => return
            // Err(e) }` (N23) — no new machinery: the Ok payload is an
            // ordinary field move, the Err path diverges via return.
            let (st, prov) = expr_tv(inner, scopes, ctx);
            if matches!(
                &**inner,
                Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..)
            ) {
                require_init(inner, scopes, ctx);
            }
            let (ok_ty, err_ty) = match result_parts(&st, ctx) {
                Some(parts) => parts,
                None => {
                    push_diag(ctx, Diag::new(
                        *span,
                        format!(
                            "`?` requires a `Result<T,E>` value (an enum with `Ok`/`Err` payloads), found {st}"
                        ),
                    ));
                    return (Ty::Void, Prov::Unknown);
                }
            };
            // Inside `spawn` the "return" is the task's result — a
            // different channel than fn propagation. Reject rather
            // than guess (N23 v1; task-result propagation is its own
            // question).
            // Inside `spawn` the "return" is the task's result — a
            // different channel than fn propagation. Reject rather
            // than guess (N23 v1; task-result `?` is its own question).
            if ctx.task.is_some() {
                push_diag(ctx, Diag::new(
                    *span,
                    "`?` inside `spawn` is not supported — a task's result is not a propagation channel".to_string(),
                ));
            }
            // The propagation target: the fn's return must be
            // Result<_, E> with the SAME error type — no conversion.
            // (Skipped inside spawn — the rejection above is the
            // whole diagnostic; the parent's fn_ret isn't relevant.)
            if ctx.task.is_none() {
                let want = ctx.fn_ret.clone();
                match result_parts(&want, ctx) {
                    Some((_, rerr)) if rerr == err_ty => {}
                    _ => push_diag(ctx, Diag::new(
                        *span,
                        format!(
                            "`?` propagates `Err({err_ty})`, but this function returns `{want}`"
                        ),
                    )),
                }
            }
            let skeys = resolve_place(inner, scopes, ctx);
            if skeys.is_empty() {
                // Rvalue scrutinee (`f()?`) — the temporary is fresh
                // and owned; no place facts to update.
                return (ok_ty, prov);
            }
            // If every tag source already knows a non-`Ok` variant,
            // the Ok continuation is statically dead. Symmetrically,
            // a provably-`Ok` scrutinee makes the Err return dead —
            // its abandon audit must not run or it false-positives.
            let dead_ok = !skeys.is_empty()
                && skeys.iter().all(|k| {
                    matches!(ctx.tag.get(k), Some(Some(v)) if v != "Ok")
                });
            let dead_err = !skeys.is_empty()
                && skeys.iter().all(|k| {
                    matches!(ctx.tag.get(k), Some(Some(v)) if v == "Ok")
                });
            // Err arm — its payload move is CHECKED (a `read`
            // scrutinee can't move a box payload out on either path),
            // but its state dies with the early return.
            let saved_cons = ctx.consumed.clone();
            let saved_edges = ctx.edges.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_holds = ctx.holds.clone();
            let saved_dres = ctx.defer_res.clone();
            rec("(err-path)".to_string());
            for k in &skeys {
                tag_set(ctx, k.clone(), Some("Err".to_string()));
            }
            let fe_err = Expr::Field(
                Box::new((**inner).clone()),
                "Err".to_string(),
                *span,
            );
            consume_owned_src(&fe_err, scopes, ctx);
            // N45R — this path unwinds out of the function: run the
            // same abandon audit `return` does BEFORE the Err-arm
            // state is discarded. `p` held across `f()?` leaks on the
            // Err path and must be freed first (or the call reordered).
            if !dead_err && !ctx.terminated && ctx.task.is_none() {
                // N54 — `?` unwinds every scope: the deferred calls
                // replay (their consumes discharge R19) before the
                // audit runs. The Ok continuation keeps its
                // reservations — `defer_res` is restored below.
                replay_all_defers(scopes, ctx);
                flag_abandoned(
                    Some(*span),
                    "this `?` can return",
                    scopes.iter().flat_map(|s| s.iter().map(|(n, b)| (n, b))),
                    ctx,
                );
            }
            ctx.consumed = saved_cons;
            ctx.edges = saved_edges;
            ctx.edge_kills = saved_kills;
            ctx.holds = saved_holds;
            ctx.defer_res = saved_dres;
            // Ok arm — persists: the tag refines and the payload moves
            // out exactly like `let v = scrut.Ok` (N22 field move).
            rec("(ok-path)".to_string());
            for k in &skeys {
                tag_set(ctx, k.clone(), Some("Ok".to_string()));
            }
            let fe_ok = Expr::Field(
                Box::new((**inner).clone()),
                "Ok".to_string(),
                *span,
            );
            let (got, gprov) = expr_tv(&fe_ok, scopes, ctx);
            consume_owned_src(&fe_ok, scopes, ctx);
            if dead_ok {
                ctx.diverged = true;
                rec_flags(ctx);
            }
            (if got == Ty::Void { ok_ty } else { got }, gprov)
        }
    }
}

/// (type, provenance) of a place — for `&x` and write targets. Unlike
/// expr_tv this never read-checks the addressed place (taking its
/// address or storing into it is not a value read); index expressions
/// ARE evaluated normally.
fn place_tv(e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    match e {
        Expr::Var(n, vs) => {
            if let Some(b) = lookup(scopes, n) {
                index::record(index::Occ {
                    span: *vs,
                    name: n.clone(),
                    kind: if b.auth.is_some() { "param" } else { "local" },
                    hover: format!("`{n}`: {}", b.ty),
                    def: Some((b.decl, None)),
                    file: None,
                });
                (b.ty.clone(), b.prov.clone())
            } else {
                (Ty::Void, Prov::Unknown)
            }
        }
        Expr::Field(b, f, s) => {
            let (bt, _) = place_tv(b, scopes, ctx);
            // N22 — variant payload access needs a statically known
            // tag (writes go through this path too).
            if let Ty::Struct(en) = &bt {
                if let Some(vars) = ctx.enums.get(en).cloned() {
                    if let Some((_, pt)) = vars.iter().find(|(vn, _)| vn == f) {
                        if pt.is_none() {
                            push_diag(ctx, Diag::new(
                                *s,
                                format!("variant `{f}` of `{en}` carries no payload"),
                            ));
                        } else {
                            for k in resolve_place(b, scopes, ctx) {
                                match ctx.tag.get(&k) {
                                    Some(Some(v)) if v == f => {}
                                    _ => push_diag(ctx, Diag::new(
                                        *s,
                                        format!(
                                            "cannot access `{f}` payload: active variant is not statically `{f}` — `match` on it first"
                                        ),
                                    )),
                                }
                            }
                        }
                    }
                }
            }
            // `s.bytes` is a valid path SEGMENT (indexing through it
            // reaches the bytes) — assigning to it directly is not.
            if bt == Ty::Str && f == "bytes" {
                return (
                    Ty::Slice {
                        mutable: false,
                        elem: Box::new(Ty::U8),
                    },
                    Prov::Unknown,
                );
            }
            let fty = field_ty(&bt, f, ctx.structs, *s, &mut ctx.errs);
            let def = if let Ty::Struct(en) = &bt {
                index::def(en).map(|d| (d.span, d.file.clone()))
            } else {
                None
            };
            index::record(index::Occ {
                span: *s,
                name: f.clone(),
                kind: "field",
                hover: format!("`{f}`: {fty}"),
                def,
                file: None,
            });
            (fty, Prov::Unknown)
        }
        Expr::Index(b, i, s) => {
            let (bt, _) = place_tv(b, scopes, ctx);
            let it = expr_tv(i, scopes, ctx).0;
            if !it.is_int() {
                push_diag(ctx, Diag::new(
                    i.span(),
                    format!("array index must be an integer, found {it}"),
                ));
            }
            match &bt {
                Ty::Array(elem, n) => {
                    if let Expr::Int(k, ks) = &**i {
                        if *k < 0 || *k >= *n as i64 {
                            push_diag(ctx, Diag::new(
                                *ks,
                                format!("index {k} out of bounds for [{n}]"),
                            ));
                        }
                    }
                    ((**elem).clone(), Prov::Unknown)
                }
                Ty::Slice { elem, .. } => {
                    if let Expr::Int(k, ks) = &**i {
                        if *k < 0 {
                            push_diag(ctx, Diag::new(
                                *ks,
                                format!("negative index {k} into a slice"),
                            ));
                        } else if let Some(len) = slice_len_at(b, scopes, ctx) {
                            if *k >= len {
                                push_diag(ctx, Diag::new(
                                    *ks,
                                    format!("index {k} out of bounds for slice of length {len}"),
                                ));
                            }
                        }
                    }
                    ((**elem).clone(), Prov::Unknown)
                }
                Ty::BoxSlice(elem) => {
                    // Same bounds discipline as slices: literal indices
                    // check at compile time against the stored length.
                    if let Expr::Int(k, ks) = &**i {
                        if *k < 0 {
                            push_diag(ctx, Diag::new(
                                *ks,
                                format!("negative index {k} into a box slice"),
                            ));
                        } else if let Some(len) = slice_len_at(b, scopes, ctx) {
                            if *k >= len {
                                push_diag(ctx, Diag::new(
                                    *ks,
                                    format!("index {k} out of bounds for box slice of length {len}"),
                                ));
                            }
                        }
                    }
                    ((**elem).clone(), Prov::Unknown)
                }
                Ty::Str => {
                    push_diag(ctx, Diag::new(
                        *s,
                        "`str` is not element-indexed — byte access is explicit: `s.bytes[i]`"
                            .to_string(),
                    ));
                    (Ty::Void, Prov::Unknown)
                }
                t => {
                    push_diag(ctx, Diag::new(*s, format!("indexing non-array {t}")));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        Expr::Deref(inner, _) => {
            let (t, prov) = expr_tv(inner, scopes, ctx);
            match t {
                Ty::Ref { elem, .. } | Ty::Handle(elem) | Ty::Guard(elem) | Ty::Box(elem) => {
                    (*elem, prov)
                }
                _ => (Ty::Void, Prov::Unknown),
            }
        }
        Expr::Handle(..) => expr_ty_at(e, scopes, ctx),
        _ => (Ty::Void, Prov::Unknown),
    }
}

/// A `*h` use where the path runs through a handle: every resolved
/// target must still be alive — root bind present (same epoch) and
/// not `own`-moved. `$arg*` synthetic targets are always live.
fn stale_check(inner: &Expr, use_site: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    for hk in resolve_place(inner, scopes, ctx) {
        for ((h, t), _) in ctx.caps.iter() {
            if *h != hk {
                continue;
            }
            let root = path_root(t);
            if root.starts_with('$') {
                continue;
            }
            let name = root.split('#').next().unwrap_or(root);
            let alive = match lookup(scopes, name) {
                Some(b) => {
                    format!("{name}#{}", b.epoch) == root
                        && consumed_state(ctx, t).is_none()
                }
                None => false,
            };
            if !alive {
                push_diag(ctx, Diag::new(
                    use_site.span(),
                    format!(
                        "use of stale handle `{}`: its target no longer exists",
                        target_name(inner)
                    ),
                ));
                return;
            }
        }
    }
}

/// What a reference *value* carries when it flows to a binding —
/// computed on demand (never mutates): AddrOf names its target, Var
/// inherits its holder's live edges (and `&mut` marks the move),
/// Call returns the union of its ref args' targets (the callee can
/// only hand back what it was given), deref refs are opaque `*k`.
fn refs_of(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Option<RefInfo> {
    match e {
        Expr::AddrOf { mutable, place, .. } => {
            let ts = resolve_place(place, scopes, ctx);
            Some(RefInfo {
                targets: if ts.is_empty() {
                    vec!["*?".to_string()]
                } else {
                    ts
                },
                mutable: *mutable,
                // `&mut` through a borrowed-through holder consumes the
                // parent's authority (reborrow-move); `&` derives.
                move_from: if *mutable {
                    derivation_source(place, scopes, ctx)
                } else {
                    None
                },
                foreign: false,
            })
        }
        Expr::Var(n, _) => {
            let b = lookup(scopes, n)?;
            // `str` is never mutable — its edges are Read claims on
            // the viewed bytes (literal strs carry none: static).
            let mutable = matches!(
                b.ty,
                Ty::Ref { mutable: true, .. } | Ty::Slice { mutable: true, .. }
            );
            if !matches!(b.ty, Ty::Ref { .. } | Ty::Slice { .. } | Ty::Str) {
                return None;
            }
            let key = format!("{n}#{}", b.epoch);
            if b.auth.is_some() {
                // `read r: &T` — the ref VALUE lives behind n's alias
                // edge; its target is caller-side, opaque to us.
                return Some(RefInfo {
                    targets: vec![format!("*{key}")],
                    mutable,
                    move_from: if mutable { Some(key) } else { None },
                    foreign: false,
                });
            }
            let targets: Vec<String> = ctx
                .edges
                .iter()
                .filter(|((h, _), _)| *h == key)
                .map(|((_, t), _)| t.clone())
                .collect();
            Some(RefInfo {
                targets,
                mutable,
                move_from: if mutable { Some(key.clone()) } else { None },
                foreign: ctx.foreign.contains(&key),
            })
        }
        // `s.bytes` — the str's Read claim rides along on the slice.
        Expr::Field(b, name, _)
            if name == "bytes"
                && matches!(expr_ty_at(b, scopes, ctx).0, Ty::Str) =>
        {
            refs_of(b, scopes, ctx).map(|mut ri| {
                ri.mutable = false;
                ri.move_from = None;
                ri
            })
        }
        Expr::Field(..) | Expr::Index(..) => {
            let key = place_id(e, scopes)?;
            let (t, _) = expr_ty_at(e, scopes, ctx);
            if !matches!(t, Ty::Ref { .. } | Ty::Slice { .. } | Ty::Str) {
                return None;
            }
            let mutable = matches!(
                t,
                Ty::Ref { mutable: true, .. } | Ty::Slice { mutable: true, .. }
            );
            let targets: Vec<String> = ctx
                .edges
                .iter()
                .filter(|((h, _), _)| *h == key)
                .map(|((_, t), _)| t.clone())
                .collect();
            Some(RefInfo {
                targets,
                mutable,
                move_from: if mutable { Some(key.clone()) } else { None },
                foreign: ctx.foreign.contains(&key),
            })
        }
        Expr::Join(inner, _) => {
            // `let r = join t` — the task's recorded return targets
            // (already filtered to spawner-visible storage) become r's
            // edges when the suspended transform lands.
            let key = place_id(inner, scopes)?;
            let eff = &ctx.tasks.get(&key)?.eff;
            if eff.ret_edges.is_empty() {
                return None;
            }
            let mutable = eff.ret_edges.iter().any(|(_, m)| *m);
            Some(RefInfo {
                targets: eff.ret_edges.iter().map(|(t, _)| t.clone()).collect(),
                mutable,
                move_from: None,
                foreign: false,
            })
        }
        // `slice(p, n)` — a trusted assertion over foreign storage:
        // no edges to claim, no authority to transfer. The foreign
        // flag frees the binding from edge-liveness checks.
        Expr::Call(n, ..) if n == "slice" && !ctx.local_fns.contains(n) => Some(RefInfo {
            targets: Vec::new(),
            mutable: false,
            move_from: None,
            foreign: true,
        }),
        // `str_from_utf8(b)` — the str inherits the slice's claim as
        // Read: a mutable slice's authority moves in (reborrow-move,
        // the slice dies); a read slice shares. A foreign slice's
        // trust carries over untouched.
        Expr::Call(n, _tg, args, _) if n == "str_from_utf8" && !ctx.local_fns.contains(n) => {
            refs_of(&args[0], scopes, ctx).map(|mut ri| {
                ri.mutable = false;
                ri
            })
        }
        Expr::Call(n, _tg, args, _) => {
            // `replace(place, v)` — the result is the OLD value; its
            // root edge was captured before the store rewrote it.
            if n == "replace" && !ctx.local_fns.contains(n) {
                let sp0 = e.span();
                return ctx
                    .replace_outs
                    .get(&(sp0.line, sp0.col))
                    .and_then(|(ri, _)| {
                        (!ri.targets.is_empty() || ri.foreign).then(|| ri.clone())
                    });
            }
            // With a summary, the return's targets are exactly what
            // the callee proved — translated onto this call's places.
            if let Some(sum) = ctx.sums.get(n) {
                if !sum.ret_edges.is_empty() {
                    let sp = e.span();
                    let empty = Vec::new();
                    let bases = ctx
                        .call_bases
                        .get(&(sp.line, sp.col))
                        .unwrap_or(&empty);
                    let mut targets = Vec::new();
                    let mut mutable = false;
                    for (t, m) in &sum.ret_edges {
                        let mut ts = translate_key(t, bases);
                        if ts.is_empty() {
                            ts.push(t.clone()); // opaque `*k` survives
                        }
                        for u in ts {
                            if !targets.contains(&u) {
                                targets.push(u);
                            }
                        }
                        mutable |= *m;
                    }
                    if !targets.is_empty() {
                        return Some(RefInfo {
                            targets,
                            mutable,
                            move_from: None,
                            foreign: false,
                        });
                    }
                }
                // N24C/B1 — a summary with EMPTY ret_edges PROVES the
                // call hands back no refs. Falling through to the
                // arg-union bound a phantom `<local> -> span` borrow
                // onto every `x = f(slice_arg, ..)` result (B1).
                return None;
            }
            // An extern return is opaque — raw pointers never come
            // back alive, so no caller-side view attaches to it.
            if matches!(ctx.fns.get(n), Some((_, _, true))) {
                return None;
            }
            // No summary (cyclic or unchecked): conservative union of
            // every ref arg's targets — the callee can only hand back
            // what it was handed.
            let mut targets = Vec::new();
            let mut mutable = false;
            for a in args {
                if let Some(ri) = refs_of(a, scopes, ctx) {
                    for t in ri.targets {
                        if !targets.contains(&t) {
                            targets.push(t);
                        }
                    }
                    if ri.mutable {
                        mutable = true;
                    }
                }
            }
            if targets.is_empty() {
                None
            } else {
                Some(RefInfo {
                    targets,
                    mutable,
                    move_from: None,
                    foreign: false,
                })
            }
        }
        Expr::Deref(inner, _) => place_id(inner, scopes).map(|k| RefInfo {
            targets: vec![format!("*{k}")],
            mutable: matches!(
                expr_ty_at(inner, scopes, ctx).0,
                Ty::Ref { mutable: true, .. }
            ),
            move_from: None,
            foreign: false,
        }),
        // N28 — the arm-tail union captured while arm scopes lived.
        Expr::Match { span, .. } => ctx
            .match_yields
            .get(&(span.line, span.col))
            .and_then(|y| y.refs.clone()),
        Expr::StructLit(..) => {
            let (t, _) = expr_ty_at(e, scopes, ctx);
            if matches!(t, Ty::Struct(_)) {
                Some(RefInfo {
                    targets: Vec::new(), // fields recorded separately
                    mutable: false,
                    move_from: None,
                    foreign: false,
                })
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Read-only (type, prov) lookup used by refs_of — never pushes diags.
fn expr_ty_at(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> (Ty, Prov) {
    // reuse the small shallow resolver: only Var/Field/Index needed
    fn go(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Option<Ty> {
        match e {
            Expr::Var(n, _) => lookup(scopes, n).map(|b| b.ty.clone()),
            Expr::Field(b, f, _) => match go(b, scopes, ctx)? {
                Ty::Struct(sn) => ctx
                    .structs
                    .get(&sn)
                    .and_then(|fs| fs.iter().find(|(n, _)| n == f))
                    .map(|(_, t)| t.clone()),
                Ty::Guard(inner) | Ty::Mutex(inner) => match *inner {
                    Ty::Struct(sn) => ctx
                        .structs
                        .get(&sn)
                        .and_then(|fs| fs.iter().find(|(n, _)| n == f))
                        .map(|(_, t)| t.clone()),
                    _ => None,
                },
                Ty::Slice { mutable, elem } => match f.as_str() {
                    "len" => Some(Ty::U64),
                    "ptr" => Some(Ty::Ptr {
                        mutable,
                        elem,
                    }),
                    _ => None,
                },
                Ty::Str => match f.as_str() {
                    "len" => Some(Ty::U64),
                    "bytes" => Some(Ty::Slice {
                        mutable: false,
                        elem: Box::new(Ty::U8),
                    }),
                    _ => None,
                },
                Ty::BoxSlice(elem) => match f.as_str() {
                    // A heap span mirrors slice members — owning means
                    // `.ptr` is the writable `*T`.
                    "len" => Some(Ty::U64),
                    "ptr" => Some(Ty::Ptr {
                        mutable: true,
                        elem,
                    }),
                    _ => None,
                },
                _ => None,
            },
            Expr::Index(b, _, _) => match go(b, scopes, ctx)? {
                Ty::Array(el, _) => Some(*el),
                Ty::Slice { elem, .. } => Some(*elem),
                Ty::BoxSlice(elem) => Some(*elem),
                Ty::Guard(inner) | Ty::Mutex(inner) => match *inner {
                    Ty::Array(el, _) => Some(*el),
                    _ => None,
                },
                _ => None,
            },
            Expr::Deref(i, _) => match go(i, scopes, ctx)? {
                Ty::Ref { elem, .. } | Ty::Handle(elem) | Ty::Guard(elem) | Ty::Box(elem) => {
                    Some(*elem)
                }
                _ => None,
            },
            Expr::Handle(p, _) => Some(Ty::Handle(Box::new(go(p, scopes, ctx)?))),
            _ => None,
        }
    }
    (go(e, scopes, ctx).unwrap_or(Ty::Void), Prov::Unknown)
}

/// All live edges to a path (or a containing/prefixed path).
fn edges_to(ctx: &Ctx, key: &str) -> Vec<(String, EdgeFact)> {
    // Map-order collection — sort for canonical diag/event order.
    let mut out: Vec<(String, EdgeFact)> = ctx
        .edges
        .iter()
        .filter(|((_, t), _)| overlap(t, key))
        .map(|((h, _), f)| (h.clone(), *f))
        .collect();
    // CHAN-004 S3a — a `${bot}` target may-alias every name in its
    // candidate set: an access on `key` that overlaps a candidate
    // conflicts with the unknown claim exactly as a named edge
    // would (the physical cap it hides may BE `key`'s).
    for ((h, t), f) in ctx.edges.iter() {
        if overlap(t, key) {
            continue;
        }
        if let Some(cs) = ctx.out.bot_cands.get(t) {
            if cs.iter().any(|c| overlap(c, key)) {
                out.push((h.clone(), *f));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

// ---------- synchronized access (N10) ----------
//
// `atomic T` is a storage class: same bits as T, but inside a task
// window only the atomic builtins may touch it. Sequentially, plain
// access stays legal — the discipline exists to keep concurrent
// overlap synchronized.

/// Builtin atomic ops — call names, not keywords. Each is ONE
/// synchronized access to the atomic storage its first arg resolves to.
/// `u64(x)`/`i32(x)` — the int→int cast names (N24B). Type names
/// lex as idents, so casts arrive as ordinary `Call`s. This is the
/// ONLY numeric conversion: no implicit widening or narrowing, so
/// every width/signedness crossing is written at the call site.
pub(crate) fn int_cast_ty(n: &str) -> Option<Ty> {
    Some(match n {
        "u8" => Ty::U8,
        "u16" => Ty::U16,
        "u32" => Ty::U32,
        "u64" => Ty::U64,
        "i8" => Ty::I8,
        "i16" => Ty::I16,
        "i32" => Ty::I32,
        "i64" => Ty::I64,
        _ => return None,
    })
}

pub(crate) fn is_atomic_op(n: &str) -> bool {
    matches!(
        n,
        "atomic_load" | "atomic_store" | "atomic_add" | "compare_exchange"
    )
}

/// The payload type if `key` names atomic storage: a `name#e` root
/// bound `atomic T`, a `.f`/`[]` path tail ending in `atomic T`, or a
/// `$argN` param-place declared atomic (ctx.arg_atomic).
fn atomic_inner(key: &str, scopes: &mut Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::Atomic(inner) => Some(*inner),
        _ => None,
    }
}

/// The payload type if `key` names mutex storage — same key walk as
/// `atomic_inner`, ending in `mutex T` instead.
fn mutex_inner(key: &str, scopes: &mut Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::Mutex(inner) => Some(*inner),
        _ => None,
    }
}

/// The type stored at a place key — `name#e[.f/[]]` through binds and
/// struct fields; `$argN` param-places reconstruct their declared
/// storage class from the arg side-maps.
fn key_ty(key: &str, scopes: &mut Scopes, ctx: &Ctx) -> Option<Ty> {
    let root = path_root(key);
    let mut ty = if root.starts_with('$') {
        if let Some(i) = ctx.arg_atomic.get(root) {
            Ty::Atomic(Box::new(i.clone()))
        } else if let Some(i) = ctx.arg_mutex.get(root) {
            Ty::Mutex(Box::new(i.clone()))
        } else if let Some(i) = ctx.arg_chan.get(root) {
            Ty::Channel(Box::new(i.clone()))
        } else {
            return None;
        }
    } else {
        let name = root.split('#').next()?;
        let b = lookup(scopes, name)?;
        if format!("{name}#{}", b.epoch) != root {
            return None;
        }
        b.ty.clone()
    };
    let mut rest = &key[root.len()..];
    while !rest.is_empty() {
        // Storage classes are transparent to the path walk: `mutex P`.x
        // sees P's fields (the interior check is what disciplines it).
        ty = match ty {
            Ty::Mutex(i) | Ty::Guard(i) | Ty::Atomic(i) | Ty::Channel(i) => *i,
            t => t,
        };
        if let Some(r) = rest.strip_prefix("[]") {
            ty = match ty {
                Ty::Array(e, _) => *e,
                _ => return None,
            };
            rest = r;
        } else if let Some(r) = rest.strip_prefix('.') {
            let end = r.find(|c: char| c == '.' || c == '[').unwrap_or(r.len());
            let f = &r[..end];
            ty = match &ty {
                Ty::Struct(sn) => ctx
                    .structs
                    .get(sn)?
                    .iter()
                    .find(|(n, _)| n == f)?
                    .1
                    .clone(),
                _ => return None,
            };
            rest = &r[end..];
        } else {
            return None;
        }
    }
    Some(ty)
}

/// The domain protecting `key`, if any: a strict prefix whose type is
/// `mutex T` (a mutex's own interior — `m.x` under domain `m`), or a
/// foreign root from `mutex h` — `doms[d] = x#e` means d claims x's
/// whole closure INCLUDING the root itself.
fn protecting_domain(key: &str, scopes: &mut Scopes, ctx: &Ctx) -> Option<String> {
    let root = path_root(key);
    // Strict prefixes at segment boundaries.
    let mut i = root.len();
    let bytes = key.as_bytes();
    while i < key.len() {
        if bytes[i] == b'.' || bytes[i] == b'[' {
            let prefix = &key[..i];
            if matches!(key_ty(prefix, scopes, ctx), Some(Ty::Mutex(_))) {
                return Some(prefix.to_string());
            }
        }
        i += 1;
    }
    for (d, r) in &ctx.doms {
        if r != d && key_under(key, r) {
            return Some(d.clone());
        }
    }
    None
}

/// If `e`'s path root is a guard binding: (guard key, domain, protected
/// root). `g.x` names the domain's payload, not the guard token.
/// Descends derefs too — `*g`'s accessor is still the guard.
fn root_guard(e: &Expr, scopes: &mut Scopes) -> Option<(String, String, String, Vec<String>)> {
    let n = match e {
        Expr::Var(n, _) => n.as_str(),
        Expr::Field(b, ..) | Expr::Index(b, ..) | Expr::Deref(b, _) => {
            return root_guard(b, scopes)
        }
        _ => return None,
    };
    let b = lookup(scopes, n)?;
    b.guard.as_ref().map(|(d, r, s)| {
        (format!("{n}#{}", b.epoch), d.clone(), r.clone(), s.clone())
    })
}

/// Is `dom` currently claimed? Derived, not stored: scan live guard
/// bindings — scope exit, branch merges and `unlock` all follow for
/// free (the guard bind dies / gets consumed).
fn dom_held(scopes: &mut Scopes, ctx: &Ctx, dom: &str) -> bool {
    scopes.iter().flatten().any(|(n, b)| {
        b.guard
            .as_ref()
            .map(|(d, _, _)| d == dom)
            .unwrap_or(false)
            && consumed_state(ctx, &format!("{n}#{}", b.epoch)).is_none()
    })
}

/// The synchronization matrix: every `Sync` edge `(h -> d)` claims
/// domain d for holder h. An access under `accessor_dom` to `k` is
/// compatible iff the covering domain IS the accessor's — same mutex
/// serializes, different mutex races.
fn sync_scan(e: &Expr, k: &str, accessor_dom: &str, _scopes: &mut Scopes, ctx: &mut Ctx) {
    let edges: Vec<(String, String)> = ctx
        .edges
        .iter()
        .filter(|(_, f)| f.kind == Auth::Sync)
        .map(|((h, d), _)| (h.clone(), d.clone()))
        .collect();
    for (h, d) in edges {
        let r = ctx.doms.get(&d).cloned().unwrap_or_else(|| d.clone());
        let covers = if r == d {
            k != r && key_under(k, &r)
        } else {
            key_under(k, &r)
        };
        if !covers || d == accessor_dom {
            continue;
        }
        let m = if ctx.tasks.contains_key(&h) {
            let tn = h.split('#').next().unwrap_or(&h);
            format!(
                "`{}` under mutex `{d}` races unjoined task `{tn}`'s synchronized access (a different mutex domain)",
                target_name(e),
            )
        } else {
            format!(
                "`{}` under mutex `{d}` conflicts with a different mutex domain",
                target_name(e),
            )
        };
        push_diag(ctx, Diag::new(e.span(), m));
        return;
    }
}

/// Unguarded access to mutex-protected storage — the interior rule:
/// `m.x`, `x.f` (via `mutex h`), `*r` into a protected subtree all
/// reject when reached without a live guard of the protecting domain.
fn unguarded_check(e: &Expr, k: &str, scopes: &mut Scopes, ctx: &mut Ctx) {
    if root_guard(e, scopes).is_some() {
        return; // accessor is itself a guard — its own check covers it
    }
    if let Some(d) = protecting_domain(k, scopes, ctx) {
        let dn = d.split('#').next().unwrap_or(&d);
        push_diag(ctx, Diag::new(
            e.span(),
            format!(
                "cannot access `{}`: `{dn}`'s interior requires `lock`",
                target_name(e),
            ),
        ));
    }
}

/// A guarded access: the guard must be live and local to this ctx
/// (guards never cross a spawn boundary), the key must be inside the
/// guard's protected root, and every other sync claim must share the
/// domain. The claim recorded for task windows is the DOMAIN — one
/// `dom -> Sync` hold covers all of the window's guarded touches.
fn guarded_check(
    e: &Expr,
    k: &str,
    write: bool,
    gk: &str,
    dom: &str,
    srcs: &[String],
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    use_check(gk, e, ctx);
    if let Some(t) = ctx.task.as_ref() {
        if t.ext.contains(gk) {
            push_diag(ctx, Diag::new(
                e.span(),
                format!("guard `{}` cannot cross a spawn boundary", target_name(e)),
            ));
            return;
        }
    }
    sync_scan(e, k, dom, scopes, ctx);
    transit_check(e, k, ctx);
    record_hold(ctx, dom, Auth::Sync);
    // Foreign unsynchronized claims on the interior still race the
    // window; the lock's own derivation edges (`srcs`) and the guard
    // itself are exempt — they ARE the access.
    for (h, f) in edges_to(ctx, k) {
        if h == gk || srcs.contains(&h) || f.kind == Auth::Sync {
            continue;
        }
        let bad = match (write, f.kind) {
            (true, _) => true,
            (false, Auth::Read) => false,
            (false, _) => true,
        };
        if !bad {
            continue;
        }
        let m = if ctx.tasks.contains_key(&h) {
            let tn = h.split('#').next().unwrap_or(&h);
            format!(
                "`{}` under a lock races unjoined task `{tn}`'s unsynchronized access",
                target_name(e),
            )
        } else {
            format!("`{}` under a lock conflicts with a live borrow", target_name(e))
        };
        push_diag(ctx, Diag::new(e.span(), m));
        return;
    }
}

/// Semantics of `atomic_load`/`atomic_store`/`atomic_add`/`compare_exchange`.
/// The first arg resolves to atomic STORAGE — the place itself, or the
/// referent when a handle/ref names it (identity is not authority: the
/// op still needs live, unconsumed targets). Each resolved atomic key
/// records an `Atomic` hold and conflicts with every non-Atomic edge —
/// shared borrows and task holds are unsynchronized windows.
fn atomic_op(
    n: &str,
    args: &[Expr],
    s: Span,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    let arity = match n {
        "atomic_load" => 1,
        "atomic_store" | "atomic_add" => 2,
        _ => 3,
    };
    if args.len() != arity {
        push_diag(ctx, Diag::new(
            s,
            format!("`{n}` takes {arity} argument(s), got {}", args.len()),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    let place = &args[0];
    match place {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
        _ => {
            push_diag(ctx, Diag::new(
                place.span(),
                format!("`{n}`'s first argument must name a place"),
            ));
            return (Ty::Void, Prov::Unknown);
        }
    }
    let (pt, _) = place_tv(place, scopes, ctx);
    // Resolve to the storage the op synchronizes on: handles and refs
    // name their REFERENT; everything else is the place itself.
    let mut holders = path_holders(place, scopes, ctx);
    let targets = match &pt {
        Ty::Handle(_) => {
            if let Some(k) = place_id(place, scopes) {
                holders.insert(k);
            }
            caps_of(place, scopes, ctx).unwrap_or_default()
        }
        Ty::Ref { .. } => {
            // A bare ref argument names its referent — inline the
            // Deref arm's edge-follow (Expr isn't Clone).
            let mut out = Vec::new();
            for h in resolve_place(place, scopes, ctx) {
                for ((eh, t), _) in ctx.edges.iter() {
                    if *eh == h && !out.contains(t) {
                        out.push(t.clone());
                    }
                }
            }
            out
        }
        _ => resolve_place(place, scopes, ctx),
    };
    // Liveness for identity-reached targets — same rule as cap_check.
    let via_caps = matches!(pt, Ty::Handle(_))
        || holders
            .iter()
            .any(|h| ctx.caps.keys().any(|(ch, _)| ch == h));
    if via_caps {
        for t in &targets {
            let root = path_root(t);
            if root.starts_with('$') {
                continue;
            }
            let name = root.split('#').next().unwrap_or(root);
            let alive = match lookup(scopes, name) {
                Some(b) => {
                    format!("{name}#{}", b.epoch) == root
                        && consumed_state(ctx, t).is_none()
                }
                None => false,
            };
            if !alive {
                push_diag(ctx, Diag::new(
                    s,
                    format!(
                        "use of stale handle `{}`: its target no longer exists",
                        target_name(place)
                    ),
                ));
                return (Ty::Void, Prov::Unknown);
            }
        }
    }
    let atomics: Vec<(String, Ty)> = targets
        .iter()
        .filter_map(|k| atomic_inner(k, scopes, ctx).map(|i| (k.clone(), i)))
        .collect();
    if atomics.is_empty() {
        push_diag(ctx, Diag::new(
            s,
            format!("non-atomic-op: `{}` is not atomic storage", target_name(place)),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    for (k, _) in &atomics {
        let gsrcs = match root_guard(place, scopes) {
            // `atomic_add(g.cnt, 1)` — interior access through a live
            // guard; atomicity remains the op's own synchronization.
            Some((gk, _, _, srcs)) => {
                use_check(&gk, place, ctx);
                if let Some(t) = ctx.task.as_ref() {
                    if t.ext.contains(&gk) {
                        push_diag(ctx, Diag::new(
                            s,
                            format!(
                                "guard `{}` cannot cross a spawn boundary",
                                target_name(place)
                            ),
                        ));
                    }
                }
                srcs
            }
            None => {
                unguarded_check(place, k, scopes, ctx);
                transit_check(place, k, ctx);
                Vec::new()
            }
        };
        record_hold(ctx, k, Auth::Atomic);
        for (h, f) in edges_to(ctx, k) {
            if holders.contains(&h) || gsrcs.contains(&h) || f.kind == Auth::Atomic {
                continue;
            }
            let m = if ctx.tasks.contains_key(&h) {
                let tn = h.split('#').next().unwrap_or(&h);
                format!(
                    "`{n}` on `{}` races unjoined task `{tn}`'s unsynchronized access",
                    target_name(place)
                )
            } else {
                format!(
                    "`{n}` on `{}` conflicts with a live borrow",
                    target_name(place)
                )
            };
            push_diag(ctx, Diag::new(s, m));
            break;
        }
    }
    let inner = atomics[0].1.clone();
    for extra in &args[1..] {
        let (got, _) = expr_tv(extra, scopes, ctx);
        coerce(&inner, extra, &got, &mut ctx.errs);
    }
    if n == "atomic_add" && !inner.is_int() {
        push_diag(ctx, Diag::new(
            s,
            format!("`atomic_add` requires integer storage, found atomic {inner}"),
        ));
    }
    match n {
        "atomic_load" => (inner, Prov::Unknown),
        "compare_exchange" => (Ty::Bool, Prov::Unknown),
        _ => (Ty::Void, Prov::Unknown),
    }
}

/// `lock`/`unlock` — builtin names like the atomics (N11).
pub(crate) fn is_mutex_op(n: &str) -> bool {
    n == "lock" || n == "unlock"
}

/// Resolve `lock`'s argument to mutex domains: the place itself when
/// mutex-typed, else its referent (handles/refs/cap params all name
/// the storage — `lock(hm)` claims m's domain, not hm's). Returns
/// (domain key, protected root, payload ty) per resolved domain.
fn mutex_place(place: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) -> Vec<(String, String, Ty)> {
    let (pt, _) = place_tv(place, scopes, ctx);
    let targets: Vec<String> = match &pt {
        Ty::Handle(_) => caps_of(place, scopes, ctx).unwrap_or_default(),
        Ty::Ref { .. } => {
            let mut out = Vec::new();
            for h in resolve_place(place, scopes, ctx) {
                for ((eh, t), _) in ctx.edges.iter() {
                    if *eh == h && !out.contains(t) {
                        out.push(t.clone());
                    }
                }
            }
            out
        }
        _ => {
            let mut ts = resolve_place(place, scopes, ctx);
            if ts.is_empty() {
                ts.extend(place_id(place, scopes));
            }
            ts
        }
    };
    // Identity-reached targets still need liveness.
    if matches!(pt, Ty::Handle(_)) && stale_targets(place, &targets, scopes, ctx) {
        return Vec::new();
    }
    let mut out = Vec::new();
    for d in targets {
        if let Some(p) = mutex_inner(&d, scopes, ctx) {
            let root = ctx.doms.get(&d).cloned().unwrap_or_else(|| d.clone());
            ctx.doms.entry(d.clone()).or_insert_with(|| root.clone());
            if !out.iter().any(|(od, _, _)| *od == d) {
                out.push((d, root, p));
            }
        }
    }
    out
}

/// Identity-reached targets need liveness — a handle names storage
/// that may already be consumed (same rule as `*h` derefs).
fn stale_targets(e: &Expr, targets: &[String], scopes: &mut Scopes, ctx: &mut Ctx) -> bool {
    let mut dead = false;
    for t in targets {
        let root = path_root(t);
        if root.starts_with('$') {
            continue;
        }
        let name = root.split('#').next().unwrap_or(root);
        let alive = match lookup(scopes, name) {
            Some(b) => {
                format!("{name}#{}", b.epoch) == root && consumed_state(ctx, t).is_none()
            }
            None => false,
        };
        if !alive {
            dead = true;
        }
    }
    if dead {
        push_diag(ctx, Diag::new(
            e.span(),
            format!(
                "use of stale handle `{}`: its target no longer exists",
                target_name(e)
            ),
        ));
    }
    dead
}

/// `lock(m)` — claim the domain: it must be mutex storage, not already
/// locked in this ctx (recursive lock = sequential deadlock), and no
/// unsynchronized claim may hold it. On success the pending tuple tells
/// the Let site to bind a guard; inside a task the `Sync` hold rides
/// out to the parent as a `t -> domain` edge.
fn lock_op(args: &[Expr], s: Span, scopes: &mut Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    if args.len() != 1 {
        push_diag(ctx, Diag::new(s, "`lock` takes 1 argument".to_string()));
        return (Ty::Void, Prov::Unknown);
    }
    match &args[0] {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
        _ => {
            push_diag(ctx, Diag::new(
                args[0].span(),
                "`lock` requires a place (a mutex)".to_string(),
            ));
            return (Ty::Void, Prov::Unknown);
        }
    }
    let ms = mutex_place(&args[0], scopes, ctx);
    if ms.is_empty() {
        push_diag(ctx, Diag::new(
            s,
            format!("cannot lock `{}`: not mutex storage", target_name(&args[0])),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    if ms.len() > 1 {
        push_diag(ctx, Diag::new(
            s,
            format!("`lock` on `{}` is ambiguous: multiple targets", target_name(&args[0])),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    let (dom, root, payload) = ms.into_iter().next().unwrap();
    // `lock(m)` IS a use of m — a consumed mutex can't be claimed.
    if consumed_state(ctx, &dom).is_some() {
        push_diag(ctx, consume_labels(
            ctx,
            &dom,
            Diag::new(s, format!("use after consume `{}`", target_name(&args[0]))),
        ));
    }
    // A `mutex h` domain is only as live as the referent it guards.
    if root != dom {
        stale_targets(&args[0], &[root.clone()], scopes, ctx);
    }
    if dom_held(scopes, ctx, &dom) {
        let dn = dom.split('#').next().unwrap_or(&dom);
        push_diag(ctx, Diag::new(
            s,
            format!("mutex `{dn}` is already locked in this context"),
        ));
    }
    // The lock borrows THROUGH the arg's holders — their edges onto
    // the domain are the derivation itself, not foreign claims. But
    // a Read edge can't grant interior mutation rights.
    let srcs: Vec<String> = path_holders(&args[0], scopes, ctx).into_iter().collect();
    for (h, f) in edges_to(ctx, &dom) {
        if srcs.contains(&h) {
            if f.kind == Auth::Read {
                push_diag(ctx, Diag::new(
                    s,
                    format!("cannot lock `{}` through read authority", target_name(&args[0])),
                ));
            }
            continue;
        }
        if f.kind == Auth::Sync {
            continue; // another task's same-domain claim serializes fine
        }
        let m = if ctx.tasks.contains_key(&h) {
            let tn = h.split('#').next().unwrap_or(&h);
            format!("cannot lock `{}`: held by unjoined task `{tn}`", target_name(&args[0]))
        } else {
            format!("cannot lock `{}`: it is borrowed", target_name(&args[0]))
        };
        push_diag(ctx, Diag::new(s, m));
    }
    record_hold(ctx, &dom, Auth::Sync);
    ctx.last_lock = Some((dom, root, payload.clone(), srcs));
    (Ty::Guard(Box::new(payload)), Prov::Unknown)
}

/// `unlock(g)` — affine release: the guard token is consumed (double
/// unlock is a double consume), its `g -> domain` edge dies with it.
fn unlock_op(args: &[Expr], s: Span, scopes: &mut Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    if args.len() != 1 {
        push_diag(ctx, Diag::new(s, "`unlock` takes 1 argument".to_string()));
        return (Ty::Void, Prov::Unknown);
    }
    match root_guard(&args[0], scopes) {
        Some((gk, _, _, _)) => {
            if let Some(t) = ctx.task.as_ref() {
                if t.ext.contains(&gk) {
                    push_diag(ctx, Diag::new(
                        s,
                        format!(
                            "guard `{}` cannot cross a spawn boundary",
                            target_name(&args[0])
                        ),
                    ));
                    return (Ty::Void, Prov::Unknown);
                }
            }
            consume_place(&args[0], scopes, ctx);
        }
        None => {
            push_diag(ctx, Diag::new(
                s,
                format!("cannot unlock `{}`: not a lock guard", target_name(&args[0])),
            ));
        }
    }
    (Ty::Void, Prov::Unknown)
}

// ---------- channels (N12) ----------
//
// A channel transports authority; it does not duplicate it. `send`
// cuts a graph delta out of the sender (a `Packet`), parks it on the
// endpoint's queue, and marks exclusive targets `transit` — authority
// location, not a new state. `recv` installs the delta at the binding.
// Endpoint ops record a `Sync` hold on the endpoint itself — same
// channel serializes, plain access races, different channels don't.

pub(crate) fn is_chan_op(n: &str) -> bool {
    n == "send" || n == "recv" || n == "close"
}

/// Is `key` channel storage — `channel T` at the end of the key walk.
fn chan_inner(key: &str, scopes: &mut Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::Channel(inner) => Some(*inner),
        _ => None,
    }
}

/// CHAN-004 S3b — the bounded endpoint's declared payload.
fn bchan_inner(key: &str, scopes: &mut Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::BChannel(inner) => Some(*inner),
        _ => None,
    }
}

/// CHAN-004 S3b — the bounded-send result type: `send` on a
/// `bchannel T` yields `Result<void, T>` — `Ok` delivered the
/// packet (ownership moved into the queue), `Err(v)` hands the
/// value's authority back. The name rides `std.result.Result`'s
/// instantiation vocabulary, so a program importing `std.result`
/// sees the SAME type either way; `bchan_payloads` pre-registers
/// the instance for programs that don't.
pub(crate) fn send_result_name(payload: &Ty) -> String {
    format!("result.Result<void,{payload}>")
}

/// CHAN-004 S3b — collect every `bchannel T` payload mentioned in
/// the program's type positions (decls, signatures, annotations,
/// explicit type args). The send-result enum instances register
/// from this set so `match`/`?` resolve `Ok`/`Err` even when
/// `std.result` was never imported.
pub(crate) fn bchan_payloads(p: &Program) -> Vec<Ty> {
    fn ty(t: &Ty, out: &mut Vec<Ty>) {
        match t {
            Ty::BChannel(i) => {
                if !out.contains(i) {
                    out.push((**i).clone());
                }
                ty(i, out);
            }
            Ty::Array(e, _)
            | Ty::Handle(e)
            | Ty::Task(e)
            | Ty::Atomic(e)
            | Ty::Mutex(e)
            | Ty::Guard(e)
            | Ty::Channel(e)
            | Ty::Box(e)
            | Ty::BoxSlice(e) => ty(e, out),
            Ty::Ref { elem: e, .. } | Ty::Ptr { elem: e, .. } | Ty::Slice { elem: e, .. } => {
                ty(e, out)
            }
            Ty::App(_, args) => {
                for a in args {
                    ty(a, out);
                }
            }
            _ => {}
        }
    }
    fn ex(e: &Expr, out: &mut Vec<Ty>) {
        match e {
            Expr::Unary(_, a, _)
            | Expr::Field(a, _, _)
            | Expr::Deref(a, _)
            | Expr::Handle(a, _)
            | Expr::Join(a, _)
            | Expr::Mutex(a, _)
            | Expr::Try { inner: a, .. } => ex(a, out),
            Expr::Bin(_, a, b, _) | Expr::Index(a, b, _) => {
                ex(a, out);
                ex(b, out);
            }
            Expr::Call(_, tg, args, _) => {
                for t in tg {
                    ty(t, out);
                }
                for a in args {
                    ex(a, out);
                }
            }
            Expr::IndexRange { base, lo, hi, .. } => {
                ex(base, out);
                if let Some(e) = lo {
                    ex(e, out);
                }
                if let Some(e) = hi {
                    ex(e, out);
                }
            }
            Expr::StructLit(_, tg, fields, _) => {
                for t in tg {
                    ty(t, out);
                }
                for (_, e) in fields {
                    ex(e, out);
                }
            }
            Expr::AddrOf { place, .. } => ex(place, out),
            Expr::Spawn(stmts, _) => block(stmts, out),
            Expr::Alloc { ty: t, alloc, count, .. } => {
                ty(t, out);
                ex(alloc, out);
                if let Some(c) = count {
                    ex(c, out);
                }
            }
            Expr::Sizeof(t, _) => ty(t, out),
            Expr::MethodCall { recv, targs, args, .. } => {
                ex(recv, out);
                for t in targs {
                    ty(t, out);
                }
                for a in args {
                    ex(a, out);
                }
            }
            Expr::AssocFn { ty: t, args, .. } => {
                ty(t, out);
                for a in args {
                    ex(a, out);
                }
            }
            Expr::EnumLit { ty: t, payload, .. } => {
                ty(t, out);
                if let Some(e) = payload {
                    ex(e, out);
                }
            }
            Expr::Match { scrut, arms, .. } => {
                ex(scrut, out);
                for a in arms {
                    block(&a.body, out);
                }
            }
            _ => {}
        }
    }
    fn block(stmts: &[Stmt], out: &mut Vec<Ty>) {
        for s in stmts {
            match s {
                Stmt::Let { ty: t, init, .. } => {
                    if let Some(t) = t {
                        ty(t, out);
                    }
                    if let Some(e) = init {
                        ex(e, out);
                    }
                }
                Stmt::Assign { target, value, .. } => {
                    ex(target, out);
                    ex(value, out);
                }
                Stmt::Return { value, .. } => {
                    if let Some(e) = value {
                        ex(e, out);
                    }
                }
                Stmt::If { cond, then_body, else_body, .. } => {
                    ex(cond, out);
                    block(then_body, out);
                    if let Some(eb) = else_body {
                        block(eb, out);
                    }
                }
                Stmt::While { cond, body, .. } => {
                    ex(cond, out);
                    block(body, out);
                }
                Stmt::Expr { expr, .. } | Stmt::Defer { expr, .. } => ex(expr, out),
                Stmt::Match { scrut, arms, .. } => {
                    ex(scrut, out);
                    for a in arms {
                        block(&a.body, out);
                    }
                }
                Stmt::Region { body, .. } => block(body, out),
                Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => {
                    ex(target, out)
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {}
            }
        }
    }
    let mut out: Vec<Ty> = Vec::new();
    for st in &p.structs {
        for (_, t) in &st.fields {
            ty(t, &mut out);
        }
    }
    for e in &p.enums {
        for (_, vt) in &e.variants {
            if let Some(t) = vt {
                ty(t, &mut out);
            }
        }
    }
    for f in &p.fns {
        for (_, _, t) in &f.params {
            ty(t, &mut out);
        }
        ty(&f.ret, &mut out);
        block(&f.body, &mut out);
    }
    out
}

/// The region a place key's owner lives in — `$arg`/`*` roots are
/// caller-side (None = treated as outliving everything local).
fn owner_of_key(k: &str, scopes: &mut Scopes) -> Option<u32> {
    let root = path_root(k);
    if root.starts_with('$') || root.starts_with('*') {
        return None;
    }
    let name = root.split('#').next()?;
    lookup(scopes, name).map(|b| b.owner)
}

/// Channel-held authority blocks access: `k` overlapping a transit
/// key means the capability is in flight and `recv` must claim it.
/// CHAN-004 S3a — before rejecting, re-ask `MayOutstanding`: an
/// unordered claim leaves its sources transit, and the mark lifts
/// only when no queue/pending/live-unknown residence can still
/// hold it.
///
/// INERTNESS AUDIT (S3b review): for `channel` endpoints this
/// release is unreachable-by-construction. A mark enters transit
/// only beside a queue push (`chan_push` counts it); `chan_pop`
/// counts only physical packet pops (the Some arm, inside
/// `len > 0`); ordered claims un-transit every `sent` name
/// verbatim. So while a packet carrying `t` is queued, U(t) is
/// nonempty (`pops < pushes`); once its pop lands, the claim
/// already lifted the mark verbatim — `MayOutstanding` can only
/// fire on marks whose residence is provably gone. `channel`
/// programs therefore see the identical accept/reject frontier; the
/// gate exists solely for the S3b unordered-claim path, where a
/// claim deliberately leaves sources marked.
fn transit_check(e: &Expr, k: &str, ctx: &mut Ctx) -> bool {
    let stuck: Vec<String> = ctx
        .transit
        .iter()
        .filter(|t| overlap(k, t))
        .cloned()
        .collect();
    for t in stuck {
        if !may_outstanding(ctx, &t) {
            untransit(ctx, &t);
        }
    }
    if ctx.transit.iter().any(|t| overlap(k, t)) {
        push_diag(ctx, Diag::new(
            e.span(),
            format!(
                "cannot use `{}`: its authority was sent through a channel — `recv` must claim it first",
                target_name(e)
            ),
        ));
        true
    } else {
        false
    }
}

/// CHAN-004 S3b — which endpoint family an op resolves against.
/// `channel` and `bchannel` never mix: `send`/`recv`/`close` on a
/// `channel` keep the legacy void-send path; only `send` on a
/// `bchannel` is wired (conditional ownership via `Result<void,T>`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum ChanKind {
    Unbounded,
    Bounded,
}

/// Resolve a channel argument to endpoint keys — the place itself
/// when endpoint-typed, else its referent (`send(hc, ..)` through a
/// handle names ch's endpoint, not hc's). `kind` selects which
/// endpoint family qualifies: a `channel` never answers for a
/// `bchannel` op or vice versa.
fn ep_place(
    place: &Expr,
    kind: ChanKind,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> Vec<String> {
    let (pt, _) = place_tv(place, scopes, ctx);
    let targets: Vec<String> = match &pt {
        Ty::Handle(_) => caps_of(place, scopes, ctx)
            .unwrap_or_default()
            .into_iter()
            .collect(),
        _ => {
            let mut ts = resolve_place(place, scopes, ctx);
            if ts.is_empty() {
                ts.extend(place_id(place, scopes));
            }
            ts
        }
    };
    if matches!(pt, Ty::Handle(_)) && stale_targets(place, &targets, scopes, ctx) {
        return Vec::new();
    }
    targets
        .into_iter()
        .filter(|t| {
            match kind {
                ChanKind::Unbounded => chan_inner(t, scopes, ctx),
                ChanKind::Bounded => bchan_inner(t, scopes, ctx),
            }
            .is_some()
        })
        .collect()
}

/// Endpoint prelude for every channel op: the endpoint must exist,
/// be channel storage, live, and untransited; the op is a Sync claim.
/// Returns the endpoint keys plus the value payload coercion target.
fn ep_gate(
    place: &Expr,
    s: Span,
    kind: ChanKind,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> Vec<(String, Option<Ty>)> {
    let eps = ep_place(place, kind, scopes, ctx);
    if eps.is_empty() {
        push_diag(ctx, Diag::new(
            s,
            format!("`{}` is not a channel endpoint", target_name(place)),
        ));
        return Vec::new();
    }
    ep_gated(eps, place, s, kind, scopes, ctx)
}

/// The gating half of `ep_gate`, split so a caller that already
/// resolved endpoints (the `send` kind-probe) doesn't re-walk the
/// place.
fn ep_gated(
    eps: Vec<String>,
    place: &Expr,
    s: Span,
    kind: ChanKind,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> Vec<(String, Option<Ty>)> {
    let mut out = Vec::new();
    for ep in eps {
        match consumed_state(ctx, &ep) {
            Some(ConsState::Cons) => {
                push_diag(ctx, consume_labels(
                    ctx,
                    &ep,
                    Diag::new(s, format!("use after consume `{}`", target_name(place))),
                ));
                continue;
            }
            Some(ConsState::Maybe) => {
                push_diag(ctx, consume_labels(
                    ctx,
                    &ep,
                    Diag::new(
                        s,
                        format!(
                            "use of maybe-consumed `{}`",
                            target_name(place)
                        ),
                    ),
                ));
                continue;
            }
            None => {}
        }
        if transit_check(place, &ep, ctx) {
            continue;
        }
        // The op is a synchronized claim on the endpoint — sibling
        // sends/recvs serialize; a plain touch of `ch` races.
        record_hold(ctx, &ep, Auth::Sync);
        for (h, f) in edges_to(ctx, &ep) {
            if path_holders(place, scopes, ctx).contains(&h) || f.kind == Auth::Sync {
                continue;
            }
            // CHAN-004 S3b — a task's `Own` claim on a BOUNDED
            // endpoint it only `close`d is mutex-serialized against
            // an in-flight send at runtime (close wakes the blocked
            // sender, which gets `Err`). It is not an ownership steal
            // — don't read it as an unsynchronized access. Unbounded
            // endpoints keep the old rejection.
            if kind == ChanKind::Bounded
                && ctx
                    .tasks
                    .get(&h)
                    .map_or(false, |sr| sr.closed_eps.contains(&ep))
            {
                continue;
            }
            let m = if ctx.tasks.contains_key(&h) {
                let tn = h.split('#').next().unwrap_or(&h);
                format!(
                    "channel op on `{}` races unjoined task `{tn}`'s unsynchronized access",
                    target_name(place)
                )
            } else {
                format!(
                    "channel op on `{}` conflicts with a live borrow",
                    target_name(place)
                )
            };
            push_diag(ctx, Diag::new(s, m));
            break;
        }
        out.push((
            ep.clone(),
            match kind {
                ChanKind::Unbounded => chan_inner(&ep, scopes, ctx),
                ChanKind::Bounded => bchan_inner(&ep, scopes, ctx),
            },
        ));
    }
    out
}

fn chan_gate(
    place: &Expr,
    s: Span,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> Vec<(String, Option<Ty>)> {
    ep_gate(place, s, ChanKind::Unbounded, scopes, ctx)
}

/// CHAN-004 S3a — named sources a packet may carry authority
/// over: its `sent` marks plus the candidate sets of any `${bot}`
/// edges inside it (a forwarded unknown still names its
/// candidates — names move WITH the packet).
fn packet_names(pkt: &Packet, bots: &HashMap<String, BTreeSet<String>>) -> BTreeSet<String> {
    let mut out: BTreeSet<String> = pkt.sent.iter().cloned().collect();
    for (_, t, ..) in pkt.edges.iter() {
        if let Some(cs) = bots.get(t) {
            out.extend(cs.iter().cloned());
        }
    }
    for (_, t, _) in pkt.caps.iter() {
        if let Some(cs) = bots.get(t) {
            out.extend(cs.iter().cloned());
        }
    }
    out
}

/// CHAN-004 S3a — record a push on `ep`: the may-meta counts it
/// and the packet's names join the endpoint's candidate union
/// (which never shrinks — a pop can't identify which departed).
fn chan_push(ctx: &mut Ctx, ep: &str, pkt: &Packet) {
    ctx.out.chan_meta.entry(ep.to_string()).or_default().pushes += 1;
    let ns = packet_names(pkt, &ctx.out.bot_cands);
    ctx.out
        .chan_names
        .entry(ep.to_string())
        .or_default()
        .extend(ns);
}

/// CHAN-004 S3a — record a `Some`-arm departure on `ep`; joins
/// take the min, so a pop credited on one arm isn't one on both.
fn chan_pop(ctx: &mut Ctx, ep: &str) {
    ctx.out.chan_meta.entry(ep.to_string()).or_default().pops += 1;
}

/// CHAN-004 S3a — `ep`'s queue is provably drained: every pushed
/// packet departed on this path. The only absence proof a
/// may-bag admits — membership can't shrink per-pop.
#[allow(dead_code)] // dormant until S3b wires the claim path into recv
fn chan_drained(ctx: &Ctx, ep: &str) -> bool {
    ctx.out
        .chan_meta
        .get(ep)
        .is_none_or(|m| m.pops >= m.pushes)
}

/// CHAN-004 S3a — candidate set for a claim off `ep`: the union
/// of names every not-provably-departed packet may carry —
/// pushed history plus unresolved pending sends aimed at `ep`
/// (a pending SENT message is physically deliverable).
#[allow(dead_code)] // dormant until S3b wires the claim path into recv
fn chan_candidates(ctx: &Ctx, ep: &str) -> BTreeSet<String> {
    let mut c = ctx.out.chan_names.get(ep).cloned().unwrap_or_default();
    for alts in ctx.out.pending_send.values() {
        for p in alts.iter().filter(|p| p.ep == ep) {
            c.extend(packet_names(&p.pkt, &ctx.out.bot_cands));
        }
    }
    c
}

/// CHAN-004 S3a — `MayOutstanding(t) = U ∪ L ∪ P`: every
/// unprovable residence a capability for `t` could still occupy.
///   U — a pushed packet naming `t` on a not-drained endpoint;
///   L — a live `${bot}` edge whose candidate set contains `t`;
///   P — a pending-send latent packet naming `t`.
/// Provable residences are handled by ordinary edge exclusivity;
/// this query is what a may-abstraction can still hide. A named
/// source's transit mark lifts only when this returns false.
fn may_outstanding(ctx: &Ctx, t: &str) -> bool {
    for (ep, m) in &ctx.out.chan_meta {
        if m.pops < m.pushes
            && ctx
                .out
                .chan_names
                .get(ep)
                .is_some_and(|ns| ns.contains(t))
        {
            return true;
        }
    }
    for ((_, target), _) in ctx.edges.iter() {
        if let Some(cs) = ctx.out.bot_cands.get(target) {
            if cs.contains(t) {
                return true;
            }
        }
    }
    for alts in ctx.out.pending_send.values() {
        for p in alts {
            if packet_names(&p.pkt, &ctx.out.bot_cands).contains(t) {
                return true;
            }
        }
    }
    false
}

/// CHAN-004 S3a — the channel may-state join shared by if/match/
/// loop/binop merges: queues union longer-first (existing); any
/// endpoint whose two path-views differ marks `unordered` —
/// membership is may and order is unprovable from then on. Meta
/// joins unordered-OR / pushes-max / pops-min; name, pending,
/// and candidate sets union; `${bot}` mint counters take max.
/// CHAN-004 S3b — pending bounded-send Results must merge
/// symmetrically: a Result place latent on some live arms and
/// absent on others would let one path's in-flight packet phantom-
/// deliver onto a path where no send ran. Resolve inside the arms
/// (or after the join on a symmetric state) — asymmetric pending is
/// rejected at the merge.
fn pend_keys(o: &Outstanding) -> std::collections::BTreeSet<String> {
    o.pending_send.keys().cloned().collect()
}

fn pending_symmetry(arms: &[std::collections::BTreeSet<String>], s: Span, ctx: &mut Ctx) {
    let mut keys: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    for a in arms {
        keys.extend(a.iter().cloned());
    }
    for k in keys {
        let anywhere = arms.iter().any(|a| a.contains(&k));
        let everywhere = arms.iter().all(|a| a.contains(&k));
        if anywhere && !everywhere {
            let n = k.split('#').next().unwrap_or(&k);
            push_diag(ctx, Diag::new(
                s,
                format!(
                    "bounded-send result `{n}` must be examined on every path before they merge"
                ),
            ));
        }
    }
}

fn join_chan_may(
    ctx: &mut Ctx,
    oq: &HashMap<String, Vec<Packet>>,
    oo: &Outstanding,
) {
    let eps: HashSet<String> = ctx
        .chan_queues
        .keys()
        .chain(oq.keys())
        .cloned()
        .collect();
    for ep in eps {
        let a = ctx.chan_queues.get(&ep);
        let b = oq.get(&ep);
        let differ = match (a, b) {
            (Some(x), Some(y)) => x != y,
            (None, None) => false,
            _ => true,
        };
        if b.is_some_and(|q| q.len() > a.map_or(0, Vec::len)) {
            ctx.chan_queues.insert(ep.clone(), b.cloned().unwrap_or_default());
        }
        if differ {
            ctx.out.chan_meta.entry(ep).or_default().unordered = true;
        }
    }
    let keys: HashSet<String> = ctx
        .out
        .chan_meta
        .keys()
        .chain(oo.chan_meta.keys())
        .cloned()
        .collect();
    for ep in keys {
        let a = ctx.out.chan_meta.get(&ep).cloned().unwrap_or_default();
        let b = oo.chan_meta.get(&ep).cloned().unwrap_or_default();
        ctx.out.chan_meta.insert(
            ep,
            ChanMeta {
                unordered: a.unordered || b.unordered,
                pushes: a.pushes.max(b.pushes),
                pops: a.pops.min(b.pops),
            },
        );
    }
    for (ep, ns) in &oo.chan_names {
        ctx.out
            .chan_names
            .entry(ep.clone())
            .or_default()
            .extend(ns.iter().cloned());
    }
    // Pending facts join as path alternatives — `r` holding pkt₁
    // on one arm and pkt₂ on the other means EITHER may resolve;
    // never both (no co-owners).
    for (r, alts) in &oo.pending_send {
        ctx.out
            .pending_send
            .entry(r.clone())
            .or_default()
            .extend(alts.iter().cloned());
    }
    for (k, cs) in &oo.bot_cands {
        ctx.out
            .bot_cands
            .entry(k.clone())
            .or_default()
            .extend(cs.iter().cloned());
    }
    ctx.out.next_bot = ctx.out.next_bot.max(oo.next_bot);
}

/// CHAN-004 S3a — after a join, a binding may carry `${bot}` edges
/// from different paths (v holds cap₁ on one arm, cap₂ on the
/// other). That is ONE physical delivery whose identity is the
/// union — coalesce to a single unknown target so the claim stays
/// "one cap, may-alias {C₁ ∪ C₂}", never two independent caps.
/// ⊥ positions held under each holder on ONE join arm — the
/// arm-local view of unknown-identity facts. A POSITION is the
/// union of an arm's ⊥ targets across edges and caps: a ⊥ key
/// appearing in both maps is one physical position carrying an
/// authority edge AND a capability fact — the shared key is the
/// correlation, so it is counted once per arm, never twice.
fn holder_bots(
    edges: &HashMap<(String, String), EdgeFact>,
    caps: &HashMap<(String, String), Cert>,
    out: &Outstanding,
) -> HashMap<String, Vec<String>> {
    let mut m: HashMap<String, Vec<String>> = HashMap::new();
    for (h, t) in edges.keys().chain(caps.keys()) {
        if out.bot_cands.contains_key(t) {
            m.entry(h.clone()).or_default().push(t.clone());
        }
    }
    for v in m.values_mut() {
        v.sort();
        v.dedup();
    }
    m
}

/// Authority meet for path alternatives — the joined edge may
/// authorize only what EVERY feasible incoming path's edge
/// permitted. Data authority orders Read < Write < Own (the
/// derive relation: Write satisfies Read, Own satisfies both);
/// Atomic/Sync are orthogonal synchronization holds — a mix with
/// data kinds degrades to Read, since any held kind still
/// satisfies a read derivation. This is a MEET, never a join:
/// selecting the strongest permission would grant operations a
/// path never authorized.
fn meet_auth(a: Auth, b: Auth) -> Auth {
    use Auth::*;
    if a == b {
        return a;
    }
    match (a, b) {
        (Read, _) | (_, Read) => Read,
        (Write, Own) | (Own, Write) => Write,
        _ => Read,
    }
}

/// CHAN-004 S3b — join-time ⊥ coalescing on PROVABLE
/// path-alternative evidence. `arms` is one `holder_bots` map per
/// execution path joining here; ⊥ positions drawn from DIFFERENT
/// arms are mutually exclusive alternatives for the same position
/// (no single path ever held both). ⊥ positions co-resident in
/// ONE arm are independent positions — preserved, never merged.
///
/// Each emitted slot is ONE fresh ⊥ key shared across fact maps:
/// installed into `edges` iff any constituent ⊥ carried an edge
/// fact, into `caps` iff any carried a cap fact. The shared key
/// preserves the edge↔cap correlation the claim path installs
/// (`(v,⊥)` naming one physical position in both maps), while
/// the per-map condition keeps the categories conserved.
/// Every emitted slot carries the union of ALL contributing ⊥
/// candidate sets — alignment-free. Single-arm ⊥ sets carry
/// through untouched.
fn coalesce_join_bots(ctx: &mut Ctx, arms: &[HashMap<String, Vec<String>>]) {
    let mut seen: HashMap<&String, usize> = HashMap::new();
    for arm in arms {
        for h in arm.keys() {
            *seen.entry(h).or_insert(0) += 1;
        }
    }
    for (h, n) in seen {
        if n >= 2 {
            coalesce_pos(ctx, arms, h);
        }
    }
}

/// Merge one holder's cross-arm ⊥ positions into `arity` shared-
/// key slots.
fn coalesce_pos(ctx: &mut Ctx, arms: &[HashMap<String, Vec<String>>], h: &str) {
    let lists: Vec<&Vec<String>> =
        arms.iter().filter_map(|a| a.get(h)).collect();
    let arity = lists.iter().map(|l| l.len()).max().unwrap_or(0);
    if arity == 0 {
        return;
    }
    // Which fact categories did any constituent carry? A merged
    // slot can represent any arm position, so it inherits every
    // category present among the constituents — edge iff some ⊥
    // had an edge fact, cap iff some had a cap fact.
    let mut edge_present = false;
    let mut cap_present = false;
    let mut kind = Auth::Own;
    let mut born = None;
    for t in lists.iter().flat_map(|l| l.iter()) {
        let k = (h.to_string(), t.clone());
        if let Some(f) = ctx.edges.get(&k) {
            edge_present = true;
            kind = meet_auth(kind, f.kind);
            born = born.or(f.born);
        }
        if ctx.caps.contains_key(&k) {
            cap_present = true;
        }
    }
    // Union of EVERY contributing ⊥'s candidates.
    let mut cands = BTreeSet::new();
    for t in lists.iter().flat_map(|l| l.iter()) {
        if let Some(cs) = ctx.out.bot_cands.get(t.as_str()) {
            cands.extend(cs.iter().cloned());
        }
    }
    // Drop the constituent entries from whichever map held them.
    for t in lists.iter().flat_map(|l| l.iter()) {
        let k = (h.to_string(), t.clone());
        ctx.edges.remove(&k);
        ctx.caps.remove(&k);
    }
    // Re-emit `arity` slots — ONE key per slot shared across the
    // maps the constituents lived in (cert Maybe: each arm's ⊥
    // is absent on the other arms by construction).
    for _ in 0..arity {
        let bk = bot_key(ctx.out.next_bot);
        ctx.out.next_bot += 1;
        ctx.out.bot_cands.insert(bk.clone(), cands.clone());
        let key = (h.to_string(), bk);
        if edge_present {
            ctx.edges.insert(
                key.clone(),
                EdgeFact { kind, cert: Cert::Maybe, born },
            );
        }
        if cap_present {
            ctx.caps.insert(key, Cert::Maybe);
        }
    }
}

/// CHAN-004 S3a — merge a finished task's outstanding world into
/// the parent's. A spawn boundary is a PHYSICAL merge, not a path
/// join: the child's effects really happened interleaved with
/// ours. Effects compose additively over the spawn-time base
/// (pushes/pops sum — under-counting pushes would make the drain
/// proof easier, the unsafe direction); every endpoint the child
/// pushed to or popped from goes `unordered` — its interleavings
/// destroyed any provable order.
fn merge_task_out(
    ctx: &mut Ctx,
    co: &Outstanding,
    base: &HashMap<String, ChanMeta>,
    ext: &HashSet<String>,
) {
    let mut ck: Vec<(&String, &ChanMeta)> = co.chan_meta.iter().collect();
    ck.sort_by(|a, b| a.0.cmp(b.0));
    for (c, cm) in ck {
        if is_external(c, ext) {
            let b = base.get(c).cloned().unwrap_or_default();
            let e = ctx.out.chan_meta.entry(c.clone()).or_default();
            e.unordered =
                e.unordered || cm.unordered || cm.pushes > b.pushes || cm.pops > b.pops;
            e.pushes = e.pushes.saturating_add(cm.pushes.saturating_sub(b.pushes));
            e.pops = e.pops.saturating_add(cm.pops.saturating_sub(b.pops));
        }
    }
    let mut cn: Vec<(&String, &BTreeSet<String>)> = co.chan_names.iter().collect();
    cn.sort_by(|a, b| a.0.cmp(b.0));
    for (c, ns) in cn {
        if is_external(c, ext) {
            ctx.out
                .chan_names
                .entry(c.clone())
                .or_default()
                .extend(ns.iter().cloned());
        }
    }
    for (r, alts) in &co.pending_send {
        if is_external(r, ext) {
            ctx.out
                .pending_send
                .entry(r.clone())
                .or_default()
                .extend(alts.iter().cloned());
        }
    }
    // Child-minted candidate sets merge unconditionally — a packet
    // the child re-sent can carry a `${bot}` target.
    for (k, cs) in &co.bot_cands {
        ctx.out
            .bot_cands
            .entry(k.clone())
            .or_default()
            .extend(cs.iter().cloned());
    }
    ctx.out.next_bot = ctx.out.next_bot.max(co.next_bot);
}

/// CHAN-004 S3a — claim a packet off an unordered endpoint.
/// Order is unprovable → the front packet is a REPRESENTATIVE,
/// not the physically-delivered one: capability and cap edges
/// re-root to a fresh `${bot}` whose candidate set is every named
/// source still possibly outstanding on `ep` — may-alias, never
/// distinct identity. Own edges keep real labels (A.3.4:
/// region-immortal targets can't reach a label-sensitive gate).
/// `sent` sources STAY transit — release needs MayOutstanding=∅,
/// not a pop. Singleton promotion: |C|=1 proves identity.
#[allow(dead_code)] // dormant until S3b wires the claim path into recv
fn claim_packet_unknown(
    key: &str,
    pkt: &Packet,
    ep: &str,
    e: &Expr,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    let cands = chan_candidates(ctx, ep);
    if cands.len() == 1 {
        claim_packet(key, pkt, e, scopes, ctx);
        for t in pkt.sent.clone() {
            untransit(ctx, &t);
        }
        return;
    }
    // Each capability POSITION gets its own unknown identity — a
    // multi-capability packet (a struct of `&mut`s) delivers
    // independent physical capabilities, and sharing one `${bot}`
    // across positions would alias-collapse them (S3b review: a
    // claim must mint one ⊥ per non-Own edge/cap, never per
    // packet). Distinct TARGETS are distinct positions; the SAME
    // target in edges and caps is one position carrying both fact
    // categories — mint once so the shared key preserves the
    // edge↔cap correlation `claim_packet` installs.
    let mut uk = pkt.clone();
    let mut minted: HashMap<String, String> = HashMap::new();
    for (_, t, f) in uk.edges.iter_mut() {
        if f.kind != Auth::Own && !is_bot_key(t) {
            let bk = match minted.get(t) {
                Some(b) => b.clone(),
                None => {
                    let bk = bot_key(ctx.out.next_bot);
                    ctx.out.next_bot += 1;
                    ctx.out.bot_cands.insert(bk.clone(), cands.clone());
                    rec(format!("(bot {})", qs(&bk)));
                    minted.insert(t.clone(), bk.clone());
                    bk
                }
            };
            *t = bk;
        }
    }
    for (_, t, _) in uk.caps.iter_mut() {
        if !is_bot_key(t) {
            let bk = match minted.get(t) {
                Some(b) => b.clone(),
                None => {
                    let bk = bot_key(ctx.out.next_bot);
                    ctx.out.next_bot += 1;
                    ctx.out.bot_cands.insert(bk.clone(), cands.clone());
                    rec(format!("(bot {})", qs(&bk)));
                    minted.insert(t.clone(), bk.clone());
                    bk
                }
            };
            *t = bk;
        }
    }
    claim_packet(key, &uk, e, scopes, ctx);
}

/// CHAN-004 S3a — the claim dispatcher bounded channels use:
/// ordered endpoints claim verbatim (proven FIFO); unordered
/// claim by candidate set. `channel` keeps the verbatim path —
/// the unordered rule is S3a machinery, not yet reachable from
/// an accepted program.
#[allow(dead_code)] // dormant until S3b wires the claim path into recv
fn chan_claim(
    ep: &str,
    key: &str,
    pkt: Packet,
    e: &Expr,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    let unordered = ctx
        .out
        .chan_meta
        .get(ep)
        .is_some_and(|m| m.unordered);
    if unordered {
        claim_packet_unknown(key, &pkt, ep, e, scopes, ctx);
    } else {
        for t in pkt.sent.clone() {
            untransit(ctx, &t);
        }
        claim_packet(key, &pkt, e, scopes, ctx);
    }
}

/// Cut the authority packet `send` moves into the channel. Owned
/// values move (affine consume); `&mut` refs move exclusive authority
/// into transit; `&` refs move a read capability; handles move
/// identity only. Embedded raw refs can't outlive the boundary
/// (`send-escape`); guards never cross (`guard-escape`).
fn packet_for(v: &Expr, s: Span, scopes: &mut Scopes, ctx: &mut Ctx) -> Packet {
    let mut pkt = Packet {
        ty: Ty::Void,
        edges: Vec::new(),
        caps: Vec::new(),
        sent: Vec::new(),
        auth: None,
        is_handle: false,
    };
    match v {
        Expr::Handle(..) => {
            // Identity only — the ticket travels, the sender keeps
            // its authority over the target.
            let (t, _) = expr_tv(v, scopes, ctx);
            pkt.ty = t;
            pkt.is_handle = true;
            for (t, cert) in caps_of_cert(v, scopes, ctx).unwrap_or_default() {
                // Cert travels verbatim — a `Maybe`-held handle cap
                // must not arrive as a definite claim.
                pkt.caps.push((String::new(), t, cert));
            }
        }
        Expr::AddrOf { mutable, place, .. } => {
            // The capability law runs first (`&mut x` needs Write).
            let (t, _) = expr_tv(v, scopes, ctx);
            pkt.ty = t;
            pkt.auth = Some(if *mutable { Auth::Write } else { Auth::Read });
            let ri = refs_of(v, scopes, ctx);
            for t in ri.iter().flat_map(|r| r.targets.iter()) {
                pkt.edges.push((
                    String::new(),
                    t.clone(),
                    EdgeFact::new(
                        if *mutable { Auth::Write } else { Auth::Read },
                        Cert::Def,
                        Some(s),
                    ),
                ));
            }
            if *mutable {
                // Exclusive authority moves: the derivation source's
                // edge dies (reborrow-move) and the targets transit.
                if let Some(src) = ri.and_then(|r| r.move_from) {
                    kill_set(ctx, src.clone(), Cert::Def);
                    edges_retain(ctx, |(h, _), _| h != &src);
                }
                for (_, t, _) in pkt.edges.clone() {
                    if ctx.transit.contains(&t) {
                        push_diag(ctx, Diag::new(
                            s,
                            format!(
                                "`{}`'s authority is already channel-held",
                                target_name(place)
                            ),
                        ));
                        continue;
                    }
                    if consumed_state(ctx, &t).is_some() {
                        push_diag(ctx, consume_labels(
                            ctx,
                            &t,
                            Diag::new(s, format!("use after consume `{}`", target_name(place))),
                        ));
                        continue;
                    }
                    pkt.sent.push(t.clone());
                    transit_set(ctx, t.clone());
                    // The authority is channel-held, not held by THIS
                    // context — a sibling `recv` must see it free of
                    // our send's momentary claim.
                    unhold(ctx, &t);
                }
            }
        }
        Expr::Var(n, _) => {
            let Some(b) = lookup(scopes, n) else {
                push_diag(ctx, Diag::new(s, format!("undefined variable `{n}`")));
                return pkt;
            };
            let key = format!("{n}#{}", b.epoch);
            if b.guard.is_some() {
                push_diag(ctx, Diag::new(
                    s,
                    format!("a lock guard cannot cross a channel boundary"),
                ));
                return pkt;
            }
            pkt.ty = b.ty.clone();
            match &b.ty {
                Ty::Handle(_) => {
                    // Identity copies across; the sender keeps the ticket.
                    use_check(&key, v, ctx);
                    pkt.is_handle = true;
                    for t in resolve_place(v, scopes, ctx) {
                        pkt.caps.push((String::new(), t, Cert::Def));
                    }
                    // `caps_of` targets carry their certainty — a
                    // Maybe-held cap stays Maybe in the packet.
                    for (t, cert) in caps_of_cert(v, scopes, ctx).unwrap_or_default() {
                        pkt.caps.push((String::new(), t, cert));
                    }
                }
                _ if b.auth.is_some()
                    || matches!(b.ty, Ty::Ref { .. } | Ty::Slice { .. } | Ty::Str) =>
                {
                    // A capability binding moves wholesale: its edges
                    // travel, exclusive targets transit, the name dies.
                    // The binding's own edges ARE the payload — only
                    // FOREIGN claims on the targets race the transfer.
                    // A `str` sends the same way: a literal-backed one
                    // carries no edges, a converted one moves its Read.
                    let mutable = matches!(
                        b.ty,
                        Ty::Ref { mutable: true, .. } | Ty::Slice { mutable: true, .. }
                    ) || matches!(b.auth, Some(Auth::Write) | Some(Auth::Own));
                    let ts = resolve_place(v, scopes, ctx);
                    let holders = path_holders(v, scopes, ctx);
                    for t in &ts {
                        unguarded_check(v, t, scopes, ctx);
                        transit_check(v, t, ctx);
                        for (h, f) in edges_to(ctx, t) {
                            if holders.contains(&h) || f.kind == Auth::Sync {
                                continue;
                            }
                            let mut d = Diag::new(
                                s,
                                format!(
                                    "cannot send `{n}`: its referent is borrowed"
                                ),
                            );
                            if let Some(l) = born_label(&h, &f, ctx) {
                                d.labels.push(l);
                            }
                            push_diag(ctx, d);
                            break;
                        }
                    }
                    for ((h, t), f) in ctx.edges.clone().iter() {
                        if *h == key {
                            pkt.edges
                                .push((String::new(), t.clone(), EdgeFact { born: Some(s), ..*f }));
                        } else if h.starts_with(&format!("{key}."))
                            || h.starts_with(&format!("{key}["))
                        {
                            pkt.edges
                                .push((h[key.len()..].to_string(), t.clone(), EdgeFact { born: Some(s), ..*f }));
                        }
                    }
                    for ((h, t), cert) in ctx.caps.clone().iter() {
                        if h.starts_with(&key) {
                            pkt.caps
                                .push((h[key.len()..].to_string(), t.clone(), *cert));
                        }
                    }
                    cons_set(ctx, key.clone(), ConsFact::cons(s));
                    kill_set(ctx, key.clone(), Cert::Def);
                    capkill_set(ctx, key.clone(), Cert::Def);
                    edges_retain(ctx, |(h, _), _| {
                        h != &key
                            && !h.starts_with(&format!("{key}."))
                            && !h.starts_with(&format!("{key}["))
                    });
                    caps_retain(ctx, |(h, _), _| {
                        h != &key
                            && !h.starts_with(&format!("{key}."))
                            && !h.starts_with(&format!("{key}["))
                    });
                    if mutable {
                        pkt.auth = Some(Auth::Write);
                        for (_, t, _) in pkt.edges.clone() {
                            pkt.sent.push(t.clone());
                            transit_set(ctx, t.clone());
                            unhold(ctx, &t);
                        }
                    } else {
                        pkt.auth = Some(Auth::Read);
                    }
                }
                _ => {
                    // An owned value moves. Embedded raw refs could not
                    // outlive the transport boundary — send-escape.
                    let key = key.clone();
                    let mut esc = false;
                    for ((h, _), _) in ctx.edges.iter() {
                        if h.starts_with(&format!("{key}."))
                            || h.starts_with(&format!("{key}["))
                        {
                            esc = true;
                        }
                    }
                    if esc {
                        push_diag(ctx, Diag::new(
                            s,
                            format!(
                                "cannot send `{n}`: it still carries a reference into sender memory"
                            ),
                        ));
                        return pkt;
                    }
                    // Embedded handles are identity — they travel.
                    for ((h, t), cert) in ctx.caps.clone().iter() {
                        if h.starts_with(&format!("{key}."))
                            || h.starts_with(&format!("{key}["))
                        {
                            pkt.caps
                                .push((h[key.len()..].to_string(), t.clone(), *cert));
                        }
                    }
                    consume_place(v, scopes, ctx);
                }
            }
        }
        _ => {
            let (t, _) = expr_tv(v, scopes, ctx);
            pkt.ty = t;
        }
    }
    pkt
}

/// `send`/`recv`/`close` — the endpoint gate runs first; `send` enqueues
/// a cut packet, `recv` pops one into `last_recv` for the enclosing
/// `let`/`=` to claim, `close` consumes the endpoint via N7.
/// CHAN-004 S3b — `send(bch, v)` on a bounded endpoint: returns
/// `Result<void, T>`; the authority packet stays LATENT in
/// `last_send` until the binding site keys it into `pending_send`.
/// Ownership is conditional — `Ok` delivers the packet (the queue
/// owns it), `Err` hands it back — so the Result MUST be examined:
/// a send result left unbound errors at the statement boundary, a
/// pending one that survives a join or dies with its binding errors
/// at those points.
fn send_bounded(
    beps: Vec<String>,
    place: &Expr,
    v: &Expr,
    s: Span,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    let eps = ep_gated(beps, place, s, ChanKind::Bounded, scopes, ctx);
    if eps.is_empty() {
        return (Ty::Void, Prov::Unknown);
    }
    let pkt = packet_for(v, s, scopes, ctx);
    let mut res_ty = Ty::Void;
    for (ep, want) in &eps {
        match want {
            Some(w) if *w != Ty::Void => {
                coerce(w, v, &pkt.ty, &mut ctx.errs);
                res_ty = Ty::Struct(send_result_name(w));
            }
            // `bchannel void` is opaque — no declared payload means no
            // packet shape and no `Result<void,T>` to build.
            _ => push_diag(ctx, Diag::new(
                s,
                "`bchannel` send needs a declared payload type (`let c: bchannel T`)"
                    .to_string(),
            )),
        }
        // Same lifetime closure as `channel`: a transported authority
        // must outlive the endpoint.
        if let Some(eo) = owner_of_key(ep, scopes) {
            for (_, t, ..) in &pkt.edges {
                if let Some(to) = owner_of_key(t, scopes) {
                    if !is_ancestor_or_eq(to, eo, &ctx.regs) {
                        push_diag(ctx, Diag::new(
                            s,
                            format!(
                                "send-escape: `{}`'s region does not outlive the channel",
                                target_name(v)
                            ),
                        ));
                    }
                }
            }
        }
        rec(format!("(send-pending {} @{}:{})", qs(ep), s.line, s.col));
    }
    ctx.last_send = Some(
        eps.iter()
            .map(|(ep, _)| PendingPkt {
                ep: ep.clone(),
                pkt: pkt.clone(),
            })
            .collect(),
    );
    (res_ty, Prov::Unknown)
}

/// CHAN-004 S3b — resolution of a pending bounded send at its
/// `match`. The scrutinee place carries `pending_send[key]`
/// alternatives (a disjunction — at most one was physically sent on
/// this path). `Ok` delivers every alternative into its queue (the
/// ownership transfer completed); `Err` claims each back at the
/// payload path, restoring the caller's authority.
fn resolve_pending_send(k: &str, variant: &str, e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    let alts = ctx.out.pending_send.remove(k).unwrap_or_default();
    if alts.is_empty() {
        return;
    }
    match variant {
        "Ok" => {
            for p in alts {
                ctx.chan_queues.entry(p.ep.clone()).or_default().push(p.pkt.clone());
                chan_push(ctx, &p.ep, &p.pkt);
            }
        }
        "Err" => {
            for p in alts {
                // The send failed — the value's authority returns to
                // the caller at the Err payload place.
                for t in p.pkt.sent.clone() {
                    untransit(ctx, &t);
                }
                claim_packet(&format!("{k}.Err"), &p.pkt, e, scopes, ctx);
            }
        }
        _ => {
            // A `_` (or any non-Ok/Err) arm cannot resolve the send:
            // re-park the alternatives so the unexamined-result rule
            // still fires on this path.
            ctx.out.pending_send.entry(k.to_string()).or_default().extend(alts);
        }
    }
}

fn chan_op(n: &str, args: &[Expr], s: Span, scopes: &mut Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    match n {
        "send" => {
            if args.len() != 2 {
                ctx.errs
                    .push(Diag::new(s, "`send` takes (channel, value)".to_string()));
                return (Ty::Void, Prov::Unknown);
            }
            // CHAN-004 S3b — a `bchannel` endpoint takes the
            // conditional-ownership path (`Result<void,T>`); the
            // legacy void-send branch below stays `channel`-only.
            // The families never mix: endpoint resolution keys off
            // the declared endpoint type, not the op's spelling.
            let beps = ep_place(&args[0], ChanKind::Bounded, scopes, ctx);
            if !beps.is_empty() {
                return send_bounded(beps, &args[0], &args[1], s, scopes, ctx);
            }
            let eps = chan_gate(&args[0], s, scopes, ctx);
            if eps.is_empty() {
                return (Ty::Void, Prov::Unknown);
            }
            let pkt = packet_for(&args[1], s, scopes, ctx);
            for (ep, want) in &eps {
                if let Some(w) = want {
                    if *w != Ty::Void {
                        // `channel T` declares the packet shape —
                        // `channel &mut P` carries write authority,
                        // `channel handle P` carries identity.
                        coerce(w, &args[1], &pkt.ty, &mut ctx.errs);
                    }
                }
                // Lifetime closure, promotion-style: a transported
                // authority must outlive the endpoint. A target in an
                // inner region dies while the channel is still live —
                // the recv'd edge would dangle. Handles are exempt:
                // identity goes stale at use, it can't dangle.
                if let Some(eo) = owner_of_key(ep, scopes) {
                    for (_, t, ..) in &pkt.edges {
                        if let Some(to) = owner_of_key(t, scopes) {
                            if !is_ancestor_or_eq(to, eo, &ctx.regs) {
                                push_diag(ctx, Diag::new(
                                    s,
                                    format!(
                                        "send-escape: `{}`'s region does not outlive the channel",
                                        target_name(&args[1])
                                    ),
                                ));
                            }
                        }
                    }
                }
                rec(format!("(send {} @{}:{})", qs(ep), s.line, s.col));
                ctx.chan_queues
                    .entry(ep.clone())
                    .or_default()
                    .push(pkt.clone());
                chan_push(ctx, ep, &pkt);
            }
            (Ty::Void, Prov::Unknown)
        }
        "recv" => {
            if args.len() != 1 {
                ctx.errs
                    .push(Diag::new(s, "`recv` takes (channel)".to_string()));
                return (Ty::Void, Prov::Unknown);
            }
            // CHAN-004 S3b — bounded recv (`Option<T>`) stays gated
            // until its ownership tests land.
            if !ep_place(&args[0], ChanKind::Bounded, scopes, ctx).is_empty() {
                push_diag(ctx, Diag::new(
                    s,
                    "`recv` on a `bchannel` is not supported yet".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let eps = chan_gate(&args[0], s, scopes, ctx);
            let mut ty = Ty::Void;
            let mut prov = Prov::Unknown;
            if let Some((ep, want)) = eps.into_iter().next() {
                if let Some(pkt) = ctx
                    .chan_queues
                    .get_mut(&ep)
                    .and_then(|q| if q.is_empty() { None } else { Some(q.remove(0)) })
                {
                    chan_pop(ctx, &ep);
                    rec(format!("(recv {} @{}:{})", qs(&ep), s.line, s.col));
                    // The claimed authority leaves transit HERE; the
                    // binding install rides `last_recv` to `let`.
                    for t in pkt.sent.clone() {
                        untransit(ctx, &t);
                    }
                    // An opaque endpoint (`channel()` untyped) yields
                    // void — bindable recv's need a declared `channel T`.
                    ty = match &want {
                        Some(w) if *w != Ty::Void => pkt.ty.clone(),
                        _ => Ty::Void,
                    };
                    // A received capability/handle names remote storage
                    // — Param prov, like any callee-supplied edge. A
                    // received box is a live root, not a region value.
                    prov = if pkt.auth.is_some() || pkt.is_handle {
                        Prov::Param
                    } else if matches!(pkt.ty, Ty::Box(_) | Ty::BoxSlice(_)) {
                        Prov::Region(cur_region(ctx))
                    } else {
                        Prov::Unknown
                    };
                    ctx.last_recv = Some(pkt);
                } else {
                    // No recorded packet — the recv is an opaque live
                    // value of the declared payload type (v1 makes no
                    // claim about queue depth).
                    ty = want.unwrap_or(Ty::Void);
                }
            }
            (ty, prov)
        }
        "close" => {
            if args.len() != 1 {
                ctx.errs
                    .push(Diag::new(s, "`close` takes (channel)".to_string()));
                return (Ty::Void, Prov::Unknown);
            }
            // CHAN-004 S3b — `close` is endpoint-kind-agnostic: it
            // consumes the endpoint and, at runtime, turns in-flight
            // bounded sends into `Err` (closed-before-admission).
            // Latent `pending_send` state rides the Result place, not
            // the endpoint, so nothing here needs to touch it.
            let eps = {
                let beps = ep_place(&args[0], ChanKind::Bounded, scopes, ctx);
                if beps.is_empty() {
                    chan_gate(&args[0], s, scopes, ctx)
                } else {
                    ep_gated(beps, &args[0], s, ChanKind::Bounded, scopes, ctx)
                }
            };
            if !eps.is_empty() {
                consume_place(&args[0], scopes, ctx);
                // A `close` inside a task is a serialized endpoint op,
                // not an ownership steal — tag the eps so the parent's
                // bounded sends don't read the claim as a race.
                if let Some(t) = ctx.task.as_mut() {
                    for (ep, _) in &eps {
                        t.closed_eps.insert(ep.clone());
                    }
                }
            }
            (Ty::Void, Prov::Unknown)
        }
        _ => (Ty::Void, Prov::Unknown),
    }
}

/// Install a claimed packet's graph delta at binding key `key`:
/// capability packets become `auth` bindings, handle packets get cap
/// edges, value packets re-root their travelled edges/caps. Returns
/// the binding's authority kind (None for plain values and handles).
fn claim_packet(key: &str, pkt: &Packet, e: &Expr, scopes: &mut Scopes, ctx: &mut Ctx) {
    for (rel, t, f) in &pkt.edges {
        ctx.edges
            .insert((format!("{key}{rel}"), t.clone()), *f);
    }
    for (rel, t, cert) in &pkt.caps {
        ctx.caps
            .insert((format!("{key}{rel}"), t.clone()), *cert);
    }
    // Received targets must still exist — a handle to storage that
    // died in flight is stale at claim time.
    let mut targets: Vec<String> = Vec::new();
    for (_, t, ..) in pkt.edges.iter() {
        targets.push(t.clone());
    }
    for (_, t, _) in pkt.caps.iter() {
        targets.push(t.clone());
    }
    for t in targets {
        let root = path_root(&t);
        if root.starts_with('$') || root.starts_with('*') {
            continue;
        }
        let name = root.split('#').next().unwrap_or(root);
        let alive = match lookup(scopes, name) {
            Some(b) => {
                format!("{name}#{}", b.epoch) == root && consumed_state(ctx, &t).is_none()
            }
            None => false,
        };
        if !alive {
            push_diag(ctx, Diag::new(
                e.span(),
                format!(
                    "received `{}`: its target no longer exists (stale in transit)",
                    target_name(e)
                ),
            ));
        }
    }
}

/// Direct-access matrix (N4):
///   Read place  — conflicts with a live Mutable edge overlapping it
///   Write place — conflicts with ANY live edge overlapping it
///   New `&`     — conflicts with a live Mutable edge to the target
///   New `&mut`  — conflicts with ANY live edge to the target
fn access_check(e: &Expr, key: &str, write: bool, scopes: &mut Scopes, ctx: &mut Ctx) {
    access_check_excl(e, key, write, "", scopes, ctx)
}

/// `excl` skips one holder's edges — accesses THROUGH a reference or
/// capability aren't blocked by that holder's own edge (it is the
/// authority), but still conflict with every other holder's.
fn access_check_excl(
    e: &Expr,
    key: &str,
    write: bool,
    excl: &str,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    unguarded_check(e, key, scopes, ctx);
    transit_check(e, key, ctx);
    record_hold(
        ctx,
        key,
        if write { Auth::Write } else { Auth::Read },
    );
    for (h, f) in edges_to(ctx, key) {
        if h == excl {
            continue;
        }
        let msg = match (write, f.kind) {
            (true, _) => Some(format!(
                "cannot write to `{}`: it is borrowed",
                target_name(e)
            )),
            (false, Auth::Write) | (false, Auth::Own) => Some(format!(
                "cannot read `{}`: it is mutably borrowed",
                target_name(e)
            )),
            // An outstanding atomic hold races every UNSYNCHRONIZED
            // touch — only sibling atomic ops may overlap it.
            (false, Auth::Atomic) => Some(format!(
                "cannot read `{}`: atomically held",
                target_name(e)
            )),
            // A Sync claim on the domain blocks plain touches of the
            // mutex object itself.
            (false, Auth::Sync) => Some(format!(
                "cannot read `{}`: held by a lock",
                target_name(e)
            )),
            (false, Auth::Read) => None,
        };
        if let Some(m) = msg {
            // A conflicting TASK hold names its owner, not a borrow.
            let m = if ctx.tasks.contains_key(&h) {
                let tn = h.split('#').next().unwrap_or(&h);
                let how = if f.kind == Auth::Atomic {
                    "atomically held"
                } else {
                    "held"
                };
                format!(
                    "cannot {} `{}`: {how} by unjoined task `{tn}`",
                    if write { "write to" } else { "read" },
                    target_name(e),
                )
            } else {
                m
            };
            let mut d = Diag::new(e.span(), m);
            if let Some(l) = born_label(&h, &f, ctx) {
                d.labels.push(l);
            }
            push_diag(ctx, d);
            return;
        }
    }
}

/// Structural death + birth: kill the holder's outgoing edges (rebind
/// frees them), apply a `&mut` move, then insert edges to the targets.
fn record_edges(holder: &str, ri: &RefInfo, born: Option<Span>, ctx: &mut Ctx) {
    // Overwrite/rebind frees every edge the holder subtree carried.
    kill_set(ctx, holder.to_string(), Cert::Def);
    edges_retain(ctx, |(h, _), _| h != holder && !h.starts_with(&format!("{holder}."))
            && !h.starts_with(&format!("{holder}[")));
    if let Some(src) = &ri.move_from {
        // N54 — moving out a reserved view deserts the deferred call.
        if let Some(sp) = defer_conflict(ctx, src) {
            push_diag(ctx, Diag::new(
                born.unwrap_or(Span { line: 0, col: 0 }),
                format!(
                    "cannot move `{src}`: it is reserved by the `defer` at line {}",
                    sp.line
                ),
            ));
        }
        kill_set(ctx, src.clone(), Cert::Def);
        edges_retain(ctx, |(h, _), _| h != src);
    }
    let kind = if ri.mutable {
        Auth::Write
    } else {
        Auth::Read
    };
    let cert = if ri.targets.len() > 1 { Cert::Maybe } else { Cert::Def };
    for t in &ri.targets {
        edge_set(ctx, holder.to_string(), t.clone(), EdgeFact::new(kind, cert, born));
    }
}

/// Holder-keyed out-edges for liveness (`*r` needs r's edge live).
fn live_out_edge(ctx: &Ctx, holder: &str) -> bool {
    ctx.edges.keys().any(|(h, _)| h == holder)
}

// ---------- identity edges (N6B) ----------
//
// `caps` parallels `edges` but means only "h names t" — no authority,
// no liveness. Birth: `handle p` and handle copies; death: holder
// scope/rebind/overwrite only — a dead TARGET stays (stale at use).

/// Where a handle value points when it flows somewhere.
fn caps_of(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Option<Vec<String>> {
    caps_of_cert(e, scopes, ctx)
        .map(|ts| ts.into_iter().map(|(t, _)| t).collect())
}

/// `caps_of` with each fact's certainty: CHAN-004 S3b — a `Maybe`
/// cap is possible possession, never proof. Packet builders must
/// carry the cert through verbatim; promoting it to `Def` would mint
/// authority no path provably held (a mixed-category join emits a
/// cap on the shared ⊥ key precisely when one arm lacked it).
fn caps_of_cert(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> Option<Vec<(String, Cert)>> {
    match e {
        Expr::Handle(place, _) => Some(
            resolve_place(place, scopes, ctx)
                .into_iter()
                .map(|t| (t, Cert::Def))
                .collect(),
        ),
        Expr::Var(n, _) => {
            let b = lookup(scopes, n)?;
            if !matches!(b.ty, Ty::Handle(_)) {
                return None;
            }
            let key = format!("{n}#{}", b.epoch);
            Some(
                ctx.caps
                    .iter()
                    .filter(|((h, _), _)| *h == key)
                    .map(|((_, t), c)| (t.clone(), *c))
                    .collect(),
            )
        }
        Expr::Field(..) | Expr::Index(..) => {
            let key = place_id(e, scopes)?;
            let (t, _) = expr_ty_at(e, scopes, ctx);
            if !matches!(t, Ty::Handle(_)) {
                return None;
            }
            Some(
                ctx.caps
                    .iter()
                    .filter(|((h, _), _)| *h == key)
                    .map(|((_, t), c)| (t.clone(), *c))
                    .collect(),
            )
        }
        Expr::Join(inner, _) => {
            let key = place_id(inner, scopes)?;
            let eff = &ctx.tasks.get(&key)?.eff;
            if eff.ret_caps.is_empty() {
                None
            } else {
                // Effect-summary caps are claimed by the callee's
                // contract — definite for the caller.
                Some(eff.ret_caps.iter().map(|t| (t.clone(), Cert::Def)).collect())
            }
        }
        Expr::Call(n, _tg, args, _) => {
            if n == "replace" && !ctx.local_fns.contains(n) {
                let sp0 = e.span();
                return ctx
                    .replace_outs
                    .get(&(sp0.line, sp0.col))
                    .and_then(|(_, ts)| {
                        (!ts.is_empty())
                            .then(|| ts.iter().map(|t| (t.clone(), Cert::Def)).collect())
                    });
            }
            if let Some(sum) = ctx.sums.get(n) {
                if !sum.ret_caps.is_empty() {
                    let sp = e.span();
                    let empty = Vec::new();
                    let bases = ctx
                        .call_bases
                        .get(&(sp.line, sp.col))
                        .unwrap_or(&empty);
                    let mut ts: Vec<(String, Cert)> = Vec::new();
                    for t in &sum.ret_caps {
                        let mut ks = translate_key(t, bases);
                        if ks.is_empty() {
                            ks.push(t.clone());
                        }
                        for u in ks {
                            if !ts.iter().any(|(v, _)| *v == u) {
                                ts.push((u, Cert::Def));
                            }
                        }
                    }
                    return if ts.is_empty() { None } else { Some(ts) };
                }
            }
            // Same rule for externs: an opaque C return carries no
            // caller-side caps (`read(fd, &mut buf, n)` must not weld
            // `buf`'s claim onto the byte count).
            if matches!(ctx.fns.get(n), Some((_, _, true))) {
                return None;
            }
            let mut ts: Vec<(String, Cert)> = Vec::new();
            for a in args {
                if let Some(inner) = caps_of_cert(a, scopes, ctx) {
                    for (t, c) in inner {
                        match ts.iter_mut().find(|(v, _)| *v == t) {
                            // Two paths to one target keep the weaker
                            // certainty — Maybe wins over Def.
                            Some(slot) => {
                                if c == Cert::Maybe {
                                    slot.1 = Cert::Maybe;
                                }
                            }
                            None => ts.push((t, c)),
                        }
                    }
                }
            }
            if ts.is_empty() {
                None
            } else {
                Some(ts)
            }
        }
        // N28 — the arm-tail union captured while arm scopes lived.
        Expr::Match { span, .. } => ctx
            .match_yields
            .get(&(span.line, span.col))
            .and_then(|y| y.caps.clone()),
        _ => None,
    }
}

/// Rebind a cap holder: old identity edges die, new ones attach.
fn record_caps(holder: &str, targets: &[String], ctx: &mut Ctx) {
    capkill_set(ctx, holder.to_string(), Cert::Def);
    caps_retain(ctx, |(h, _), _| {
        h != holder
            && !h.starts_with(&format!("{holder}."))
            && !h.starts_with(&format!("{holder}["))
    });
    let cert = if targets.len() > 1 { Cert::Maybe } else { Cert::Def };
    for t in targets {
        cap_set(ctx, holder.to_string(), t.clone(), cert);
    }
}

fn cur_region(ctx: &Ctx) -> u32 {
    (ctx.regs.len() - 1) as u32
}

/// Record the provenance of a reference stored at a place: locals via
/// their binding, field/index paths via the path map.
fn record_prov(e: &Expr, prov: &Prov, scopes: &mut Scopes, ctx: &mut Ctx) {
    match e {
        Expr::Var(n, _) => {
            if let Some(b) = lookup_mut(scopes, n) {
                b.prov = prov.clone();
            }
        }
        _ => {
            if let Some(k) = place_id(e, scopes) {
                prov_set(ctx, k, prov.clone());
            }
        }
    }
}

/// N22 — install the tag the init produced onto the bound place.
/// EnumLit sets it directly; a copied enum place propagates the
/// source's; any other enum-typed value records "initialized, case
/// unknown"; non-enum targets drop stale info.
fn record_tag(init: Option<&Expr>, key: &str, ty: &Ty, ctx: &mut Ctx) {
    if !matches!(ty, Ty::Struct(n) if ctx.enums.contains_key(n)) {
        tag_unset(ctx, key);
        ctx.last_tag = None;
        return;
    }
    match init {
        Some(
            Expr::EnumLit { .. }
            | Expr::Var(..)
            | Expr::Field(..)
            | Expr::Index(..)
            | Expr::Deref(..),
        ) => match ctx.last_tag.take() {
            Some(t) => {
                tag_set(ctx, key.to_string(), t);
            }
            None => {
                tag_set(ctx, key.to_string(), None);
            }
        },
        _ => {
            ctx.last_tag = None;
            tag_set(ctx, key.to_string(), None);
        }
    }
}

/// After a `S{ r: &x, .. }` fill, lift each ref field's prov into the
/// target's path map (`s.r` etc.). Called by Let/Assign right after the
/// literal's expr_tv populated ctx.last_lit.
/// `{key}.{rel}` — except a rel already starting `[` (an element
/// family from a moved array/slice place) joins without the dot.
fn join_rel(key: &str, rel: &str) -> String {
    if rel.is_empty() {
        key.to_string()
    } else if rel.starts_with('[') {
        format!("{key}{rel}")
    } else {
        format!("{key}.{rel}")
    }
}

fn record_lit_fields(key: &str, born: Option<Span>, ctx: &mut Ctx) {
    if let Some(provs) = ctx.last_lit.take() {
        for (fn_, prov) in provs {
            prov_set(ctx, join_rel(key, &fn_), prov);
        }
    }
    // Shallow rels first — `record_edges(h)` kills `h`'s subtree, so
    // `""`/`f` must land before `f.g` survives.
    let mut edges = std::mem::take(&mut ctx.lit_edges);
    edges.sort_by_key(|(r, _)| r.matches(['.', '[']).count());
    for (rel, ri) in edges {
        let h = join_rel(key, &rel);
        if ri.foreign {
            foreign_set(ctx, h.clone());
        }
        record_edges(&h, &ri, born, ctx);
    }
    for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
        record_caps(&join_rel(key, &rel), &ts, ctx);
    }
    // N40 — the moved value's interior temporal/fact state, same
    // re-root: a dead field stays dead, a known length survives, a
    // nested enum keeps its tag.
    for (rel, f) in std::mem::take(&mut ctx.lit_dead) {
        cons_set(ctx, join_rel(key, &rel), f);
    }
    for (rel, l) in std::mem::take(&mut ctx.lit_lens) {
        slen_set(ctx, join_rel(key, &rel), l);
    }
    for (rel, t) in std::mem::take(&mut ctx.lit_tags) {
        tag_set(ctx, join_rel(key, &rel), t);
    }
    for rel in std::mem::take(&mut ctx.lit_csafe) {
        csafe_set(ctx, join_rel(key, &rel));
    }
}

/// The fact-side of a store landing `value` at resolved key `k` —
/// shared by `=` and `replace`: edges/caps/lit/tag/dead/init/len/cstr
/// bookkeeping all update the same way. The overwrite law lives at
/// the call sites — `replace` exempts itself by handing the place's
/// old value to its result, so nothing is silently lost.
fn install_store(
    k: &str,
    value: &Expr,
    tt: &Ty,
    vtv: &(Ty, Prov),
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    let reslot: Option<String> = match value {
        Expr::EnumLit { variant, .. } => Some(variant.clone()),
        _ => None,
    };
    let tvars: Option<Vec<String>> = match tt {
        Ty::Struct(n) => ctx
            .enums
            .get(n)
            .map(|vs| vs.iter().map(|(v, _)| v.clone()).collect()),
        _ => None,
    };
    // Rebind/overwrite kills the holder's old edges, then the new
    // value's refs attach (RefInfo applies moves).
    if let Some(ri) = refs_of(value, scopes, ctx) {
        record_edges(k, &ri, Some(value.span()), ctx);
    } else {
        kill_set(ctx, k.to_string(), Cert::Def);
        edges_retain(ctx, |(h, _), _| {
            h != k && !h.starts_with(&format!("{k}.")) && !h.starts_with(&format!("{k}["))
        });
    }
    // Same for identity edges — `h = handle y` moves the name, never
    // borrows.
    if let Some(ts) = caps_of(value, scopes, ctx) {
        record_caps(k, &ts, ctx);
    } else {
        capkill_set(ctx, k.to_string(), Cert::Def);
        caps_retain(ctx, |(h, _), _| {
            h != k && !h.starts_with(&format!("{k}.")) && !h.starts_with(&format!("{k}["))
        });
    }
    if matches!(value, Expr::StructLit(..) | Expr::EnumLit { .. })
        || !ctx.lit_edges.is_empty()
        || !ctx.lit_caps.is_empty()
        || !ctx.lit_dead.is_empty()
        || !ctx.lit_lens.is_empty()
        || !ctx.lit_tags.is_empty()
        || ctx.last_lit.is_some()
    {
        record_lit_fields(k, Some(value.span()), ctx);
    }
    record_tag(Some(value), k, tt, ctx);
    // Reassignment resurrects: a store to `p` revives `p` and every
    // path under it; `p.f = v` revives `.f`. An enum reslot is
    // narrower — `e = Enum::W` revives only `.W`'s subtree; a
    // different variant's payload died with the old tag and stays
    // dead.
    cons_retain(ctx, |c, _| {
        if c != k && !c.starts_with(&format!("{k}.")) && !c.starts_with(&format!("{k}[")) {
            return true;
        }
        if let (Some(vs), Some(v)) = (&tvars, &reslot) {
            let rest = &c[k.len() + 1..];
            let seg = rest.split(['.', '[']).next().unwrap_or(rest);
            if vs.iter().any(|vn| vn == seg) && seg != v.as_str() {
                return true;
            }
        }
        false
    });
    // N18 — the store lands a value: `x = v` covers `x`'s whole
    // subtree, `p.f = v` marks only `p#e.f`, and a resurrected place
    // is Init+Live again. `$arg`-rooted stores are the callee's
    // definite writes — they export through the FnSummary.
    init_set(ctx, k.to_string(), Cert::Def);
    if k.starts_with("$arg") {
        ctx.init_writes.insert(k.to_string());
    }
    // Slice bookkeeping follows the value: a `slice(p, n)` value makes
    // the target foreign; a re-slice updates the statically-known
    // length (and an unknowable one erases it — dynamic checks still
    // apply).
    if matches!(vtv.0, Ty::Slice { .. }) {
        match slice_len_of(value, scopes, ctx) {
            Some(len) => {
                slen_set(ctx, k.to_string(), len);
            }
            None => {
                unslen(ctx, k);
            }
        }
        if refs_of(value, scopes, ctx)
            .map(|ri| ri.foreign)
            .unwrap_or(false)
        {
            foreign_set(ctx, k.to_string());
        } else {
            unforeign(ctx, k);
        }
    }
    // `str` bookkeeping follows the value too: byte length where
    // provable, NUL-provenance only when the source still carries it.
    if vtv.0 == Ty::Str {
        match slice_len_of(value, scopes, ctx) {
            Some(len) => {
                slen_set(ctx, k.to_string(), len);
            }
            None => {
                unslen(ctx, k);
            }
        }
        let safe = match value {
            Expr::Str(..) => true,
            Expr::Var(vn, _) => lookup(scopes, vn)
                .map(|vb| ctx.cstr_safe.contains(&format!("{vn}#{}", vb.epoch)))
                .unwrap_or(false),
            _ => false,
        };
        if safe {
            csafe_set(ctx, k.to_string());
        } else {
            uncsafe(ctx, k);
        }
        if let Some(b) = lookup_mut(scopes, {
            let r = path_root(k);
            r.split('#').next().unwrap_or(r)
        }) {
            if b.prov == Prov::Unknown {
                b.prov = Prov::Region(cur_region(ctx));
            }
        }
    }
}

/// Ref/cap/prov state living under `{key}.<variant>` — an enum place's
/// payload paths — re-keyed relative (`r#3.Ok.f` -> `Ok.f`) so a
/// return summary can hand them to the caller's `r.<variant>` paths.
fn payload_edges(key: &str, ctx: &Ctx) -> Vec<(String, RefInfo)> {
    let mut out: Vec<(String, RefInfo)> = Vec::new();
    for ((h, t), f) in &ctx.edges {
        if let Some(rel) = h.strip_prefix(&format!("{key}.")) {
            let ri = if let Some((_, ri)) = out.iter_mut().find(|(r, _)| r == rel) {
                ri
            } else {
                out.push((
                    rel.to_string(),
                    RefInfo {
                        targets: Vec::new(),
                        mutable: false,
                        move_from: None,
                        foreign: ctx.foreign.contains(h),
                    },
                ));
                &mut out.last_mut().unwrap().1
            };
            if !ri.targets.contains(t) {
                ri.targets.push(t.clone());
            }
            ri.mutable |= f.kind == Auth::Write;
        }
    }
    // Map-order collection — sort for canonical replay order.
    out.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, ri) in &mut out {
        ri.targets.sort();
    }
    out
}

fn payload_caps(key: &str, ctx: &Ctx) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(String, Vec<String>)> = Vec::new();
    for ((h, t), _) in &ctx.caps {
        if let Some(rel) = h.strip_prefix(&format!("{key}.")) {
            if let Some((_, ts)) = out.iter_mut().find(|(r, _)| r == rel) {
                if !ts.contains(t) {
                    ts.push(t.clone());
                }
            } else {
                out.push((rel.to_string(), vec![t.clone()]));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    for (_, ts) in &mut out {
        ts.sort();
    }
    out
}

fn payload_provs(key: &str, ctx: &Ctx) -> Vec<(String, Prov)> {
    let mut out: Vec<(String, Prov)> = ctx
        .path_provs
        .iter()
        .filter_map(|(k, p)| {
            k.strip_prefix(&format!("{key}."))
                .map(|rel| (rel.to_string(), p.clone()))
        })
        .collect();
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

/// The write rule: storing `val` at `target`. Type-checked by callers;
/// this enforces (a) target is a place, (b) no writes through `&`,
/// (c) provenance outlives the location's owner region.
/// Does a write-path hop index through a read-only (`slice`, not
/// `slice mut`) view? `s[i] = v` and `s[i].f = v` both die here.
fn read_only_slice_hop(e: &Expr, scopes: &mut Scopes, ctx: &Ctx) -> bool {
    match e {
        Expr::Index(b, ..) => {
            matches!(
                expr_ty_at(b, scopes, ctx).0,
                Ty::Slice { mutable: false, .. }
            ) || read_only_slice_hop(b, scopes, ctx)
        }
        Expr::Field(b, ..) => read_only_slice_hop(b, scopes, ctx),
        _ => false,
    }
}

fn check_store(
    target: &Expr,
    val: &Expr,
    vtv: &(Ty, Prov),
    scopes: &mut Scopes,
    ctx: &mut Ctx,
) {
    match target {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
        _ => {
            push_diag(ctx, Diag::new(
                target.span(),
                "assignment target is not a place".to_string(),
            ));
            return;
        }
    }
    // Capability law for writes: a `read` param can't mutate, and a
    // moved param can't be touched at all. Then exclusivity over the
    // RESOLVED targets — `*m = v` sees every OTHER holder's edge into
    // m's referent, so `let s = &*m` genuinely freezes m.
    if let Some((root, auth)) = root_auth(target, scopes) {
        match auth {
            Some(Auth::Read) => {
                push_diag(ctx, Diag::new(
                    target.span(),
                    format!("cannot write through read capability `{root}`"),
                ));
                return;
            }
            Some(_) if !live_out_edge(ctx, &format!("{root}#{}", root_epoch(target, scopes))) => {
                push_diag(ctx, Diag::new(
                    target.span(),
                    format!("use of moved authority `{root}`"),
                ));
                return;
            }
            _ => {}
        }
    }
    let holders = path_holders(target, scopes, ctx);
    let guard = root_guard(target, scopes);
    for t in resolve_place(target, scopes, ctx) {
        // N54 — rebinding a reserved place (or an ancestor of one)
        // would leave the deferred call holding the wrong operand.
        if let Some(sp) = defer_conflict(ctx, &t) {
            push_diag(ctx, Diag::new(
                target.span(),
                format!(
                    "cannot write to `{}`: it is reserved by the `defer` at line {}",
                    target_name(target),
                    sp.line
                ),
            ));
            return;
        }
        match &guard {
            // A guarded store's claim is the DOMAIN (Sync), not a plain
            // Write hold — same-domain tasks may overlap it. The
            // foreign-edge scan lives inside guarded_check too.
            Some((gk, dom, _, srcs)) => {
                guarded_check(target, &t, true, gk, dom, srcs, scopes, ctx);
                continue;
            }
            None => {
                unguarded_check(target, &t, scopes, ctx);
                transit_check(target, &t, ctx);
                record_hold(ctx, &t, Auth::Write);
            }
        }
        for (h, f) in edges_to(ctx, &t) {
            if holders.contains(&h) {
                continue;
            }
            let _ = f;
            let m = if ctx.tasks.contains_key(&h) {
                let tn = h.split('#').next().unwrap_or(&h);
                format!(
                    "cannot write to `{}`: held by unjoined task `{tn}`",
                    target_name(target)
                )
            } else {
                format!("cannot write to `{}`: it is borrowed", target_name(target))
            };
            push_diag(ctx, Diag::new(target.span(), m));
            return;
        }
    }
    // `s.bytes = v` — the pseudo-member is a view, not a place:
    // rebinding it would smuggle bytes past `str_from_utf8`'s gate.
    if let Expr::Field(b, name, _) = target {
        if name == "bytes" && matches!(expr_ty_at(b, scopes, ctx).0, Ty::Str) {
            push_diag(ctx, Diag::new(
                target.span(),
                "cannot assign to `.bytes` — a `str`'s view is not a place".to_string(),
            ));
            return;
        }
    }
    // `s[i] = v` needs `slice mut` — the holder-skip above only says
    // the slice's own edge isn't foreign; the TYPE still decides
    // whether the span may be written through it.
    if read_only_slice_hop(target, scopes, ctx) {
        push_diag(ctx, Diag::new(
            target.span(),
            "cannot write through `slice` (needs `slice mut`)".to_string(),
        ));
        return;
    }
    // `*r = v` needs `&mut`; the place's root ref must be mutable.
    // `*h = v` needs a live target — the handle grants no authority
    // and no liveness, so the staleness check is the rule here.
    if let Expr::Deref(inner, s) = target {
        let (rt, _) = expr_tv(inner, scopes, ctx);
        match &rt {
            Ty::Ref { mutable: false, .. } => {
                ctx.errs
                    .push(Diag::new(*s, "cannot write through `&` (needs `&mut`)".to_string()));
            }
            Ty::Ref { mutable: true, .. } => {}
            Ty::Handle(_) => stale_check(inner, target, scopes, ctx),
            _ => {} // type error already reported
        }
    }
    let req = place_req(target, scopes, ctx);
    if let Ty::Ref { .. } | Ty::Slice { .. } = vtv.0 {
        if !outlives(&vtv.1, &req, &ctx.regs) {
            push_diag(ctx, Diag::new(
                val.span(),
                format!(
                    "reference escapes region: value lives in {}, but `{}` requires it to outlive {}",
                    vtv.1.describe(&ctx.regs),
                    target_name(target),
                    req.describe(&ctx.regs),
                ),
            ));
            return;
        }
        record_prov(target, &vtv.1, scopes, ctx);
    }
}

/// The root variable a place path hangs from (`(*r).f` → its root is
/// the deref — no bind): for capability-auth checks on stores.
fn root_var(e: &Expr) -> Option<&str> {
    match e {
        Expr::Var(n, _) => Some(n.as_str()),
        Expr::Field(b, ..) | Expr::Index(b, ..) => root_var(b),
        _ => None,
    }
}

/// (root name, its capability auth) — None auth = ordinary local.
fn root_auth<'a>(e: &Expr, scopes: &'a Scopes) -> Option<(String, Option<Auth>)> {
    let n = root_var(e)?;
    lookup(scopes, n).map(|b| (n.to_string(), b.auth))
}

fn root_epoch(e: &Expr, scopes: &mut Scopes) -> u32 {
    root_var(e)
        .and_then(|n| lookup(scopes, n))
        .map(|b| b.epoch)
        .unwrap_or(0)
}

fn target_name(e: &Expr) -> String {
    match e {
        Expr::Var(n, _) => n.clone(),
        Expr::Field(b, f, _) => format!("{}.{}", target_name(b), f),
        Expr::Index(b, i, _) => match &**i {
            Expr::Int(k, _) => format!("{}[{k}]", target_name(b)),
            _ => format!("{}[i]", target_name(b)),
        },
        Expr::IndexRange { base, .. } => format!("{}[..]", target_name(base)),
        Expr::Deref(b, _) => format!("*{}", target_name(b)),
        _ => "?".into(),
    }
}

/// Merge the environment after a branch: bind-provs and path-provs join
/// pointwise (stack shapes are identical — branch-locals are scoped).
fn merge_envs(a: &Scopes, b: &Scopes, ctx: &mut Ctx) -> Scopes {
    let mut out = a.clone();
    for (i, scope) in a.iter().enumerate() {
        for (name, bind) in scope {
            if let Prov::Region(id) = &bind.prov {
                if *id as usize >= ctx.regs.len() {
                    continue; // dead region — store check already ruled
                }
            }
            match b[i].get(name) {
                Some(other) => {
                    let j = join_prov(&bind.prov, &other.prov, &ctx.regs);
                    let e = out[i].get_mut(name).unwrap();
                    e.prov = j;
                    // A `promote` on only one side doesn't count — the
                    // binding keeps the deeper (unpromoted) owner.
                    e.owner = e.owner.max(other.owner);
                }
                // Present on only one branch — only `promote` can make
                // a binding outlive its block, so this is a conditional
                // promote: drop it, the join must stay conservative.
                None => {
                    out[i].remove(name);
                }
            }
        }
    }
    out
}

/// N24A — expire a statement-scoped materialization temp (`$rvN`,
/// emitted by the lowering pass): drop the binding and every place
/// state keyed under its root — the same structural death a binding
/// gets at scope exit. No promote/task cases apply: user code can't
/// name a temp, so nothing can promote or capture one.
fn kill_temp(name: &str, scopes: &mut Scopes, ctx: &mut Ctx) {
    let Some(b) = scopes.last_mut().and_then(|m| m.remove(name)) else {
        return;
    };
    let dead: HashSet<String> = HashSet::from([format!("{name}#{}", b.epoch)]);
    edges_retain(ctx, |(h, t), _| {
        !dead.contains(&path_root(h).to_string()) && !dead.contains(&path_root(t).to_string())
    });
    caps_retain(ctx, |(h, _), _| !dead.contains(&path_root(h).to_string()));
    provs_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
    cons_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
    init_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
    tag_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
}

fn check_block(stmts: &[Stmt], scopes: &mut Scopes, ctx: &mut Ctx, ret: &Ty) {
    check_block_y(stmts, scopes, ctx, ret, None, false);
}

/// check_block + an optional arm yield (N28): with `yield_tail`, a
/// trailing `Stmt::Expr` is checked as an expression under `want`
/// while the scope is still live — payload binders and `$rv` temps
/// are visible to it, and the temps die right after (the tail IS
/// "the next real statement" N24A's rule expects). Diverged arms
/// leave the return `None`.
/// N54 — every place key a whitelisted `defer` operand reads: the
/// operand's own targets plus the index operands it consults
/// (`v.buf[i]` freezes `i` too — the deferred call re-reads it).
fn collect_place_keys(e: &Expr, scopes: &mut Scopes, ctx: &Ctx, out: &mut Vec<String>) {
    match e {
        Expr::Index(b, i, _) => {
            collect_place_keys(b, scopes, ctx, out);
            collect_place_keys(i, scopes, ctx, out);
            out.extend(resolve_place(e, scopes, ctx));
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            collect_place_keys(base, scopes, ctx, out);
            if let Some(l) = lo {
                collect_place_keys(l, scopes, ctx, out);
            }
            if let Some(h) = hi {
                collect_place_keys(h, scopes, ctx, out);
            }
            out.extend(resolve_place(e, scopes, ctx));
        }
        Expr::Var(..) | Expr::Field(..) | Expr::Deref(..) => {
            out.extend(resolve_place(e, scopes, ctx));
        }
        _ => {}
    }
}

/// N54 — does any live `defer` freeze a place rooted at name `n`?
/// A `let n`/binder shadowing it would re-point the deferred call
/// (which re-resolves names at each exit edge) at a different
/// binding — reject the shadow instead.
fn defer_name_conflict(ctx: &Ctx, n: &str) -> Option<Span> {
    ctx.defer_res.iter().find_map(|(k, sp)| {
        let root = path_root(k);
        let name = root.split('#').next().unwrap_or(root);
        (name == n).then_some(*sp)
    })
}

/// N54 — is `t` a place a live `defer` froze, or a strict ancestor of
/// one? Writing through or INTO a reserved place stays legal (the
/// operand's identity is unchanged); moving it, consuming it,
/// rebinding it, or overwriting an ancestor does not.
fn defer_conflict(ctx: &Ctx, t: &str) -> Option<Span> {
    ctx.defer_res.iter().find_map(|(k, sp)| {
        (k == t
            || k.starts_with(&format!("{t}."))
            || k.starts_with(&format!("{t}[")))
        .then_some(*sp)
    })
}

/// N54 — run one defer record's call with real effects: the rec's
/// reservations drop first so the replay isn't blocked by its own
/// freeze. Consumes discharge R19; an `own` param transfer leaves
/// ownership with the callee, as the user wrote.
fn replay_defer(rec: &DeferRec, scopes: &mut Scopes, ctx: &mut Ctx) {
    let sp = rec.expr.span();
    emit(format!("(defer-run @{}:{})", sp.line, sp.col));
    for k in rec.keys() {
        undres(ctx, k);
    }
    expr_tv(&rec.expr, scopes, ctx);
}

/// N54 — every live defer across every scope, innermost first —
/// `return` and `?` unwind the whole stack. Marks apply to the real
/// ctx so the abandon audit sees the discharge.
fn replay_all_defers(scopes: &mut Scopes, ctx: &mut Ctx) {
    for level in (0..ctx.defers.len()).rev() {
        for i in (0..ctx.defers[level].len()).rev() {
            let rec = ctx.defers[level][i].clone();
            replay_defer(&rec, scopes, ctx);
        }
    }
}

/// N54 — pop this scope's defer list parallel to its scope map:
/// replay LIFO on a live path while the bindings still resolve, drop
/// reservations either way. Terminated (`process.exit`) and diverged
/// (each exit already replayed) paths skip execution.
fn pop_scope_defers(scopes: &mut Scopes, ctx: &mut Ctx) {
    let list = ctx.defers.pop().unwrap_or_default();
    if ctx.terminated || ctx.diverged {
        for rec in &list {
            for k in rec.keys() {
                undres(ctx, k);
            }
        }
        return;
    }
    for rec in list.iter().rev() {
        replay_defer(rec, scopes, ctx);
    }
}

/// N54 — defers are PATH state: an arm that unwinds mid-block
/// (`break`/`return`/`?` inside `if`/`match`) drains frames the
/// surviving paths still owe. Union-merge two stacks frame-by-frame;
/// a record present on either path stays pending. Identical frames
/// (the common case — both paths live, no mid-arm exit) merge to
/// themselves; records dedup by their call's span.
fn merge_defer_frames(a: &[Vec<DeferRec>], b: &[Vec<DeferRec>]) -> Vec<Vec<DeferRec>> {
    let n = a.len().max(b.len());
    (0..n)
        .map(|i| {
            let mut frame = a.get(i).cloned().unwrap_or_default();
            for rec in b.get(i).cloned().unwrap_or_default() {
                if !frame.iter().any(|r| r.expr.span() == rec.expr.span()) {
                    frame.push(rec);
                }
            }
            frame
        })
        .collect()
}

/// N54 — the reservation map joins by union: a key frozen on either
/// surviving path stays frozen (its defer may still replay).
fn merge_defer_res(
    a: &HashMap<String, Span>,
    b: &HashMap<String, Span>,
) -> HashMap<String, Span> {
    let mut m = a.clone();
    m.extend(b.iter().map(|(k, v)| (k.clone(), *v)));
    m
}

fn check_block_y(
    stmts: &[Stmt],
    scopes: &mut Scopes,
    ctx: &mut Ctx,
    ret: &Ty,
    want: Option<&Ty>,
    yield_tail: bool,
) -> Option<ArmYieldInfo> {
    rec("(scope)".to_string());
    scopes.push(HashMap::new());
    ctx.defers.push(Vec::new());
    // N24A — a temp `let $rvN = ..` belongs to the NEXT real statement
    // (the lowering emits it immediately before its only use), so it
    // dies as soon as that statement checks. Otherwise a borrow held
    // by a temp — `to_cstr(heap, s.as_str())` → `let $rv0 =
    // s.as_str(); ..` — would keep `s` borrowed until scope end and
    // reject `s.destroy()` further down.
    let mut pending: Vec<String> = Vec::new();
    let mut yield_info = None;
    for (i, s) in stmts.iter().enumerate() {
        // N32 — nothing on a terminated path executes: checking it
        // against the approximated post-exit state would only
        // produce false positives (`exit(1); return x` can't read
        // an uninit `x` — the read is unreachable). `diverged` is
        // NOT skipped: dead `return`/`break` tails keep their
        // historical checking.
        if ctx.terminated {
            break;
        }
        let is_tail = yield_tail
            && i + 1 == stmts.len()
            && matches!(s, Stmt::Expr { .. });
        if is_tail {
            if let Stmt::Expr { expr, .. } = s {
                if !ctx.diverged {
                    let (t, p) = expr_tv_w(expr, want, scopes, ctx);
                    // The yielded value moves out on this path: an
                    // owned tail (`=> p`) is consumed HERE so the join
                    // sees it consumed on this arm only — not Def-
                    // consumed for all paths.
                    push_place_lit(expr, scopes, ctx);
                    consume_owned_src(expr, scopes, ctx);
                    // Ref/cap/len info must be captured while arm-
                    // locals are still alive — after scope exit the
                    // yield's targets (e.g. a payload binder's slice)
                    // are unresolvable.
                    yield_info = Some(ArmYieldInfo {
                        ty: t,
                        prov: p,
                        refs: refs_of(expr, scopes, ctx),
                        caps: caps_of_cert(expr, scopes, ctx),
                        len: slice_len_of(expr, scopes, ctx),
                        lit_edges: std::mem::take(&mut ctx.lit_edges),
                        lit_caps: std::mem::take(&mut ctx.lit_caps),
                        lit_provs: ctx.last_lit.take().unwrap_or_default(),
                        lit_dead: std::mem::take(&mut ctx.lit_dead),
                        lit_lens: std::mem::take(&mut ctx.lit_lens),
                        lit_tags: std::mem::take(&mut ctx.lit_tags),
                    });
                }
            }
        } else {
            check_stmt(s, scopes, ctx, ret);
        }
        if let Stmt::Let { name, .. } = s {
            if name.starts_with("$rv") {
                pending.push(name.clone());
                continue;
            }
        }
        for n in pending.drain(..) {
            kill_temp(&n, scopes, ctx);
        }
    }
    // Any leftover temps (a materialization that ended the block — the
    // trailing-expr idiom) die with the scope below.
    // Scope exit splits the popped bindings: `promote`d ones (owner
    // already moved shallower than the region this block belongs to)
    // percolate into the enclosing scope; the rest die — killing every
    // edge born from or pointing into them (structural death, N4).
    // Holder and target roots are `name#epoch`, so shadowed outer
    // bindings survive.
    // N54 — deferred calls fire here, before the bindings die:
    // operands must still resolve to their names.
    pop_scope_defers(scopes, ctx);
    let popped = scopes.pop().unwrap_or_default();
    let end = stmts.last().map(|s| s.span());
    expire_scope(popped, scopes, ctx, end, "the enclosing scope ends");
    rec("(pop)".to_string());
    yield_info
}

/// The structural death half of scope exit (N4): `promote`d bindings
/// (owner already shallower than this region) percolate outward; the
/// rest die, killing every edge born from or pointing into them —
/// with the task-escape and unjoined-task diagnostics that belong to
/// a binding dying early. Shared by `}` and N25D's `break`/`continue`,
/// whose exit runs the same death mid-block.
fn expire_scope(
    popped: HashMap<String, Bind>,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
    exit: Option<Span>,
    phrase: &str,
) {
    let cur = cur_region(ctx);
    let (survivors, mut dead_binds): (Vec<_>, Vec<_>) = popped
        .into_iter()
        .partition(|(_, b)| b.owner < cur);
    // `popped` is a scope HashMap — iteration order is map order.
    // Diagnostics/abandon events sort by place key so the log is
    // canonical.
    dead_binds.sort_by(|a, b| a.0.cmp(&b.0));
    if let Some(outer) = scopes.last_mut() {
        for (n, b) in survivors {
            outer.insert(n, b);
        }
    }
    let dead: HashSet<String> = dead_binds
        .iter()
        .map(|(n, b)| format!("{n}#{}", b.epoch))
        .collect();
    // Task-escape: a SURVIVING task's hold on a dying place would
    // outlive its target — `region r { let x; t = spawn { read x } }`
    // with `t` outside is exactly this. Terminated paths (N32 — the
    // process died; `exit` performs no unwinding) skip both task
    // diagnostics: there is no continuation the obligation protects.
    if !ctx.terminated {
        let mut escapes: Vec<(String, String)> = ctx
            .edges
            .keys()
            .filter(|(h, t)| {
                ctx.tasks.contains_key(h.as_str())
                    && !dead.contains(h.as_str())
                    && dead.contains(path_root(t))
            })
            .cloned()
            .collect();
        escapes.sort();
        for (h, t) in escapes
        {
            let tn = h.split('#').next().unwrap_or(&h).to_string();
            let sp = ctx.tasks.get(&h).map(|sr| sr.span).unwrap_or(Span { line: 0, col: 0 });
            let tgt = path_root(&t).split('#').next().unwrap_or(path_root(&t));
            push_diag(ctx, Diag::new(
                sp,
                format!(
                "task `{tn}` escapes its target's region: it still holds `{tgt}`, which dies at the end of this scope"
            ),
            ));
        }
        // A task binding that dies unjoined drops a suspended
        // transform — the child's claimed effects never land. Only
        // `consume`d (joined) or maybe-joined tasks are excused.
        let mut dead_sorted: Vec<&String> = dead.iter().collect();
        dead_sorted.sort();
        for d in dead_sorted {
            if ctx.tasks.contains_key(d) && consumed_state(ctx, d).is_none() {
                let tn = d.split('#').next().unwrap_or(d);
                let sp = ctx.tasks.get(d).map(|sr| sr.span).unwrap_or(Span { line: 0, col: 0 });
                push_diag(ctx, Diag::new(
                    sp,
                    format!("task `{tn}` goes out of scope without `join`"),
                ));
            }
        }
        // Live own authority may not die with its place — scope exit
        // is the same abandonment law as overwrite (N45R). On a
        // diverged path the `}` is never reached (the return/try
        // audits already ran at their own sites), so there is no
        // exit to abandon at.
        if !ctx.diverged {
            flag_abandoned(exit, phrase, dead_binds.iter().map(|(n, b)| (n, b)), ctx);
        }
        // CHAN-004 S3b — a dying Result place still carrying latent
        // sends is an unexamined result: its conditional ownership
        // was never resolved.
        let mut pending_dead: Vec<String> = ctx
            .out
            .pending_send
            .keys()
            .filter(|k| dead.contains(path_root(k)))
            .cloned()
            .collect();
        pending_dead.sort();
        for k in pending_dead {
            let n = k.split('#').next().unwrap_or(&k);
            push_diag(ctx, Diag::new(
                exit.unwrap_or(Span { line: 0, col: 0 }),
                format!(
                    "bounded-send result `{n}` was never examined — match on its `Ok`/`Err`"
                ),
            ));
            ctx.out.pending_send.remove(&k);
        }
    }
    edges_retain(ctx, |(h, t), _| {
        !dead.contains(&path_root(h).to_string()) && !dead.contains(&path_root(t).to_string())
    });
    // Cap edges die only with their HOLDER — a dead target leaves a
    // live, stale edge (that's the identity/lifetime split's point).
    caps_retain(ctx, |(h, _), _| !dead.contains(&path_root(h).to_string()));
    provs_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
    // Consumed state dies with the place — but a handle that names the
    // dead key goes stale at use, which is the point of the split.
    cons_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
    // Init state dies the same way — a resurrected shadow binding
    // starts Uninit again, not inherited.
    init_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
    tag_retain(ctx, |k, _| !dead.contains(&path_root(k).to_string()));
}

/// N45R — flag bindings that still hold live `own` authority at the
/// point their place dies or the path abandons them: live owning
/// authority may not simply disappear (strict discharge). `read`/
/// `write` params alias the caller's storage, so their end abandons
/// nothing; `own` params and locals are the real holders. Keys already
/// reported on an earlier path are skipped so a `}`-pop after a
/// flagged `return` doesn't double-report the same place.
fn flag_abandoned<'b>(
    exit: Option<Span>,
    phrase: &str,
    binds: impl Iterator<Item = (&'b String, &'b Bind)>,
    ctx: &mut Ctx,
) {
    // `binds` walks scope HashMaps — sort by place key so abandon
    // reports (and the log) are canonical.
    let mut binds: Vec<(&String, &Bind)> = binds.collect();
    binds.sort_by(|a, b| {
        format!("{}#{}", a.0, a.1.epoch).cmp(&format!("{}#{}", b.0, b.1.epoch))
    });
    for (n, b) in binds {
        if matches!(b.auth, Some(Auth::Read) | Some(Auth::Write)) {
            continue;
        }
        let key = format!("{n}#{e}", e = b.epoch);
        if ctx.abandoned.contains(&key) {
            continue;
        }
        // An `own` param's place facts live under its `$argN` summary
        // key — the bind edges `n#e -> $argN` and every op resolves
        // through it. Probe the canonical key or a consumed `return p`
        // reads as live here.
        let probe = ctx
            .edges
            .keys()
            .find(|(h, _)| h == &key)
            .filter(|(_, t)| t.starts_with("$arg"))
            .map(|(_, t)| t.clone())
            .unwrap_or_else(|| key.clone());
        if let Some(held) = live_own_at(&probe, &b.ty, ctx) {
            let tail = held.strip_prefix(&probe).unwrap_or(&held);
            rec(format!("(abandon {})", qs(&key)));
            ctx.abandoned.insert(key);
            // `$rv`/`$tmp`/`$arg` names are compiler-internal — render
            // the subject generically, the held path without its root.
            let internal = n.starts_with('$');
            let subj = if internal {
                "the `match`ed value".to_string()
            } else {
                format!("`{n}`")
            };
            let what = if tail.is_empty() {
                "live resources".to_string()
            } else if internal {
                format!("`{}`", tail.trim_start_matches('.'))
            } else {
                format!("`{n}{tail}`")
            };
            let mut d = Diag::new(
                b.decl,
                format!("cannot leave scope while {subj} still owns {what}"),
            );
            if let Some(sp) = exit {
                let rel = if internal {
                    "the `match`ed value".to_string()
                } else {
                    format!("`{n}`")
                };
                d = d.label(sp, format!("{phrase} before {rel} is released"));
            }
            push_diag(ctx, d);
        }
    }
}

/// N25D — a full path-sensitive state snapshot for a loop exit.
/// `break`/`continue` capture one at their site (after expiring
/// everything bound deeper than the loop's scope); the `while` merge
/// joins all of them plus the fall-through end against the pre-loop
/// state — the same cert discipline the `if` join uses.
#[derive(Clone)]
struct LoopSnap {
    edges: Edges,
    caps: HashMap<(String, String), Cert>,
    consumed: HashMap<String, ConsFact>,
    init: HashMap<String, Cert>,
    edge_kills: HashMap<String, Cert>,
    cap_kills: HashMap<String, Cert>,
    path_provs: HashMap<String, Prov>,
    transit: HashSet<String>,
    chans: HashMap<String, Vec<Packet>>,
    out: Outstanding,
    tag: HashMap<String, Option<String>>,
    defers: Vec<Vec<DeferRec>>,
    defer_res: HashMap<String, Span>,
}

/// One loop frame on `ctx.loops`: `scope_depth` is the scopes len at
/// loop entry — everything deeper dies at a break/continue site.
#[derive(Clone)]
struct LoopCtx {
    scope_depth: usize,
    breaks: Vec<LoopSnap>,
    conts: Vec<LoopSnap>,
}

fn restore_snap(ctx: &mut Ctx, s: LoopSnap) {
    ctx.edges = s.edges;
    ctx.caps = s.caps;
    ctx.consumed = s.consumed;
    ctx.init = s.init;
    ctx.edge_kills = s.edge_kills;
    ctx.cap_kills = s.cap_kills;
    ctx.path_provs = s.path_provs;
    ctx.transit = s.transit;
    ctx.chan_queues = s.chans;
    ctx.out = s.out;
    ctx.tag = s.tag;
    ctx.defers = s.defers;
    ctx.defer_res = s.defer_res;
}

fn take_snap(ctx: &Ctx) -> LoopSnap {
    LoopSnap {
        edges: ctx.edges.clone(),
        caps: ctx.caps.clone(),
        consumed: ctx.consumed.clone(),
        init: ctx.init.clone(),
        edge_kills: ctx.edge_kills.clone(),
        cap_kills: ctx.cap_kills.clone(),
        path_provs: ctx.path_provs.clone(),
        transit: ctx.transit.clone(),
        chans: ctx.chan_queues.clone(),
        out: ctx.out.clone(),
        tag: ctx.tag.clone(),
        defers: ctx.defers.clone(),
        defer_res: ctx.defer_res.clone(),
    }
}

/// ctx := join(ctx, s) — the pathwise merge rules shared with `if`:
/// Def survives only where both paths agree; maybe on either side
/// weakens; unions for the channel/transit may-sets.
fn join_snap(ctx: &mut Ctx, s: &LoopSnap, span: Span) {
    pending_symmetry(&[pend_keys(&ctx.out), pend_keys(&s.out)], span, ctx);
    // Arm-level ⊥ view BEFORE the edge merge — after `ctx.edges`
    // becomes the union, co-resident ⊥s from different arms are
    // indistinguishable from same-arm independent positions.
    let pre_bots = holder_bots(&ctx.edges, &ctx.caps, &ctx.out);
    let s_bots = holder_bots(&s.edges, &s.caps, &s.out);
    let mut merged_edges = s.edges.clone();
    for (k, f) in ctx.edges.iter().chain(s.edges.iter()) {
        let cert = match (ctx.edges.get(k), s.edges.get(k)) {
            (Some(c), Some(sk)) if c.kind == sk.kind && c.cert == Cert::Def && sk.cert == Cert::Def => Cert::Def,
            _ => Cert::Maybe,
        };
        merged_edges.insert(k.clone(), EdgeFact {
            kind: f.kind,
            cert,
            born: ctx.edges.get(k).and_then(|x| x.born).or_else(|| s.edges.get(k).and_then(|x| x.born)),
        });
    }
    ctx.edges = merged_edges;
    let mut merged_caps = s.caps.clone();
    for (k, _) in ctx.caps.iter().chain(s.caps.iter()) {
        let cert = match (ctx.caps.get(k), s.caps.get(k)) {
            (Some(Cert::Def), Some(Cert::Def)) => Cert::Def,
            _ => Cert::Maybe,
        };
        merged_caps.insert(k.clone(), cert);
    }
    ctx.caps = merged_caps;
    ctx.consumed = join_consumed(&ctx.consumed.clone(), &s.consumed);
    ctx.init = join_init(&ctx.init, &s.init);
    ctx.edge_kills = join_cert_map(&ctx.edge_kills, &s.edge_kills);
    ctx.cap_kills = join_cert_map(&ctx.cap_kills, &s.cap_kills);
    let saved_paths = std::mem::take(&mut ctx.path_provs);
    ctx.path_provs = s.path_provs.clone();
    for (k, v) in saved_paths.iter() {
        let j = join_prov(v, s.path_provs.get(k).unwrap_or(v), &ctx.regs);
        ctx.path_provs.insert(k.clone(), j);
    }
    ctx.transit = ctx.transit.union(&s.transit).cloned().collect();
    join_chan_may(ctx, &s.chans, &s.out);
    coalesce_join_bots(ctx, &[pre_bots, s_bots]);
    // Tag join: both paths must name the same variant to keep it.
    let tkeys: HashSet<String> = ctx.tag.keys().chain(s.tag.keys()).cloned().collect();
    for k in tkeys {
        let same = match (ctx.tag.get(&k), s.tag.get(&k)) {
            (Some(Some(a)), Some(Some(b))) => a == b,
            _ => false,
        };
        if !same {
            ctx.tag.insert(k, None);
        }
    }
    // N54 — pending defers/reservations join by union: an exit path
    // that already replayed a frame owes nothing; other paths do.
    ctx.defers = merge_defer_frames(&ctx.defers, &s.defers);
    ctx.defer_res = merge_defer_res(&ctx.defer_res, &s.defer_res);
    rec_joined(ctx, None);
}

/// `match` — shared by `Stmt::Match` and `Expr::Match` (N28). The
/// scrutinee checks, arm validation, per-arm snapshot, and N-arm join
/// are identical in both positions; what differs is only whether the
/// arms' yields are gathered into a value type. `=> expr` arms yield
/// their tail expression (checked under `want` — a `let` annotation
/// flows into arm literals); `=> { stmts }` arms yield Void unless
/// they diverge (`=> { return e; }`, `=> return e`).
fn check_match(
    scrut: &Expr,
    arms: &[MatchArm],
    span: &Span,
    scopes: &mut Scopes,
    ctx: &mut Ctx,
    ret: &Ty,
    want: Option<&Ty>,
) -> Ty {
    rec(format!("(match @{}:{})", span.line, span.col));
            // The scrutinee's tag is read — it must be an initialized
            // enum place. Payload extraction is by-value only (N22):
            // `V(b)` moves the payload into a fresh local.
            let (st, _) = expr_tv(scrut, scopes, ctx);
            if matches!(
                scrut,
                Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..)
            ) {
                require_init(scrut, scopes, ctx);
            }
            let ename = match &st {
                Ty::Struct(n) if ctx.enums.contains_key(n) => n.clone(),
                _ => {
                    push_diag(ctx, Diag::new(
                        *span,
                        format!("`match` scrutinee must be an enum, found {st}"),
                    ));
                    return Ty::Void;
                }
            };
            let skeys = resolve_place(scrut, scopes, ctx);
            if skeys.is_empty() {
                push_diag(ctx, Diag::new(
                    *span,
                    "`match` scrutinee must be a place".to_string(),
                ));
                // A direct `match send(bch, v)` can't resolve: bind
                // the Result first (`let r = ..; match r`).
                ctx.last_send = None;
                return Ty::Void;
            }
            // Operand temps materialized INSIDE the scrutinee
            // (`match take(d.as_slice())` — the arg's `let $rvN`)
            // belong to the call, not the match statement: their
            // borrows must expire before the arms check, or every
            // borrowed arg stays borrowed for the whole match.
            let sroots: HashSet<String> = skeys
                .iter()
                .map(|k| path_root(k).to_string())
                .collect();
            let mut temps: Vec<String> = scopes
                .iter()
                .flat_map(|m| m.keys().cloned())
                .filter(|n| n.starts_with("$rv"))
                .collect();
            temps.sort();
            temps.dedup();
            for n in temps {
                if let Some(b) = lookup(scopes, &n) {
                    if !sroots.contains(&format!("{n}#{}", b.epoch)) {
                        kill_temp(&n, scopes, ctx);
                    }
                }
            }
            let variants: Vec<(String, Option<Ty>)> =
                (*ctx.enums.get(&ename).unwrap()).clone();
            // Arm validation: known variant, no duplicates, `_` is the
            // wildcard (covers everything remaining, binds nothing).
            let mut covered: HashSet<&str> = HashSet::new();
            let mut wildcard = false;
            for a in arms {
                if a.variant == "_" {
                    wildcard = true;
                    if a.bind.is_some() {
                        push_diag(ctx, Diag::new(
                            *span,
                            "`_` binds no payload".to_string(),
                        ));
                    }
                    continue;
                }
                match variants.iter().find(|(vn, _)| vn == &a.variant) {
                    Some((_, pt)) => {
                        if !covered.insert(a.variant.as_str()) {
                            push_diag(ctx, Diag::new(
                                *span,
                                format!("duplicate match arm `{}`", a.variant),
                            ));
                        }
                        if pt.is_none() && a.bind.is_some() {
                            push_diag(ctx, Diag::new(
                                *span,
                                format!(
                                    "variant `{}` of `{ename}` carries no payload to bind",
                                    a.variant
                                ),
                            ));
                        }
                    }
                    None => push_diag(ctx, Diag::new(
                        *span,
                        format!("enum `{ename}` has no variant `{}`", a.variant),
                    )),
                }
            }
            let missing: Vec<&String> = variants
                .iter()
                .map(|(vn, _)| vn)
                .filter(|vn| !covered.contains(vn.as_str()))
                .collect();
            if !missing.is_empty() && !wildcard {
                push_diag(ctx, Diag::new(
                    *span,
                    format!(
                        "non-exhaustive `match` on `{ename}` — missing: {}",
                        missing
                            .iter()
                            .map(|s| s.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ));
            }
            // Per-arm analysis: each arm runs on the pre-match state
            // with the tag forced to its variant; the post-states join
            // like `if` branches — an N-arm version of that merge.
            struct ArmSnap {
                diverged: bool,
                terminated: bool,
                scopes: Scopes,
                path_provs: HashMap<String, Prov>,
                edges: Edges,
                caps: HashMap<(String, String), Cert>,
                consumed: HashMap<String, ConsFact>,
                init: HashMap<String, Cert>,
                edge_kills: HashMap<String, Cert>,
                cap_kills: HashMap<String, Cert>,
                transit: HashSet<String>,
                chans: HashMap<String, Vec<Packet>>,
                out: Outstanding,
                tag: HashMap<String, Option<String>>,
                defers: Vec<Vec<DeferRec>>,
                defer_res: HashMap<String, Span>,
            }
            let pre_dead = ctx.diverged;
            let pre_term = ctx.terminated;
            let saved_scopes = scopes.clone();
            let saved_defers = ctx.defers.clone();
            let saved_dres = ctx.defer_res.clone();
            let saved_paths = ctx.path_provs.clone();
            let saved_edges = ctx.edges.clone();
            let saved_caps = ctx.caps.clone();
            let saved_cons = ctx.consumed.clone();
            let saved_init = ctx.init.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_ck = ctx.cap_kills.clone();
            let saved_transit = ctx.transit.clone();
            let saved_chans = ctx.chan_queues.clone();
            let saved_out = ctx.out.clone();
            let saved_tag = ctx.tag.clone();
            let mut snaps: Vec<ArmSnap> = Vec::new();
            // N28 — each reachable arm contributes its yield; a
            // diverged arm (`=> { return e; }`) contributes none.
            let mut yields: Vec<(Option<&Expr>, Span, ArmYieldInfo)> = Vec::new();
            for a in arms {
                // Wildcard and unknown-variant arms see the scrutinee
                // with unknown tag — payload paths stay illegal.
                let force = if a.variant == "_"
                    || !variants.iter().any(|(vn, _)| vn == &a.variant)
                {
                    None
                } else {
                    Some(a.variant.clone())
                };
                for k in &skeys {
                    tag_set(ctx, k.clone(), force.clone());
                }
                // Variant-scoped payload authority: an edge/cap whose
                // holder or target lives under `k.W` — a payload path
                // of a variant this arm can't hold — doesn't exist
                // here. `match m.get_mut(k) { Err(_) => m.insert(..) }`
                // needs `m` free of the `Ok` borrow in the Err arm.
                let dead: Vec<String> = if a.variant == "_" {
                    covered.iter().map(|s| (*s).to_string()).collect()
                } else {
                    variants
                        .iter()
                        .filter(|(vn, _)| vn != &a.variant)
                        .map(|(vn, _)| vn.clone())
                        .collect()
                };
                for k in &skeys {
                    for vn in &dead {
                        let pref = format!("{k}.{vn}");
                        edges_retain(ctx, |(h, t), _| {
                            !(h == &pref
                                || h.starts_with(&format!("{pref}."))
                                || t == &pref
                                || t.starts_with(&format!("{pref}.")))
                        });
                        caps_retain(ctx, |(h, t), _| {
                            !(h == &pref
                                || h.starts_with(&format!("{pref}."))
                                || t == &pref
                                || t.starts_with(&format!("{pref}.")))
                        });
                    }
                }
                // A variant's payload is live only in an arm that can
                // hold it — mark `k.V` dead BEFORE the body too (not
                // just for the join): `?`/`return`/abandon audits
                // inside the arm must not see covered variants as
                // live (a `_` arm can hold no named variant's
                // payload). Only own-authority payloads have a
                // liveness to kill — a scalar/view payload's "dead"
                // mark would just poison the join for re-extraction.
                for k in &skeys {
                    for vn in &dead {
                        let own = variants
                            .iter()
                            .find(|(n, _)| n == vn)
                            .and_then(|(_, t)| t.clone())
                            .map_or(false, |pt| {
                                reaches_own(&pt, ctx, &mut HashSet::new())
                            });
                        if own {
                            ctx.consumed
                                .insert(format!("{k}.{vn}"), ConsFact::cons(*span));
                        }
                    }
                }
                // CHAN-004 S3b — the scrutinee's pending sends
                // resolve per arm: `Ok` delivers the latent packet
                // (ownership moves into the queue); `Err` claims it
                // back at `k.Err`, where this arm's synthesized
                // `let b = scrut.Err` picks the authority up. Any
                // other arm leaves the entry parked — the result
                // stays unexamined on this path.
                for k in &skeys {
                    resolve_pending_send(k, &a.variant, scrut, scopes, ctx);
                }
                // `V(b)` — synthesize the payload bind for the arm.
                // By value (`let b = scrut.V`): a POD payload copies,
                // a movable one moves — legal wherever the payload
                // path can supply own-authority, which includes
                // `write` scrutinees (extract-then-overwrite is the
                // map.remove/`insert` shape: the arm must restore
                // before the join). Only a `read` scrutinee can't
                // give up its payload — there an affine binder
                // arrives as `&T`, a view of the live payload, never
                // a second owner. (N40's whole-subtree move made the
                // naive by-value synth reject those arms outright —
                // they were always meant to be borrows.)
                let payload_ty = variants
                    .iter()
                    .find(|(vn, _)| vn == &a.variant)
                    .and_then(|(_, t)| t.clone());
                let mut body: Vec<Stmt> = Vec::new();
                if let (Some(b), Some(pt)) = (&a.bind, &payload_ty) {
                    let pexpr = Expr::Field(
                        Box::new(scrut.clone()),
                        a.variant.clone(),
                        *span,
                    );
                    let init = if !reaches_own(pt, ctx, &mut HashSet::new())
                        || place_auth(&pexpr, scopes, ctx) != Auth::Read
                    {
                        record_arm_bind(*span, &a.variant, ArmBind::Value);
                        pexpr
                    } else {
                        record_arm_bind(*span, &a.variant, ArmBind::Ref);
                        Expr::AddrOf {
                            mutable: false,
                            place: Box::new(pexpr),
                            span: *span,
                        }
                    };
                    body.push(Stmt::Let {
                        name: b.clone(),
                        ty: None,
                        init: Some(init),
                        span: *span,
                        name_span: *span,
                    });
                }
                body.extend(a.body.iter().cloned());
                let arm_yield = if a.expr_arm {
                    // `=> expr` — the tail is the arm's yield, checked
                    // under the match's `want` (N26 context flows in).
                    let yi = check_block_y(&body, scopes, ctx, ret, want, true);
                    match (yi, a.body.last()) {
                        (Some(y), Some(Stmt::Expr { expr, span: es })) => {
                            Some((Some(expr), *es, y))
                        }
                        _ => None,
                    }
                } else {
                    check_block(&body, scopes, ctx, ret);
                    if ctx.diverged {
                        None
                    } else {
                        Some((
                            None,
                            *span,
                            ArmYieldInfo {
                                ty: Ty::Void,
                                prov: Prov::Unknown,
                                refs: None,
                                caps: None,
                                len: None,
                                lit_edges: Vec::new(),
                                lit_caps: Vec::new(),
                                lit_provs: Vec::new(),
                                lit_dead: Vec::new(),
                                lit_lens: Vec::new(),
                                lit_tags: Vec::new(),
                            },
                        ))
                    }
                };
                if !ctx.diverged {
                    if let Some((e, es, y)) = arm_yield {
                        yields.push((e, es, y));
                    }
                }
                // A variant's payload is live only in an arm that can
                // hold it — `Full(p)` extraction should read as
                // consumed after the join, not maybe-live, when every
                // sibling arm excludes `Full`. Mark `k.V` dead here
                // for each V this arm can't hold (same `dead` set the
                // authority retain used).
                for k in &skeys {
                    for vn in &dead {
                        // Only own-authority payloads have a liveness
                        // to kill — a scalar/view payload's "dead"
                        // mark would just poison the join for
                        // re-extraction (`r.Ok` in a later match).
                        let own = variants
                            .iter()
                            .find(|(n, _)| n == vn)
                            .and_then(|(_, t)| t.clone())
                            .map_or(false, |pt| {
                                reaches_own(&pt, ctx, &mut HashSet::new())
                            });
                        if own {
                            ctx.consumed
                                .insert(format!("{k}.{vn}"), ConsFact::cons(*span));
                        }
                    }
                }
                snaps.push(ArmSnap {
                    diverged: ctx.diverged,
                    terminated: ctx.terminated,
                    scopes: scopes.clone(),
                    path_provs: ctx.path_provs.clone(),
                    edges: ctx.edges.clone(),
                    caps: ctx.caps.clone(),
                    consumed: ctx.consumed.clone(),
                    init: ctx.init.clone(),
                    edge_kills: ctx.edge_kills.clone(),
                    cap_kills: ctx.cap_kills.clone(),
                    transit: ctx.transit.clone(),
                    chans: ctx.chan_queues.clone(),
                    out: ctx.out.clone(),
                    tag: ctx.tag.clone(),
                    defers: ctx.defers.clone(),
                    defer_res: ctx.defer_res.clone(),
                });
                ctx.path_provs = saved_paths.clone();
                ctx.defers = saved_defers.clone();
                ctx.defer_res = saved_dres.clone();
                ctx.edges = saved_edges.clone();
                ctx.caps = saved_caps.clone();
                ctx.consumed = saved_cons.clone();
                ctx.init = saved_init.clone();
                ctx.edge_kills = saved_kills.clone();
                ctx.cap_kills = saved_ck.clone();
                ctx.transit = saved_transit.clone();
                ctx.chan_queues = saved_chans.clone();
                ctx.out = saved_out.clone();
                ctx.tag = saved_tag.clone();
                ctx.diverged = false;
                ctx.terminated = false;
                rec_flags(ctx);
                *scopes = saved_scopes.clone();
            }
            if snaps.is_empty() {
                return Ty::Void;
            }
            // ── N-arm merge (mirrors the `if` join) ──
            let all_dead = snaps.iter().all(|s| s.diverged);
            let all_term = snaps.iter().all(|s| s.terminated);
            let live: Vec<&ArmSnap> = snaps.iter().filter(|s| !s.diverged).collect();
            ctx.diverged = pre_dead || (all_dead && missing.is_empty() || all_dead && wildcard);
            // Continuation is terminated only when every arm that
            // could reach it terminated — a `return` arm unwinds, it
            // doesn't terminate.
            ctx.terminated = pre_term
                || (all_term && (missing.is_empty() || wildcard));
            rec_flags(ctx);
            ctx.init = if live.is_empty() {
                saved_init
            } else {
                let mut acc = live[0].init.clone();
                for s in &live[1..] {
                    acc = join_init(&acc, &s.init);
                }
                acc
            };
            // Edges/caps: union over LIVE arms only — a diverged arm's
            // post-state is unreachable and can't weaken the survivors
            // (`Err => { pairs.destroy(); return }` can't mark `pairs`
            // Maybe-consumed/dead-edged for the Ok path's join).
            let arms: Vec<&ArmSnap> = if live.is_empty() {
                snaps.iter().collect()
            } else {
                live.clone()
            };
            let mut merged_edges: Edges = HashMap::new();
            let mut keys: HashSet<(String, String)> = HashSet::new();
            for s in &arms {
                keys.extend(s.edges.keys().cloned());
            }
            for k in keys {
                let mut fact: Option<EdgeFact> = None;
                let mut all_def = true;
                for s in &arms {
                    match s.edges.get(&k) {
                        Some(ef) if ef.cert == Cert::Def => {
                            if fact.is_none() {
                                fact = Some(*ef);
                            }
                        }
                        Some(ef) => {
                            if fact.is_none() {
                                fact = Some(*ef);
                            }
                            all_def = false;
                        }
                        None => all_def = false,
                    }
                }
                if let Some(mut ef) = fact {
                    ef.cert = if all_def { Cert::Def } else { Cert::Maybe };
                    merged_edges.insert(k, ef);
                }
            }
            ctx.edges = merged_edges;
            let mut merged_caps: HashMap<(String, String), Cert> = HashMap::new();
            let mut ckeys: HashSet<(String, String)> = HashSet::new();
            for s in &arms {
                ckeys.extend(s.caps.keys().cloned());
            }
            for k in ckeys {
                let all_def = arms
                    .iter()
                    .all(|s| s.caps.get(&k) == Some(&Cert::Def));
                if arms.iter().any(|s| s.caps.contains_key(&k)) {
                    merged_caps.insert(k, if all_def { Cert::Def } else { Cert::Maybe });
                }
            }
            ctx.caps = merged_caps;
            let mut acc = arms[0].consumed.clone();
            for s in &arms[1..] {
                acc = join_consumed(&acc, &s.consumed);
            }
            ctx.consumed = acc;
            let mut acc = arms[0].edge_kills.clone();
            for s in &arms[1..] {
                acc = join_cert_map(&acc, &s.edge_kills);
            }
            ctx.edge_kills = acc;
            let mut acc = arms[0].cap_kills.clone();
            for s in &arms[1..] {
                acc = join_cert_map(&acc, &s.cap_kills);
            }
            ctx.cap_kills = acc;
            {
                let arm_keys: Vec<std::collections::BTreeSet<String>> =
                    arms.iter().map(|s| pend_keys(&s.out)).collect();
                pending_symmetry(&arm_keys, *span, ctx);
            }
            // CHAN-004 S3b — pending sends re-basis on the ARMS, not
            // the pre-match state: `join_chan_may` unions into the
            // seed, and the seed here is `saved_out` — a send result
            // resolved in every arm would otherwise resurrect through
            // the union. Arms carry unrelated pendings forward, so
            // union-over-arms is the complete post-match set.
            ctx.out.pending_send.clear();
            for s in &arms {
                ctx.transit = ctx.transit.union(&s.transit).cloned().collect();
                join_chan_may(ctx, &s.chans, &s.out);
            }
            // N-arm alternative coalescing: every arm's ⊥ set is
            // mutually exclusive with every other's — one multi-arm
            // pass, not pairwise folds.
            let arm_bots: Vec<HashMap<String, Vec<String>>> = arms
                .iter()
                .map(|s| holder_bots(&s.edges, &s.caps, &s.out))
                .collect();
            coalesce_join_bots(ctx, &arm_bots);
            // Tag join: every live arm agreeing on the same case keeps
            // it; any disagreement (or a wildcard arm) means unknown.
            let mut tkeys: HashSet<String> = HashSet::new();
            for s in &arms {
                tkeys.extend(s.tag.keys().cloned());
            }
            let mut merged_tag: HashMap<String, Option<String>> = HashMap::new();
            for k in tkeys {
                let first: Option<&String> = arms
                    .iter()
                    .find_map(|s| s.tag.get(&k).and_then(|o| o.as_ref()));
                let same = first.is_some()
                    && arms.iter().all(|s| {
                        matches!(s.tag.get(&k), Some(Some(v)) if Some(v) == first)
                    });
                merged_tag.insert(k, if same { first.cloned() } else { None });
            }
            ctx.tag = merged_tag;
            // CHAN-004 S3b — a scrutinee still carrying pending sends
            // after the join was never given an `Err` arm to resolve
            // into (or only a `_` covered it).
            check_pending_resolved(&skeys, *span, ctx);
            // Path provs: join each key across live arms, falling back
            // to the pre-match value where an arm doesn't carry it.
            let mut pkeys: HashSet<String> = HashSet::new();
            for s in &arms {
                pkeys.extend(s.path_provs.keys().cloned());
            }
            let mut merged_paths: HashMap<String, Prov> = HashMap::new();
            for k in pkeys {
                let vals: Vec<Prov> = arms
                    .iter()
                    .filter_map(|s| s.path_provs.get(&k).cloned().or_else(|| saved_paths.get(&k).cloned()))
                    .collect();
                if let Some(first) = vals.first().cloned() {
                    let j = vals[1..].iter().fold(first, |a, v| join_prov(&a, v, &ctx.regs));
                    merged_paths.insert(k, j);
                }
            }
            ctx.path_provs = merged_paths;
            // Scopes fold over LIVE arms only — a diverged arm's
            // drained frames (break/return popped them) would erase
            // bindings the surviving paths still hold. Same rule the
            // init join uses above.
            let mut acc_scopes = live
                .first()
                .map(|s| s.scopes.clone())
                .unwrap_or_else(|| snaps[0].scopes.clone());
            for s in live.iter().skip(1) {
                acc_scopes = merge_envs(&acc_scopes, &s.scopes, ctx);
            }
            // N54 — defer stacks fold over the same arms scopes do: a
            // frame drained by an unwinding arm still owes its calls
            // on the survivors.
            let mut acc_defers = arms[0].defers.clone();
            for s in &arms[1..] {
                acc_defers = merge_defer_frames(&acc_defers, &s.defers);
            }
            ctx.defers = acc_defers;
            let mut acc_dres: HashMap<String, Span> = HashMap::new();
            for s in &arms {
                acc_dres.extend(s.defer_res.iter().map(|(k, v)| (k.clone(), *v)));
            }
            ctx.defer_res = acc_dres;
            *scopes = acc_scopes;
            rec_joined(ctx, Some(*span));
            // ── yield unification (N28): every live arm must produce
            // the same type. A default-int yield adopts the concrete
            // peer's int type — peer propagation inside the match —
            // and its literal range re-checks against the winner.
            let mut mty: Option<Ty> = None;
            for (_, es, yi) in &yields {
                let t = &yi.ty;
                mty = Some(match mty.take() {
                    None => t.clone(),
                    Some(a) if a == *t => a,
                    Some(Ty::INT_DEFAULT) if t.is_int() => t.clone(),
                    Some(a) if *t == Ty::INT_DEFAULT && a.is_int() => a,
                    Some(a) => {
                        push_diag(ctx, Diag::new(
                            *es,
                            format!(
                                "match arms produce different types: `{a}` vs `{t}`"
                            ),
                        ));
                        a
                    }
                });
            }
            // Every arm diverged → the expression never yields; adopt
            // `want` so `let x: T = <diverging match>` still type-
            // checks (the binding is dead code). Without `want` the
            // Void result trips the ordinary "binds void" rule.
            let mty = mty.unwrap_or_else(|| want.cloned().unwrap_or(Ty::Void));
            if mty != Ty::INT_DEFAULT && mty.is_int() {
                for (e, _, yi) in &yields {
                    if let (Some(e), true) = (*e, yi.ty == Ty::INT_DEFAULT) {
                        coerce(&mty, e, &Ty::INT_DEFAULT, &mut ctx.errs);
                    }
                }
            }
            // Value-side join (N28): the match's prov is the per-arm
            // prov join; refs/caps union the live arm tails; a static
            // slice len survives only when every arm agrees. Captured
            // during arm checking because arm scopes are already dead.
            let mut info = ArmYieldInfo {
                ty: mty.clone(),
                prov: Prov::Unknown,
                refs: None,
                caps: None,
                len: None,
                lit_edges: Vec::new(),
                lit_caps: Vec::new(),
                lit_provs: Vec::new(),
                lit_dead: Vec::new(),
                lit_lens: Vec::new(),
                lit_tags: Vec::new(),
            };
            let mut first = true;
            for (_, _, yi) in &yields {
                if yi.ty == Ty::Void {
                    continue;
                }
                info.prov = if first {
                    yi.prov.clone()
                } else {
                    join_prov(&info.prov, &yi.prov, &ctx.regs)
                };
                match (&mut info.refs, &yi.refs) {
                    (m @ None, Some(ri)) => *m = Some(ri.clone()),
                    (Some(m), Some(ri)) => {
                        for t in &ri.targets {
                            if !m.targets.contains(t) {
                                m.targets.push(t.clone());
                            }
                        }
                        m.mutable |= ri.mutable;
                        if m.move_from.is_none() {
                            m.move_from = ri.move_from.clone();
                        }
                        m.foreign &= ri.foreign;
                    }
                    (Some(m), None) => m.foreign = false,
                    (None, None) => {}
                }
                match (&mut info.caps, &yi.caps) {
                    (m @ None, Some(cs)) => *m = Some(cs.clone()),
                    (Some(m), Some(cs)) => {
                        for (t, c) in cs {
                            match m.iter_mut().find(|(v, _)| v == t) {
                                // One target reached through arms of
                                // differing certainty — Maybe wins.
                                Some(slot) => {
                                    if *c == Cert::Maybe {
                                        slot.1 = Cert::Maybe;
                                    }
                                }
                                None => m.push((t.clone(), *c)),
                            }
                        }
                    }
                    _ => {}
                }
                info.len = if first {
                    yi.len
                } else if info.len == yi.len {
                    info.len
                } else {
                    None
                };
                // N40 — subtree facts union across arms the way refs
                // do: edge/cap rels merge their targets, a dead rel is
                // Def-dead only where every yield says so, equal lens/
                // tags survive, provs join pointwise.
                for (rel, ri) in &yi.lit_edges {
                    match info.lit_edges.iter_mut().find(|(r, _)| r == rel) {
                        Some((_, m)) => {
                            for t in &ri.targets {
                                if !m.targets.contains(t) {
                                    m.targets.push(t.clone());
                                }
                            }
                            m.mutable |= ri.mutable;
                            m.foreign &= ri.foreign;
                        }
                        None => info.lit_edges.push((rel.clone(), ri.clone())),
                    }
                }
                for (rel, ts) in &yi.lit_caps {
                    match info.lit_caps.iter_mut().find(|(r, _)| r == rel) {
                        Some((_, m)) => {
                            for t in ts {
                                if !m.contains(t) {
                                    m.push(t.clone());
                                }
                            }
                        }
                        None => info.lit_caps.push((rel.clone(), ts.clone())),
                    }
                }
                for (rel, f) in &yi.lit_dead {
                    match info.lit_dead.iter_mut().find(|(r, _)| r == rel) {
                        Some((_, m)) => {
                            if m.st != ConsState::Cons || f.st != ConsState::Cons {
                                m.st = ConsState::Maybe;
                            }
                            m.at = merge_origins(&m.at, &f.at);
                        }
                        None => info
                            .lit_dead
                            .push((rel.clone(), ConsFact {
                                st: ConsState::Maybe,
                                at: f.at.clone(),
                            })),
                    }
                }
                for (rel, p) in &yi.lit_provs {
                    match info.lit_provs.iter_mut().find(|(r, _)| r == rel) {
                        Some((_, m)) => *m = join_prov(m, p, &ctx.regs),
                        None => info.lit_provs.push((rel.clone(), p.clone())),
                    }
                }
                if first {
                    info.lit_lens = yi.lit_lens.clone();
                    info.lit_tags = yi.lit_tags.clone();
                } else {
                    info.lit_lens.retain(|(rel, l)| {
                        yi.lit_lens.iter().any(|(r2, l2)| r2 == rel && l2 == l)
                    });
                    info.lit_tags.retain(|(rel, t)| {
                        yi.lit_tags.iter().any(|(r2, t2)| r2 == rel && t2 == t)
                    });
                }
                first = false;
            }
            rec(format!("(yield @{}:{})", span.line, span.col));
            ctx.match_yields.insert((span.line, span.col), info);
            mty
        }

/// CHAN-004 S3b — after a match, a scrutinee place still carrying
/// pending sends was never given an `Err` arm to resolve into.
fn check_pending_resolved(skeys: &[String], s: Span, ctx: &mut Ctx) {
    for k in skeys {
        if ctx.out.pending_send.contains_key(k) {
            let n = k.split('#').next().unwrap_or(k);
            push_diag(ctx, Diag::new(
                s,
                format!(
                    "bounded-send result `{n}` needs an `Err` arm to resolve the send"
                ),
            ));
        }
    }
}

fn check_stmt(s: &Stmt, scopes: &mut Scopes, ctx: &mut Ctx, ret: &Ty) {
    match s {
        Stmt::Let {
            name,
            ty,
            init,
            span,
            name_span,
        } => {
            // N54 — shadowing a name a live `defer` froze re-points
            // the deferred call at a different binding on every
            // deeper exit edge (name capture, not value capture).
            // Reject without installing: `b` keeps meaning the
            // reserved binding, so later uses diagnose cleanly.
            if let Some(sp) = defer_name_conflict(ctx, name) {
                push_diag(ctx, Diag::new(
                    *name_span,
                    format!(
                        "cannot shadow `{name}`: it is reserved by the `defer` at line {}",
                        sp.line
                    ),
                ));
                return;
            }
            if let Some(t) = ty {
                check_ty(t, ctx.structs, *span, &mut ctx.errs);
            }
            let owner = cur_region(ctx);
            let (t, prov) = match (ty, init) {
                (Some(w), Some(init)) => {
                    let (got, prov) = expr_tv_w(init, Some(w), scopes, ctx);
                    let t = coerce(w, init, &got, &mut ctx.errs);
                    // `let q: box T = p` — an owning value moves.
                    push_place_lit(init, scopes, ctx);
                    consume_owned_src(init, scopes, ctx);
                    if let Ty::Ref { .. } | Ty::Slice { .. } = got {
                        if !outlives(&prov, &Prov::Region(owner), &ctx.regs) {
                            push_diag(ctx, Diag::new(
                                *span,
                                format!(
                                    "reference escapes region: value lives in {}, but `{name}` requires {}",
                                    prov.describe(&ctx.regs),
                                    Prov::Region(owner).describe(&ctx.regs),
                                ),
                            ));
                        }
                        (t, prov)
                    } else if let Ty::Handle(_) = got {
                        (t, Prov::Region(owner))
                    } else if t == Ty::Str {
                        // prov doubles as the init marker for `str` —
                        // the type carries no region info otherwise.
                        (t, Prov::Region(owner))
                    } else if matches!(t, Ty::Box(_) | Ty::BoxSlice(_)) {
                        // Same for `box` — `let p: box T = alloc` marks
                        // the root initialized; the cell's liveness is
                        // the binding's, not a region's.
                        (t, Prov::Region(owner))
                    } else {
                        (t, Prov::Unknown)
                    }
                }
                (Some(w), None) => (w.clone(), Prov::Unknown),
                (None, Some(init)) => {
                    let (got, prov) = expr_tv(init, scopes, ctx);
                    push_place_lit(init, scopes, ctx);
                    consume_owned_src(init, scopes, ctx);
                    if let Ty::Handle(_) = got {
                        (got, Prov::Region(owner))
                    } else if got == Ty::Str {
                        (got, Prov::Region(owner))
                    } else {
                        (got, prov)
                    }
                }
                (None, None) => {
                    push_diag(ctx, Diag::new(
                        *span,
                        format!("`{name}` needs a type annotation or an initializer"),
                    ));
                    (Ty::Void, Prov::Unknown)
                }
            };
            if t == Ty::Void {
                ctx.errs
                    .push(Diag::new(*span, format!("`{name}` binds a void value")));
            }
            ctx.next_epoch += 1;
            let epoch = ctx.next_epoch;
            let key = format!("{name}#{epoch}");
            // Ref-valued init: the binding becomes an edge holder.
            // Handle-valued init: an identity edge — never a borrow.
            // `slice(p, n)` init: the binding is foreign — no edges.
            if let Some(i) = init {
                // N18 — an initializer stores a value; `let x: T`
                // without one stays absent (Uninit).
                init_set(ctx, key.clone(), Cert::Def);
                if let Some(ri) = refs_of(i, scopes, ctx) {
                    if ri.foreign {
                        foreign_set(ctx, key.clone());
                    }
                    record_edges(&key, &ri, Some(i.span()), ctx);
                }
                if let Some(ts) = caps_of(i, scopes, ctx) {
                    record_caps(&key, &ts, ctx);
                }
                if let Some(len) = slice_len_of(i, scopes, ctx) {
                    slen_set(ctx, key.clone(), len);
                }
                // `str` NUL-provenance: a literal (or a binding already
                // proven literal-backed) marks the view `cstr`-safe.
                if t == Ty::Str {
                    match i {
                        Expr::Str(..) => {
                            csafe_set(ctx, key.clone());
                        }
                        Expr::Var(vn, _) => {
                            if let Some(vb) = lookup(scopes, vn) {
                                let vk = format!("{vn}#{}", vb.epoch);
                                if ctx.cstr_safe.contains(&vk) {
                                    csafe_set(ctx, key.clone());
                                }
                            }
                        }
                        _ => {}
                    }
                }
            }
            // `let g = lock(m)` — the pending lock tuple becomes a
            // guard: `g -> domain` rides the edge map so both plain
            // accessors and other domains see the claim.
            let mut guard = None;
            if let Some(Expr::Call(n, ..)) = init {
                if n == "lock" {
                    if let Some((d, r, _, srcs)) = ctx.last_lock.take() {
                        ctx.edges
                            .insert((key.clone(), d.clone()), EdgeFact::new(Auth::Sync, Cert::Def, Some(*span)));
                        guard = Some((d, r, srcs));
                    }
                }
            }
            // `let m = mutex e` — register the domain: own payload by
            // default, `mutex h`'s referent for the wrapped-handle form.
            if let Some(Expr::Mutex(..)) = init {
                let root = ctx
                    .last_mutex
                    .take()
                    .and_then(|rs| rs.into_iter().next())
                    .unwrap_or_else(|| key.clone());
                dom_set(ctx, key.clone(), root);
            }
            // Affine move — `let g2 = g` hands the domain to g2 and
            // consumes g.
            if let Some(Expr::Var(vn, _)) = init {
                let moved = lookup(scopes, vn).and_then(|b| {
                    b.guard
                        .clone()
                        .map(|gr| (format!("{vn}#{}", b.epoch), gr))
                });
                if let Some((old, (d, r, s2))) = moved {
                    cons_set(ctx, old.clone(), ConsFact::cons(*span));
                    edges_retain(ctx, |(h, _), _| !key_under(h, &old));
                    ctx.edges
                        .insert((key.clone(), d.clone()), EdgeFact::new(Auth::Sync, Cert::Def, Some(*span)));
                    guard = Some((d, r, s2));
                }
            }
            // `let g = recv(ch)` — the popped packet's graph delta
            // installs here. A `&mut` packet binds as an ordinary
            // `Ref` local (identical to `let g = &mut x` — the ref
            // itself traveled); handle packets get identity edges;
            // value packets re-root their travelled subtree edges.
            if let Some(i @ Expr::Call(n, ..)) = init {
                if n == "recv" {
                    if let Some(pkt) = ctx.last_recv.take() {
                        claim_packet(&key, &pkt, i, scopes, ctx);
                    }
                }
            }
            // CHAN-004 S3b — `let r = send(bch, v)` keys the latent
            // packets to the Result place; `match r` resolves them.
            if let Some(pends) = ctx.last_send.take() {
                ctx.out
                    .pending_send
                    .entry(key.clone())
                    .or_default()
                    .extend(pends);
            }
            record_lit_fields(&key, init.as_ref().map(|i| i.span()), ctx);
            record_tag(init.as_ref(), &key, &t, ctx);
            bind_task(&key, matches!(init, Some(Expr::Spawn(..))), ctx);
            index::record(index::Occ {
                span: *name_span,
                name: name.clone(),
                kind: "local",
                hover: format!("`{name}`: {t}"),
                def: None,
                file: None,
            });
            rec(format!(
                "(bind {} @{}:{} {} {} {}{})",
                qs(&key),
                span.line,
                span.col,
                qs(&format!("{t}")),
                prov_s(&prov, &ctx.regs),
                prov_s(&Prov::Region(owner), &ctx.regs),
                match &guard {
                    Some((d, r, _)) => format!(" (guard {} {})", qs(d), qs(r)),
                    None => String::new(),
                }
            ));
            scopes.last_mut().unwrap().insert(
                name.clone(),
                Bind {
                    ty: t,
                    decl: *span,
                    prov,
                    owner,
                    epoch,
                    auth: None,
                    guard,
                },
            );
        }
        Stmt::Assign { target, value, .. } => {
            let tt = place_tv(target, scopes, ctx).0;
            let vtv = expr_tv_w(value, Some(&tt), scopes, ctx);
            coerce(&tt, value, &vtv.0, &mut ctx.errs);
            check_store(target, value, &vtv, scopes, ctx);
            // N18 — writing through a pointer reads the pointer:
            // `*p = v`/`b[i] = v` require p's/b's own init first.
            require_place_bases(target, scopes, ctx);
            // `x = p` — an owning value moves to the new place.
            push_place_lit(value, scopes, ctx);
            consume_owned_src(value, scopes, ctx);
            // Writes act on the RESOLVED place: through a cap param or
            // a deref, the edge bookkeeping lands on the referent.
            let mut ks = resolve_place(target, scopes, ctx);
            if ks.is_empty() {
                ks.extend(place_id(target, scopes));
            }
            // A store is not a destructor: overwriting a place whose
            // subtree still holds live own authority leaks it. The
            // previous value must be extracted or `free`d first —
            // `let old = s.p; s.p = alloc(..)`, `match` then reslot.
            for k in &ks {
                if let Some(held) = live_own_at(k, &tt, ctx) {
                    let tail = held
                        .strip_prefix(k.as_str())
                        .unwrap_or(held.as_str());
                    push_diag(ctx, Diag::new(
                        value.span(),
                        format!(
                            "cannot overwrite `{}`: the value it holds still owns `{}{}` — move or `free` it first",
                            target_name(target),
                            k,
                            tail
                        ),
                    ));
                }
            }
            for k in &ks {
                install_store(k, value, &tt, &vtv, scopes, ctx);
            }
            if let (Ty::Handle(_), Expr::Var(n, _)) = (&vtv.0, target) {
                if let Some(b) = lookup_mut(scopes, n) {
                    b.prov = Prov::Region(cur_region(ctx));
                }
            }
            // `x = recv(ch)` — the assign target claims the packet's
            // travelled delta; a capability packet needs a fresh `let`
            // (an existing binding can't become a place-alias).
            if let Expr::Call(n, ..) = value {
                if n == "recv" {
                    if let Some(pkt) = ctx.last_recv.take() {
                        if pkt.auth.is_some() || pkt.is_handle {
                            push_diag(ctx, Diag::new(
                                value.span(),
                                "a `recv` of a capability or handle packet must bind a fresh `let`"
                                    .to_string(),
                            ));
                        } else {
                            for k in &ks {
                                for (rel, t, f) in pkt.edges.clone() {
                                    ctx.edges
                                        .insert((format!("{k}{rel}"), t), f);
                                }
                                for (rel, t, cert) in pkt.caps.clone() {
                                    cap_set(ctx, format!("{k}{rel}"), t, cert);
                                }
                            }
                        }
                    }
                }
            }
            // CHAN-004 S3b — `r = send(bch, v)` keys the latent
            // packets to the target place, same as the `let` path.
            if let Some(pends) = ctx.last_send.take() {
                for k in &ks {
                    ctx.out
                        .pending_send
                        .entry(k.clone())
                        .or_default()
                        .extend(pends.iter().cloned());
                }
            }
            if let Some(k) = place_id(target, scopes) {
                bind_task(&k, matches!(value, Expr::Spawn(..)), ctx);
            }
        }
        Stmt::Return { value, span } => {
            rec(format!("(return @{}:{})", span.line, span.col));
            // N18 — control leaves here: branch joins merge only the
            // paths that actually reach them.
            ctx.diverged = true;
            rec_flags(ctx);
            if ctx.task.is_some() {
                // Inside a spawn body this is the task's RESULT, not
                // the enclosing fn's — its effects are already
                // suspended; only the value/edges re-enter at `join`.
                match value {
                    Some(v) => {
                        let tw = ctx.task.as_ref().unwrap().ret.clone();
                        let (got, _) = expr_tv_w(
                            v,
                            Some(&tw).filter(|t| t.is_int()),
                            scopes,
                            ctx,
                        );
                        if let Ty::Guard(_) = got {
                            push_diag(ctx, Diag::new(
                                *span,
                                "a lock guard cannot leave its scope".to_string(),
                            ));
                        }
                        // `return p` hands the task's box to the joiner.
                        consume_owned_src(v, scopes, ctx);
                        let want = tw;
                        if want == Ty::Void {
                            ctx.task.as_mut().unwrap().ret = got.clone();
                        } else if got != Ty::Void && got != want {
                            push_diag(ctx, Diag::new(
                                *span,
                                format!(
                                    "task returns {got}, but an earlier return yielded {want}"
                                ),
                            ));
                        }
                        if let Ty::Ref { mutable, .. } | Ty::Slice { mutable, .. } = got {
                            // A returned raw ref or slice must name
                            // storage that predates the spawn — a
                            // task-local place dies with the body and
                            // can't re-enter.
                            let ext = ctx.task.as_ref().unwrap().ext.clone();
                            if let Some(ri) = refs_of(v, scopes, ctx) {
                                for t in ri.targets {
                                    let r = path_root(&t).to_string();
                                    if r.starts_with('$') || ext.contains(&r) {
                                        ctx.ret_edges.push((t.clone(), mutable));
                                    } else {
                                        let n = r.split('#').next().unwrap_or(&r);
                                        push_diag(ctx, Diag::new(
                                            *span,
                                            format!(
                                                "task returns a reference to task-local `{n}`, which dies with the body"
                                            ),
                                        ));
                                    }
                                }
                            }
                        } else if got == Ty::Str {
                            // A `str` return carries the same lifetime
                            // rule through its Read claims.
                            let ext = ctx.task.as_ref().unwrap().ext.clone();
                            if let Some(ri) = refs_of(v, scopes, ctx) {
                                for t in ri.targets {
                                    let r = path_root(&t).to_string();
                                    if r.starts_with('$') || ext.contains(&r) {
                                        ctx.ret_edges.push((t.clone(), false));
                                    } else {
                                        let n = r.split('#').next().unwrap_or(&r);
                                        push_diag(ctx, Diag::new(
                                            *span,
                                            format!(
                                                "task returns a view of task-local `{n}`, which dies with the body"
                                            ),
                                        ));
                                    }
                                }
                            }
                        }
                        if let Ty::Handle(_) = got {
                            // Identity survives naming dead storage —
                            // the cap goes stale at use, which is the
                            // model's point.
                            if let Some(ts) = caps_of(v, scopes, ctx) {
                                ctx.ret_caps.extend(ts);
                            }
                        }
                    }
                    None => {
                        if ctx.task.as_ref().unwrap().ret != Ty::Void {
                            push_diag(ctx, Diag::new(
                                *span,
                                "bare `return` in a task that returns a value".to_string(),
                            ));
                        }
                    }
                }
                // N54 — `return` in a task body still unwinds the
                // body's scopes: its defers replay before the result
                // leaves.
                replay_all_defers(scopes, ctx);
                return;
            }
            match (value, ret) {
            (Some(_), Ty::Void) => push_diag(ctx, Diag::new(
                *span,
                "returning a value from a void function".to_string(),
            )),
            (None, Ty::Void) => {}
            (Some(v), want) => {
                let (got, prov) = expr_tv_w(v, Some(want), scopes, ctx);
                if let Ty::Guard(_) = got {
                    push_diag(ctx, Diag::new(
                        *span,
                        "a lock guard cannot leave its scope".to_string(),
                    ));
                }
                coerce(want, v, &got, &mut ctx.errs);
                if let Ty::Ref { mutable, .. } = want {
                    if !outlives(&prov, &Prov::Param, &ctx.regs) {
                        push_diag(ctx, Diag::new(
                            *span,
                            format!(
                                "reference escapes region: returned value lives in {}",
                                prov.describe(&ctx.regs),
                            ),
                        ));
                    }
                    // N8: the summary learns what this return hands
                    // back — `$arg`-keyed where it names caller storage.
                    if let Some(ri) = refs_of(v, scopes, ctx) {
                        for t in ri.targets {
                            ctx.ret_edges.push((t, *mutable));
                        }
                    }
                }
                // Slice and `str` returns feed the same summary — the
                // view's claim targets are what the binding's edges
                // will name caller-side.
                if matches!(want, Ty::Slice { .. } | Ty::Str) {
                    let mutable = matches!(want, Ty::Slice { mutable: true, .. });
                    if let Some(ri) = refs_of(v, scopes, ctx) {
                        for t in ri.targets {
                            ctx.ret_edges.push((t, mutable));
                        }
                    }
                }
                // Aggregate returns export their subtree refs/caps/
                // provs — `return Ok(&x[..])` lit channels, `return r`
                // the place's `r.<variant>`/field subtree. The caller
                // replays them under `r.<path>` so payload binds and
                // interior refs land with live edges.
                if let Ty::Struct(en) = &want {
                    let is_enum = ctx.enums.contains_key(en);
                    if is_enum {
                        if let Expr::EnumLit { .. } = v {
                            for (rel, ri) in std::mem::take(&mut ctx.lit_edges) {
                                ctx.ret_lit_edges.push((rel, ri));
                            }
                            for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
                                ctx.ret_lit_caps.push((rel, ts));
                            }
                            if let Some(lps) = ctx.last_lit.take() {
                                for (rel, p) in lps {
                                    ctx.ret_lit_provs.push((rel, p));
                                }
                            }
                        } else {
                            push_place_lit(v, scopes, ctx);
                            for (rel, ri) in std::mem::take(&mut ctx.lit_edges) {
                                ctx.ret_lit_edges.push((rel, ri));
                            }
                            for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
                                ctx.ret_lit_caps.push((rel, ts));
                            }
                            if let Some(lps) = ctx.last_lit.take() {
                                for (rel, p) in lps {
                                    ctx.ret_lit_provs.push((rel, p));
                                }
                            }
                        }
                    } else {
                        // `return h` — a non-enum struct place's subtree
                        // exports the same way; a `StructLit` value
                        // already filled the channels at field eval.
                        if !matches!(v, Expr::StructLit(..)) {
                            push_place_lit(v, scopes, ctx);
                        }
                        for (rel, ri) in std::mem::take(&mut ctx.lit_edges) {
                            ctx.ret_lit_edges.push((rel, ri));
                        }
                        for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
                            ctx.ret_lit_caps.push((rel, ts));
                        }
                        if let Some(lps) = ctx.last_lit.take() {
                            for (rel, p) in lps {
                                ctx.ret_lit_provs.push((rel, p));
                            }
                        }
                    }
                }
                if let Ty::Handle(_) = want {
                    if let Some(ts) = caps_of(v, scopes, ctx) {
                        ctx.ret_caps.extend(ts);
                    }
                }
                // The value leaves with the return — mark its source
                // consumed so the abandon audit below doesn't flag
                // the moved place (same as the task-return path).
                consume_owned_src(v, scopes, ctx);
            }
            (None, want) => push_diag(ctx, Diag::new(
                *span,
                format!("bare `return` in a function returning {want}"),
            )),
            }
            // N45R — the returned value was consumed above; anything
            // still holding live `own` authority is abandoned by this
            // path, even when the merged `}`-pop can't see it (a
            // sibling path may have discharged the same place).
            if !ctx.terminated {
                // N54 — `return` unwinds every scope: the deferred
                // calls replay (their consumes discharge R19) before
                // the audit runs.
                replay_all_defers(scopes, ctx);
                flag_abandoned(
                    Some(*span),
                    "this `return` exits",
                    scopes.iter().flat_map(|s| s.iter().map(|(n, b)| (n, b))),
                    ctx,
                );
            }
        }
        Stmt::If {
            cond,
            then_body,
            else_body,
            span,
        } => {
            let t = expr_tv(cond, scopes, ctx).0;
            if t != Ty::Bool {
                push_diag(ctx, Diag::new(
                    *span,
                    format!("`if` condition must be bool, found {t}"),
                ));
            }
            // Branch joins: prov state merges pointwise, weaker wins.
            let pre_dead = ctx.diverged;
            let pre_term = ctx.terminated;
            let saved_scopes = scopes.clone();
            let saved_paths = ctx.path_provs.clone();
            let saved_edges = ctx.edges.clone();
            let saved_caps = ctx.caps.clone();
            let saved_cons = ctx.consumed.clone();
            let saved_init = ctx.init.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_ck = ctx.cap_kills.clone();
            // S3a — `out` IS per-path: pops join by min, so the else
            // arm must not inherit the then-arm's departure count
            // (the may-sets inside it re-union at the join anyway).
            let saved_out = ctx.out.clone();
            // N54 — pending defers are per-path too: an exit inside
            // one arm drains frames the other arm still owes.
            let saved_defers = ctx.defers.clone();
            let saved_dres = ctx.defer_res.clone();
            check_block(then_body, scopes, ctx, ret);
            let then_d = ctx.diverged;
            let then_t = ctx.terminated;
            let then_paths = ctx.path_provs.clone();
            let then_defers = ctx.defers.clone();
            let then_dres = ctx.defer_res.clone();
            let then_edges = ctx.edges.clone();
            let then_caps = ctx.caps.clone();
            let then_cons = ctx.consumed.clone();
            let then_init = ctx.init.clone();
            let then_kills = ctx.edge_kills.clone();
            let then_ck = ctx.cap_kills.clone();
            let then_transit = ctx.transit.clone();
            let then_chans = ctx.chan_queues.clone();
            let then_out = ctx.out.clone();
            ctx.path_provs = saved_paths.clone();
            ctx.edges = saved_edges.clone();
            ctx.caps = saved_caps.clone();
            ctx.consumed = saved_cons.clone();
            ctx.init = saved_init.clone();
            ctx.edge_kills = saved_kills.clone();
            ctx.cap_kills = saved_ck.clone();
            ctx.diverged = false;
            ctx.terminated = false;
            rec_flags(ctx);
            ctx.defers = saved_defers;
            ctx.defer_res = saved_dres;
            ctx.out = saved_out;
            let then_scopes = std::mem::replace(scopes, saved_scopes);
            if let Some(eb) = else_body {
                check_block(eb, scopes, ctx, ret);
            }
            let else_d = ctx.diverged;
            let else_t = ctx.terminated;
            let else_init = ctx.init.clone();
            let else_defers = ctx.defers.clone();
            let else_dres = ctx.defer_res.clone();
            let else_paths = ctx.path_provs.clone();
            let else_edges = ctx.edges.clone();
            let else_caps = ctx.caps.clone();
            let else_kills = ctx.edge_kills.clone();
            let else_ck = ctx.cap_kills.clone();
            let else_cons = ctx.consumed.clone();
            let else_out = ctx.out.clone();
            // `if true {}`/`if false {}` — a statically-dead arm is
            // still checked for diagnostics above but contributes
            // nothing to the join: the same cond fold `while true`
            // gets. The `if true {}` block-scope idiom (bare `{}`
            // doesn't parse) relies on this: `if true { free(p) }`
            // must leave `p` Cons, not Maybe — else every scoped
            // discharge reads as live authority at the outer `}`.
            let lit = match cond {
                Expr::Bool(v, _) => Some(*v),
                _ => None,
            };
            if let Some(then_live) = lit {
                if then_live {
                    ctx.path_provs = then_paths;
                    ctx.edges = then_edges;
                    ctx.caps = then_caps;
                    ctx.consumed = then_cons;
                    ctx.init = then_init;
                    ctx.edge_kills = then_kills;
                    ctx.cap_kills = then_ck;
                    ctx.transit = then_transit;
                    ctx.chan_queues = then_chans;
                    ctx.out = then_out;
                    *scopes = then_scopes;
                    ctx.defers = then_defers;
                    ctx.defer_res = then_dres;
                    ctx.diverged = then_d;
                    ctx.terminated = then_t;
                    rec_flags(ctx);
                }
                // `false`: the else arm (or the pre-state when absent)
                // is already installed in `ctx`/`scopes`.
            } else if then_d || else_d {
            // A diverged arm contributes nothing to the join — its
            // post-state is unreachable, so every merged map takes
            // the survivor wholesale (the init/scopes rule, applied
            // to edges/caps/consumed/kills/provs/transit too: a
            // `destroy`/`free`/`send` on a `return`/`break` arm must
            // not mark the survivor's bindings Maybe-consumed).
                let (sed, scp, scn, skl, sck, spa, ssc, sdf, sdr, sou) = if then_d {
                    (
                        else_edges,
                        else_caps,
                        else_cons,
                        else_kills,
                        else_ck,
                        else_paths,
                        scopes.clone(),
                        else_defers,
                        else_dres,
                        else_out,
                    )
                } else {
                    (
                        then_edges,
                        then_caps,
                        then_cons,
                        then_kills,
                        then_ck,
                        then_paths,
                        then_scopes,
                        then_defers,
                        then_dres,
                        then_out,
                    )
                };
                ctx.edges = sed;
                ctx.caps = scp;
                ctx.consumed = scn;
                ctx.edge_kills = skl;
                ctx.cap_kills = sck;
                ctx.init = if then_d { else_init } else { then_init };
                ctx.transit = if then_d {
                    ctx.transit.clone()
                } else {
                    then_transit
                };
                ctx.out = sou;
                *scopes = ssc;
                ctx.defers = sdf;
                ctx.defer_res = sdr;
                ctx.path_provs = spa;
            } else {
            // Edge join: union of both sides; definite only where the
            // same (holder,target,kind) edge exists on BOTH paths.
            let mut merged_edges = else_edges.clone();
            for (k, f) in then_edges.iter().chain(else_edges.iter()) {
                let cert = match (then_edges.get(k), else_edges.get(k)) {
                    (Some(t), Some(e)) if t.kind == e.kind && t.cert == Cert::Def && e.cert == Cert::Def => Cert::Def,
                    _ => Cert::Maybe,
                };
                merged_edges.insert(k.clone(), EdgeFact {
                    kind: f.kind,
                    cert,
                    born: then_edges.get(k).and_then(|x| x.born).or_else(|| else_edges.get(k).and_then(|x| x.born)),
                });
            }
            ctx.edges = merged_edges;
            // Cap-edge join: same definite/possible discipline.
            let mut merged_caps = else_caps.clone();
            for (k, _) in then_caps.iter().chain(else_caps.iter()) {
                let cert = match (then_caps.get(k), else_caps.get(k)) {
                    (Some(Cert::Def), Some(Cert::Def)) => Cert::Def,
                    _ => Cert::Maybe,
                };
                merged_caps.insert(k.clone(), cert);
            }
            ctx.caps = merged_caps;
            // Affine join: consumed-on-both stays Cons; one-sided or
            // disagreeing states merge to Maybe (the lab's 3rd state).
            ctx.consumed = join_consumed(&then_cons, &else_cons);
            // Init join (N18): a diverged arm never reaches the join —
            // the merge takes the surviving side's map wholesale, so
            // `if c { x = 1 } else { return 0 }` leaves x Init.
            ctx.init = join_init(&then_init, &else_init);
            // Kill join: must only where both sides killed; one-sided
            // kills are may — the edge survives weakened.
            ctx.edge_kills = join_cert_map(&then_kills, &else_kills);
            ctx.cap_kills = join_cert_map(&then_ck, &else_ck);
            // Channel state joins by union: authority in flight on
            // EITHER path may be in flight after the join; a queued
            // packet on either path may be there. (The then-side's
            // transit persisted into the else arm's analysis — the
            // union restores the may-set explicitly.)
            ctx.transit = then_transit.union(&ctx.transit).cloned().collect();
            pending_symmetry(&[pend_keys(&else_out), pend_keys(&then_out)], *span, ctx);
            join_chan_may(ctx, &then_chans, &then_out);
            coalesce_join_bots(ctx, &[
                holder_bots(&else_edges, &else_caps, &else_out),
                holder_bots(&then_edges, &then_caps, &then_out),
            ]);
            let merged = merge_envs(&then_scopes, scopes, ctx);
            *scopes = merged;
            ctx.defers = merge_defer_frames(&then_defers, &else_defers);
            ctx.defer_res = merge_defer_res(&then_dres, &else_dres);
            for (k, v) in &then_paths {
                let j = match else_paths.get(k) {
                    Some(o) => join_prov(v, o, &ctx.regs),
                    None => join_prov(v, saved_paths.get(k).unwrap_or(v), &ctx.regs),
                };
                ctx.path_provs.insert(k.clone(), j);
            }
            rec_joined(ctx, Some(*span));
            }
            if lit.is_none() {
                ctx.diverged = pre_dead || (else_body.is_some() && then_d && else_d);
                // Continuation is terminated only when every arm that
                // could reach it terminated — a `return` arm unwinds,
                // it doesn't terminate.
                ctx.terminated = pre_term || (then_t && else_t);
                rec_flags(ctx);
            }
        }
        Stmt::While { cond, body, span } => {
            rec(format!("(while @{}:{})", span.line, span.col));
            let t = expr_tv(cond, scopes, ctx).0;
            if t != Ty::Bool {
                push_diag(ctx, Diag::new(
                    *span,
                    format!("`while` condition must be bool, found {t}"),
                ));
            }
            let pre_dead = ctx.diverged;
            let pre_term = ctx.terminated;
            let saved_scopes = scopes.clone();
            // The pre-loop state — with the condition's own effects
            // applied — is one arm of the exit join (the loop may run
            // zero times).
            let pre = take_snap(ctx);
            ctx.loops.push(LoopCtx {
                scope_depth: scopes.len(),
                breaks: Vec::new(),
                conts: Vec::new(),
            });
            check_block(body, scopes, ctx, ret);
            let lc = ctx.loops.pop().unwrap();
            let body_d = ctx.diverged;
            let body_t = ctx.terminated;
            let body_scopes = std::mem::replace(scopes, saved_scopes.clone());
            let body_snap = take_snap(ctx);
            // Loop exits: every break path, every continue path (it
            // re-enters the head and can exit a later iteration), and
            // the fall-through end — each merged against `pre`. For
            // `while true` the cond-false arm is statically dead:
            // `continue`s and the body fall-through are back-edges,
            // not exits — only `break` paths leave the loop.
            let is_true = matches!(cond, Expr::Bool(true, _));
            let mut exits: Vec<LoopSnap> = lc.breaks;
            if !is_true {
                exits.extend(lc.conts);
                if !body_d {
                    exits.push(body_snap);
                }
            }
            let no_out = exits.is_empty();
            if is_true && !no_out {
                // `while true` has no cond-false arm — the join base
                // is the exits themselves, not the zero-trip state.
                let first = exits.remove(0);
                restore_snap(ctx, first);
            } else {
                restore_snap(ctx, pre);
            }
            for ex in &exits {
                join_snap(ctx, ex, *span);
            }
            if is_true && no_out {
                // `while true` with no `break`: no path reaches the
                // continuation — unreachable whether the body paths
                // `return`ed, terminated, or fall through forever.
                // `terminated` iff the body's exits terminated
                // (`return`-only bodies unwind through the loop; the
                // fn still returns).
                ctx.diverged = true;
                ctx.terminated = body_t;
                rec_flags(ctx);
            } else {
                ctx.diverged = pre_dead;
                ctx.terminated = pre_term;
                rec_flags(ctx);
            }
            let merged = merge_envs(&body_scopes, &saved_scopes, ctx);
            *scopes = merged;
        }
        Stmt::Break { span } | Stmt::Continue { span } => {
            let is_break = matches!(s, Stmt::Break { .. });
            let kw = if is_break { "break" } else { "continue" };
            let Some(lc) = ctx.loops.last() else {
                push_diag(ctx, Diag::new(
                    *span,
                    format!("`{kw}` outside a loop"),
                ));
                return;
            };
            let depth = lc.scope_depth;
            // Everything bound deeper than the loop's scope dies at
            // the exit — the same structural death `}` performs,
            // including the task diagnostics that belong to a binding
            // dying early. Pop innermost-first so promoted survivors
            // cascade correctly, then re-push empty frames so each
            // enclosing check_block's natural pop stays balanced.
            let mut drained = 0;
            while scopes.len() > depth {
                // N54 — each exited scope replays its defers before its
                // bindings die; the loop's own scope survives.
                pop_scope_defers(scopes, ctx);
                let map = scopes.pop().unwrap_or_default();
                expire_scope(map, scopes, ctx, Some(*span), &format!("this `{kw}` exits"));
                drained += 1;
            }
            // Snapshot BEFORE the balance re-push — the exit state the
            // loop join wants ends at `depth`; the empty frames below
            // exist only so each enclosing check_block's natural pop
            // stays balanced on this (dead) continuation.
            let snap = take_snap(ctx);
            for _ in 0..drained {
                scopes.push(HashMap::new());
                ctx.defers.push(Vec::new());
            }
            let lc = ctx.loops.last_mut().unwrap();
            if is_break {
                lc.breaks.push(snap);
            } else {
                lc.conts.push(snap);
            }
            // This path left the body: joins from here treat it like
            // `return` — dead for block purposes, alive in the
            // loop-exit merge through its snapshot.
            ctx.diverged = true;
            rec_flags(ctx);
        }
        Stmt::Match { scrut, arms, span } => {
            check_match(scrut, arms, span, scopes, ctx, ret, None);
        }

        Stmt::Region { name, body, span } => {
            // A lexical lifetime frame: bindings inside get owner = this
            // region; provenance may flow in, never out.
            rec(format!(
                "(region {} {} @{}:{}",
                qs(name),
                qs(&ctx.regs[cur_region(ctx) as usize].name),
                span.line,
                span.col
            ));
            ctx.regs.push(RegionFrame {
                name: name.clone(),
                parent: Some(cur_region(ctx)),
            });
            check_block(body, scopes, ctx, ret);
            ctx.regs.pop();
            rec(format!("(region-pop {})", qs(name)));
        }
        Stmt::Consume { target, .. } => {
            // `consume p` — the standalone affine transfer. Same law
            // as an `own` call arg, minus the call.
            consume_place(target, scopes, ctx);
        }
        Stmt::Promote { target, span } => {
            // `promote x` — x's owner moves one region outward. The
            // closure rule: every ALIAS edge held inside x's subtree
            // must target a place that outlives the destination —
            // otherwise the promoted value dangles (edge closure, not
            // a graph walk: refs are all already edges).
            let n = match target {
                Expr::Var(n, _) => n.clone(),
                _ => {
                    ctx.errs
                        .push(Diag::new(*span, "promote requires a variable".to_string()));
                    return;
                }
            };
            let (key, dest) = match lookup(scopes, &n) {
                Some(b) => {
                    let key = format!("{n}#{}", b.epoch);
                    match ctx.regs[b.owner as usize].parent {
                        Some(p) => (key, p),
                        None => {
                            push_diag(ctx, Diag::new(
                                *span,
                                format!("cannot promote `{n}` out of the function frame"),
                            ));
                            return;
                        }
                    }
                }
                None => {
                    ctx.errs
                        .push(Diag::new(*span, format!("undefined variable `{n}`")));
                    return;
                }
            };
            // N54 — promotion escapes the scope that would replay the
            // deferred call; a reserved place must not leave it.
            if let Some(sp) = defer_conflict(ctx, &key) {
                push_diag(ctx, Diag::new(
                    *span,
                    format!(
                        "cannot promote `{n}`: it is reserved by the `defer` at line {}",
                        sp.line
                    ),
                ));
                return;
            }
            for ((h, t), _) in ctx.edges.clone().iter() {
                let under_x = h == &key
                    || h.starts_with(&format!("{key}."))
                    || h.starts_with(&format!("{key}["));
                if !under_x {
                    continue;
                }
                // Internal edges (x.f -> x.g) move with the value.
                if path_root(t) == key {
                    continue;
                }
                let troot = path_root(t);
                let tname = troot.split('#').next().unwrap_or(troot);
                // $arg*/param targets and unknowns outlive everything.
                let towner = match lookup(scopes, tname) {
                    Some(b) if format!("{tname}#{}", b.epoch) == troot => Some(b.owner),
                    _ => None,
                };
                if let Some(to) = towner {
                    if !is_ancestor_or_eq(to, dest, &ctx.regs) {
                        push_diag(ctx, Diag::new(
                            *span,
                            format!(
                                "dangling edge after promote: `{n}` holds a reference to `{tname}`, which dies in region `{}`",
                                ctx.regs[to as usize].name
                            ),
                        ));
                    }
                }
            }
            // Identity edges out of x never block — a stale handle is
            // a use-site error, not a promote error.
            if let Some(b) = lookup_mut(scopes, &n) {
                b.owner = dest;
            }
        }
        Stmt::Defer { expr, span } => {
            // N54 — `defer f(..)` registers a void call to run, LIFO,
            // at every normal exit of THIS scope. Not a destructor:
            // the operand places are frozen at registration so the
            // deferred call sees exactly what it named (moves, stores,
            // and rebinding them all reject), and the exit-time replay
            // applies the call's own effects — `defer free(p)` IS
            // `p`'s discharge.
            let Expr::Call(_, _, args, _) = expr else {
                push_diag(ctx, Diag::new(
                    *span,
                    "`defer` expects a call: `defer f(place, ..)`".to_string(),
                ));
                return;
            };
            // Operands must be place paths or literals — the deferred
            // call re-evaluates its operand places at exit, so a
            // compound operand would smuggle a hidden evaluation past
            // the freeze. `let r = &mut x; defer f(r);` is the escape.
            fn place_form(e: &Expr) -> bool {
                match e {
                    Expr::Var(..) | Expr::Int(..) | Expr::Bool(..) | Expr::Str(..) => true,
                    Expr::Deref(i, ..) => place_form(i),
                    Expr::Field(b, ..) => place_form(b),
                    Expr::Index(b, i, ..) => place_form(b) && place_form(i),
                    Expr::IndexRange { base, lo, hi, .. } => {
                        place_form(base)
                            && lo.as_ref().is_none_or(|l| place_form(l))
                            && hi.as_ref().is_none_or(|h| place_form(h))
                    }
                    _ => false,
                }
            }
            if args.iter().any(|a| !place_form(a)) {
                push_diag(ctx, Diag::new(
                    *span,
                    "`defer` operands must be place paths or literals — bind computed values first".to_string(),
                ));
            }
            // Dry-run on a clone: full call checking — arg types,
            // borrow law, move legality — without applying effects;
            // the effects belong to the exit replay. New diagnostics
            // surface at the defer site now.
            rec(format!("(defer @{}:{})", span.line, span.col));
            let mut s2 = scopes.clone();
            let mut c2 = ctx.clone();
            let (rt, _) = muted(|| expr_tv(expr, &mut s2, &mut c2));
            for d in c2.errs.drain(ctx.errs.len()..) {
                push_diag(ctx, d);
            }
            if rt != Ty::Void {
                push_diag(ctx, Diag::new(
                    *span,
                    format!("`defer` runs a `void` call, but it returns {rt}"),
                ));
            }
            // Operand keys: every place the call reads.
            let mut operands: Vec<String> = Vec::new();
            for a in args {
                collect_place_keys(a, scopes, ctx, &mut operands);
            }
            operands.sort();
            operands.dedup();
            // Consumed-on-exit keys: the dry-run's consume/kill marks
            // that weren't set before. They join the reservation —
            // `defer free(b); free(b)` must reject (the same freeze
            // the operand keys get).
            let mut consumes: Vec<String> = c2
                .consumed
                .keys()
                .filter(|k| !ctx.consumed.contains_key(*k))
                .cloned()
                .collect();
            consumes.extend(
                c2.edge_kills
                    .keys()
                    .filter(|k| !ctx.edge_kills.contains_key(*k))
                    .cloned(),
            );
            consumes.sort();
            consumes.dedup();
            for k in operands.iter().chain(consumes.iter()) {
                dres_set(ctx, k.clone(), *span);
            }
            if let Some(list) = ctx.defers.last_mut() {
                rec(format!("(defer-reg @{}:{})", span.line, span.col));
                list.push(DeferRec {
                    operands,
                    consumes,
                    expr: expr.clone(),
                });
            }
        }
        Stmt::Expr { expr, .. } => {
            expr_tv(expr, scopes, ctx);
            if ctx.last_spawn.is_some() {
                ctx.last_spawn = None;
                push_diag(ctx, Diag::new(
                    expr.span(),
                    "a `spawn` must be bound directly: `let t = spawn { .. }`".to_string(),
                ));
            }
        }
    }
    // Side-channels are consumed by their binding site or dropped —
    // a leftover pending lock/mutex tuple can never survive a stmt.
    // An unclaimed recv'd packet drops too: its authority left transit
    // at the pop but installs on nobody — released, not leaked.
    ctx.last_lock = None;
    ctx.last_mutex = None;
    ctx.last_recv = None;
    ctx.last_tag = None;
    // CHAN-004 S3b — an unbound bounded-send Result is an unexamined
    // result: the latent packet's ownership has nowhere to resolve.
    if ctx.last_send.is_some() {
        ctx.last_send = None;
        push_diag(ctx, Diag::new(
            s.span(),
            "a bounded `send` result must be bound and examined — match on its `Ok`/`Err`".to_string(),
        ));
    }
    // Same for a lit packet nobody claimed — `replace(w.n, v);` in
    // statement position yields an old value that dies unbound, and
    // an arg-position literal's fields install nowhere. Dropping the
    // channels keeps the next `let` from inheriting a dead value's
    // edges (phantom authority is the unsound direction).
    ctx.lit_edges.clear();
    ctx.lit_caps.clear();
    ctx.lit_dead.clear();
    ctx.lit_lens.clear();
    ctx.lit_tags.clear();
    ctx.lit_csafe.clear();
    ctx.last_lit = None;
}


/// `(summary …)` — the fn's exported effect contract. Every field
/// sorts: `creates`/`kills` come from HashMap iteration, so order in
/// the Vec is nondeterministic — canonical order is by key.
fn rec_sum(name: &str, s: &FnSummary, regs: &[RegionFrame]) {
    let mut o = format!("(summary {}", qs(name));
    let mut cr: Vec<_> = s.creates.iter().collect();
    cr.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (h, t, a, c) in cr {
        o.push_str(&format!(
            " (cr {} {} {} {})",
            qs(h),
            qs(t),
            auth_s(*a),
            cert_s(*c)
        ));
    }
    let mut kl: Vec<_> = s.kills.iter().collect();
    kl.sort_by(|a, b| a.0.cmp(b.0));
    for (k, c) in kl {
        o.push_str(&format!(" (kl {} {})", qs(k), cert_s(*c)));
    }
    let mut cc: Vec<_> = s.creates_caps.iter().collect();
    cc.sort_by(|a, b| (&a.0, &a.1).cmp(&(&b.0, &b.1)));
    for (h, t, c) in cc {
        o.push_str(&format!(" (cc {} {} {})", qs(h), qs(t), cert_s(*c)));
    }
    let mut ck: Vec<_> = s.cap_kills.iter().collect();
    ck.sort_by(|a, b| a.0.cmp(b.0));
    for (k, c) in ck {
        o.push_str(&format!(" (ck {} {})", qs(k), cert_s(*c)));
    }
    let mut ip: Vec<_> = s.init_paths.iter().collect();
    ip.sort();
    for k in ip {
        o.push_str(&format!(" (ip {})", qs(k)));
    }
    let mut re: Vec<_> = s.ret_edges.iter().collect();
    re.sort_by(|a, b| a.0.cmp(&b.0));
    for (k, m) in re {
        o.push_str(&format!(" (re {} {})", qs(k), if *m { "mut" } else { "im" }));
    }
    let mut rc: Vec<_> = s.ret_caps.iter().collect();
    rc.sort();
    for k in rc {
        o.push_str(&format!(" (rc {})", qs(k)));
    }
    let mut sy: Vec<_> = s.syncs.iter().collect();
    sy.sort();
    for k in sy {
        o.push_str(&format!(" (sy {})", qs(k)));
    }
    for (rel, ri) in &s.ret_lit_edges {
        let mut ts = String::new();
        for t in &ri.targets {
            ts.push_str(&format!(" {}", qs(t)));
        }
        o.push_str(&format!(
            " (rle {}{} {} {} {})",
            qs(rel),
            ts,
            if ri.mutable { "mut" } else { "im" },
            ri.move_from
                .as_deref()
                .map(|m| qs(m).to_string())
                .unwrap_or_else(|| "-".to_string()),
            if ri.foreign { "foreign" } else { "local" }
        ));
    }
    for (rel, caps) in &s.ret_lit_caps {
        o.push_str(&format!(" (rlc {}", qs(rel)));
        for c in caps {
            o.push_str(&format!(" {}", qs(c)));
        }
        o.push(')');
    }
    for (rel, p) in &s.ret_lit_provs {
        o.push_str(&format!(" (rlp {} {})", qs(rel), prov_s(p, regs)));
    }
    if s.diverges {
        o.push_str(" diverges");
    }
    o.push(')');
    rec(o);
}

/// The one Ctx construction literal — `check_fn` and the S3a
/// state tests share it so a new field can't silently default.
#[allow(clippy::too_many_arguments)]
fn fresh_ctx<'a>(
    name: &str,
    ret: &Ty,
    fns: &'a HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
    local_fns: &'a HashSet<String>,
    structs: &'a Structs,
    enums: &'a Enums,
    sums: &'a HashMap<String, FnSummary>,
    private_syms: &'a HashSet<String>,
) -> Ctx<'a> {
    Ctx {
        fns,
        local_fns,
        structs,
        enums,
        tag: HashMap::new(),
        last_tag: None,
        regs: vec![RegionFrame {
            name: name.to_string(),
            parent: None,
        }],
        path_provs: HashMap::new(),
        last_lit: None,
        lit_edges: Vec::new(),
        lit_dead: Vec::new(),
        lit_lens: Vec::new(),
        lit_tags: Vec::new(),
        lit_csafe: Vec::new(),
        edges: HashMap::new(),
        caps: HashMap::new(),
        lit_caps: Vec::new(),
        next_epoch: 0,
        consumed: HashMap::new(),
        edge_kills: HashMap::new(),
        cap_kills: HashMap::new(),
        ret_edges: Vec::new(),
        ret_caps: Vec::new(),
        ret_lit_edges: Vec::new(),
        ret_lit_caps: Vec::new(),
        ret_lit_provs: Vec::new(),
        call_bases: HashMap::new(),
        replace_outs: HashMap::new(),
        arg_atomic: HashMap::new(),
        arg_mutex: HashMap::new(),
        doms: HashMap::new(),
        last_lock: None,
        last_mutex: None,
        chan_queues: HashMap::new(),
        transit: HashSet::new(),
        last_recv: None,
        last_send: None,
        out: Outstanding::default(),
        arg_chan: HashMap::new(),
        foreign: HashSet::new(),
        slice_lens: HashMap::new(),
        cstr_safe: HashSet::new(),
        init: HashMap::new(),
        init_writes: HashSet::new(),
        diverged: false,
        terminated: false,
        holds: HashMap::new(),
        tasks: HashMap::new(),
        task: None,
        fn_ret: ret.clone(),
        last_spawn: None,
        loops: Vec::new(),
        sums,
        private_syms,
        match_yields: HashMap::new(),
        abandoned: HashSet::new(),
        defers: vec![Vec::new()],
        defer_res: HashMap::new(),
        errs: Vec::new(),
    }
}

fn check_fn<'a>(
    f: &FnDef,
    fns: &'a HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
    local_fns: &'a HashSet<String>,
    structs: &'a Structs,
    enums: &'a Enums,
    sums: &'a HashMap<String, FnSummary>,
    private_syms: &'a HashSet<String>,
    errs: &mut Vec<Diag>,
) -> FnSummary {
    let mut ctx = fresh_ctx(
        &f.name,
        &f.ret,
        fns,
        local_fns,
        structs,
        enums,
        sums,
        private_syms,
    );
    rec(format!(
        "(fn {} @{}:{}",
        qs(&f.name),
        f.span.line,
        f.span.col
    ));
    let mut scopes: Scopes = vec![HashMap::new()];
    for (i, (n, a, t)) in f.params.iter().enumerate() {
        // `&T` params AND capability params carry caller provenance —
        // both name caller storage, so both can feed a `&T` return.
        let prov = if matches!(
            t,
            Ty::Ref { .. } | Ty::Handle(_) | Ty::Slice { .. } | Ty::Str
        ) || a.is_some()
        {
            Prov::Param
        } else {
            Prov::Unknown
        };
        ctx.next_epoch += 1;
        let epoch = ctx.next_epoch;
        let key = format!("{n}#{epoch}");
        // N18 — params arrive initialized by caller contract; cap
        // params' storage is caller storage, so `$arg{i}` is Def too.
        init_set(&mut ctx, key.clone(), Cert::Def);
        init_set(&mut ctx, format!("$arg{i}"), Cert::Def);
        if let Some(auth) = a {
            // The param IS the caller's place — its own authority edge
            // to synthetic $argN carries the contract.
            edge_set(&mut ctx, key.clone(), format!("$arg{i}"), EdgeFact::new(*auth, Cert::Def, None));
        } else if let Ty::Handle(_) = t {
            // Handle params get a synthetic IDENTITY edge — `*h` inside
            // resolves to $argN, which is always treated live.
            cap_set(&mut ctx, key.clone(), format!("$arg{i}"), Cert::Def);
        } else if let Ty::Ref { mutable, .. } | Ty::Slice { mutable, .. } = t {
            // A `&T`/`slice` param's edge to caller storage is
            // synthetic: distinct params can't be proven non-aliased
            // inside, but the call SITE rejects aliasing `&mut`s at
            // creation. `slice` carries the same contract — the edge
            // IS its span authority (`s[i]` resolves through it).
            let kind = if *mutable { Auth::Write } else { Auth::Read };
            edge_set(&mut ctx, key.clone(), format!("$arg{i}"), EdgeFact::new(kind, Cert::Def, None));
        } else if t == &Ty::Str {
            // A `str` param reads caller storage through the same
            // synthetic edge — `s.bytes[i]` resolves through it, and
            // `$arg{i}` survives in the summary for str returns.
            edge_set(&mut ctx, key.clone(), format!("$arg{i}"), EdgeFact::new(Auth::Read, Cert::Def, None));
        }
        // `atomic`-typed param storage — direct or behind `&T` —
        // records what `$arg{i}` atomic ops act on.
        let atom = match t {
            Ty::Atomic(inner) => Some((**inner).clone()),
            Ty::Ref { elem, .. } | Ty::Slice { elem, .. } => match &**elem {
                Ty::Atomic(inner) => Some((**inner).clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(inner) = atom {
            ctx.arg_atomic.insert(format!("$arg{i}"), inner);
        }
        // `mutex`-typed param storage — the callee's domain key for
        // this arg IS `$arg{i}`; the summary replays it caller-side.
        let mx = match t {
            Ty::Mutex(inner) => Some((**inner).clone()),
            Ty::Ref { elem, .. } | Ty::Handle(elem) => match &**elem {
                Ty::Mutex(inner) => Some((**inner).clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(inner) = mx {
            let k = format!("$arg{i}");
            ctx.arg_mutex.insert(k.clone(), inner);
            dom_set(&mut ctx, k.clone(), k);
        }
        // `channel`-typed param storage — `$arg{i}` names the caller's
        // endpoint; queue facts stay callee-local (v1).
        let chn = match t {
            Ty::Channel(inner) => Some((**inner).clone()),
            Ty::Ref { elem, .. } | Ty::Handle(elem) => match &**elem {
                Ty::Channel(inner) => Some((**inner).clone()),
                _ => None,
            },
            _ => None,
        };
        if let Some(inner) = chn {
            ctx.arg_chan.insert(format!("$arg{i}"), inner);
        }
        index::record(index::Occ {
            span: f.span,
            name: n.clone(),
            kind: "param",
            hover: format!("param `{n}`: {t}"),
            def: None,
            file: None,
        });
        rec(format!(
            "(param {} @{}:{} {} {} {})",
            qs(&key),
            f.span.line,
            f.span.col,
            qs(&format!("{t}")),
            prov_s(&prov, &ctx.regs),
            match a {
                Some(x) => auth_s(*x),
                None => "-",
            }
        ));
        scopes[0].insert(
            n.clone(),
            Bind {
                ty: t.clone(),
                decl: f.span,
                prov,
                owner: 0,
                epoch,
                auth: *a,
                guard: None,
            },
        );
    }
    check_block(&f.body, &mut scopes, &mut ctx, &f.ret);
    errs.extend(ctx.errs);

    // The effect summary: everything the body did to PARAM places,
    // keyed in the callee's `$argN[.path]` vocabulary. `own` params
    // name private callee storage (the caller's copy is already
    // consumed) — their effects stay local.
    let own: HashSet<String> = f
        .params
        .iter()
        .enumerate()
        .filter(|(_, (_, a, _))| *a == Some(Auth::Own))
        .map(|(i, _)| format!("$arg{i}"))
        .collect();
    let own_rooted = |k: &str| {
        own.iter().any(|o| {
            k == o || k.starts_with(&format!("{o}.")) || k.starts_with(&format!("{o}["))
        })
    };
    let sum = FnSummary {
        creates: ctx
            .edges
            .iter()
            .filter(|((h, t), _)| {
                h.starts_with("$arg") && t.starts_with("$arg")
                    && !own_rooted(h)
                    && !own_rooted(t)
            })
            .map(|((h, t), f)| (h.clone(), t.clone(), f.kind, f.cert))
            .collect(),
        kills: ctx
            .edge_kills
            .iter()
            .filter(|(k, _)| k.starts_with("$arg") && !own_rooted(k))
            .map(|(k, c)| (k.clone(), *c))
            .collect(),
        creates_caps: ctx
            .caps
            .iter()
            .filter(|((h, t), _)| {
                h.starts_with("$arg") && t.starts_with("$arg")
                    && !own_rooted(h)
                    && !own_rooted(t)
            })
            .map(|((h, t), c)| (h.clone(), t.clone(), *c))
            .collect(),
        cap_kills: ctx
            .cap_kills
            .iter()
            .filter(|(k, _)| k.starts_with("$arg") && !own_rooted(k))
            .map(|(k, c)| (k.clone(), *c))
            .collect(),
        // N18 — a `write` param's definite stores: `h.h = handle x`
        // inside `link(write h)` exports `$arg0.h`, so the caller's
        // `h#e.h` provably holds a value after the call. `own` params
        // are consumed — their stores die with the transfer.
        init_paths: ctx
            .init_writes
            .iter()
            .filter(|k| {
                !own_rooted(k) && matches!(ctx.init.get(*k), Some(Cert::Def))
            })
            .cloned()
            .collect(),
        ret_edges: ctx.ret_edges,
        ret_caps: ctx.ret_caps,
        ret_lit_edges: ctx.ret_lit_edges,
        ret_lit_caps: ctx.ret_lit_caps,
        ret_lit_provs: ctx.ret_lit_provs,
        syncs: ctx
            .holds
            .iter()
            .filter(|(k, a)| **a == Auth::Sync && k.starts_with("$arg") && !own_rooted(k))
            .map(|(k, _)| k.clone())
            .collect(),
        diverges: ctx.terminated,
    };
    rec_sum(&f.name, &sum, &ctx.regs);
    sum
}

// ---------- CHAN-004 S3a — checker-state tests ----------
//
// Unit tests for the outstanding-capability machinery, mirroring
// bench/chan4/t/mayoutstanding.py's scenarios wherever semantics
// overlap. The invariant under test: no named source is released
// from transit while any modeled execution path can still hold a
// capability for it (MayOutstanding(t) != ∅). The machinery is
// dormant for `channel` — these drive the state ops directly.
#[cfg(test)]
mod s3a_tests {
    use super::*;

    struct Env {
        fns: HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
        local_fns: HashSet<String>,
        structs: Structs<'static>,
        enums: Enums<'static>,
        sums: HashMap<String, FnSummary>,
        private_syms: HashSet<String>,
    }

    fn env() -> Env {
        Env {
            fns: HashMap::new(),
            local_fns: HashSet::new(),
            structs: HashMap::new(),
            enums: HashMap::new(),
            sums: HashMap::new(),
            private_syms: HashSet::new(),
        }
    }

    fn mk<'a>(e: &'a Env) -> Ctx<'a> {
        fresh_ctx(
            "t",
            &Ty::Void,
            &e.fns,
            &e.local_fns,
            &e.structs,
            &e.enums,
            &e.sums,
            &e.private_syms,
        )
    }

    fn bind(scopes: &mut Scopes, name: &str, epoch: u32) {
        scopes[0].insert(
            name.to_string(),
            Bind {
                ty: Ty::I32,
                decl: Span { line: 0, col: 0 },
                prov: Prov::Unknown,
                owner: 0,
                epoch,
                auth: None,
                guard: None,
            },
        );
    }

    /// A `&mut t` send packet — Write edge plus a transit mark,
    /// exactly what packet_for cuts for `send(ch, &mut t)`.
    fn pkt_mut(t: &str) -> Packet {
        Packet {
            ty: Ty::Ref {
                mutable: true,
                elem: Box::new(Ty::I32),
            },
            edges: vec![(
                String::new(),
                t.to_string(),
                EdgeFact::new(Auth::Write, Cert::Def, None),
            )],
            caps: vec![],
            sent: vec![t.to_string()],
            auth: Some(Auth::Write),
            is_handle: false,
        }
    }

    /// `send(ch, pkt)` as chan_op records it.
    fn send_push(ctx: &mut Ctx, ep: &str, pkt: &Packet) {
        ctx.chan_queues
            .entry(ep.to_string())
            .or_default()
            .push(pkt.clone());
        chan_push(ctx, ep, pkt);
    }

    /// `recv(ch)`'s pop as chan_op records it.
    fn recv_pop(ctx: &mut Ctx, ep: &str) -> Option<Packet> {
        let pkt = ctx
            .chan_queues
            .get_mut(ep)
            .and_then(|q| if q.is_empty() { None } else { Some(q.remove(0)) })?;
        chan_pop(ctx, ep);
        Some(pkt)
    }

    fn var_e() -> Expr {
        Expr::Var("v".to_string(), Span { line: 0, col: 0 })
    }

    /// Does `key`'s access conflict with any live (or may-alias)
    /// edge — the borrow matrix's view.
    fn conflicts(ctx: &Ctx, key: &str) -> bool {
        !edges_to(ctx, key).is_empty()
    }

    /// Arm-view builder: `(holder, bot_index)` ⊥ positions on one
    /// arm — the union view across edge and cap facts (a ⊥ key
    /// naming the same position in both maps counts once).
    fn arm_bots(pairs: &[(&str, u64)]) -> HashMap<String, Vec<String>> {
        let mut m: HashMap<String, Vec<String>> = HashMap::new();
        for (h, i) in pairs {
            m.entry(h.to_string())
                .or_default()
                .push(bot_key(*i));
        }
        for v in m.values_mut() {
            v.sort();
            v.dedup();
        }
        m
    }

    /// S1 — ordered endpoint: two pushes, two verbatim claims in
    /// FIFO order; each claim installs the packet's real target
    /// and releases its transit mark.
    #[test]
    fn ordered_claims_verbatim() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());

        let p1 = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p1, &e0, &mut scopes, &mut ctx);
        assert!(ctx.edges.contains_key(&("v#0".into(), "x#1".into())));
        assert!(!ctx.transit.contains("x#1"));
        // No unknown claim minted on the ordered path.
        assert!(ctx.out.bot_cands.is_empty());

        let p2 = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "w#0", p2, &e0, &mut scopes, &mut ctx);
        assert!(ctx.edges.contains_key(&("w#0".into(), "y#1".into())));
        assert!(!ctx.transit.contains("y#1"));
    }

    /// S6a — an unordered claim gets `${bot}` whose candidate set
    /// is every name still possibly outstanding on the endpoint:
    /// may-alias, never distinct identity.
    #[test]
    fn unordered_claims_bot_with_candidates() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());
        // A branch join where the other arm sent nothing —
        // membership may → unordered.
        let other_q: HashMap<String, Vec<Packet>> = HashMap::new();
        let other_out = Outstanding::default();
        join_chan_may(&mut ctx, &other_q, &other_out);
        assert!(ctx.out.chan_meta.get(ep).unwrap().unordered);

        let p = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p, &e0, &mut scopes, &mut ctx);
        // The claim is unknown-identity: v's edge targets ${bot0},
        // whose candidates are {x,y} — NOT a real named edge.
        let bk = bot_key(0);
        assert!(ctx.edges.contains_key(&("v#0".into(), bk.clone())));
        let cs = ctx.out.bot_cands.get(&bk).unwrap();
        assert!(cs.contains("x#1") && cs.contains("y#1"));
        // Named sources stay transit — a pop can't prove which
        // departed.
        assert!(ctx.transit.contains("x#1") && ctx.transit.contains("y#1"));
        // And the unknown claim may-aliases BOTH candidates.
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "y#1"));
    }

    /// Equal queues on both paths keep order provable.
    #[test]
    fn join_equal_queues_stays_ordered() {
        let e = env();
        let mut ctx = mk(&e);
        let ep = "ch#0";
        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        let q = ctx.chan_queues.clone();
        let o = ctx.out.clone();
        join_chan_may(&mut ctx, &q, &o);
        assert!(!ctx
            .out
            .chan_meta
            .get(ep)
            .is_some_and(|m| m.unordered));
    }

    /// S2 — a branch-join ghost: membership is may, so a claim
    /// can't prove which candidate physically departed. After all
    /// pops account for the pushes the counter says "drained" —
    /// but the live unknown claim STILL keeps both names
    /// outstanding (L); only its death releases them.
    #[test]
    fn ghost_packet_keeps_name_outstanding() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());

        // Two deliveries claimed into v and w — both unknown, each
        // may-aliasing {x,y}.
        let p1 = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p1, &e0, &mut scopes, &mut ctx);
        let p2 = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "w#0", p2, &e0, &mut scopes, &mut ctx);
        // pops == pushes → U is empty. x stays outstanding through
        // the live unknown claims — release needs those gone too.
        assert!(chan_drained(&ctx, ep));
        assert!(may_outstanding(&ctx, "x#1"));
        assert!(may_outstanding(&ctx, "y#1"));
        // Kill both claims (v, w die) → L empties → releasable.
        ctx.edges.remove(&("v#0".into(), bot_key(0)));
        ctx.edges.remove(&("w#0".into(), bot_key(1)));
        assert!(!may_outstanding(&ctx, "x#1"));
        assert!(!may_outstanding(&ctx, "y#1"));
        assert!(!transit_check(&e0, "x#1", &mut ctx));
        assert!(!transit_check(&e0, "y#1", &mut ctx));
    }

    /// Join rule: a departure credited on one arm is not one on
    /// both — pops merge by MIN, pushes by MAX.
    #[test]
    fn pops_join_min_pushes_max() {
        let e = env();
        let mut ctx = mk(&e);
        let ep = "ch#0";
        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        recv_pop(&mut ctx, ep);
        recv_pop(&mut ctx, ep);
        // Other path: same pushes, only one pop.
        let mut oo = Outstanding::default();
        oo.chan_meta.insert(
            ep.to_string(),
            ChanMeta {
                unordered: false,
                pushes: 2,
                pops: 1,
            },
        );
        let q2 = ctx.chan_queues.clone();
        join_chan_may(&mut ctx, &q2, &oo);
        let m = ctx.out.chan_meta.get(ep).unwrap();
        assert_eq!((m.pushes, m.pops), (2, 1));
        assert!(!chan_drained(&ctx, ep));
    }

    /// Spawn merge: a child's interleaved endpoint effects mark
    /// unordered and compose counters additively over the
    /// spawn-time base.
    #[test]
    fn spawn_merge_unordered_and_counters() {
        let e = env();
        let mut ctx = mk(&e);
        let ep = "ch#0";
        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        let base = ctx.out.chan_meta.clone();
        let ext: HashSet<String> = [ep.to_string()].into_iter().collect();

        let mut co = Outstanding::default();
        co.chan_meta.insert(
            ep.to_string(),
            ChanMeta {
                unordered: false,
                pushes: 3, // base 1 + child's own 2
                pops: 0,
            },
        );
        co.chan_names
            .entry(ep.to_string())
            .or_default()
            .insert("z#1".into());
        merge_task_out(&mut ctx, &co, &base, &ext);
        let m = ctx.out.chan_meta.get(ep).unwrap();
        assert!(m.unordered);
        assert_eq!(m.pushes, 3);
        assert!(ctx.out.chan_names.get(ep).unwrap().contains("z#1"));
    }

    /// S3 — a pending bounded send's latent packet names its
    /// sources: they are outstanding (P) and join the endpoint's
    /// candidate set (a pending SENT is physically deliverable).
    #[test]
    fn pending_send_names_outstanding_and_candidates() {
        let e = env();
        let mut ctx = mk(&e);
        let ep = "ch#0";
        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        ctx.out.pending_send.insert(
            "r#0".to_string(),
            vec![PendingPkt {
                ep: ep.to_string(),
                pkt: pkt_mut("z#1"),
            }],
        );
        assert!(may_outstanding(&ctx, "z#1"));
        let c = chan_candidates(&ctx, ep);
        assert!(c.contains("x#1") && c.contains("z#1"));
        // Resolving the fact (either outcome) drops the residence.
        ctx.out.pending_send.clear();
        assert!(!may_outstanding(&ctx, "z#1"));
    }

    /// S4 — forwarding: a packet carrying a `${bot}` edge names
    /// the unknown's candidates — names move WITH the packet.
    #[test]
    fn forwarded_bot_names() {
        let e = env();
        let mut ctx = mk(&e);
        let bk = bot_key(ctx.out.next_bot);
        ctx.out.next_bot += 1;
        ctx.out
            .bot_cands
            .insert(bk.clone(), ["x#1".to_string()].into_iter().collect());
        let mut fwd = pkt_mut("q#7");
        fwd.edges.push((
            String::new(),
            bk,
            EdgeFact::new(Auth::Write, Cert::Def, None),
        ));
        let ns = packet_names(&fwd, &ctx.out.bot_cands);
        assert!(ns.contains("x#1") && ns.contains("q#7"));
        send_push(&mut ctx, "ch#0", &fwd);
        assert!(may_outstanding(&ctx, "x#1"));
    }

    /// A `${bot}` edge moved to a new holder still may-aliases its
    /// candidates — claims survive moves anywhere the binding goes.
    #[test]
    fn moved_bot_edge_still_aliases() {
        let e = env();
        let mut ctx = mk(&e);
        let bk = bot_key(0);
        ctx.out
            .bot_cands
            .insert(bk.clone(), ["x#1".to_string()].into_iter().collect());
        ctx.edges.insert(
            ("v#0".into(), bk.clone()),
            EdgeFact::new(Auth::Write, Cert::Def, None),
        );
        assert!(conflicts(&ctx, "x#1"));
        // Move v's edge wholesale to w — the alias follows.
        let f = ctx.edges.remove(&("v#0".into(), bk.clone())).unwrap();
        ctx.edges.insert(("w#0".into(), bk), f);
        let hs: Vec<String> = edges_to(&ctx, "x#1").into_iter().map(|(h, _)| h).collect();
        assert_eq!(hs, vec!["w#0".to_string()]);
    }

    /// Multi-resource packet: one delivery can carry caps to x
    /// AND y — the unknown claim may-aliases both.
    #[test]
    fn multi_resource_packet_candidates() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";
        let mut p = pkt_mut("x#1");
        p.edges.push((
            String::new(),
            "y#1".into(),
            EdgeFact::new(Auth::Write, Cert::Def, None),
        ));
        p.sent.push("y#1".into());
        send_push(&mut ctx, ep, &p);
        // Two pushes' worth of names from different paths merge.
        send_push(&mut ctx, ep, &pkt_mut("z#1"));
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());
        let pkt = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", pkt, &e0, &mut scopes, &mut ctx);
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "y#1") && conflicts(&ctx, "z#1"));
    }

    /// |C| = 1 proves identity even on an unordered endpoint —
    /// the claim installs verbatim and releases transit.
    #[test]
    fn singleton_promotes_to_verbatim() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        let e0 = var_e();
        let ep = "ch#0";
        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        transit_set(&mut ctx, "x#1".into());
        // Ghost-only union → unordered, but the only possible
        // source is still x.
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());
        let pkt = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", pkt, &e0, &mut scopes, &mut ctx);
        assert!(ctx.edges.contains_key(&("v#0".into(), "x#1".into())));
        assert!(!ctx.transit.contains("x#1"));
        assert!(ctx.out.bot_cands.is_empty());
    }

    /// Nested-struct edges (rel suffixes) re-target to `${bot}`
    /// with the packet — sub-path claims stay unknown too.
    #[test]
    fn nested_suffix_edges_retarget() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        let e0 = var_e();
        let ep = "ch#0";
        let mut p = pkt_mut("x#1");
        p.edges.push((
            ".f".into(),
            "x#1.f".into(),
            EdgeFact::new(Auth::Write, Cert::Def, None),
        ));
        p.sent.push("x#1.f".into());
        send_push(&mut ctx, ep, &p);
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());
        let pkt = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", pkt, &e0, &mut scopes, &mut ctx);
        // Per-position ⊥s: the root cap and the nested .f cap are
        // independent positions — distinct unknown identities,
        // each may-aliasing every candidate name.
        assert!(ctx.edges.contains_key(&("v#0".into(), bot_key(0))));
        assert!(ctx.edges.contains_key(&("v#0.f".into(), bot_key(1))));
        let cs0 = ctx.out.bot_cands.get(&bot_key(0)).unwrap();
        let cs1 = ctx.out.bot_cands.get(&bot_key(1)).unwrap();
        assert!(cs0.contains("x#1") && cs0.contains("x#1.f"));
        assert!(cs1.contains("x#1") && cs1.contains("x#1.f"));
    }

    /// U/L/P separation — each residence class alone is enough to
    /// hold a name outstanding; only all-three-empty releases.
    /// (The model's mutation tests kill one component each.)
    #[test]
    fn may_outstanding_components() {
        let e = env();
        let mut ctx = mk(&e);
        // U only.
        send_push(&mut ctx, "ch#0", &pkt_mut("u#1"));
        assert!(may_outstanding(&ctx, "u#1"));
        // L only: counters drained, a live unknown names v.
        ctx.out.chan_meta.get_mut("ch#0").unwrap().pops = 1;
        let bk = bot_key(ctx.out.next_bot);
        ctx.out.next_bot += 1;
        ctx.out
            .bot_cands
            .insert(bk.clone(), ["v#1".to_string()].into_iter().collect());
        ctx.edges.insert(
            ("w#0".into(), bk),
            EdgeFact::new(Auth::Write, Cert::Def, None),
        );
        assert!(may_outstanding(&ctx, "v#1"));
        // P only.
        ctx.out.pending_send.insert(
            "r#0".into(),
            vec![PendingPkt {
                ep: "ch#1".into(),
                pkt: pkt_mut("p#1"),
            }],
        );
        assert!(may_outstanding(&ctx, "p#1"));
        // A name with no residence is free.
        assert!(!may_outstanding(&ctx, "z#9"));
    }

    /// A transit mark lifts ONLY through MayOutstanding — the
    /// release path every access re-asks.
    #[test]
    fn transit_release_needs_empty_outstanding() {
        let e = env();
        let mut ctx = mk(&e);
        let e0 = var_e();
        transit_set(&mut ctx, "x#1".into());
        send_push(&mut ctx, "ch#0", &pkt_mut("x#1"));
        assert!(transit_check(&e0, "x#1", &mut ctx));
        ctx.errs.clear();
        // Drain provably → the mark lifts at the next query.
        recv_pop(&mut ctx, "ch#0");
        ctx.chan_queues.clear(); // claimed — residence is named now
        // x is claimed into a named edge below; remove the queued
        // packet's residence by counting the pop (done above) and
        // simulating the verbatim claim's untransit.
        untransit(&mut ctx, "x#1");
        assert!(!transit_check(&e0, "x#1", &mut ctx));
        assert!(ctx.errs.is_empty());
    }

    /// Join-coalescing: v claimed cap₁ on one arm and cap₂ on the
    /// other — ONE physical delivery, so the merged binding holds
    /// a single `${bot}` whose candidates UNION, not two edges
    /// pretending to be two independent deliveries. Alternatives
    /// always arrive Maybe (an arm-local fact downgrades at join);
    /// Def ⊥ edges are independent definite positions and must
    /// NEVER coalesce.
    #[test]
    fn coalesce_merges_branch_bot_edges() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, t) in ["x#1", "y#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            ctx.edges.insert(
                ("v#0".into(), bk),
                EdgeFact::new(Auth::Write, Cert::Maybe, None),
            );
        }
        ctx.out.next_bot = 2;
        // Path-alternative evidence: ⊥0 on arm A, ⊥1 on arm B —
        // mutually exclusive claims for one position. Merge to ONE
        // slot with the unioned candidate set.
        // Path-alternative evidence: ⊥0 on arm A, ⊥1 on arm B —
        // mutually exclusive claims for one position. Merge to ONE
        // slot with the unioned candidate set.
        coalesce_join_bots(
            &mut ctx,
            &[arm_bots(&[("v#0", 0)]), arm_bots(&[("v#0", 1)])],
        );
        let bots: Vec<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(bots.len(), 1);
        let cs = ctx.out.bot_cands.get(bots[0]).unwrap();
        assert!(cs.contains("x#1") && cs.contains("y#1"));
        assert_eq!(
            ctx.edges.get(&("v#0".into(), bots[0].clone())).unwrap().cert,
            Cert::Maybe
        );
    }

    /// `Cert::Maybe` is NOT exclusivity evidence: two Maybe ⊥ edges
    /// co-resident on ONE arm are two independent physical
    /// capabilities that both live on that path. An arm holding
    /// them joined against an arm holding none must preserve both
    /// — cardinality is part of the state, not a guess from certs.
    #[test]
    fn coalesce_preserves_same_arm_maybe_positions() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, t) in ["x#1", "y#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            ctx.edges.insert(
                ("v#0".into(), bk),
                EdgeFact::new(Auth::Write, Cert::Maybe, None),
            );
        }
        ctx.out.next_bot = 2;
        // Arm A holds BOTH Maybe ⊥s co-resident; arm B holds none.
        // Co-residency is the disproof of exclusivity — preserve.
        let arm_a = arm_bots(&[("v#0", 0), ("v#0", 1)]);
        let arm_b = HashMap::new();
        coalesce_join_bots(&mut ctx, &[arm_a, arm_b]);
        let bots: Vec<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        // No merge: both ⊥ identities and both candidate sets live.
        assert_eq!(bots.len(), 2);
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "y#1"));
    }

    /// Capability cardinality across arms: arm A holds two ⊥
    /// positions on `v`, arm B holds one. The join emits
    /// max(2,1) = 2 slots — never 1 (under-count) — and slot
    /// candidates union across the arms that contribute a slot.
    #[test]
    fn coalesce_preserves_max_cardinality() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, t) in ["x#1", "y#1", "z#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            ctx.edges.insert(
                ("v#0".into(), bk),
                EdgeFact::new(Auth::Write, Cert::Maybe, None),
            );
        }
        ctx.out.next_bot = 3;
        let arm_a = arm_bots(&[("v#0", 0), ("v#0", 1)]);
        let arm_b = arm_bots(&[("v#0", 2)]);
        coalesce_join_bots(&mut ctx, &[arm_a, arm_b]);
        let bots: BTreeSet<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(bots.len(), 2);
        // Alignment-free union: EVERY emitted slot may-alias the
        // union of all contributing candidates — a merged slot can
        // represent any arm position.
        for b in &bots {
            let cs = ctx.out.bot_cands.get(*b).unwrap();
            assert!(
                cs.contains("x#1") && cs.contains("y#1") && cs.contains("z#1")
            );
        }
    }

    /// Task joins are physical merges, not alternatives: a child
    /// spawn's ⊥s join the parent's state additively and must never
    /// be treated as path-exclusive — `merge_task_out` runs no
    /// coalescing at all.
    #[test]
    fn task_merge_preserves_bot_cardinality() {
        let e = env();
        let mut ctx = mk(&e);
        // Parent holds ⊥0 on v; the child (executed, joined by
        // delta) holds ⊥1 on v — BOTH capabilities exist after the
        // merge, concurrently.
        let mut child_out = Outstanding::default();
        let mut child_edges: HashMap<(String, String), EdgeFact> =
            HashMap::new();
        for (i, t) in ["x#1", "y#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            child_out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            child_edges.insert(
                ("v#0".into(), bk),
                EdgeFact::new(Auth::Write, Cert::Def, None),
            );
        }
        child_out.next_bot = 2;
        // merge_task_out takes the child's out + spawn-time base +
        // captured ext set; `${bot}` keys are $-rooted → external.
        merge_task_out(&mut ctx, &child_out, &HashMap::new(), &HashSet::new());
        // Child edges merged additively by the caller — simulate
        // the physical edge union here (task merge does no
        // coalescing, so both ⊥s survive verbatim).
        ctx.edges.extend(child_edges);
        let bots: Vec<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(bots.len(), 2);
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "y#1"));
    }

    /// CAP-map conservation: ⊥ facts living in `ctx.caps` coalesce
    /// back INTO `caps` — never migrate to `edges` or vanish.
    /// (The pre-fix implementation removed cap entries but only
    /// re-emitted edges, losing the capability fact entirely.)
    #[test]
    fn coalesce_conserves_cap_facts() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, t) in ["x#1", "y#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            // CAP facts — no edges on these ⊥ targets at all.
            ctx.caps.insert(("v#0".into(), bk), Cert::Maybe);
        }
        ctx.out.next_bot = 2;
        coalesce_join_bots(
            &mut ctx,
            &[arm_bots(&[("v#0", 0)]), arm_bots(&[("v#0", 1)])],
        );
        // One merged CAP — in caps, and NO edge was fabricated.
        let cap_bots: Vec<&String> = ctx
            .caps
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(cap_bots.len(), 1);
        assert_eq!(
            ctx.caps.get(&("v#0".into(), cap_bots[0].clone())),
            Some(&Cert::Maybe)
        );
        assert!(
            !ctx.edges
                .keys()
                .any(|(h, t)| h == "v#0" && is_bot_key(t)),
            "a cap-only position must not sprout an edge fact"
        );
        let cs = ctx.out.bot_cands.get(cap_bots[0]).unwrap();
        assert!(cs.contains("x#1") && cs.contains("y#1"));
    }

    /// BOTH-map position: an arm's ⊥ carrying an edge AND a cap
    /// coalesces into ONE slot whose fresh key is SHARED by both
    /// maps — the edge↔cap correlation `claim_packet` installs
    /// must survive the join, not split into two unrelated ⊥s.
    #[test]
    fn coalesce_conserves_edge_plus_cap() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, t) in ["x#1", "y#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            ctx.edges.insert(
                ("v#0".into(), bk.clone()),
                EdgeFact::new(Auth::Read, Cert::Maybe, None),
            );
            ctx.caps.insert(("v#0".into(), bk), Cert::Maybe);
        }
        ctx.out.next_bot = 2;
        coalesce_join_bots(
            &mut ctx,
            &[arm_bots(&[("v#0", 0)]), arm_bots(&[("v#0", 1)])],
        );
        let e_bots: Vec<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        let c_bots: Vec<&String> = ctx
            .caps
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(e_bots.len(), 1);
        assert_eq!(c_bots.len(), 1);
        // The correlation itself: ONE ⊥ key names the position in
        // BOTH maps — not ⊥2 for edges and ⊥3 for caps.
        assert_eq!(e_bots[0], c_bots[0]);
    }

    /// Mixed-category alternatives: arm A's position is edge-only,
    /// arm B's is cap-only — the merged slot conservatively takes
    /// BOTH categories under one shared key (a slot can represent
    /// either constituent position; the union direction is
    /// restrictive, never permissive).
    #[test]
    fn coalesce_correlates_mixed_category_positions() {
        let e = env();
        let mut ctx = mk(&e);
        let b0 = bot_key(0);
        let b1 = bot_key(1);
        ctx.out
            .bot_cands
            .insert(b0.clone(), ["x#1".to_string()].into_iter().collect());
        ctx.out
            .bot_cands
            .insert(b1.clone(), ["y#1".to_string()].into_iter().collect());
        ctx.edges.insert(
            ("v#0".into(), b0),
            EdgeFact::new(Auth::Write, Cert::Maybe, None),
        );
        ctx.caps.insert(("v#0".into(), b1), Cert::Maybe);
        ctx.out.next_bot = 2;
        coalesce_join_bots(
            &mut ctx,
            &[arm_bots(&[("v#0", 0)]), arm_bots(&[("v#0", 1)])],
        );
        let e_bots: Vec<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        let c_bots: Vec<&String> = ctx
            .caps
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(e_bots.len(), 1);
        assert_eq!(c_bots.len(), 1);
        assert_eq!(e_bots[0], c_bots[0]);
        // Candidates union across both constituents.
        let cs = ctx.out.bot_cands.get(e_bots[0]).unwrap();
        assert!(cs.contains("x#1") && cs.contains("y#1"));
    }

    /// Independent CAPS sharing one holder: arm A holds two cap
    /// ⊥s co-resident — alternatives would need different arms,
    /// so both survive; arm B's singleton is exclusive only with
    /// itself. Result: max(2,1) = 2 cap slots.
    #[test]
    fn coalesce_independent_caps_same_holder() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, t) in ["x#1", "y#1", "z#1"].iter().enumerate() {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            ctx.caps.insert(("v#0".into(), bk), Cert::Maybe);
        }
        ctx.out.next_bot = 3;
        coalesce_join_bots(
            &mut ctx,
            &[arm_bots(&[("v#0", 0), ("v#0", 1)]), arm_bots(&[("v#0", 2)])],
        );
        let cap_bots: Vec<&String> = ctx
            .caps
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(cap_bots.len(), 2);
    }

    /// Permission MEET — the joined authority must be valid on
    /// EVERY incoming path. Write on arm A + Read on arm B meets
    /// to Read: a post-join write would be unjustified on B.
    #[test]
    fn coalesce_meets_permissions() {
        let e = env();
        let mut ctx = mk(&e);
        let cases = [
            (Auth::Write, Auth::Read, Auth::Read),
            (Auth::Read, Auth::Write, Auth::Read),
            (Auth::Write, Auth::Own, Auth::Write),
            (Auth::Own, Auth::Write, Auth::Write),
            (Auth::Read, Auth::Own, Auth::Read),
            (Auth::Write, Auth::Write, Auth::Write),
        ];
        for (ka, kb, want) in cases {
            ctx.edges.clear();
            ctx.caps.clear();
            ctx.out.bot_cands.clear();
            ctx.out.next_bot = 0;
            for (i, (t, k)) in
                [("x#1", ka), ("y#1", kb)].iter().enumerate()
            {
                let bk = bot_key(i as u64);
                ctx.out
                    .bot_cands
                    .insert(bk.clone(), [t.to_string()].into_iter().collect());
                ctx.edges.insert(
                    ("v#0".into(), bk),
                    EdgeFact::new(*k, Cert::Maybe, None),
                );
            }
            ctx.out.next_bot = 2;
            coalesce_join_bots(
                &mut ctx,
                &[arm_bots(&[("v#0", 0)]), arm_bots(&[("v#0", 1)])],
            );
            let bots: Vec<&String> = ctx
                .edges
                .keys()
                .filter(|(h, t)| h == "v#0" && is_bot_key(t))
                .map(|(_, t)| t)
                .collect();
            assert_eq!(bots.len(), 1);
            assert_eq!(
                ctx.edges.get(&("v#0".into(), bots[0].clone())).unwrap().kind,
                want,
                "meet({ka:?}, {kb:?})"
            );
        }
        // Direct unit checks, including the orthogonal kinds.
        assert_eq!(meet_auth(Auth::Atomic, Auth::Write), Auth::Read);
        assert_eq!(meet_auth(Auth::Sync, Auth::Sync), Auth::Sync);
        assert_eq!(meet_auth(Auth::Atomic, Auth::Sync), Auth::Read);
    }
    /// different holders — even Maybe ⊥ edges on them must never
    /// coalesce (they are two physical capabilities, not
    /// alternatives for one).
    #[test]
    fn coalesce_preserves_independent_fields() {
        let e = env();
        let mut ctx = mk(&e);
        for (i, (h, t)) in [("v#0.a", "x#1"), ("v#0.b", "y#1")]
            .iter()
            .enumerate()
        {
            let bk = bot_key(i as u64);
            ctx.out
                .bot_cands
                .insert(bk.clone(), [t.to_string()].into_iter().collect());
            ctx.edges.insert(
                (h.to_string(), bk),
                EdgeFact::new(Auth::Write, Cert::Maybe, None),
            );
        }
        ctx.out.next_bot = 2;
        // Nested fields are different positions — even presented as
        // arm alternatives there is no same-position relationship
        // to coalesce under.
        let arm_a = arm_bots(&[("v#0.a", 0)]);
        let arm_b = arm_bots(&[("v#0.b", 1)]);
        coalesce_join_bots(&mut ctx, &[arm_a, arm_b]);
        // Two distinct holders — nothing to merge.
        assert_eq!(
            ctx.edges
                .keys()
                .filter(|(h, t)| h.starts_with("v#0") && is_bot_key(t))
                .count(),
            2
        );
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "y#1"));
    }

    /// Multi-capability packet: a struct of two `&mut`s claims two
    /// INDEPENDENT unknown identities — one ⊥ per position, each
    /// may-aliasing the full candidate set. Sharing one ⊥ across
    /// positions would alias-collapse v.a and v.b.
    #[test]
    fn multi_cap_packet_distinct_positions() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        let mut pkt = pkt_mut("x#1");
        pkt.edges.push((
            ".b".into(),
            "y#1".into(),
            EdgeFact::new(Auth::Write, Cert::Def, None),
        ));
        pkt.sent.push("y#1".into());
        send_push(&mut ctx, ep, &pkt);
        // A second, differently-named send forces the candidate set
        // wider than a singleton.
        send_push(&mut ctx, ep, &pkt_mut("z#1"));
        bind(&mut scopes, "z", 1);
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());
        transit_set(&mut ctx, "z#1".into());
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());

        let p = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p, &e0, &mut scopes, &mut ctx);
        // v#0 and v#0.b hold DISTINCT ${bot} identities.
        let bots: BTreeSet<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h.starts_with("v#0") && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(bots.len(), 2);
        // Both may-alias the full outstanding union {x,y,z}.
        for bk in &bots {
            let cs = ctx.out.bot_cands.get(*bk).unwrap();
            assert!(cs.contains("x#1") && cs.contains("y#1") && cs.contains("z#1"));
        }
        // And both still block named access.
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "z#1"));
    }

    /// Edge/cap correlation at claim: a packet whose edge AND cap
    /// target the same name `x` carries ONE physical position —
    /// the mint must share a single ⊥ key across both maps, not
    /// split the position into unrelated identities.
    #[test]
    fn claim_shares_bot_across_edge_and_cap() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        // Packet: Write-edge to x AND a cap fact over x — one
        // position, two fact categories.
        let mut pkt = pkt_mut("x#1");
        pkt.caps.push((String::new(), "x#1".into(), Cert::Def));
        send_push(&mut ctx, ep, &pkt);
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());

        let p = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p, &e0, &mut scopes, &mut ctx);
        // Exactly ONE ⊥ minted — edges and caps share the key.
        let eb: Vec<&String> = ctx
            .edges
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        let cb: Vec<&String> = ctx
            .caps
            .keys()
            .filter(|(h, t)| h == "v#0" && is_bot_key(t))
            .map(|(_, t)| t)
            .collect();
        assert_eq!(eb.len(), 1);
        assert_eq!(cb.len(), 1);
        assert_eq!(eb[0], cb[0]);
        assert_eq!(ctx.out.bot_cands.len(), 1);
    }

    /// Nested-struct claim: a two-level path gets a ⊥ at the leaf
    /// position only — sibling/interior structure keeps real
    /// labels, and the leaf ⊥ may-aliases every candidate.
    #[test]
    fn nested_struct_leaf_position() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        let mut pkt = pkt_mut("x#1");
        pkt.edges[0].0 = ".inner.p".into(); // nested rel path
        send_push(&mut ctx, ep, &pkt);
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());

        let p = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p, &e0, &mut scopes, &mut ctx);
        assert!(ctx
            .edges
            .keys()
            .any(|(h, t)| h == "v#0.inner.p" && is_bot_key(t)));
        // Named candidates still alias-blocked through the leaf.
        assert!(conflicts(&ctx, "x#1") && conflicts(&ctx, "y#1"));
    }

    /// Region-escape input: a scoped-region source stays in the
    /// candidate set after claim — the future send-escape gate for
    /// ⊥ re-sends must quantify over exactly these names (no
    /// candidate may be dropped before drain proof).
    #[test]
    fn scoped_region_names_stay_candidates() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());
        let p = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p, &e0, &mut scopes, &mut ctx);
        // Forwarding the ⊥ packet preserves its names: packet_names
        // must see x,y through the bot's candidate set.
        let bk = bot_key(0);
        let names = packet_names(&Packet {
            ty: Ty::Ref { mutable: true, elem: Box::new(Ty::I32) },
            edges: vec![(
                String::new(),
                bk.clone(),
                EdgeFact::new(Auth::Write, Cert::Def, None),
            )],
            caps: vec![],
            sent: vec![],
            auth: Some(Auth::Write),
            is_handle: false,
        }, &ctx.out.bot_cands);
        assert!(names.contains("x#1") && names.contains("y#1"));
        // MayOutstanding still sees both — no release while v lives.
        assert!(may_outstanding(&ctx, "x#1") && may_outstanding(&ctx, "y#1"));
    }

    /// Transit release is MayOutstanding-gated end-to-end: marks
    /// survive an unordered claim (pop proves nothing), lift only
    /// once every residence is provably empty — queue drained AND
    /// the live ⊥ claim dropped.
    #[test]
    fn transit_releases_only_when_all_residences_empty() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes = vec![HashMap::new()];
        bind(&mut scopes, "x", 1);
        bind(&mut scopes, "y", 1);
        let e0 = var_e();
        let ep = "ch#0";

        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        transit_set(&mut ctx, "x#1".into());
        transit_set(&mut ctx, "y#1".into());
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());
        let p = recv_pop(&mut ctx, ep).unwrap();
        chan_claim(ep, "v#0", p, &e0, &mut scopes, &mut ctx);

        // One pop of two pushes — nothing drains, nothing releases.
        assert!(transit_check(&e0, "x#1", &mut ctx));
        assert!(transit_check(&e0, "y#1", &mut ctx));
        ctx.errs.clear();

        // Second pop drains the queue, but v#0's ⊥ is LIVE —
        // the marks must still hold.
        recv_pop(&mut ctx, ep);
        assert!(chan_drained(&ctx, ep));
        assert!(transit_check(&e0, "x#1", &mut ctx));
        ctx.errs.clear();

        // v#0 dies (capability leaves every residence) — now the
        // marks release at the next query.
        ctx.edges.remove(&("v#0".into(), bot_key(0)));
        assert!(!transit_check(&e0, "x#1", &mut ctx));
        assert!(ctx.errs.is_empty());
    }

    /// Candidate unions never narrow: a second pop on an unordered
    /// endpoint still draws on EVERY pushed name — a pop can't
    /// identify which candidate physically departed.
    #[test]
    fn candidates_never_narrow_per_pop() {
        let e = env();
        let mut ctx = mk(&e);
        let ep = "ch#0";
        send_push(&mut ctx, ep, &pkt_mut("x#1"));
        send_push(&mut ctx, ep, &pkt_mut("y#1"));
        join_chan_may(&mut ctx, &HashMap::new(), &Outstanding::default());
        recv_pop(&mut ctx, ep);
        // After one pop, the union still names both — only a drain
        // proof (pops >= pushes) closes it.
        let c = chan_candidates(&ctx, ep);
        assert!(c.contains("x#1") && c.contains("y#1"));
        recv_pop(&mut ctx, ep);
        assert!(chan_drained(&ctx, ep));
    }

    /// S3b activation guard — a `Cert::Maybe` cap fact minted by a
    /// mixed-category branch join (edge on one arm, cap on the
    /// other, one shared ⊥ key) must never satisfy a definite
    /// capability requirement. `packet_for` carries the cert into
    /// the packet verbatim; promoting it to `Def` would grant the
    /// receiver possession neither individual path held. Before the
    /// fix, `caps_of` dropped the cert and `pkt.caps` was stamped
    /// `Def` unconditionally.
    #[test]
    fn mixed_category_maybe_cap_cannot_claim_definite() {
        let e = env();
        let mut ctx = mk(&e);
        let mut scopes: Scopes = vec![HashMap::new()];
        scopes[0].insert(
            "h".to_string(),
            Bind {
                ty: Ty::Handle(Box::new(Ty::I32)),
                decl: Span { line: 0, col: 0 },
                prov: Prov::Unknown,
                owner: 0,
                epoch: 0,
                auth: None,
                guard: None,
            },
        );
        // Arm A holds an edge on x, arm B holds a cap on x — the join
        // emits ONE shared ⊥ carrying both categories at Maybe.
        let b0 = bot_key(0);
        let b1 = bot_key(1);
        ctx.out
            .bot_cands
            .insert(b0.clone(), ["x#1".to_string()].into_iter().collect());
        ctx.out
            .bot_cands
            .insert(b1.clone(), ["x#1".to_string()].into_iter().collect());
        ctx.edges.insert(
            ("h#0".into(), b0),
            EdgeFact::new(Auth::Read, Cert::Maybe, None),
        );
        ctx.caps.insert(("h#0".into(), b1), Cert::Maybe);
        ctx.out.next_bot = 2;
        coalesce_join_bots(
            &mut ctx,
            &[arm_bots(&[("h#0", 0)]), arm_bots(&[("h#0", 1)])],
        );
        // One shared key, both categories, both Maybe.
        let shared: Vec<String> = ctx
            .caps
            .keys()
            .filter(|(h, t)| h == "h#0" && is_bot_key(t))
            .map(|(_, t)| t.clone())
            .collect();
        assert_eq!(shared.len(), 1);
        let bk = shared[0].clone();
        assert_eq!(ctx.caps.get(&("h#0".into(), bk.clone())), Some(&Cert::Maybe));
        assert_eq!(
            ctx.edges.get(&("h#0".into(), bk.clone())).map(|f| f.cert),
            Some(Cert::Maybe)
        );

        // The definite-requirement probe: a packet cut for
        // `send(ch, h)` claims cap facts — the Maybe fact must arrive
        // Maybe, never Def.
        let sp = Span { line: 0, col: 0 };
        let pkt = packet_for(
            &Expr::Var("h".to_string(), sp),
            sp,
            &mut scopes,
            &mut ctx,
        );
        let cap = pkt
            .caps
            .iter()
            .find(|(_, t, _)| *t == bk)
            .expect("the packet must carry the shared-key cap fact");
        assert_eq!(
            cap.2,
            Cert::Maybe,
            "a Maybe-held capability must not satisfy a definite claim"
        );
    }
}
