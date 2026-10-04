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
use crate::{Diag, Span};
use std::collections::{HashMap, HashSet};

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

/// `(holder, target) -> (kind, certainty)`.
type Edges = HashMap<(String, String), (EdgeKind, Cert)>;

/// N9 — a spawned task's suspended transform, in PARENT-context keys
/// (the child ran on a clone of the parent's key space — no $arg
/// translation needed). Computed at `spawn`, applied at `join`.
#[derive(Debug, Clone)]
struct TaskEffects {
    creates: Vec<(String, String, Auth, Cert)>,
    kills: HashMap<String, Cert>,
    creates_caps: Vec<(String, String, Cert)>,
    cap_kills: HashMap<String, Cert>,
    consumed: HashMap<String, ConsState>,
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
struct SpawnResult {
    /// Where the `spawn` sits — escape/leak diagnostics point at it.
    span: Span,
    /// External places the child touched, strongest auth recorded.
    /// Becomes real `t -> place` edges while the task is outstanding.
    holds: HashMap<String, Auth>,
    /// The suspended transform, applied at `join`.
    eff: TaskEffects,
}

/// Per-task-body checker state: the accumulated result type, the
/// region watermark (ids >= base_regs are task-internal), and the
/// binding keys visible at spawn (what may legitimately be named
/// by the task's effects and returned refs).
struct TaskScope {
    ret: Ty,
    /// Binding keys present at spawn — the only places a returned
    /// reference may legitimately name.
    ext: HashSet<String>,
}

/// A suspended authority packet (N12) — the graph delta `send` cut out
/// of the sender's graph. `edges`/`caps` hold (rel-suffix, target, cert)
/// tuples re-rooted at the receiver's binding key; `sent` names targets
/// whose exclusive authority is channel-held (`transit`) until `recv`.
#[derive(Clone)]
struct Packet {
    /// The payload's full type — what `recv` hands the binding.
    ty: Ty,
    /// Alias/authority edges that travelled (suffix, target, kind, cert).
    /// For capability packets the suffix is "" — the binding IS the ref.
    edges: Vec<(String, String, Auth, Cert)>,
    /// Identity (handle) edges that travelled — never authority.
    caps: Vec<(String, String, Cert)>,
    /// Targets whose authority moved into the channel (transit set).
    sent: Vec<String>,
    /// Capability packets install as `auth`-kind bindings on recv.
    auth: Option<Auth>,
    /// Handle packets install a cap edge — identity only.
    is_handle: bool,
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
}

/// N15 — the statically-known element count of a slice place, if any:
/// `let s = &a[2..5]` records `s#e -> 3`; `slice(p, k)` records k.
/// `None` = dynamic — the runtime bounds check still applies.
fn slice_len_at(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> Option<i64> {
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
fn slice_len_of(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) -> Option<i64> {
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
        ctx.errs.push(Diag::new(
            sr.span,
            "a `spawn` must be bound directly: `let t = spawn { .. }`".to_string(),
        ));
        return;
    }
    // Rebinding a task place abandons the old task: its hold edges die
    // with the binding (the suspended transform never lands).
    ctx.edges.retain(|(h, _), _| !key_under(h, key));
    for (k, a) in &sr.holds {
        ctx.edges
            .insert((key.to_string(), k.clone()), (*a, Cert::Def));
        // Propagate the hold upward: inside a task body this feeds the
        // outer spawn's hold set (nested spawn shares the window).
        record_hold(ctx, k, *a);
    }
    ctx.tasks.insert(key.to_string(), sr);
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
    /// N11 — `$argN` domains the callee claimed via `lock` — a caller
    /// inside a task replays them as Sync holds so two tasks calling
    /// `update(m)` share the serialization domain.
    syncs: Vec<String>,
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
fn place_id(e: &Expr, scopes: &Scopes) -> Option<String> {
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
fn guard_base(e: &Expr, scopes: &Scopes) -> Option<String> {
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
fn path_holders(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> HashSet<String> {
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
fn resolve_place(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> Vec<String> {
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
fn derivation_source(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> Option<String> {
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
fn cap_hold(t: &str, required: Auth, scopes: &Scopes, ctx: &Ctx) -> Auth {
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
fn cap_check(e: &Expr, required: Auth, scopes: &Scopes, ctx: &mut Ctx) -> Vec<String> {
    let ts = resolve_place(e, scopes, ctx);
    let holders = path_holders(e, scopes, ctx);
    for t in &ts {
        record_hold(ctx, t, cap_hold(t, required, scopes, ctx));
    }
    if ts.is_empty() && !holders.is_empty() {
        // Every authority on the path was consumed — reborrow-move or
        // own-transfer already claimed it.
        ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    e.span(),
                    format!(
                        "stale handle `{}`: its target no longer exists",
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
        for (h, kind, _) in edges_to(ctx, t) {
            if holders.contains(&h) {
                // Deriving through our own authority — the parent edge.
                let ok = match (required, kind) {
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
                    ctx.errs.push(Diag::new(e.span(), msg));
                }
            } else {
                // An edge whose holder lives INSIDE the consumed target
                // dies with the root — `p.next = &*p` is internal, not
                // an outstanding borrow that could dangle.
                if required == Auth::Own && key_under(&h, t) {
                    continue;
                }
                let conflict = match cap_hold(t, required, scopes, ctx) {
                    Auth::Read => kind != Auth::Read,
                    // Two tasks' claims on the same mutex domain
                    // serialize — only same-target Sync edges reach
                    // this comparison, so they share the domain.
                    Auth::Sync => kind != Auth::Sync,
                    _ => true,
                };
                if conflict {
                    let msg = if ctx.tasks.contains_key(&h) {
                        let tn = h.split('#').next().unwrap_or(&h);
                        format!(
                            "cannot touch `{}`: it is held by unjoined task `{tn}`",
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
                    ctx.errs.push(Diag::new(e.span(), msg));
                }
            }
        }
    }
    ts
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
    scopes: &Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    if params.len() != args.len() {
        ctx.errs.push(Diag::new(
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
            let (got, _) = expr_tv(a, scopes, ctx);
            if let Ty::Guard(_) = got {
                ctx.errs.push(Diag::new(
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
                    ctx.errs.push(Diag::new(
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
                    ctx.errs.push(Diag::new(
                        a.span(),
                        format!("`{n}` wants a writable `*T` — a string literal is `*const`"),
                    ));
                }
                if **elem != Ty::Void && **elem != Ty::U8 {
                    ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
                            a.span(),
                            format!("`{n}` wants a writable `*T`, found `{got}`"),
                        ));
                    }
                    if **elem != Ty::Void && **ge != Ty::Void && **ge != **elem {
                        ctx.errs.push(Diag::new(
                            a.span(),
                            format!("type mismatch: expected {want}, found {got}"),
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
                    for (h, _, _) in edges_to(ctx, t) {
                        if holders.contains(&h) {
                            continue;
                        }
                        ctx.errs.push(Diag::new(
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
                let (got, _) = expr_tv(a, scopes, ctx);
                match &got {
                    Ty::Ptr {
                        mutable: gm,
                        elem: ge,
                    } => {
                        if *mutable && !gm {
                            ctx.errs.push(Diag::new(
                                a.span(),
                                format!("`{n}` wants a writable `*T`, found `{got}`"),
                            ));
                        }
                        if **elem != Ty::Void && **ge != Ty::Void && **ge != **elem {
                            ctx.errs.push(Diag::new(
                                a.span(),
                                format!("type mismatch: expected {want}, found {got}"),
                            ));
                        }
                    }
                    Ty::Ref { mutable: rm, elem: re } => {
                        if *mutable && !rm {
                            ctx.errs.push(Diag::new(
                                a.span(),
                                format!("`{n}` wants a writable `*T`, found `{got}`"),
                            ));
                        }
                        if **elem != Ty::Void && **re != Ty::Void && **re != **elem {
                            ctx.errs.push(Diag::new(
                                a.span(),
                                format!("type mismatch: expected {want}, found {got}"),
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
                ctx.edges.insert((h.clone(), t), (req, Cert::Def));
            }
            arg_holders.push(h);
        }
    }
    for h in arg_holders {
        ctx.edges.retain(|(eh, _), _| eh != &h);
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
fn consume_place(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) {
    // N18 — consuming is a read+move: there must be a value to take.
    // `let x: box T; free(x)` dies here, as does `f(own x)` on uninit.
    require_init(e, scopes, ctx);
    for h in path_holders(e, scopes, ctx) {
        let via_cap = ctx.caps.keys().any(|(ch, _)| ch == &h);
        let via_edge = ctx.edges.keys().any(|(eh, _)| eh == &h);
        if via_cap && !via_edge {
            ctx.errs.push(Diag::new(
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
        match consumed_overlap(ctx, &t) {
            Some(ConsState::Cons) => ctx.errs.push(Diag::new(
                e.span(),
                format!("double consume `{}`", target_name(e)),
            )),
            Some(ConsState::Maybe) => ctx.errs.push(Diag::new(
                e.span(),
                format!(
                    "consume of maybe-consumed `{}` (it may be dead on another path)",
                    target_name(e)
                ),
            )),
            None => {}
        }
        ctx.consumed.insert(t.clone(), ConsState::Cons);
        ctx.edge_kills.insert(t.clone(), Cert::Def);
        ctx.edges.retain(|(eh, _), _| {
            eh != &t
                && !eh.starts_with(&format!("{t}."))
                && !eh.starts_with(&format!("{t}["))
        });
    }
}

/// An owning value moving to a new binding/argument/return consumes
/// its source place: `let q = p` transfers p's heap root to q and
/// marks p spent — the same temporal transition `free` performs,
/// minus the byte release. Only `box`/`box slice` count: `alloc`
/// tokens are shareable, and everything else moves by value or edge.
fn consume_owned_src(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) {
    match e {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {
            if matches!(
                expr_ty_at(e, scopes, ctx).0,
                Ty::Box(_) | Ty::BoxSlice(_)
            ) {
                consume_place(e, scopes, ctx);
            }
        }
        _ => {}
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
fn join_consumed(a: &HashMap<String, ConsState>, b: &HashMap<String, ConsState>) -> HashMap<String, ConsState> {
    let mut out = HashMap::new();
    for k in a.keys().chain(b.keys()) {
        let st = match (a.get(k), b.get(k)) {
            (Some(ConsState::Cons), Some(ConsState::Cons)) => ConsState::Cons,
            _ => ConsState::Maybe,
        };
        out.insert(k.clone(), st);
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
fn require_init(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) {
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
            ctx.errs.push(Diag::new(e.span(), msg));
        }
    }
}

/// Bases whose VALUE computes a path's address: `*p` reads p's
/// pointer, `b[i]`/`b[lo..hi]` on a slice reads its (ptr, len). Struct
/// fields and array elements are direct storage — the slot exists
/// whether or not a value ever landed in it. Applies to write targets
/// and `&`/`&mut` places alike — neither may ride a wild pointer.
fn require_place_bases(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) {
    match e {
        Expr::Deref(inner, _) => {
            require_init(inner, scopes, ctx);
            require_place_bases(inner, scopes, ctx);
        }
        Expr::Index(b, ..) | Expr::IndexRange { base: b, .. } => {
            if matches!(
                expr_ty_at(b, scopes, ctx).0,
                Ty::Slice { .. } | Ty::BoxSlice(_)
            ) {
                require_init(b, scopes, ctx);
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
    for (c, st) in ctx.consumed.iter() {
        let covers = key == c
            || key.starts_with(&format!("{c}."))
            || key.starts_with(&format!("{c}["));
        if covers {
            worst = match worst {
                Some(ConsState::Cons) => Some(ConsState::Cons),
                _ => Some(*st),
            };
        }
    }
    worst
}

/// Does `key` overlap a consumed place in EITHER direction? Used by
/// transfer ops — moving `p` when `p.f` is dead revives nothing.
fn consumed_overlap(ctx: &Ctx, key: &str) -> Option<ConsState> {
    let mut worst = consumed_state(ctx, key);
    for (c, st) in ctx.consumed.iter() {
        let covered = c == key
            || c.starts_with(&format!("{key}."))
            || c.starts_with(&format!("{key}["));
        if covered {
            worst = match worst {
                Some(ConsState::Cons) => Some(ConsState::Cons),
                _ => Some(*st),
            };
        }
    }
    worst
}

/// Diagnose a read of a maybe/dead place — Var and path arms share it.
fn use_check(key: &str, e: &Expr, ctx: &mut Ctx) {
    if transit_check(e, key, ctx) {
        return;
    }
    match consumed_state(ctx, key) {
        Some(ConsState::Cons) => ctx.errs.push(Diag::new(
            e.span(),
            format!("use after consume `{}`", target_name(e)),
        )),
        Some(ConsState::Maybe) => ctx.errs.push(Diag::new(
            e.span(),
            format!(
                "use of maybe-consumed `{}` (it may have been consumed on another path)",
                target_name(e)
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
    consumed: HashMap<String, ConsState>,
    /// N8 bookkeeping: holder-prefix edge kills (`h.ptr = v` kills
    /// edges under `h.ptr`), joined must/may like the edge map. Only
    /// `$arg`-rooted entries survive into a FnSummary.
    edge_kills: HashMap<String, Cert>,
    cap_kills: HashMap<String, Cert>,
    /// Returned alias/identity targets, callee-side `$arg`-keyed.
    ret_edges: Vec<(String, bool)>,
    ret_caps: Vec<String>,
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
    errs: Vec<Diag>,
}

/// Kill edges held under `key`: a must-kill removes them, a may-kill
/// only weakens certainty — the edge is still a possible alias.
fn apply_kill(edges: &mut Edges, key: &str, cert: Cert) {
    match cert {
        Cert::Def => edges.retain(|(h, _), _| {
            h != key
                && !h.starts_with(&format!("{key}."))
                && !h.starts_with(&format!("{key}["))
        }),
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
            for k in under {
                if let Some(v) = edges.get_mut(&k) {
                    v.1 = Cert::Maybe;
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
    scopes: &Scopes,
    ctx: &mut Ctx,
) {
    for (hk, cert) in &sum.kills {
        for ck in translate_key(hk, bases) {
            ctx.edge_kills.insert(ck.clone(), *cert);
            apply_kill(&mut ctx.edges, &ck, *cert);
        }
    }
    for (hk, cert) in &sum.cap_kills {
        for ck in translate_key(hk, bases) {
            ctx.cap_kills.insert(ck.clone(), *cert);
            let kill = ck.clone();
            match cert {
                Cert::Def => ctx.caps.retain(|(h, _), _| {
                    h != &kill
                        && !h.starts_with(&format!("{kill}."))
                        && !h.starts_with(&format!("{kill}["))
                }),
                Cert::Maybe => {
                    let under: Vec<(String, String)> = ctx
                        .caps
                        .keys()
                        .filter(|(h, _)| {
                            h == &kill
                                || h.starts_with(&format!("{kill}."))
                                || h.starts_with(&format!("{kill}["))
                        })
                        .cloned()
                        .collect();
                    for k in under {
                        if let Some(c) = ctx.caps.get_mut(&k) {
                            *c = Cert::Maybe;
                        }
                    }
                }
            }
        }
    }
    for (h, t, kind, cert) in &sum.creates {
        for ch in translate_key(h, bases) {
            for ct in translate_key(t, bases) {
                ctx.edges.insert((ch.clone(), ct.clone()), (*kind, *cert));
                // The stored ref's prov at the holder path = its
                // target's owner region — what a direct store does.
                let root = path_root(&ct);
                let name = root.split('#').next().unwrap_or(root);
                if let Some(b) = lookup(scopes, name) {
                    if format!("{name}#{}", b.epoch) == root {
                        ctx.path_provs.insert(ch.clone(), Prov::Region(b.owner));
                    }
                }
            }
        }
    }
    for (h, t, cert) in &sum.creates_caps {
        for ch in translate_key(h, bases) {
            for ct in translate_key(t, bases) {
                ctx.caps.insert((ch.clone(), ct.clone()), *cert);
            }
        }
    }
    // Synchronized domain claims replay as holds: inside a task they
    // become `t -> caller-domain` Sync edges — task-window compat.
    for dk in &sum.syncs {
        for cd in translate_key(dk, bases) {
            record_hold(ctx, &cd, Auth::Sync);
        }
    }
    // N18 — definite callee writes replay as definite caller-side
    // initialization: `link(write h)` that stores `h.h` inside marks
    // `h#e.h` Init here. `$arg`-rooted targets re-export — the fact
    // flows through nested write-param chains unchanged.
    for k in &sum.init_paths {
        for ck in translate_key(k, bases) {
            ctx.init.insert(ck.clone(), Cert::Def);
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
        if structs.insert(s.name.clone(), &s.fields).is_some() {
            errs.push(Diag::new(s.span, format!("duplicate struct `{}`", s.name)));
        }
        let mut seen = HashSet::new();
        for (fn_, _) in &s.fields {
            if !seen.insert(fn_) {
                errs.push(Diag::new(
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
    let mut enums: Enums = HashMap::new();
    for (n, vs) in &enum_storage {
        if enums.insert(n.clone(), vs).is_some() {
            errs.push(Diag::new(
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
                errs.push(Diag::new(
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
                    errs.push(Diag::new(
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
                errs.push(Diag::new(
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
                    errs.push(Diag::new(
                        f.span,
                        format!(
                            "extern `{n}` parameter `{pn}` takes no capability — \
                             use `*T` (writable) or `*const T`",
                            n = f.name
                        ),
                    ));
                }
                if !is_ffi_ty(t) {
                    errs.push(Diag::new(
                        f.span,
                        format!(
                            "extern `{n}` parameter `{pn}` has non-FFI type `{t}`",
                            n = f.name
                        ),
                    ));
                }
            }
            if f.ret != Ty::Void && !is_ffi_ty(&f.ret) {
                errs.push(Diag::new(
                    f.span,
                    format!("extern `{}` returns non-FFI type `{}`", f.name, f.ret),
                ));
            }
        }
        if fns
            .insert(
                f.name.clone(),
                (
                    f.params.iter().map(|(_, a, t)| (*a, t.clone())).collect(),
                    f.ret.clone(),
                    f.is_extern,
                ),
            )
            .is_some()
        {
            errs.push(Diag::new(f.span, format!("duplicate function `{}`", f.name)));
        }
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
                errs.push(Diag::new(
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
                errs.push(Diag::new(
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
        if s.is_pub {
            out.structs.insert(s.name.clone(), s.fields.clone());
        } else {
            out.private.insert(s.name.clone());
        }
    }
    for e in &p.enums {
        if !e.tparams.is_empty() {
            continue;
        }
        if e.is_pub {
            out.enums.insert(e.name.clone(), e.variants.clone());
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
            continue;
        }
        if f.is_pub {
            out.fns.insert(f.name.clone(), fns.get(&f.name).unwrap().clone());
            if let Some(sum) = sums.get(&f.name) {
                out.sums.insert(f.name.clone(), sum.clone());
            }
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
                | Stmt::Promote { target: e, .. } => expr(e, out),
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
fn is_ffi_ty(t: &Ty) -> bool {
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
            errs.push(Diag::new(span, format!("unknown struct `{n}`")));
        }
        Ty::Array(e, _) => check_ty(e, structs, span, errs),
        Ty::Ref { elem, .. } => check_ty(elem, structs, span, errs),
        Ty::Handle(e) | Ty::Task(e) | Ty::Atomic(e) | Ty::Mutex(e) | Ty::Channel(e) => {
            check_ty(e, structs, span, errs)
        }
        Ty::Ptr { elem, .. } => check_ty(elem, structs, span, errs),
        Ty::Slice { elem, .. } => check_ty(elem, structs, span, errs),
        // `box T`/`box slice T` — the indirection is what makes
        // recursive structures legal: `struct Node { next: box Node }`
        // embeds a pointer, not a value.
        Ty::Box(e) | Ty::BoxSlice(e) => check_ty(e, structs, span, errs),
        // Guards are scope-bound authority — lock-produced, never a
        // declared field/param/return type.
        Ty::Guard(_) => errs.push(Diag::new(
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
    // A fresh `channel()` is unbound — `channel void` takes whatever
    // payload the annotation declares.
    if let (Ty::Channel(w), Ty::Channel(g)) = (want, got) {
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
    errs.push(Diag::new(
        e.span(),
        format!("type mismatch: expected {want}, found {got}"),
    ));
    want.clone()
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
fn place_req(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) -> Prov {
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
    scopes: &Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    match e {
        Expr::Field(b, name, s) => {
            let t = field_ty(&base.0, name, ctx.structs, *s, &mut ctx.errs);
            // N22 — a variant payload is a place only while the tag
            // statically holds it; `match` is how you learn it.
            if let Ty::Struct(en) = &base.0 {
                if let Some(vars) = ctx.enums.get(en).cloned() {
                    if let Some((_, pt)) = vars.iter().find(|(vn, _)| vn == name) {
                        if pt.is_none() {
                            ctx.errs.push(Diag::new(
                                *s,
                                format!("variant `{name}` of `{en}` carries no payload"),
                            ));
                        } else {
                            for k in resolve_place(b, scopes, ctx) {
                                match ctx.tag.get(&k) {
                                    Some(Some(v)) if v == name => {}
                                    _ => ctx.errs.push(Diag::new(
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
                    if matches!(t, Ty::Ref { .. })
                        && prov_of(&ctx.path_provs, &k) != Prov::Unknown
                        && !live_out_edge(ctx, &k)
                    {
                        ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    idx.span(),
                    format!("array index must be an integer, found {it}"),
                ));
            }
            match &base.0 {
                Ty::Array(elem, n) => {
                    if let Expr::Int(k, s) = &**idx {
                        if *k < 0 || *k >= *n as i64 {
                            ctx.errs.push(Diag::new(
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
                            ctx.errs.push(Diag::new(
                                *s,
                                format!("negative index {k} into a slice"),
                            ));
                        } else if let Some(len) = slice_len_at(base_e, scopes, ctx) {
                            if *k >= len {
                                ctx.errs.push(Diag::new(
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
                        for (h, kind, _) in edges_to(ctx, &t) {
                            if holders.contains(&h) {
                                continue;
                            }
                            if kind != Auth::Read {
                                ctx.errs.push(Diag::new(
                                    *s,
                                    format!(
                                        "cannot read `{}`: it is mutably borrowed",
                                        target_name(e)
                                    ),
                                ));
                                break;
                            }
                        }
                    }
                    ((**elem).clone(), Prov::Param)
                }
                t => {
                    ctx.errs.push(Diag::new(*s, format!("indexing non-array {t}")));
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
            errs.push(Diag::new(
                span,
                format!("slice member `.{name}` is a computed view, not a place"),
            ));
            Ty::Void
        }
        Ty::Str => {
            errs.push(Diag::new(
                span,
                format!("str member `.{name}` is a computed view, not a place"),
            ));
            Ty::Void
        }
        Ty::BoxSlice(_) => {
            errs.push(Diag::new(
                span,
                format!("box slice member `.{name}` is computed, not a place"),
            ));
            Ty::Void
        }
        Ty::Struct(sn) => match structs.get(sn) {
            Some(fields) => match fields.iter().find(|(fn_, _)| fn_ == name) {
                Some((_, t)) => t.clone(),
                None => {
                    errs.push(Diag::new(span, format!("no field `{name}` on struct `{sn}`")));
                    Ty::Void
                }
            },
            None => Ty::Void, // unknown struct — already reported
        },
        t => {
            errs.push(Diag::new(span, format!("field access on non-struct {t}")));
            Ty::Void
        }
    }
}

/// Every expression yields (type, provenance). Provenance is meaningful
/// only for reference-typed results; elsewhere it's Unknown and unused.
fn prov_of(m: &HashMap<String, Prov>, k: &str) -> Prov {
    m.get(k).cloned().unwrap_or(Prov::Unknown)
}

fn expr_tv(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    expr_tv_go(e, scopes, ctx, true)
}

/// `init_ok` gates the N18 read-requirement: a place evaluated as a
/// PATH PREFIX (`p` in `p.f`, `s` in `s[i]`) isn't a value read — the
/// leaf carries the requirement, so prefix evals pass `false`.
fn expr_tv_go(e: &Expr, scopes: &Scopes, ctx: &mut Ctx, init_ok: bool) -> (Ty, Prov) {
    if init_ok
        && matches!(
            e,
            Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..)
        )
    {
        require_init(e, scopes, ctx);
    }
    match e {
        Expr::Int(..) => (Ty::INT_DEFAULT, Prov::Unknown),
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
        Expr::Var(n, s) => match lookup(scopes, n) {
            Some(b) => {
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
                            ctx.errs.push(Diag::new(
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
                            ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(*s, format!("undefined variable `{n}`")));
                (Ty::Void, Prov::Unknown)
            }
        },
        Expr::Unary(op, inner, s) => {
            let t = expr_tv(inner, scopes, ctx).0;
            match op {
                UnOp::Neg => {
                    if !t.is_int() {
                        ctx.errs.push(Diag::new(
                            *s,
                            format!("`-` requires an integer, found {t}"),
                        ));
                    }
                    (t, Prov::Unknown)
                }
                UnOp::Not => {
                    if t != Ty::Bool {
                        ctx.errs.push(Diag::new(*s, format!("`!` requires bool, found {t}")));
                    }
                    (Ty::Bool, Prov::Unknown)
                }
                UnOp::BitNot => {
                    if !t.is_int() {
                        ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
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
            let tb = expr_tv(b, scopes, ctx).0;
            if tb != Ty::Bool {
                ctx.errs.push(Diag::new(
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
            let mut merged_edges = saved_edges.clone();
            for (k, (kind, _)) in after_edges.iter().chain(saved_edges.iter()) {
                let cert = match (after_edges.get(k), saved_edges.get(k)) {
                    (Some((tk, Cert::Def)), Some((ek, Cert::Def))) if tk == ek => Cert::Def,
                    _ => Cert::Maybe,
                };
                merged_edges.insert(k.clone(), (*kind, cert));
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
            ctx.chan_queues = saved_chans;
            for (c, q) in after_chans.iter() {
                let e = ctx.chan_queues.entry(c.clone()).or_default();
                if q.len() > e.len() {
                    *e = q.clone();
                }
            }
            ctx.path_provs = saved_paths.clone();
            for (k, v) in after_paths.iter() {
                let j = join_prov(v, saved_paths.get(k).unwrap_or(v), &ctx.regs);
                ctx.path_provs.insert(k.clone(), j);
            }
            (Ty::Bool, Prov::Unknown)
        }
        Expr::Bin(op, a, b, s) => {
            let mut ta = expr_tv(a, scopes, ctx).0;
            let mut tb = expr_tv(b, scopes, ctx).0;
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
            if let Expr::Int(..) = &**a {
                if tb.is_int() {
                    ta = coerce(&tb, a, &ta, &mut ctx.errs);
                }
            }
            if let Expr::Int(..) = &**b {
                if ta.is_int() {
                    tb = coerce(&ta, b, &tb, &mut ctx.errs);
                }
            }
            if !ta.is_int() || !tb.is_int() {
                ctx.errs.push(Diag::new(
                    *s,
                    format!("arithmetic on non-integers: {ta} and {tb}"),
                ));
            } else if ta != tb {
                ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
                            *s,
                            format!("shift amount {k} out of range for {ta} (0..{w})"),
                        ));
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
                ctx.errs.push(Diag::new(
                    *s,
                    "slice() takes a pointer and a length".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (pt, _) = expr_tv(&args[0], scopes, ctx);
            let (lt, _) = expr_tv(&args[1], scopes, ctx);
            if !lt.is_int() {
                ctx.errs.push(Diag::new(
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
                    ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    args[0].span(),
                    format!("`cstr` takes a `str`, found {at}"),
                ));
            } else if !safe {
                ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    *s,
                    "`str_from_utf8` takes a `slice u8`".to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (at, _) = expr_tv(&args[0], scopes, ctx);
            match &at {
                Ty::Slice { elem, .. } if **elem == Ty::U8 => (Ty::Str, Prov::Unknown),
                t => {
                    ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
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
                t => ctx.errs.push(Diag::new(
                    args[0].span(),
                    format!("`free` takes a `box`/`box slice`, found {t}"),
                )),
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
                ctx.errs.push(Diag::new(
                    alloc.span(),
                    format!("`alloc` takes an `alloc` token (e.g. system_allocator()), found {at}"),
                ));
            }
            match count {
                None => (Ty::Box(ty.clone()), Prov::Region(cur_region(ctx))),
                Some(n) => {
                    let (nt, _) = expr_tv(n, scopes, ctx);
                    if !nt.is_int() {
                        ctx.errs.push(Diag::new(
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
        Expr::Call(n, _tg, args, s) if is_chan_op(n) && !ctx.local_fns.contains(n) => {
            chan_op(n, args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s)
            if int_cast_ty(n).is_some() && !ctx.local_fns.contains(n) =>
        {
            // `u64(x)`/`i32(x)` — explicit numeric conversion; the
            // gen side emits Instr::Cast (sext/zext/trunc).
            let ty = int_cast_ty(n).unwrap();
            if args.len() != 1 {
                ctx.errs.push(Diag::new(
                    *s,
                    format!("`{n}` cast takes exactly one argument"),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            let (at, _) = expr_tv(&args[0], scopes, ctx);
            if at.is_int() {
                (ty, Prov::Unknown)
            } else {
                ctx.errs.push(Diag::new(
                    args[0].span(),
                    format!("`{n}` cast takes an int, found {at}"),
                ));
                (Ty::Void, Prov::Unknown)
            }
        }
        Expr::Mutex(inner, _) => {
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
            let (params, ret, _) = ctx.fns.get(n).cloned().unwrap();
            extern_call(n, &params, &ret, args, *s, scopes, ctx)
        }
        Expr::Call(n, _tg, args, s) => match ctx.fns.get(n) {
            Some((params, ret, _)) => {
                if params.len() != args.len() {
                    ctx.errs.push(Diag::new(
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
                    Vec<((String, String), (Auth, Cert))>,
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
                                _ => ctx.errs.push(Diag::new(
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
                                ctx.edge_kills.insert(src.clone(), Cert::Def);
                                ctx.cap_kills.insert(src.clone(), Cert::Def);
                                ctx.edges.retain(|(eh, _), _| {
                                    eh != &src
                                        && !eh.starts_with(&format!("{src}."))
                                        && !eh.starts_with(&format!("{src}["))
                                });
                                ctx.caps.retain(|(eh, _), _| {
                                    eh != &src
                                        && !eh.starts_with(&format!("{src}."))
                                        && !eh.starts_with(&format!("{src}["))
                                });
                                reborrows.push((src, pe, pc));
                            }
                            for t in ts {
                                ctx.edges.insert((h.clone(), t), (*req, Cert::Def));
                            }
                            arg_holders.push(h);
                        }
                        // `atomic`/`sync` are task-hold kinds, not
                        // param contracts — the parser can't produce them.
                        Some(Auth::Atomic) | Some(Auth::Sync) => ctx.errs.push(Diag::new(
                            *s,
                            "`atomic`/`sync` are not parameter capabilities".to_string(),
                        )),
                        // `own x: T` — the caller transfers the place
                        // outright. Any place transfers: `f(own p.f)`
                        // consumes just the field's authority.
                        Some(Auth::Own) => {
                            bases[i] = resolve_place(a, scopes, ctx);
                            let (got, _) = expr_tv(a, scopes, ctx);
                            if let Ty::Guard(_) = got {
                                ctx.errs.push(Diag::new(
                                    a.span(),
                                    "a lock guard cannot cross a call boundary".to_string(),
                                ));
                            }
                            coerce(want, a, &got, &mut ctx.errs);
                            consume_place(a, scopes, ctx);
                        }
                        None => {
                            let (got, prov) = expr_tv(a, scopes, ctx);
                            if let Ty::Guard(_) = got {
                                ctx.errs.push(Diag::new(
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
                                            record_edges(&h, &ri, ctx);
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
                                                ctx.edge_kills.insert(key.clone(), Cert::Def);
                                                ctx.edges.retain(|(h, _), _| h != &key);
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
                    ctx.edges.retain(|(eh, _), _| eh != &h);
                }
                // N8: apply the callee's derived effects — param-path
                // edges/kills replay through each arg's caller keys.
                // The body is never reopened.
                ctx.call_bases.insert((s.line, s.col), bases.clone());
                let sum = ctx.sums.get(n).cloned();
                if let Some(sum) = &sum {
                    replay_summary(sum, &bases, scopes, ctx);
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
                    ctx.edge_kills.remove(&src);
                    ctx.cap_kills.remove(&src);
                    for ((eh, et), k) in pe {
                        ctx.edges.insert((eh, et), k);
                    }
                    for ((eh, et), c) in pc {
                        ctx.caps.insert((eh, et), c);
                    }
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
                ctx.errs.push(Diag::new(*s, msg));
                (Ty::Void, Prov::Unknown)
            }
        },
        Expr::Field(..) | Expr::Index(..) => {
            let base_e = match e {
                Expr::Field(b, ..) | Expr::Index(b, ..) => b,
                _ => unreachable!(),
            };
            // The base is a path prefix, not a value read — `p.f`
            // requires `p#e.f`, not all of `p` (init-by-parts, N18).
            let base = expr_tv_go(base_e, scopes, ctx, false);
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
                        ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
                            *s,
                            "no `.ptr` on `str` — use `s.bytes.ptr` (len-delimited) or `cstr(s)` (NUL-terminated)"
                                .to_string(),
                        ));
                        (Ty::Void, Prov::Unknown)
                    }
                    _ => {
                        ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    e.span(),
                    "`str` is not element-indexed — byte access is explicit: `s.bytes[i]`"
                        .to_string(),
                ));
                return (Ty::Void, Prov::Unknown);
            }
            path_ty_prov(base, e, scopes, ctx)
        }
        Expr::IndexRange { span, .. } => {
            ctx.errs.push(Diag::new(
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
                            ctx.errs.push(Diag::new(
                                *span,
                                "cannot derive `slice mut` from a read-only view"
                                    .to_string(),
                            ));
                        }
                        ((**elem).clone(), None)
                    }
                    t => {
                        ctx.errs.push(Diag::new(
                            *span,
                            format!("cannot slice {t} (want an array or slice)"),
                        ));
                        return (Ty::Void, Prov::Unknown);
                    }
                };
                for bound in lo.iter().chain(hi.iter()) {
                    let it = expr_tv(bound, scopes, ctx).0;
                    if !it.is_int() {
                        ctx.errs.push(Diag::new(
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
                                ctx.errs.push(Diag::new(
                                    *bs,
                                    format!("range {a}..{b} out of bounds for length {lim}"),
                                ));
                            }
                        }
                    }
                    if let (Some(l), None) = (lo.as_deref(), hi.as_deref()) {
                        if let Expr::Int(a, s) = l {
                            if *a < 0 || *a > lim {
                                ctx.errs.push(Diag::new(
                                    *s,
                                    format!("range {a}.. out of bounds for length {lim}"),
                                ));
                            }
                        }
                    }
                    if let (None, Some(h)) = (lo.as_deref(), hi.as_deref()) {
                        if let Expr::Int(b, s) = h {
                            if *b < 0 || *b > lim {
                                ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
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
            match &**place {
                Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
                _ => {
                    ctx.errs.push(Diag::new(
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
            // Fork the world: the child sees the parent's graph but
            // runs on a clone — its effects suspend until `join`.
            let mut ext: HashSet<String> = HashSet::new();
            for scope in scopes.iter() {
                for (n, b) in scope {
                    ext.insert(format!("{n}#{}", b.epoch));
                }
            }
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
                edges: ctx.edges.clone(),
                caps: ctx.caps.clone(),
                lit_caps: Vec::new(),
                next_epoch: ctx.next_epoch,
                consumed: ctx.consumed.clone(),
                init: ctx.init.clone(),
                init_writes: HashSet::new(),
                diverged: false,
                edge_kills: HashMap::new(),
                cap_kills: HashMap::new(),
                ret_edges: Vec::new(),
                ret_caps: Vec::new(),
                call_bases: HashMap::new(),
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
                arg_chan: ctx.arg_chan.clone(),
                foreign: ctx.foreign.clone(),
                slice_lens: ctx.slice_lens.clone(),
                cstr_safe: ctx.cstr_safe.clone(),
                holds: HashMap::new(),
                tasks: HashMap::new(),
                task: Some(TaskScope {
                    ret: Ty::Void,
                    ext: ext.clone(),
                }),
                fn_ret: ctx.fn_ret.clone(),
                last_spawn: None,
                loops: Vec::new(),
                sums: ctx.sums,
                private_syms: ctx.private_syms,
                errs: Vec::new(),
            };
            let mut child_scopes = scopes.clone();
            check_block(body, &mut child_scopes, &mut child, &Ty::Void);
            ctx.next_epoch = child.next_epoch;
            ctx.errs.extend(child.errs);

            // Channel effects land at spawn — the endpoint is shared
            // state, not a per-task capability. External endpoints take
            // the child's final queue; external transit takes the
            // child's set (both supersede the parent's view).
            for (c, q) in child.chan_queues.iter() {
                if is_external(c, &ext) {
                    ctx.chan_queues.insert(c.clone(), q.clone());
                }
            }
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
                    .filter(|((h, t), (k, c))| {
                        is_external(h, &ext)
                            && is_external(t, &ext)
                            && ctx.edges.get(&(h.clone(), t.clone()))
                                != Some(&(*k, *c))
                    })
                    .map(|((h, t), (k, c))| (h.clone(), t.clone(), *k, *c))
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
                        is_external(k, &ext) && ctx.consumed.get(*k) != Some(v)
                    })
                    .map(|(k, v)| (k.clone(), *v))
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
                ret: child.task.map(|t| t.ret).unwrap_or(Ty::Void),
            };
            let ret = eff.ret.clone();
            ctx.last_spawn = Some(SpawnResult {
                span: *_s,
                holds,
                eff,
            });
            (Ty::Task(Box::new(ret)), Prov::Unknown)
        }
        Expr::Join(inner, s) => {
            let (ty, _) = expr_tv(inner, scopes, ctx);
            let Ty::Task(ret) = ty.clone() else {
                ctx.errs.push(Diag::new(
                    *s,
                    format!("`join` requires a task, found {ty}"),
                ));
                return (Ty::Void, Prov::Unknown);
            };
            let key = place_id(inner, scopes);
            match key.and_then(|k| ctx.tasks.get(&k).map(|sr| (k, sr.eff.clone()))) {
                Some((key, eff)) => {
                    if consumed_state(ctx, &key) == Some(ConsState::Cons) {
                        // `join t` twice: the Var read above already
                        // reported use-after-consume; don't re-apply.
                        return (*ret, Prov::Unknown);
                    }
                    // The suspended transform lands: kills then creates
                    // then caps — same order as summary replay.
                    for (k, cert) in &eff.kills {
                        ctx.edge_kills.insert(k.clone(), *cert);
                        let kk = k.clone();
                        apply_kill(&mut ctx.edges, &kk, *cert);
                    }
                    for (k, cert) in &eff.cap_kills {
                        ctx.cap_kills.insert(k.clone(), *cert);
                        let kk = k.clone();
                        match cert {
                            Cert::Def => ctx.caps.retain(|(h, _), _| !key_under(h, &kk)),
                            Cert::Maybe => {
                                let under: Vec<(String, String)> = ctx
                                    .caps
                                    .keys()
                                    .filter(|(h, _)| key_under(h, &kk))
                                    .cloned()
                                    .collect();
                                for kk2 in under {
                                    if let Some(c) = ctx.caps.get_mut(&kk2) {
                                        *c = Cert::Maybe;
                                    }
                                }
                            }
                        }
                    }
                    for (h, t, kind, cert) in &eff.creates {
                        ctx.edges
                            .insert((h.clone(), t.clone()), (*kind, *cert));
                        let root = path_root(t);
                        let name = root.split('#').next().unwrap_or(root);
                        if let Some(b) = lookup(scopes, name) {
                            if format!("{name}#{}", b.epoch) == root {
                                ctx.path_provs
                                    .insert(h.clone(), Prov::Region(b.owner));
                            }
                        }
                    }
                    for (h, t, cert) in &eff.creates_caps {
                        ctx.caps.insert((h.clone(), t.clone()), *cert);
                    }
                    for (k, st) in &eff.consumed {
                        ctx.consumed.insert(k.clone(), *st);
                    }
                    for (k, pv) in &eff.path_provs {
                        ctx.path_provs.insert(k.clone(), pv.clone());
                    }
                    // The window ends: hold edges die, token is spent.
                    ctx.edges.retain(|(h, _), _| !key_under(h, &key));
                    ctx.consumed.insert(key, ConsState::Cons);
                }
                None => {
                    ctx.errs.push(Diag::new(
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
            ctx.errs.push(Diag::new(
                *s,
                format!("`{n}` is an enum — construct a variant: `{n}::V(..)`"),
            ));
            (Ty::Void, Prov::Unknown)
        }
        Expr::StructLit(n, _tg, fields, s) => match ctx.structs.get(n) {
            Some(defs) => {
                let mut seen = HashSet::new();
                let mut field_provs: HashMap<String, Prov> = HashMap::new();
                let mut lit_prov = Prov::Param; // top; join pulls it down
                let mut any_ref = false;
                for (fn_, fe) in fields {
                    if !seen.insert(fn_) {
                        ctx.errs.push(Diag::new(
                            *s,
                            format!("field `{fn_}` initialized twice in `{n}`"),
                        ));
                        continue;
                    }
                    match defs.iter().find(|(dn, _)| dn == fn_) {
                        Some((_, want)) => {
                            let (got, prov) = expr_tv(fe, scopes, ctx);
                            field_provs.insert(fn_.clone(), prov.clone());
                            coerce(want, fe, &got, &mut ctx.errs);
                            if matches!(want, Ty::Ref { .. } | Ty::Slice { .. }) {
                                any_ref = true;
                                lit_prov = join_prov(&lit_prov, &prov, &ctx.regs);
                            }
                        }
                        None => ctx.errs.push(Diag::new(
                            *s,
                            format!("no field `{fn_}` on struct `{n}`"),
                        )),
                    }
                }
                for (dn, _) in defs.iter() {
                    if !seen.contains(dn) {
                        ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(*s, format!("unknown struct `{n}`")));
                (Ty::Void, Prov::Unknown)
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
            match vars {
                Some(vars) => match vars.iter().find(|(vn, _)| vn == variant) {
                    Some((_, want)) => {
                        let mut lit_prov = Prov::Param;
                        let mut any_ref = false;
                        match (want, payload) {
                            (Some(w), Some(pe)) => {
                                let (got, prov) = expr_tv(pe, scopes, ctx);
                                coerce(w, pe, &got, &mut ctx.errs);
                                // Payload moves like a struct field:
                                // `Some(p)` for `box` consumes `p`.
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
                            (Some(_), None) => ctx.errs.push(Diag::new(
                                *span,
                                format!("variant `{variant}` of `{name}` takes a payload"),
                            )),
                            (None, Some(_)) => ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
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
                    ctx.errs.push(Diag::new(*span, msg));
                    (Ty::Void, Prov::Unknown)
                }
            }
        }
        Expr::Try { inner, span } => {
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
                    ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
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
                    _ => ctx.errs.push(Diag::new(
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
            // the Ok continuation is statically dead.
            let dead_ok = !skeys.is_empty()
                && skeys.iter().all(|k| {
                    matches!(ctx.tag.get(k), Some(Some(v)) if v != "Ok")
                });
            // Err arm — its payload move is CHECKED (a `read`
            // scrutinee can't move a box payload out on either path),
            // but its state dies with the early return.
            let saved_cons = ctx.consumed.clone();
            let saved_edges = ctx.edges.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_holds = ctx.holds.clone();
            for k in &skeys {
                ctx.tag.insert(k.clone(), Some("Err".to_string()));
            }
            let fe_err = Expr::Field(
                Box::new((**inner).clone()),
                "Err".to_string(),
                *span,
            );
            consume_owned_src(&fe_err, scopes, ctx);
            ctx.consumed = saved_cons;
            ctx.edges = saved_edges;
            ctx.edge_kills = saved_kills;
            ctx.holds = saved_holds;
            // Ok arm — persists: the tag refines and the payload moves
            // out exactly like `let v = scrut.Ok` (N22 field move).
            for k in &skeys {
                ctx.tag.insert(k.clone(), Some("Ok".to_string()));
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
            }
            (if got == Ty::Void { ok_ty } else { got }, gprov)
        }
    }
}

/// (type, provenance) of a place — for `&x` and write targets. Unlike
/// expr_tv this never read-checks the addressed place (taking its
/// address or storing into it is not a value read); index expressions
/// ARE evaluated normally.
fn place_tv(e: &Expr, scopes: &Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    match e {
        Expr::Var(n, _) => match lookup(scopes, n) {
            Some(b) => (b.ty.clone(), b.prov.clone()),
            None => (Ty::Void, Prov::Unknown),
        },
        Expr::Field(b, f, s) => {
            let (bt, _) = place_tv(b, scopes, ctx);
            // N22 — variant payload access needs a statically known
            // tag (writes go through this path too).
            if let Ty::Struct(en) = &bt {
                if let Some(vars) = ctx.enums.get(en).cloned() {
                    if let Some((_, pt)) = vars.iter().find(|(vn, _)| vn == f) {
                        if pt.is_none() {
                            ctx.errs.push(Diag::new(
                                *s,
                                format!("variant `{f}` of `{en}` carries no payload"),
                            ));
                        } else {
                            for k in resolve_place(b, scopes, ctx) {
                                match ctx.tag.get(&k) {
                                    Some(Some(v)) if v == f => {}
                                    _ => ctx.errs.push(Diag::new(
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
            (field_ty(&bt, f, ctx.structs, *s, &mut ctx.errs), Prov::Unknown)
        }
        Expr::Index(b, i, s) => {
            let (bt, _) = place_tv(b, scopes, ctx);
            let it = expr_tv(i, scopes, ctx).0;
            if !it.is_int() {
                ctx.errs.push(Diag::new(
                    i.span(),
                    format!("array index must be an integer, found {it}"),
                ));
            }
            match &bt {
                Ty::Array(elem, n) => {
                    if let Expr::Int(k, ks) = &**i {
                        if *k < 0 || *k >= *n as i64 {
                            ctx.errs.push(Diag::new(
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
                            ctx.errs.push(Diag::new(
                                *ks,
                                format!("negative index {k} into a slice"),
                            ));
                        } else if let Some(len) = slice_len_at(b, scopes, ctx) {
                            if *k >= len {
                                ctx.errs.push(Diag::new(
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
                            ctx.errs.push(Diag::new(
                                *ks,
                                format!("negative index {k} into a box slice"),
                            ));
                        } else if let Some(len) = slice_len_at(b, scopes, ctx) {
                            if *k >= len {
                                ctx.errs.push(Diag::new(
                                    *ks,
                                    format!("index {k} out of bounds for box slice of length {len}"),
                                ));
                            }
                        }
                    }
                    ((**elem).clone(), Prov::Unknown)
                }
                Ty::Str => {
                    ctx.errs.push(Diag::new(
                        *s,
                        "`str` is not element-indexed — byte access is explicit: `s.bytes[i]`"
                            .to_string(),
                    ));
                    (Ty::Void, Prov::Unknown)
                }
                t => {
                    ctx.errs.push(Diag::new(*s, format!("indexing non-array {t}")));
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
fn stale_check(inner: &Expr, use_site: &Expr, scopes: &Scopes, ctx: &mut Ctx) {
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
                ctx.errs.push(Diag::new(
                    use_site.span(),
                    format!(
                        "stale handle `{}`: its target no longer exists",
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
fn refs_of(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> Option<RefInfo> {
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
fn expr_ty_at(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> (Ty, Prov) {
    // reuse the small shallow resolver: only Var/Field/Index needed
    fn go(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> Option<Ty> {
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
fn edges_to(ctx: &Ctx, key: &str) -> Vec<(String, Auth, Cert)> {
    ctx.edges
        .iter()
        .filter(|((_, t), _)| overlap(t, key))
        .map(|((h, _), (k, c))| (h.clone(), *k, *c))
        .collect()
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
fn atomic_inner(key: &str, scopes: &Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::Atomic(inner) => Some(*inner),
        _ => None,
    }
}

/// The payload type if `key` names mutex storage — same key walk as
/// `atomic_inner`, ending in `mutex T` instead.
fn mutex_inner(key: &str, scopes: &Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::Mutex(inner) => Some(*inner),
        _ => None,
    }
}

/// The type stored at a place key — `name#e[.f/[]]` through binds and
/// struct fields; `$argN` param-places reconstruct their declared
/// storage class from the arg side-maps.
fn key_ty(key: &str, scopes: &Scopes, ctx: &Ctx) -> Option<Ty> {
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
fn protecting_domain(key: &str, scopes: &Scopes, ctx: &Ctx) -> Option<String> {
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
fn root_guard(e: &Expr, scopes: &Scopes) -> Option<(String, String, String, Vec<String>)> {
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
fn dom_held(scopes: &Scopes, ctx: &Ctx, dom: &str) -> bool {
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
fn sync_scan(e: &Expr, k: &str, accessor_dom: &str, _scopes: &Scopes, ctx: &mut Ctx) {
    let edges: Vec<(String, String)> = ctx
        .edges
        .iter()
        .filter(|(_, (kind, _))| *kind == Auth::Sync)
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
        ctx.errs.push(Diag::new(e.span(), m));
        return;
    }
}

/// Unguarded access to mutex-protected storage — the interior rule:
/// `m.x`, `x.f` (via `mutex h`), `*r` into a protected subtree all
/// reject when reached without a live guard of the protecting domain.
fn unguarded_check(e: &Expr, k: &str, scopes: &Scopes, ctx: &mut Ctx) {
    if root_guard(e, scopes).is_some() {
        return; // accessor is itself a guard — its own check covers it
    }
    if let Some(d) = protecting_domain(k, scopes, ctx) {
        let dn = d.split('#').next().unwrap_or(&d);
        ctx.errs.push(Diag::new(
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
    scopes: &Scopes,
    ctx: &mut Ctx,
) {
    use_check(gk, e, ctx);
    if let Some(t) = ctx.task.as_ref() {
        if t.ext.contains(gk) {
            ctx.errs.push(Diag::new(
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
    for (h, kind, _) in edges_to(ctx, k) {
        if h == gk || srcs.contains(&h) || kind == Auth::Sync {
            continue;
        }
        let bad = match (write, kind) {
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
        ctx.errs.push(Diag::new(e.span(), m));
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
    scopes: &Scopes,
    ctx: &mut Ctx,
) -> (Ty, Prov) {
    let arity = match n {
        "atomic_load" => 1,
        "atomic_store" | "atomic_add" => 2,
        _ => 3,
    };
    if args.len() != arity {
        ctx.errs.push(Diag::new(
            s,
            format!("`{n}` takes {arity} argument(s), got {}", args.len()),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    let place = &args[0];
    match place {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
        _ => {
            ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    s,
                    format!(
                        "stale handle `{}`: its target no longer exists",
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
        ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
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
        for (h, kind, _) in edges_to(ctx, k) {
            if holders.contains(&h) || gsrcs.contains(&h) || kind == Auth::Atomic {
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
            ctx.errs.push(Diag::new(s, m));
            break;
        }
    }
    let inner = atomics[0].1.clone();
    for extra in &args[1..] {
        let (got, _) = expr_tv(extra, scopes, ctx);
        coerce(&inner, extra, &got, &mut ctx.errs);
    }
    if n == "atomic_add" && !inner.is_int() {
        ctx.errs.push(Diag::new(
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
fn mutex_place(place: &Expr, scopes: &Scopes, ctx: &mut Ctx) -> Vec<(String, String, Ty)> {
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
fn stale_targets(e: &Expr, targets: &[String], scopes: &Scopes, ctx: &mut Ctx) -> bool {
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
        ctx.errs.push(Diag::new(
            e.span(),
            format!(
                "stale handle `{}`: its target no longer exists",
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
fn lock_op(args: &[Expr], s: Span, scopes: &Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    if args.len() != 1 {
        ctx.errs.push(Diag::new(s, "`lock` takes 1 argument".to_string()));
        return (Ty::Void, Prov::Unknown);
    }
    match &args[0] {
        Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {}
        _ => {
            ctx.errs.push(Diag::new(
                args[0].span(),
                "`lock` requires a place (a mutex)".to_string(),
            ));
            return (Ty::Void, Prov::Unknown);
        }
    }
    let ms = mutex_place(&args[0], scopes, ctx);
    if ms.is_empty() {
        ctx.errs.push(Diag::new(
            s,
            format!("cannot lock `{}`: not mutex storage", target_name(&args[0])),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    if ms.len() > 1 {
        ctx.errs.push(Diag::new(
            s,
            format!("`lock` on `{}` is ambiguous: multiple targets", target_name(&args[0])),
        ));
        return (Ty::Void, Prov::Unknown);
    }
    let (dom, root, payload) = ms.into_iter().next().unwrap();
    // `lock(m)` IS a use of m — a consumed mutex can't be claimed.
    if consumed_state(ctx, &dom).is_some() {
        ctx.errs.push(Diag::new(
            s,
            format!("use after consume `{}`", target_name(&args[0])),
        ));
    }
    // A `mutex h` domain is only as live as the referent it guards.
    if root != dom {
        stale_targets(&args[0], &[root.clone()], scopes, ctx);
    }
    if dom_held(scopes, ctx, &dom) {
        let dn = dom.split('#').next().unwrap_or(&dom);
        ctx.errs.push(Diag::new(
            s,
            format!("mutex `{dn}` is already locked in this context"),
        ));
    }
    // The lock borrows THROUGH the arg's holders — their edges onto
    // the domain are the derivation itself, not foreign claims. But
    // a Read edge can't grant interior mutation rights.
    let srcs: Vec<String> = path_holders(&args[0], scopes, ctx).into_iter().collect();
    for (h, kind, _) in edges_to(ctx, &dom) {
        if srcs.contains(&h) {
            if kind == Auth::Read {
                ctx.errs.push(Diag::new(
                    s,
                    format!("cannot lock `{}` through read authority", target_name(&args[0])),
                ));
            }
            continue;
        }
        if kind == Auth::Sync {
            continue; // another task's same-domain claim serializes fine
        }
        let m = if ctx.tasks.contains_key(&h) {
            let tn = h.split('#').next().unwrap_or(&h);
            format!("cannot lock `{}`: it is held by unjoined task `{tn}`", target_name(&args[0]))
        } else {
            format!("cannot lock `{}`: it is borrowed", target_name(&args[0]))
        };
        ctx.errs.push(Diag::new(s, m));
    }
    record_hold(ctx, &dom, Auth::Sync);
    ctx.last_lock = Some((dom, root, payload.clone(), srcs));
    (Ty::Guard(Box::new(payload)), Prov::Unknown)
}

/// `unlock(g)` — affine release: the guard token is consumed (double
/// unlock is a double consume), its `g -> domain` edge dies with it.
fn unlock_op(args: &[Expr], s: Span, scopes: &Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    if args.len() != 1 {
        ctx.errs.push(Diag::new(s, "`unlock` takes 1 argument".to_string()));
        return (Ty::Void, Prov::Unknown);
    }
    match root_guard(&args[0], scopes) {
        Some((gk, _, _, _)) => {
            if let Some(t) = ctx.task.as_ref() {
                if t.ext.contains(&gk) {
                    ctx.errs.push(Diag::new(
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
            ctx.errs.push(Diag::new(
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
fn chan_inner(key: &str, scopes: &Scopes, ctx: &Ctx) -> Option<Ty> {
    match key_ty(key, scopes, ctx)? {
        Ty::Channel(inner) => Some(*inner),
        _ => None,
    }
}

/// The region a place key's owner lives in — `$arg`/`*` roots are
/// caller-side (None = treated as outliving everything local).
fn owner_of_key(k: &str, scopes: &Scopes) -> Option<u32> {
    let root = path_root(k);
    if root.starts_with('$') || root.starts_with('*') {
        return None;
    }
    let name = root.split('#').next()?;
    lookup(scopes, name).map(|b| b.owner)
}

/// Channel-held authority blocks access: `k` overlapping a transit
/// key means the capability is in flight and `recv` must claim it.
fn transit_check(e: &Expr, k: &str, ctx: &mut Ctx) -> bool {
    if ctx.transit.iter().any(|t| overlap(k, t)) {
        ctx.errs.push(Diag::new(
            e.span(),
            format!(
                "`{}`'s authority was sent through a channel — `recv` must claim it first",
                target_name(e)
            ),
        ));
        true
    } else {
        false
    }
}

/// Resolve a channel argument to endpoint keys — the place itself
/// when channel-typed, else its referent (`send(hc, ..)` through a
/// handle names ch's endpoint, not hc's).
fn chan_place(place: &Expr, scopes: &Scopes, ctx: &mut Ctx) -> Vec<String> {
    let (pt, _) = place_tv(place, scopes, ctx);
    let targets: Vec<String> = match &pt {
        Ty::Handle(_) => caps_of(place, scopes, ctx).unwrap_or_default(),
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
        .filter(|t| chan_inner(t, scopes, ctx).is_some())
        .collect()
}

/// Endpoint prelude for every channel op: the endpoint must exist,
/// be channel storage, live, and untransited; the op is a Sync claim.
/// Returns the endpoint keys plus the value payload coercion target.
fn chan_gate(
    place: &Expr,
    s: Span,
    scopes: &Scopes,
    ctx: &mut Ctx,
) -> Vec<(String, Option<Ty>)> {
    let eps = chan_place(place, scopes, ctx);
    if eps.is_empty() {
        ctx.errs.push(Diag::new(
            s,
            format!("`{}` is not a channel endpoint", target_name(place)),
        ));
        return Vec::new();
    }
    let mut out = Vec::new();
    for ep in eps {
        match consumed_state(ctx, &ep) {
            Some(ConsState::Cons) => {
                ctx.errs.push(Diag::new(
                    s,
                    format!("use after consume `{}`", target_name(place)),
                ));
                continue;
            }
            Some(ConsState::Maybe) => {
                ctx.errs.push(Diag::new(
                    s,
                    format!(
                        "use of maybe-consumed `{}` (it may have been consumed on another path)",
                        target_name(place)
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
        for (h, kind, _) in edges_to(ctx, &ep) {
            if path_holders(place, scopes, ctx).contains(&h) || kind == Auth::Sync {
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
            ctx.errs.push(Diag::new(s, m));
            break;
        }
        out.push((ep.clone(), chan_inner(&ep, scopes, ctx)));
    }
    out
}

/// Cut the authority packet `send` moves into the channel. Owned
/// values move (affine consume); `&mut` refs move exclusive authority
/// into transit; `&` refs move a read capability; handles move
/// identity only. Embedded raw refs can't outlive the boundary
/// (`send-escape`); guards never cross (`guard-escape`).
fn packet_for(v: &Expr, s: Span, scopes: &Scopes, ctx: &mut Ctx) -> Packet {
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
            for t in caps_of(v, scopes, ctx).unwrap_or_default() {
                pkt.caps.push((String::new(), t, Cert::Def));
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
                    if *mutable { Auth::Write } else { Auth::Read },
                    Cert::Def,
                ));
            }
            if *mutable {
                // Exclusive authority moves: the derivation source's
                // edge dies (reborrow-move) and the targets transit.
                if let Some(src) = ri.and_then(|r| r.move_from) {
                    ctx.edge_kills.insert(src.clone(), Cert::Def);
                    ctx.edges.retain(|(h, _), _| h != &src);
                }
                for (_, t, _, _) in pkt.edges.clone() {
                    if ctx.transit.contains(&t) {
                        ctx.errs.push(Diag::new(
                            s,
                            format!(
                                "`{}`'s authority is already channel-held",
                                target_name(place)
                            ),
                        ));
                        continue;
                    }
                    if consumed_state(ctx, &t).is_some() {
                        ctx.errs.push(Diag::new(
                            s,
                            format!("use after consume `{}`", target_name(place)),
                        ));
                        continue;
                    }
                    pkt.sent.push(t.clone());
                    ctx.transit.insert(t.clone());
                    // The authority is channel-held, not held by THIS
                    // context — a sibling `recv` must see it free of
                    // our send's momentary claim.
                    ctx.holds.remove(&t);
                }
            }
        }
        Expr::Var(n, _) => {
            let Some(b) = lookup(scopes, n) else {
                ctx.errs.push(Diag::new(s, format!("undefined variable `{n}`")));
                return pkt;
            };
            let key = format!("{n}#{}", b.epoch);
            if b.guard.is_some() {
                ctx.errs.push(Diag::new(
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
                    for t in resolve_place(v, scopes, ctx)
                        .into_iter()
                        .chain(caps_of(v, scopes, ctx).unwrap_or_default())
                    {
                        pkt.caps.push((String::new(), t, Cert::Def));
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
                        for (h, kind, _) in edges_to(ctx, t) {
                            if holders.contains(&h) || kind == Auth::Sync {
                                continue;
                            }
                            ctx.errs.push(Diag::new(
                                s,
                                format!(
                                    "cannot send `{n}`: its referent is borrowed"
                                ),
                            ));
                            break;
                        }
                    }
                    for ((h, t), (kind, cert)) in ctx.edges.clone().iter() {
                        if *h == key {
                            pkt.edges
                                .push((String::new(), t.clone(), *kind, *cert));
                        } else if h.starts_with(&format!("{key}."))
                            || h.starts_with(&format!("{key}["))
                        {
                            pkt.edges
                                .push((h[key.len()..].to_string(), t.clone(), *kind, *cert));
                        }
                    }
                    for ((h, t), cert) in ctx.caps.clone().iter() {
                        if h.starts_with(&key) {
                            pkt.caps
                                .push((h[key.len()..].to_string(), t.clone(), *cert));
                        }
                    }
                    ctx.consumed.insert(key.clone(), ConsState::Cons);
                    ctx.edge_kills.insert(key.clone(), Cert::Def);
                    ctx.cap_kills.insert(key.clone(), Cert::Def);
                    ctx.edges.retain(|(h, _), _| {
                        h != &key
                            && !h.starts_with(&format!("{key}."))
                            && !h.starts_with(&format!("{key}["))
                    });
                    ctx.caps.retain(|(h, _), _| {
                        h != &key
                            && !h.starts_with(&format!("{key}."))
                            && !h.starts_with(&format!("{key}["))
                    });
                    if mutable {
                        pkt.auth = Some(Auth::Write);
                        for (_, t, _, _) in pkt.edges.clone() {
                            pkt.sent.push(t.clone());
                            ctx.transit.insert(t.clone());
                            ctx.holds.remove(&t);
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
                        ctx.errs.push(Diag::new(
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
fn chan_op(n: &str, args: &[Expr], s: Span, scopes: &Scopes, ctx: &mut Ctx) -> (Ty, Prov) {
    match n {
        "send" => {
            if args.len() != 2 {
                ctx.errs
                    .push(Diag::new(s, "`send` takes (channel, value)".to_string()));
                return (Ty::Void, Prov::Unknown);
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
                                ctx.errs.push(Diag::new(
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
                ctx.chan_queues
                    .entry(ep.clone())
                    .or_default()
                    .push(pkt.clone());
            }
            (Ty::Void, Prov::Unknown)
        }
        "recv" => {
            if args.len() != 1 {
                ctx.errs
                    .push(Diag::new(s, "`recv` takes (channel)".to_string()));
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
                    // The claimed authority leaves transit HERE; the
                    // binding install rides `last_recv` to `let`.
                    for t in pkt.sent.clone() {
                        ctx.transit.remove(&t);
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
            let eps = chan_gate(&args[0], s, scopes, ctx);
            if !eps.is_empty() {
                consume_place(&args[0], scopes, ctx);
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
    for (rel, t, kind, cert) in &pkt.edges {
        ctx.edges
            .insert((format!("{key}{rel}"), t.clone()), (*kind, *cert));
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
            ctx.errs.push(Diag::new(
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
fn access_check(e: &Expr, key: &str, write: bool, scopes: &Scopes, ctx: &mut Ctx) {
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
    scopes: &Scopes,
    ctx: &mut Ctx,
) {
    unguarded_check(e, key, scopes, ctx);
    transit_check(e, key, ctx);
    record_hold(
        ctx,
        key,
        if write { Auth::Write } else { Auth::Read },
    );
    for (h, kind, _cert) in edges_to(ctx, key) {
        if h == excl {
            continue;
        }
        let msg = match (write, kind) {
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
                "cannot read `{}`: it is atomically held",
                target_name(e)
            )),
            // A Sync claim on the domain blocks plain touches of the
            // mutex object itself.
            (false, Auth::Sync) => Some(format!(
                "cannot read `{}`: it is held by a lock",
                target_name(e)
            )),
            (false, Auth::Read) => None,
        };
        if let Some(m) = msg {
            // A conflicting TASK hold names its owner, not a borrow.
            let m = if let Some(sr) = ctx.tasks.get(&h) {
                let tn = h.split('#').next().unwrap_or(&h);
                let how = if kind == Auth::Atomic {
                    "atomically held"
                } else {
                    "held"
                };
                format!(
                    "cannot {} `{}`: it is {how} by unjoined task `{tn}`",
                    if write { "write to" } else { "read" },
                    target_name(e),
                ) + &format!(" (spawned at {}:{})", sr.span.line, sr.span.col)
            } else {
                m
            };
            ctx.errs.push(Diag::new(e.span(), m));
            return;
        }
    }
}

/// Structural death + birth: kill the holder's outgoing edges (rebind
/// frees them), apply a `&mut` move, then insert edges to the targets.
fn record_edges(holder: &str, ri: &RefInfo, ctx: &mut Ctx) {
    // Overwrite/rebind frees every edge the holder subtree carried.
    ctx.edge_kills.insert(holder.to_string(), Cert::Def);
    ctx.edges
        .retain(|(h, _), _| h != holder && !h.starts_with(&format!("{holder}."))
            && !h.starts_with(&format!("{holder}[")));
    if let Some(src) = &ri.move_from {
        ctx.edge_kills.insert(src.clone(), Cert::Def);
        ctx.edges.retain(|(h, _), _| h != src);
    }
    let kind = if ri.mutable {
        Auth::Write
    } else {
        Auth::Read
    };
    let cert = if ri.targets.len() > 1 { Cert::Maybe } else { Cert::Def };
    for t in &ri.targets {
        ctx.edges.insert((holder.to_string(), t.clone()), (kind, cert));
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
fn caps_of(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> Option<Vec<String>> {
    match e {
        Expr::Handle(place, _) => Some(resolve_place(place, scopes, ctx)),
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
                    .map(|((_, t), _)| t.clone())
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
                    .map(|((_, t), _)| t.clone())
                    .collect(),
            )
        }
        Expr::Join(inner, _) => {
            let key = place_id(inner, scopes)?;
            let eff = &ctx.tasks.get(&key)?.eff;
            if eff.ret_caps.is_empty() {
                None
            } else {
                Some(eff.ret_caps.clone())
            }
        }
        Expr::Call(n, _tg, args, _) => {
            if let Some(sum) = ctx.sums.get(n) {
                if !sum.ret_caps.is_empty() {
                    let sp = e.span();
                    let empty = Vec::new();
                    let bases = ctx
                        .call_bases
                        .get(&(sp.line, sp.col))
                        .unwrap_or(&empty);
                    let mut ts = Vec::new();
                    for t in &sum.ret_caps {
                        let mut ks = translate_key(t, bases);
                        if ks.is_empty() {
                            ks.push(t.clone());
                        }
                        for u in ks {
                            if !ts.contains(&u) {
                                ts.push(u);
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
            let mut ts = Vec::new();
            for a in args {
                if let Some(inner) = caps_of(a, scopes, ctx) {
                    for t in inner {
                        if !ts.contains(&t) {
                            ts.push(t);
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
        _ => None,
    }
}

/// Rebind a cap holder: old identity edges die, new ones attach.
fn record_caps(holder: &str, targets: &[String], ctx: &mut Ctx) {
    ctx.cap_kills.insert(holder.to_string(), Cert::Def);
    ctx.caps.retain(|(h, _), _| {
        h != holder
            && !h.starts_with(&format!("{holder}."))
            && !h.starts_with(&format!("{holder}["))
    });
    let cert = if targets.len() > 1 { Cert::Maybe } else { Cert::Def };
    for t in targets {
        ctx.caps.insert((holder.to_string(), t.clone()), cert);
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
                ctx.path_provs.insert(k, prov.clone());
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
        ctx.tag.remove(key);
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
                ctx.tag.insert(key.to_string(), t);
            }
            None => {
                ctx.tag.insert(key.to_string(), None);
            }
        },
        _ => {
            ctx.last_tag = None;
            ctx.tag.insert(key.to_string(), None);
        }
    }
}

/// After a `S{ r: &x, .. }` fill, lift each ref field's prov into the
/// target's path map (`s.r` etc.). Called by Let/Assign right after the
/// literal's expr_tv populated ctx.last_lit.
fn record_lit_fields(key: &str, ctx: &mut Ctx) {
    if let Some(provs) = ctx.last_lit.take() {
        for (fn_, prov) in provs {
            ctx.path_provs.insert(format!("{key}.{fn_}"), prov);
        }
    }
    for (rel, ri) in std::mem::take(&mut ctx.lit_edges) {
        record_edges(&format!("{key}.{rel}"), &ri, ctx);
    }
    for (rel, ts) in std::mem::take(&mut ctx.lit_caps) {
        record_caps(&format!("{key}.{rel}"), &ts, ctx);
    }
}

/// The write rule: storing `val` at `target`. Type-checked by callers;
/// this enforces (a) target is a place, (b) no writes through `&`,
/// (c) provenance outlives the location's owner region.
/// Does a write-path hop index through a read-only (`slice`, not
/// `slice mut`) view? `s[i] = v` and `s[i].f = v` both die here.
fn read_only_slice_hop(e: &Expr, scopes: &Scopes, ctx: &Ctx) -> bool {
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
            ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
                    target.span(),
                    format!("cannot write through read capability `{root}`"),
                ));
                return;
            }
            Some(_) if !live_out_edge(ctx, &format!("{root}#{}", root_epoch(target, scopes))) => {
                ctx.errs.push(Diag::new(
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
        for (h, kind, _) in edges_to(ctx, &t) {
            if holders.contains(&h) {
                continue;
            }
            let _ = kind;
            let m = if ctx.tasks.contains_key(&h) {
                let tn = h.split('#').next().unwrap_or(&h);
                format!(
                    "cannot write to `{}`: it is held by unjoined task `{tn}`",
                    target_name(target)
                )
            } else {
                format!("cannot write to `{}`: it is borrowed", target_name(target))
            };
            ctx.errs.push(Diag::new(target.span(), m));
            return;
        }
    }
    // `s.bytes = v` — the pseudo-member is a view, not a place:
    // rebinding it would smuggle bytes past `str_from_utf8`'s gate.
    if let Expr::Field(b, name, _) = target {
        if name == "bytes" && matches!(expr_ty_at(b, scopes, ctx).0, Ty::Str) {
            ctx.errs.push(Diag::new(
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
        ctx.errs.push(Diag::new(
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
            ctx.errs.push(Diag::new(
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

fn root_epoch(e: &Expr, scopes: &Scopes) -> u32 {
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
    ctx.edges.retain(|(h, t), _| {
        !dead.contains(&path_root(h).to_string()) && !dead.contains(&path_root(t).to_string())
    });
    ctx.caps
        .retain(|(h, _), _| !dead.contains(&path_root(h).to_string()));
    ctx.path_provs
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
    ctx.consumed
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
    ctx.init
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
    ctx.tag
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
}

fn check_block(stmts: &[Stmt], scopes: &mut Scopes, ctx: &mut Ctx, ret: &Ty) {
    scopes.push(HashMap::new());
    // N24A — a temp `let $rvN = ..` belongs to the NEXT real statement
    // (the lowering emits it immediately before its only use), so it
    // dies as soon as that statement checks. Otherwise a borrow held
    // by a temp — `to_cstr(heap, s.as_str())` → `let $rv0 =
    // s.as_str(); ..` — would keep `s` borrowed until scope end and
    // reject `s.destroy()` further down.
    let mut pending: Vec<String> = Vec::new();
    for s in stmts {
        check_stmt(s, scopes, ctx, ret);
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
    let popped = scopes.pop().unwrap_or_default();
    expire_scope(popped, scopes, ctx);
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
) {
    let cur = cur_region(ctx);
    let (survivors, dead_binds): (Vec<_>, Vec<_>) = popped
        .into_iter()
        .partition(|(_, b)| b.owner < cur);
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
    // with `t` outside is exactly this.
    for (h, t) in ctx
        .edges
        .keys()
        .filter(|(h, t)| {
            ctx.tasks.contains_key(h.as_str())
                && !dead.contains(h.as_str())
                && dead.contains(path_root(t))
        })
        .cloned()
        .collect::<Vec<_>>()
    {
        let tn = h.split('#').next().unwrap_or(&h).to_string();
        let sp = ctx.tasks.get(&h).map(|sr| sr.span).unwrap_or(Span { line: 0, col: 0 });
        let tgt = path_root(&t).split('#').next().unwrap_or(path_root(&t));
        ctx.errs.push(Diag::new(
            sp,
            format!(
                "task `{tn}` escapes its target's region: it still holds `{tgt}`, which dies at the end of this scope"
            ),
        ));
    }
    // A task binding that dies unjoined drops a suspended transform —
    // the child's claimed effects never land. Only `consume`d (joined)
    // or maybe-joined tasks are excused.
    for d in &dead {
        if ctx.tasks.contains_key(d) && consumed_state(ctx, d).is_none() {
            let tn = d.split('#').next().unwrap_or(d);
            let sp = ctx.tasks.get(d).map(|sr| sr.span).unwrap_or(Span { line: 0, col: 0 });
            ctx.errs.push(Diag::new(
                sp,
                format!("task `{tn}` goes out of scope without `join`"),
            ));
        }
    }
    ctx.edges.retain(|(h, t), _| {
        !dead.contains(&path_root(h).to_string()) && !dead.contains(&path_root(t).to_string())
    });
    // Cap edges die only with their HOLDER — a dead target leaves a
    // live, stale edge (that's the identity/lifetime split's point).
    ctx.caps
        .retain(|(h, _), _| !dead.contains(&path_root(h).to_string()));
    ctx.path_provs
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
    // Consumed state dies with the place — but a handle that names the
    // dead key goes stale at use, which is the point of the split.
    ctx.consumed
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
    // Init state dies the same way — a resurrected shadow binding
    // starts Uninit again, not inherited.
    ctx.init
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
    ctx.tag
        .retain(|k, _| !dead.contains(&path_root(k).to_string()));
}

/// N25D — a full path-sensitive state snapshot for a loop exit.
/// `break`/`continue` capture one at their site (after expiring
/// everything bound deeper than the loop's scope); the `while` merge
/// joins all of them plus the fall-through end against the pre-loop
/// state — the same cert discipline the `if` join uses.
struct LoopSnap {
    edges: Edges,
    caps: HashMap<(String, String), Cert>,
    consumed: HashMap<String, ConsState>,
    init: HashMap<String, Cert>,
    edge_kills: HashMap<String, Cert>,
    cap_kills: HashMap<String, Cert>,
    path_provs: HashMap<String, Prov>,
    transit: HashSet<String>,
    chans: HashMap<String, Vec<Packet>>,
    tag: HashMap<String, Option<String>>,
}

/// One loop frame on `ctx.loops`: `scope_depth` is the scopes len at
/// loop entry — everything deeper dies at a break/continue site.
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
    ctx.tag = s.tag;
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
        tag: ctx.tag.clone(),
    }
}

/// ctx := join(ctx, s) — the pathwise merge rules shared with `if`:
/// Def survives only where both paths agree; maybe on either side
/// weakens; unions for the channel/transit may-sets.
fn join_snap(ctx: &mut Ctx, s: &LoopSnap) {
    let mut merged_edges = s.edges.clone();
    for (k, (kind, _)) in ctx.edges.iter().chain(s.edges.iter()) {
        let cert = match (ctx.edges.get(k), s.edges.get(k)) {
            (Some((ck, Cert::Def)), Some((sk, Cert::Def))) if ck == sk => Cert::Def,
            _ => Cert::Maybe,
        };
        merged_edges.insert(k.clone(), (*kind, cert));
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
    for (c, q) in &s.chans {
        let e = ctx.chan_queues.entry(c.clone()).or_default();
        if q.len() > e.len() {
            *e = q.clone();
        }
    }
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
}

fn check_stmt(s: &Stmt, scopes: &mut Scopes, ctx: &mut Ctx, ret: &Ty) {
    match s {
        Stmt::Let {
            name, ty, init, span,
        } => {
            if let Some(t) = ty {
                check_ty(t, ctx.structs, *span, &mut ctx.errs);
            }
            let owner = cur_region(ctx);
            let (t, prov) = match (ty, init) {
                (Some(w), Some(init)) => {
                    let (got, prov) = expr_tv(init, scopes, ctx);
                    let t = coerce(w, init, &got, &mut ctx.errs);
                    // `let q: box T = p` — an owning value moves.
                    consume_owned_src(init, scopes, ctx);
                    if let Ty::Ref { .. } | Ty::Slice { .. } = got {
                        if !outlives(&prov, &Prov::Region(owner), &ctx.regs) {
                            ctx.errs.push(Diag::new(
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
                    ctx.errs.push(Diag::new(
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
                ctx.init.insert(key.clone(), Cert::Def);
                if let Some(ri) = refs_of(i, scopes, ctx) {
                    if ri.foreign {
                        ctx.foreign.insert(key.clone());
                    }
                    record_edges(&key, &ri, ctx);
                }
                if let Some(ts) = caps_of(i, scopes, ctx) {
                    record_caps(&key, &ts, ctx);
                }
                if let Some(len) = slice_len_of(i, scopes, ctx) {
                    ctx.slice_lens.insert(key.clone(), len);
                }
                // `str` NUL-provenance: a literal (or a binding already
                // proven literal-backed) marks the view `cstr`-safe.
                if t == Ty::Str {
                    match i {
                        Expr::Str(..) => {
                            ctx.cstr_safe.insert(key.clone());
                        }
                        Expr::Var(vn, _) => {
                            if let Some(vb) = lookup(scopes, vn) {
                                let vk = format!("{vn}#{}", vb.epoch);
                                if ctx.cstr_safe.contains(&vk) {
                                    ctx.cstr_safe.insert(key.clone());
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
                            .insert((key.clone(), d.clone()), (Auth::Sync, Cert::Def));
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
                ctx.doms.insert(key.clone(), root);
            }
            // Affine move — `let g2 = g` hands the domain to g2 and
            // consumes g.
            if let Some(Expr::Var(vn, _)) = init {
                let moved = lookup(scopes, vn).and_then(|b| {
                    b.guard
                        .clone()
                        .map(|gr| (format!("{vn}#{}", b.epoch), gr))
                });
                if let Some((old, (d, r, s))) = moved {
                    ctx.consumed.insert(old.clone(), ConsState::Cons);
                    ctx.edges.retain(|(h, _), _| !key_under(h, &old));
                    ctx.edges
                        .insert((key.clone(), d.clone()), (Auth::Sync, Cert::Def));
                    guard = Some((d, r, s));
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
            record_lit_fields(&key, ctx);
            record_tag(init.as_ref(), &key, &t, ctx);
            bind_task(&key, matches!(init, Some(Expr::Spawn(..))), ctx);
            scopes.last_mut().unwrap().insert(
                name.clone(),
                Bind {
                    ty: t,
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
            let vtv = expr_tv(value, scopes, ctx);
            coerce(&tt, value, &vtv.0, &mut ctx.errs);
            check_store(target, value, &vtv, scopes, ctx);
            // N18 — writing through a pointer reads the pointer:
            // `*p = v`/`b[i] = v` require p's/b's own init first.
            require_place_bases(target, scopes, ctx);
            // `x = p` — an owning value moves to the new place.
            consume_owned_src(value, scopes, ctx);
            // Writes act on the RESOLVED place: through a cap param or
            // a deref, the edge bookkeeping lands on the referent.
            let mut ks = resolve_place(target, scopes, ctx);
            if ks.is_empty() {
                ks.extend(place_id(target, scopes));
            }
            for k in &ks {
                // Rebind/overwrite kills the holder's old edges, then
                // the new value's refs attach (RefInfo applies moves).
                if let Some(ri) = refs_of(value, scopes, ctx) {
                    record_edges(k, &ri, ctx);
                } else {
                    ctx.edge_kills.insert(k.clone(), Cert::Def);
                    ctx.edges.retain(|(h, _), _| {
                        h != k
                            && !h.starts_with(&format!("{k}."))
                            && !h.starts_with(&format!("{k}["))
                    });
                }
                // Same for identity edges — `h = handle y` moves the
                // name, never borrows.
                if let Some(ts) = caps_of(value, scopes, ctx) {
                    record_caps(k, &ts, ctx);
                } else {
                    ctx.cap_kills.insert(k.clone(), Cert::Def);
                    ctx.caps.retain(|(h, _), _| {
                        h != k
                            && !h.starts_with(&format!("{k}."))
                            && !h.starts_with(&format!("{k}["))
                    });
                }
                if matches!(value, Expr::StructLit(..) | Expr::EnumLit { .. }) {
                    record_lit_fields(k, ctx);
                }
                record_tag(Some(value), k, &tt, ctx);
                // Reassignment resurrects: a store to `p` revives `p`
                // and every path under it; `p.f = v` revives `.f`.
                ctx.consumed.retain(|c, _| {
                    c != k
                        && !c.starts_with(&format!("{k}."))
                        && !c.starts_with(&format!("{k}["))
                });
                // N18 — the store lands a value: `x = v` covers `x`'s
                // whole subtree, `p.f = v` marks only `p#e.f`, and a
                // resurrected place is Init+Live again. `$arg`-rooted
                // stores are the callee's definite writes — they
                // export through the FnSummary.
                ctx.init.insert(k.clone(), Cert::Def);
                if k.starts_with("$arg") {
                    ctx.init_writes.insert(k.clone());
                }
                // Slice bookkeeping follows the value: a `slice(p, n)`
                // value makes the target foreign; a re-slice updates
                // the statically-known length (and an unknowable one
                // erases it — dynamic checks still apply).
                if matches!(vtv.0, Ty::Slice { .. }) {
                    match slice_len_of(value, scopes, ctx) {
                        Some(len) => {
                            ctx.slice_lens.insert(k.clone(), len);
                        }
                        None => {
                            ctx.slice_lens.remove(k);
                        }
                    }
                    if refs_of(value, scopes, ctx)
                        .map(|ri| ri.foreign)
                        .unwrap_or(false)
                    {
                        ctx.foreign.insert(k.clone());
                    } else {
                        ctx.foreign.remove(k);
                    }
                }
                // `str` bookkeeping follows the value too: byte length
                // where provable, NUL-provenance only when the source
                // still carries it.
                if vtv.0 == Ty::Str {
                    match slice_len_of(value, scopes, ctx) {
                        Some(len) => {
                            ctx.slice_lens.insert(k.clone(), len);
                        }
                        None => {
                            ctx.slice_lens.remove(k);
                        }
                    }
                    let safe = match value {
                        Expr::Str(..) => true,
                        Expr::Var(vn, _) => lookup(scopes, vn)
                            .map(|vb| {
                                ctx.cstr_safe.contains(&format!("{vn}#{}", vb.epoch))
                            })
                            .unwrap_or(false),
                        _ => false,
                    };
                    if safe {
                        ctx.cstr_safe.insert(k.clone());
                    } else {
                        ctx.cstr_safe.remove(k);
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
                            ctx.errs.push(Diag::new(
                                value.span(),
                                "a `recv` of a capability or handle packet must bind a fresh `let`"
                                    .to_string(),
                            ));
                        } else {
                            for k in &ks {
                                for (rel, t, kind, cert) in pkt.edges.clone() {
                                    ctx.edges
                                        .insert((format!("{k}{rel}"), t), (kind, cert));
                                }
                                for (rel, t, cert) in pkt.caps.clone() {
                                    ctx.caps.insert((format!("{k}{rel}"), t), cert);
                                }
                            }
                        }
                    }
                }
            }
            if let Some(k) = place_id(target, scopes) {
                bind_task(&k, matches!(value, Expr::Spawn(..)), ctx);
            }
        }
        Stmt::Return { value, span } => {
            // N18 — control leaves here: branch joins merge only the
            // paths that actually reach them.
            ctx.diverged = true;
            if ctx.task.is_some() {
                // Inside a spawn body this is the task's RESULT, not
                // the enclosing fn's — its effects are already
                // suspended; only the value/edges re-enter at `join`.
                match value {
                    Some(v) => {
                        let (got, _) = expr_tv(v, scopes, ctx);
                        if let Ty::Guard(_) = got {
                            ctx.errs.push(Diag::new(
                                *span,
                                "a lock guard cannot leave its scope".to_string(),
                            ));
                        }
                        // `return p` hands the task's box to the joiner.
                        consume_owned_src(v, scopes, ctx);
                        let want = ctx.task.as_ref().unwrap().ret.clone();
                        if want == Ty::Void {
                            ctx.task.as_mut().unwrap().ret = got.clone();
                        } else if got != Ty::Void && got != want {
                            ctx.errs.push(Diag::new(
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
                                        ctx.errs.push(Diag::new(
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
                                        ctx.errs.push(Diag::new(
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
                            ctx.errs.push(Diag::new(
                                *span,
                                "bare `return` in a task that returns a value".to_string(),
                            ));
                        }
                    }
                }
                return;
            }
            match (value, ret) {
            (Some(_), Ty::Void) => ctx.errs.push(Diag::new(
                *span,
                "returning a value from a void function".to_string(),
            )),
            (None, Ty::Void) => {}
            (Some(v), want) => {
                let (got, prov) = expr_tv(v, scopes, ctx);
                if let Ty::Guard(_) = got {
                    ctx.errs.push(Diag::new(
                        *span,
                        "a lock guard cannot leave its scope".to_string(),
                    ));
                }
                coerce(want, v, &got, &mut ctx.errs);
                if let Ty::Ref { mutable, .. } = want {
                    if !outlives(&prov, &Prov::Param, &ctx.regs) {
                        ctx.errs.push(Diag::new(
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
                if let Ty::Handle(_) = want {
                    if let Some(ts) = caps_of(v, scopes, ctx) {
                        ctx.ret_caps.extend(ts);
                    }
                }
            }
            (None, want) => ctx.errs.push(Diag::new(
                *span,
                format!("bare `return` in a function returning {want}"),
            )),
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
                ctx.errs.push(Diag::new(
                    *span,
                    format!("`if` condition must be bool, found {t}"),
                ));
            }
            // Branch joins: prov state merges pointwise, weaker wins.
            let pre_dead = ctx.diverged;
            let saved_scopes = scopes.clone();
            let saved_paths = ctx.path_provs.clone();
            let saved_edges = ctx.edges.clone();
            let saved_caps = ctx.caps.clone();
            let saved_cons = ctx.consumed.clone();
            let saved_init = ctx.init.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_ck = ctx.cap_kills.clone();
            check_block(then_body, scopes, ctx, ret);
            let then_d = ctx.diverged;
            let then_paths = ctx.path_provs.clone();
            let then_edges = ctx.edges.clone();
            let then_caps = ctx.caps.clone();
            let then_cons = ctx.consumed.clone();
            let then_init = ctx.init.clone();
            let then_kills = ctx.edge_kills.clone();
            let then_ck = ctx.cap_kills.clone();
            let then_transit = ctx.transit.clone();
            let then_chans = ctx.chan_queues.clone();
            ctx.path_provs = saved_paths.clone();
            ctx.edges = saved_edges.clone();
            ctx.caps = saved_caps.clone();
            ctx.consumed = saved_cons.clone();
            ctx.init = saved_init.clone();
            ctx.edge_kills = saved_kills.clone();
            ctx.cap_kills = saved_ck.clone();
            ctx.diverged = false;
            let then_scopes = std::mem::replace(scopes, saved_scopes);
            if let Some(eb) = else_body {
                check_block(eb, scopes, ctx, ret);
            }
            let else_d = ctx.diverged;
            let else_init = ctx.init.clone();
            let else_paths = ctx.path_provs.clone();
            let else_edges = ctx.edges.clone();
            let else_caps = ctx.caps.clone();
            let else_kills = ctx.edge_kills.clone();
            let else_ck = ctx.cap_kills.clone();
            // Edge join: union of both sides; definite only where the
            // same (holder,target,kind) edge exists on BOTH paths.
            let mut merged_edges = else_edges.clone();
            for (k, (kind, _)) in then_edges.iter().chain(else_edges.iter()) {
                let cert = match (then_edges.get(k), else_edges.get(k)) {
                    (Some((tk, Cert::Def)), Some((ek, Cert::Def))) if tk == ek => Cert::Def,
                    _ => Cert::Maybe,
                };
                merged_edges.insert(k.clone(), (*kind, cert));
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
            ctx.consumed = join_consumed(&then_cons, &ctx.consumed.clone());
            // Init join (N18): a diverged arm never reaches the join —
            // the merge takes the surviving side's map wholesale, so
            // `if c { x = 1 } else { return 0 }` leaves x Init.
            ctx.init = match (then_d, else_d) {
                (true, false) => else_init,
                (false, true) => then_init,
                _ => join_init(&then_init, &else_init),
            };
            ctx.diverged = pre_dead || (else_body.is_some() && then_d && else_d);
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
            for (c, q) in then_chans.iter() {
                let e = ctx.chan_queues.entry(c.clone()).or_default();
                if q.len() > e.len() {
                    *e = q.clone();
                }
            }
            // join envs
            let merged = merge_envs(&then_scopes, scopes, ctx);
            *scopes = merged;
            for (k, v) in &then_paths {
                let j = match else_paths.get(k) {
                    Some(o) => join_prov(v, o, &ctx.regs),
                    None => join_prov(v, saved_paths.get(k).unwrap_or(v), &ctx.regs),
                };
                ctx.path_provs.insert(k.clone(), j);
            }
        }
        Stmt::While { cond, body, span } => {
            let t = expr_tv(cond, scopes, ctx).0;
            if t != Ty::Bool {
                ctx.errs.push(Diag::new(
                    *span,
                    format!("`while` condition must be bool, found {t}"),
                ));
            }
            let pre_dead = ctx.diverged;
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
            let body_scopes = std::mem::replace(scopes, saved_scopes.clone());
            let body_snap = take_snap(ctx);
            // Loop exits: every break path, every continue path (it
            // re-enters the head and can exit a later iteration), and
            // the fall-through end — each merged against `pre`.
            let mut exits: Vec<LoopSnap> = lc.breaks;
            exits.extend(lc.conts);
            if !body_d {
                exits.push(body_snap);
            }
            if matches!(cond, Expr::Bool(true, _)) && !exits.is_empty() {
                // `while true` has no cond-false arm — the join base
                // is the exits themselves, not the zero-trip state.
                let first = exits.remove(0);
                restore_snap(ctx, first);
            } else {
                restore_snap(ctx, pre);
            }
            for ex in &exits {
                join_snap(ctx, ex);
            }
            ctx.diverged = pre_dead;
            let merged = merge_envs(&body_scopes, &saved_scopes, ctx);
            *scopes = merged;
        }
        Stmt::Break { span } | Stmt::Continue { span } => {
            let is_break = matches!(s, Stmt::Break { .. });
            let kw = if is_break { "break" } else { "continue" };
            let Some(lc) = ctx.loops.last() else {
                ctx.errs.push(Diag::new(
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
                let map = scopes.pop().unwrap_or_default();
                expire_scope(map, scopes, ctx);
                drained += 1;
            }
            for _ in 0..drained {
                scopes.push(HashMap::new());
            }
            let snap = take_snap(ctx);
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
        }
        Stmt::Match { scrut, arms, span } => {
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
                    ctx.errs.push(Diag::new(
                        *span,
                        format!("`match` scrutinee must be an enum, found {st}"),
                    ));
                    return;
                }
            };
            let skeys = resolve_place(scrut, scopes, ctx);
            if skeys.is_empty() {
                ctx.errs.push(Diag::new(
                    *span,
                    "`match` scrutinee must be a place".to_string(),
                ));
                return;
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
                        ctx.errs.push(Diag::new(
                            *span,
                            "`_` binds no payload".to_string(),
                        ));
                    }
                    continue;
                }
                match variants.iter().find(|(vn, _)| vn == &a.variant) {
                    Some((_, pt)) => {
                        if !covered.insert(a.variant.as_str()) {
                            ctx.errs.push(Diag::new(
                                *span,
                                format!("duplicate match arm `{}`", a.variant),
                            ));
                        }
                        if pt.is_none() && a.bind.is_some() {
                            ctx.errs.push(Diag::new(
                                *span,
                                format!(
                                    "variant `{}` of `{ename}` carries no payload to bind",
                                    a.variant
                                ),
                            ));
                        }
                    }
                    None => ctx.errs.push(Diag::new(
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
                ctx.errs.push(Diag::new(
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
                scopes: Scopes,
                path_provs: HashMap<String, Prov>,
                edges: Edges,
                caps: HashMap<(String, String), Cert>,
                consumed: HashMap<String, ConsState>,
                init: HashMap<String, Cert>,
                edge_kills: HashMap<String, Cert>,
                cap_kills: HashMap<String, Cert>,
                transit: HashSet<String>,
                chans: HashMap<String, Vec<Packet>>,
                tag: HashMap<String, Option<String>>,
            }
            let pre_dead = ctx.diverged;
            let saved_scopes = scopes.clone();
            let saved_paths = ctx.path_provs.clone();
            let saved_edges = ctx.edges.clone();
            let saved_caps = ctx.caps.clone();
            let saved_cons = ctx.consumed.clone();
            let saved_init = ctx.init.clone();
            let saved_kills = ctx.edge_kills.clone();
            let saved_ck = ctx.cap_kills.clone();
            let saved_transit = ctx.transit.clone();
            let saved_chans = ctx.chan_queues.clone();
            let saved_tag = ctx.tag.clone();
            let mut snaps: Vec<ArmSnap> = Vec::new();
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
                    ctx.tag.insert(k.clone(), force.clone());
                }
                // `V(b)` — synthesize `let b = scrut.V`: ordinary
                // by-value field move under the forced tag.
                let payload_ty = variants
                    .iter()
                    .find(|(vn, _)| vn == &a.variant)
                    .and_then(|(_, t)| t.clone());
                let mut body: Vec<Stmt> = Vec::new();
                if let (Some(b), Some(_)) = (&a.bind, &payload_ty) {
                    body.push(Stmt::Let {
                        name: b.clone(),
                        ty: None,
                        init: Some(Expr::Field(
                            Box::new(scrut.clone()),
                            a.variant.clone(),
                            *span,
                        )),
                        span: *span,
                    });
                }
                body.extend(a.body.iter().cloned());
                check_block(&body, scopes, ctx, ret);
                snaps.push(ArmSnap {
                    diverged: ctx.diverged,
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
                    tag: ctx.tag.clone(),
                });
                ctx.path_provs = saved_paths.clone();
                ctx.edges = saved_edges.clone();
                ctx.caps = saved_caps.clone();
                ctx.consumed = saved_cons.clone();
                ctx.init = saved_init.clone();
                ctx.edge_kills = saved_kills.clone();
                ctx.cap_kills = saved_ck.clone();
                ctx.transit = saved_transit.clone();
                ctx.chan_queues = saved_chans.clone();
                ctx.tag = saved_tag.clone();
                ctx.diverged = false;
                *scopes = saved_scopes.clone();
            }
            if snaps.is_empty() {
                return;
            }
            // ── N-arm merge (mirrors the `if` join) ──
            let all_dead = snaps.iter().all(|s| s.diverged);
            let live: Vec<&ArmSnap> = snaps.iter().filter(|s| !s.diverged).collect();
            ctx.diverged = pre_dead || (all_dead && missing.is_empty() || all_dead && wildcard);
            ctx.init = if live.is_empty() {
                saved_init
            } else {
                let mut acc = live[0].init.clone();
                for s in &live[1..] {
                    acc = join_init(&acc, &s.init);
                }
                acc
            };
            // Edges/caps: union; Def only where EVERY arm is definite
            // (a diverged arm still contributes — same as `if`).
            let mut merged_edges: Edges = HashMap::new();
            let mut keys: HashSet<(String, String)> = HashSet::new();
            for s in &snaps {
                keys.extend(s.edges.keys().cloned());
            }
            for k in keys {
                let mut kind: Option<EdgeKind> = None;
                let mut all_def = true;
                for s in &snaps {
                    match s.edges.get(&k) {
                        Some((ek, Cert::Def)) => kind = Some(*ek),
                        Some((ek, _)) => {
                            kind = Some(*ek);
                            all_def = false;
                        }
                        None => all_def = false,
                    }
                }
                if let Some(ek) = kind {
                    merged_edges.insert(
                        k,
                        (ek, if all_def { Cert::Def } else { Cert::Maybe }),
                    );
                }
            }
            ctx.edges = merged_edges;
            let mut merged_caps: HashMap<(String, String), Cert> = HashMap::new();
            let mut ckeys: HashSet<(String, String)> = HashSet::new();
            for s in &snaps {
                ckeys.extend(s.caps.keys().cloned());
            }
            for k in ckeys {
                let all_def = snaps
                    .iter()
                    .all(|s| s.caps.get(&k) == Some(&Cert::Def));
                if snaps.iter().any(|s| s.caps.contains_key(&k)) {
                    merged_caps.insert(k, if all_def { Cert::Def } else { Cert::Maybe });
                }
            }
            ctx.caps = merged_caps;
            let mut acc = snaps[0].consumed.clone();
            for s in &snaps[1..] {
                acc = join_consumed(&acc, &s.consumed);
            }
            ctx.consumed = acc;
            let mut acc = snaps[0].edge_kills.clone();
            for s in &snaps[1..] {
                acc = join_cert_map(&acc, &s.edge_kills);
            }
            ctx.edge_kills = acc;
            let mut acc = snaps[0].cap_kills.clone();
            for s in &snaps[1..] {
                acc = join_cert_map(&acc, &s.cap_kills);
            }
            ctx.cap_kills = acc;
            for s in &snaps {
                ctx.transit = ctx.transit.union(&s.transit).cloned().collect();
                for (c, q) in &s.chans {
                    let e = ctx.chan_queues.entry(c.clone()).or_default();
                    if q.len() > e.len() {
                        *e = q.clone();
                    }
                }
            }
            // Tag join: every arm agreeing on the same case keeps it;
            // any disagreement (or a wildcard arm) means unknown.
            let mut tkeys: HashSet<String> = HashSet::new();
            for s in &snaps {
                tkeys.extend(s.tag.keys().cloned());
            }
            let mut merged_tag: HashMap<String, Option<String>> = HashMap::new();
            for k in tkeys {
                let first: Option<&String> = snaps
                    .iter()
                    .find_map(|s| s.tag.get(&k).and_then(|o| o.as_ref()));
                let same = first.is_some()
                    && snaps.iter().all(|s| {
                        matches!(s.tag.get(&k), Some(Some(v)) if Some(v) == first)
                    });
                merged_tag.insert(k, if same { first.cloned() } else { None });
            }
            ctx.tag = merged_tag;
            // Path provs: join each key across arms, falling back to
            // the pre-match value where an arm doesn't carry it.
            let mut pkeys: HashSet<String> = HashSet::new();
            for s in &snaps {
                pkeys.extend(s.path_provs.keys().cloned());
            }
            let mut merged_paths: HashMap<String, Prov> = HashMap::new();
            for k in pkeys {
                let vals: Vec<Prov> = snaps
                    .iter()
                    .filter_map(|s| s.path_provs.get(&k).cloned().or_else(|| saved_paths.get(&k).cloned()))
                    .collect();
                if let Some(first) = vals.first().cloned() {
                    let j = vals[1..].iter().fold(first, |a, v| join_prov(&a, v, &ctx.regs));
                    merged_paths.insert(k, j);
                }
            }
            ctx.path_provs = merged_paths;
            // Scopes fold pairwise through the ordinary env merge.
            let mut acc_scopes = snaps[0].scopes.clone();
            for s in &snaps[1..] {
                acc_scopes = merge_envs(&acc_scopes, &s.scopes, ctx);
            }
            *scopes = acc_scopes;
        }
        Stmt::Region { name, body, .. } => {
            // A lexical lifetime frame: bindings inside get owner = this
            // region; provenance may flow in, never out.
            ctx.regs.push(RegionFrame {
                name: name.clone(),
                parent: Some(cur_region(ctx)),
            });
            check_block(body, scopes, ctx, ret);
            ctx.regs.pop();
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
                            ctx.errs.push(Diag::new(
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
                        ctx.errs.push(Diag::new(
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
        Stmt::Expr { expr, .. } => {
            expr_tv(expr, scopes, ctx);
            if ctx.last_spawn.is_some() {
                ctx.last_spawn = None;
                ctx.errs.push(Diag::new(
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
    let mut ctx = Ctx {
        fns,
        local_fns,
        structs,
        enums,
        tag: HashMap::new(),
        last_tag: None,
        regs: vec![RegionFrame {
            name: f.name.clone(),
            parent: None,
        }],
        path_provs: HashMap::new(),
        last_lit: None,
        lit_edges: Vec::new(),
        edges: HashMap::new(),
        caps: HashMap::new(),
        lit_caps: Vec::new(),
        next_epoch: 0,
        consumed: HashMap::new(),
        edge_kills: HashMap::new(),
        cap_kills: HashMap::new(),
        ret_edges: Vec::new(),
        ret_caps: Vec::new(),
        call_bases: HashMap::new(),
        arg_atomic: HashMap::new(),
        arg_mutex: HashMap::new(),
        doms: HashMap::new(),
        last_lock: None,
        last_mutex: None,
        chan_queues: HashMap::new(),
        transit: HashSet::new(),
        last_recv: None,
        arg_chan: HashMap::new(),
        foreign: HashSet::new(),
        slice_lens: HashMap::new(),
        cstr_safe: HashSet::new(),
        init: HashMap::new(),
        init_writes: HashSet::new(),
        diverged: false,
        holds: HashMap::new(),
        tasks: HashMap::new(),
        task: None,
        fn_ret: f.ret.clone(),
        last_spawn: None,
        loops: Vec::new(),
        sums,
        private_syms,
        errs: Vec::new(),
    };
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
        ctx.init.insert(key.clone(), Cert::Def);
        ctx.init.insert(format!("$arg{i}"), Cert::Def);
        if let Some(auth) = a {
            // The param IS the caller's place — its own authority edge
            // to synthetic $argN carries the contract.
            ctx.edges
                .insert((key.clone(), format!("$arg{i}")), (*auth, Cert::Def));
        } else if let Ty::Handle(_) = t {
            // Handle params get a synthetic IDENTITY edge — `*h` inside
            // resolves to $argN, which is always treated live.
            ctx.caps
                .insert((key.clone(), format!("$arg{i}")), Cert::Def);
        } else if let Ty::Ref { mutable, .. } | Ty::Slice { mutable, .. } = t {
            // A `&T`/`slice` param's edge to caller storage is
            // synthetic: distinct params can't be proven non-aliased
            // inside, but the call SITE rejects aliasing `&mut`s at
            // creation. `slice` carries the same contract — the edge
            // IS its span authority (`s[i]` resolves through it).
            let kind = if *mutable { Auth::Write } else { Auth::Read };
            ctx.edges
                .insert((key.clone(), format!("$arg{i}")), (kind, Cert::Def));
        } else if t == &Ty::Str {
            // A `str` param reads caller storage through the same
            // synthetic edge — `s.bytes[i]` resolves through it, and
            // `$arg{i}` survives in the summary for str returns.
            ctx.edges
                .insert((key.clone(), format!("$arg{i}")), (Auth::Read, Cert::Def));
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
            ctx.doms.insert(k.clone(), k);
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
        scopes[0].insert(
            n.clone(),
            Bind {
                ty: t.clone(),
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
    FnSummary {
        creates: ctx
            .edges
            .iter()
            .filter(|((h, t), _)| {
                h.starts_with("$arg") && t.starts_with("$arg")
                    && !own_rooted(h)
                    && !own_rooted(t)
            })
            .map(|((h, t), (k, c))| (h.clone(), t.clone(), *k, *c))
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
        syncs: ctx
            .holds
            .iter()
            .filter(|(k, a)| **a == Auth::Sync && k.starts_with("$arg") && !own_rooted(k))
            .map(|(k, _)| k.clone())
            .collect(),
    }
}
