# Memory Model Lab — v0 Semantic Baseline

Status: **frozen candidate**. This document is empirical, not aspirational:
every rule below exists because corpus scenarios forced it into existence,
and every rule cites the scenarios that defend it. `cargo run -- test corpus`
enforces the whole baseline — currently 150 scenarios, **1050/1050 recorded
`@expect` behaviours held**, F (`region+cap+affine`) at **79 caught /
0 missed / 0 over-restrictive**.

The design point is fixed:

```text
Programmer Intent → Semantic Representation → Machine Representation → Hardware
```

preserve intent with minimal machinery: no garbage collector, no runtime
ownership tracking, no explicit lifetimes, no hidden allocations.

---

## 1. The entire programmer-visible surface

```text
regions       region name { ... }   new(r) T   promote   handle
capabilities  ref / mut / own (param modes)    &x  &mut x
temporal      free x   (and implicit consumption at own-transfer)
tasks         let t = spawn { ... }  join t    let r = join t
types         struct decls, scalar types, fn decls with mode'd params
```

That is the whole of it. Notably absent: lifetime parameters/annotations,
borrow scopes in syntax, ownership keywords, move/borrow markers on
ordinary memory, `send`, `unsafe`. `send` does not exist — a task touching
a parent `own` var *is* the transfer.

**Programmer-visible states**: `Live | Consumed | MaybeConsumed`, and only
on consumables — ordinary memory never pays for them.

## 2. The core invariant

```text
Memory Safety = Region Validity + Access Capability + Minimal Temporal State
              + Structural Alias Topology + Task Authority Partition
```

where each component answers one question:

| component        | question it answers                          |
|------------------|----------------------------------------------|
| Region           | is this memory still valid?                  |
| Capability       | who may read/write/own it?                   |
| Affine state     | has consumable authority been spent?         |
| Alias graph      | what currently points at what?               |
| Effect summary   | how does a function transform that graph?    |
| Task holds       | what authority is partitioned across tasks?  |
| Handle           | which object? (identity — never authority)   |

---

## 3. Semantic rules

Notation: `R{n}` rules are programmer-visible diagnostics. Each cites the
corpus scenarios that forced it — the spec is auditable top to bottom.

### 3.1 Region validity (model: `region`)

**R1. `region-escape`** — a value derived from `region r` may not be stored
into a binding that outlives `r`. Region lifetime is *lexical*: nested
scopes are ordered by nesting, not by annotation.
*Forced by: 13 (inner ref to outer ok), 14 (outer ref escapes inner), 15
(ref field escape), 16 (move inner region value outward), 60–70 (graph
surgery suite).*

**R2. `unknown-region`** — `new(r)` names a live enclosing `region r`.
*Forced by: corpus-wide hygiene; any `new(r)` outside `r`'s scope.*

**R3. `dangling-edge-after-promote`** — `promote x` (escape to persistent
storage) requires **edge-closure**: no live alias edge may still point at
`x` (`x.edges_to == 0`). Promoting while aliased leaves the alias dangling.
*Forced by: 60–70 — promote of cycle members, subtree roots, back-pointers.*

### 3.2 Access capability (model: `capability`)

**R4. `capability-conflict`** — a `mut` (exclusive) borrow conflicts with
any overlapping access to the same object; shared `ref` borrows overlap
freely. Rebinding a variable is **not** writing through its capability.
*Forced by: 04 (mut alias), 17 (across a call), 18 (read/write overlap),
19 (many readers ok).*

**R5. `write-through-read`** — writing through a `read` capability is
rejected.
*Forced by: 09.*

**R6. `capability-escalation`** — capabilities never widen: a `ref` cannot
become `mut`, a borrowed value cannot produce `own`.
*Forced by: 10.*

**R7. `illegal-transfer`** — `own` authority transfers wholly; residual or
partial transfer is rejected.
*Forced by: 12 (illegal transfer), 20–21 (clean transfer and restoration).*

### 3.3 Temporal state (model: `affine`)

The temporal lattice is `Live → Maybe → Consumed` merged pointwise at
control-flow joins: `Live + Consumed = Maybe`; any use of `Maybe` rejects.

**R8. `use-after-consume`** — reading a `Consumed` (or `Maybe`) binding is
rejected. Covers move-then-use, conditional consume merges, and
child-consumed vars after `join`.
*Forced by: 02 (use-after-free), 11 (move-then-use), 22, 47 (conditional
consumption), 55 (consumed inside region), 89, 91 (moved/conditionally
consumed by call), 101 (recursive `own` move), 124 (child consumed, joined,
then used), 130 (conditional send).*

**R9. `double-consume`** — consuming a `Consumed`/`Maybe` binding is
rejected, including the second loop iteration (`while` re-checks under
merged state).
*Forced by: 03 (double free), 42–43 (close after close), 48 (consumed in
loop), 57 (maybe-double).*

**R10. `consume-through-alias`** — a borrowed binding (`is_ref`) cannot be
consumed; `free a` where `a = &x` rejects.
*Forced by: 54 (consumed through alias), 12 (illegal transfer).*

**R11. `consume-while-aliased`** — consuming a value while a live alias
edge still points at it is rejected. The check reads the *current* edge
set, so kills unblock: `a = &y` frees `x` for consumption.
*Forced by: 71–84 (all alias-death sites), 127 (kills not visible until
join), 131 (task-returned alias).*

**R12. `free-region-memory`** — `free` of a `new(r)`-allocated value is
rejected; region memory is released with its region.
*Forced by: 46.*

**R13. `invalid-consume`** — `free` of a plain (Copy) value is rejected:
there is nothing to consume.
*Forced by: the anti-cheat suite's plain-value `free` case.*

### 3.4 Task authority (model: `affine`, concurrency)

`spawn` analyzes its body in a forked world and records a **task result**;
`join` applies it as a deferred graph transform. Between them, the child's
touches become **holds** on shared vars: `Shared` (read), `Write`
(written), `Transferred` (moved/consumed). Holds propagate transitively
along alias *and* capability edges — `a.fd = 1` through `a = &mut x` holds
`x`, and `h.fd = 1` through `h = handle x` holds `x` too.

**R14. `data-race`** — inside a task's window, any access conflicting with
its holds is rejected. `Shared`×`Shared` is the only legal overlap; writes
are exclusive; a `Transferred` var is entirely out of reach. Holds count
transitively (nested spawns propagate even when the child joins its
subtask early).
*Forced by: 114, 115 (both write races), 125 (write through `&mut`), 128
(nested spawn), 134 (through two handles — the flipped escape hatch), 138,
139 (handle-mediated), 149 (nested spawn through handle).*

**R15. `use-after-send`** — using a var the child consumed/moved rejects;
authority left at spawn and does not return at `join` unless re-bound from
the child's return value.
*Forced by: 117 (use-during-window), 135 (conditional join keeps holds).*

**R16. `task-escapes-scope`** — a var held by an unjoined task may not go
out of scope; a task handle may not die while its task still holds vars.
This *is* `lifetime(region) ≥ lifetime(task)`, made lexical. (The same
check fires when a task `return`s a reference to a task-local var —
implemented, awaiting a dedicated corpus case.)
*Forced by: 122 (task outlives region), 136 (unjoined task).*

**R17. `invalid-join`** — `join` of a binding that isn't a live task
handle rejects; a joined handle is consumed (double-join = use-after-
consume).
*Forced by: corpus hygiene.*

### 3.5 Handle authority (model: `affine`, identity edges)

`h = handle x` records a **capability edge** `h → x`: identical birth
(create, copy, store, join-return), death (scope, rebind, overwrite,
consume, call-effect), merge, and hold-propagation rules as alias edges —
with one difference: capability edges are invisible to `alias_count`, so
they **never pin** their target.

**R18. `stale-handle`** — when a capability edge's target dies (consume,
scope end, rebind, shadow), `(holder, path)` is recorded; a later
dereference reports `stale-handle`. The runtime generation check becomes a
compile error wherever the graph can prove staleness — including
conditional paths (maybe-stale rejects).
*Forced by: 142 (free then deref), 143 (retarget into dead region).*

Deliberately **not** a rule: `free x` while a live handle exists is legal
(150) — the ticket is stale, only its *use* fails. Identity never pins.

### 3.6 Ownership discharge (model: strict `own`)

Affine says *at most once*; strict `own` says *exactly once, with respect
to authority termination*. Live owning authority may not simply
disappear:

> A live owning capability may not be abandoned when its binding leaves
> scope. It must first be transferred, returned, consumed by an owning
> operation, or explicitly released. Process termination is exempt.

"Owning capability" means a place — or a subtree of one — carrying live
`box`/`box slice` authority. Views (`str`, `slice`, `&`/`&mut`),
handles, POD, and capabilities (mutex/guard/task/channel) owe nothing:
their death relinquishes a claim, never a resource.

**R19. `abandoned-ownership`** — every scope exit (`}`-fallthrough,
`return`, `?`, `break`, `continue`) audits the bindings it kills. A
binding still holding live `own` authority rejects as
`cannot leave scope while \`x\` still owns …` (E2005), naming the
acquisition site and the exit that outruns its release. Legal
discharges: `free x`, `x.destroy()`, `return x`, `sink(x)` into an
`own` parameter, `send(ch, x)`, or `replace`/`swap` exchanging the
authority into a place that outlives it. A `match` arm that can hold
an owning variant payload (a `_` arm over a `Put`-style result is the
canonical case) inherits the same obligation for the scrutinee's live
payloads.
*Forced by: N45R — the overwrite law (R-adjacent store check) already
rejected silently leaking a live `own` on write; letting the same
authority vanish at `}` was the asymmetry this closes.*

The exemption is deliberate: `process.exit` and `unreachable` terminate
the path — the OS reclaims the image, so no language-level unwinding is
owed. No destructors, no *implicit* drop: cleanup is explicit so the
machine's behaviour stays readable in the source. `defer` (§3.8) is the
scoped-cleanup form of the same explicitness — it schedules a call the
source already names; it invents nothing. (Pins 629–637 reject every
abandon shape; 638–645 prove each legal discharge.)

**A call is a graph transformation.** `f(args)` applies the callee's
`Effect` — kills, then temporal effects, then creates — to the caller's
alias graph. Summaries compose (`Summary(A) = Summary(B) ∘ Summary(C)`)
and are derived, never written in source. `mml effects <file>` shows them.

**Recursive summaries are monotone fixed points.** The call graph
decomposes into SCCs; acyclic components analyze once; recursive ones
iterate union-absorption to convergence (≤3 passes observed). Two lattices
coexist: edges are *may* (union at joins), kills are *must* (intersect at
joins, dropped from loop bodies). Cyclic `returns-alias` is seeded at the
borrowed-arg bound — the least fixpoint of a may-analysis under-approximates
never-returning cycles.

**A task is a suspended call.** Spawn records `G_child`; join applies
`G_parent ← G_child_exit`: exit states (Consumed survives), definite kills,
surviving edges — and a `return`ed value enters the parent's graph at the
join site. Sequential consistency resumes at join: post-join writes are
ordinary writes (132).

**Kills are time-shifted, not instantaneous.** A child's definite kill is
only visible to the parent at join — `free x` in the window still sees the
edge (127); after join the edge is gone (126).

**`Maybe` propagation is uniform.** Conditional spawn/consume/join all
downgrade to `Maybe`; any dependent use rejects — one lattice for branches,
loops, calls, and tasks.

### 3.7 Projection moves (model: `affine`, views)

Authority follows the value **regardless of the source place's shape** —
binding, field, or nested projection obey the same law:

> Reading a view field (`slice mut`, `&mut`) out of a place moves the
> claim exactly as reading the same type out of a binding would. The
> field is consumed; the destination holds the claim.

```text
let s = some_slice_mut    // binding: moves     (always true)
let s = v.buf             // projection: moves  (N52 — was: copied)
v.buf[0] = 9              // rejects: use of moved reference `v.buf`
v.len                     // still live: the move is subtree-precise
v.buf = &mut b[..]        // legal: the place survives; re-slot re-arms it
```

**R20. `use-of-moved-view-field`** — after a view field's claim moves
out, *every* use that rides the claim rejects: indexing (`v.buf[i]`),
slicing (`&v.buf[..]`), len (`v.buf.len`), passing it to a call, and
element reads. `box`/`box slice` fields move by the same law via
consume tracking (E2001). Shared `slice`/`str` fields copy instead —
read claims coexist, so nothing moves. POD fields copy trivially.
*Forced by: N52 — pins 700–712. H4 was the asymmetry: `let t = s`
moved a mutable view binding but `let s = v.buf` copied one, so field
position silently weakened `slice mut`'s exclusivity.*

The source's own authority gates extraction:

- **Local / `own` container** — the move transfers the claim; the
  field dies, the destination lives.
- **`read v`** — `let s = v.buf` cannot mint writable authority; the
  extracted view is dead.
- **`write v`** — same: the caller owns the claim and the callee may
  use the field *in place* (`v.buf[i]`, `v.buf = fresh`) but may not
  carry it out. Extraction under `write` — including via `replace` —
  yields a dead view.
- **Foreign-provenance views** (`slice(p, n)` from a raw pointer) are
  exempt from the dead-field check: their claim was never in the edge
  graph to move.

All precision lives here; none of it appears in syntax:

```text
region levels        points_to/edges_to, edge-closure at promote
alias edges          (holder, field?, target) — definite/possible via path union
capability edges     same shape, non-pinning, handle identity
killed_paths         definite kills — intersected at joins, dropped in loops
effect summaries     per-function kills/creates/consumes/maybe/ret_alias
scc-fixpoint         Tarjan SCCs + monotone union iteration + seeded ret_alias
task holds           Shared|Write|Transferred per shared var, transitive
join merge           deferred effect application; conditional → may-effects
dead_caps            stale-handle facts (compile-time generation checks)
reaches              deref-capable touches (separate from ticket reads)
borrow-counts        capability model's exclusivity bookkeeping
path-merge           Temp lattice at every control-flow join
```

### 3.8 Scoped cleanup: `defer` (N54)

```text
defer <call>;
```

`defer` **schedules an explicit call** — it is not a destructor. The
call replays, last-registered-first (LIFO), at every normal exit of the
scope that contains the statement:

```text
} fallthrough      — the block's own end
return             — unwinds every scope
?                  — the Err edge unwinds every scope
break / continue   — unwinds the scopes the jump exits
```

`process.exit` and `unreachable` skip replay entirely — consistent with
R19's no-unwind termination rule.

Pending defers are **path state**. An exit nested in a branch replays
the scopes it unwinds on that edge only — the sibling paths keep both
the obligation and the freeze:

```text
defer free(p);
if c { break; }   // break edge replays free(p)
f(read p);        // the fallthrough path still owes — and holds — it
```

**Capture is at registration, and the operand places are frozen.** The
deferred call re-evaluates its operand places at the exit edge, so any
operation that would change what those places name rejects while the
defer is live:

```text
defer free(p);
let c = p;        // rejects — move deserts the deferred call
p = q;            // rejects — rebind changes the operand's identity
promote p;        // rejects — escapes the scope that replays
let p = ..;       // rejects — shadowing re-points the captured name
return p;         // rejects — the return would transfer the operand
```

Reading or mutating *through* a reserved place stays legal: `s[0]`,
`f(read s)`, `bump(s)` all pass — the freeze is on identity, not use.
`slice mut` field operands carry R20's law: `defer set(v.buf, ..)`
freezes `v.buf`, so `let t = v.buf` rejects as a move of a reserved
view.

**The replay is a real call.** The deferred call checks at
registration (cloned dry-run — diagnostics surface at the `defer`
site, effects withheld), then again with real effects at each exit
edge. Its consumes are the discharge: `defer free(p)` counts as `p`'s
R19 release on every covered path. LIFO ordering is enforced by the
reservation itself — `defer reads(b); defer free(b)` rejects (the
younger `free` would run first and leave `reads` dead), while
`defer free(b); defer reads(b)` is accepted (reads replay first).

**Form.** The operand must be a `void` call whose arguments are place
paths or literals — `defer f(x)`, `defer v.destroy()`, `defer f(v.buf)`,
`defer f(a[i])`. Compound operands (`1 + 2`, `&mut x`, nested calls)
reject: they would smuggle a hidden at-exit evaluation past the freeze.
Bind computed values first: `let r = &mut x; defer f(r);`.

*Motivated by: the N53 cleanup census — ~25–30 hand-rolled unwind
sequences across the 16 real programs (`minijson`'s `items.destroy()`
on five exits, `swapstress`'s identical four-free block ×3). `defer`
removes the duplication without touching the ownership model. The
consequence of the freeze is honest: a resource that must rebind or
transfer before scope end (a `Vec` under `own`-style `push`, a success
handoff) can't be deferred yet — cancellation is deliberately not v1.
Pins 713–742 cover the exit matrix and every conflict shape.*

## 6. Explicit non-goals of v0

- No lifetime parameters or annotations, anywhere.
- No ownership tracking of ordinary memory — only consumables carry state.
- No `send` keyword; transfer is capture.
- ~~No atomic/mutex/channel semantics yet~~ — *this was the original v0
  boundary, recorded before the amendments below ran their falsification
  rounds. Atomic, mutex, and channel semantics are now specified in
  Amendments A, B, and C and are part of the frozen model.*
- Function summaries do not yet express capability-edge *creation* by a
  callee (a callee storing a handle into a param's field is conservatively
  alias-tracked).
- No runtime spec for handle *representation* (generation counters etc.) —
  v0 fixes only the compile-time discipline.
- No guarantee about schedules, memory ordering, or fairness — the checker
  proves data-race *representability* bounds, nothing more.

## 7. The contract the corpus defends

```text
F: 79 unsafe scenarios rejected, 0 missed, 0 over-restrictive
   71 safe scenarios accepted
   +0 programmer-visible states since the temporal round
   rules added by concurrency + handles: 5 (data-race, use-after-send,
     task-escapes-scope, invalid-join, stale-handle)
```

Any semantic change must re-run the corpus. A rule may be added only when a
scenario falsifies its absence; a rule may be relaxed only when a scenario
proves it over-restrictive.

---

## Amendment A — atomic synchronization (accepted)

Falsified in `ATOMIC-PROPOSAL.md` against scenarios 151–171, then merged.
The extension is deliberately memory-order-agnostic: atomic ops are
race-safe; relaxed/acquire/release/acq_rel/seq_cst is a separate frontier.

```text
atomic storage + atomic operation   ⇒ synchronized access
atomic storage + ordinary access    ⇒ normal capability/race rules
non-atomic storage + atomic op      ⇒ rejected
```

Syntax: `atomic<T>` in type position (`let c: atomic<u64>`, struct fields),
plus four builtin ops — `atomic_load`, `atomic_store`, `atomic_add`,
`compare_exchange`. Ops resolve their target through the existing graphs:
`atomic_add(h)` follows `h`'s capability edge, `atomic_add(s.cnt)` names an
atomic field, `atomic_add(*a)` follows an alias.

**A1. `non-atomic-op`** — an atomic operation on storage not declared
`atomic<T>` rejects.
*Forced by: 158 (plain var), 166 (non-atomic field).*

**A2. atomic ops are mutually compatible across task boundaries** — a
child's atomic access records hold `Atomic` (compiler-internal hold kind);
concurrent atomic ops — parent's or siblings' — are legal.
*Forced by: 151, 152, 156, 159, 162, 165, 168, 170.*

**A3. mixed access is still a race** — ordinary read or write vs an
outstanding `Atomic` hold, or an atomic op vs `Shared`/`Write`, is
`data-race`. Inside one task, a var touched both plainly and atomically
degenerates to `Write` (an unsynchronized access exists in its window).
*Forced by: 153, 154, 155, 160, 163, 167, 169.*

Measured cost (exactly as budgeted):

```text
new syntax:              atomic<T> + 4 builtin op names
new programmer states:   +0
new programmer rules:    +1   (non-atomic-op; every race reuses data-race)
new internal hold kind:  +1   (Hold::Atomic — never source-visible)
new machinery:           +1   (atomic-op-resolution)
```

Hold granularity is root-level (v0's documented bound): 167 shows two
different fields of one struct racing at the root — a precision limit, not
a soundness hole.

---

## Amendment B — mutex synchronization (accepted)

Falsified in `MUTEX-PROPOSAL.md` against scenarios 172–191, then merged.
The central claim survived: **a mutex does not grant permanent write
authority** — `lock` derives exclusive synchronized authority that exists
only for the guard's lifetime, and the runtime lock, not the compiler,
is what proves exclusivity:

```text
static capability partition   = compiler proves exclusivity
mutex guard                   = runtime lock enforces exclusivity

Mutex = synchronization domain     Guard = temporary authority proof
```

Syntax: `mutex<T>` in type position (params, lets, struct fields) plus
`mutex e` to wrap a value, and two builtin ops — `lock` (derives a guard),
`unlock` (consumes it). Guards are ordinary affine bindings:
`unlock(g)` twice is `double-consume` (186); `g.x` after `unlock` is
`use-after-consume` (187); scope exit releases the domain (180).

**B1. `unguarded-access`** — the mutex's interior is private: a direct
field read/write on `mutex<T>` storage (`m.x`, `m.x = 1`, `*m`) rejects.
The only lawful access is through a live guard.
*Forced by: 174, 175.*

**B2. `non-mutex-op`** — `lock` requires `mutex<T>` storage (resolved
through fields and handles); `unlock` requires a live guard.
*Forced by: 188.*

**B3. `guard-escape`** — a guard is a scope-bound authority proof: it
cannot be `return`ed, and its domain is released at scope exit or
`unlock`.
*Forced by: 181.*

**B4. `recursive-lock`** — `lock(m)` while a live guard already holds
domain `m` in the same env rejects (self-deadlock; non-recursive mutexes).
*Forced by: 183.*

**B5. synchronized holds** — a write through a live guard resolves through
the guard's capability edge into the mutex's closure and records
`sync_writes[target] += domain`; at task capture these become hold
`Sync(domains)` (compiler-internal). Compatibility: `Sync({d})` tolerates
only `Sync({d})` — same mutex, serialized; disjoint domains race (178 —
two mutexes guarding the same object); every ordinary access races (176,
177). A target touched both plainly and under a lock in one task
degenerates to `Write`.
*Forced by: 172, 173, 176–179, 189, 190.*

**B6. summaries carry the domain** — a callee's synchronized writes are a
new summary fact `syncs: (param, domain-suffix)`; a `mut` argument covered
by a sync fact does not count as an unsynchronized write in the caller.
*Forced by: 191.*

Non-goals (recorded, not claimed): deadlock freedom (nested/opposite lock
order is race-safe but can block — 184 accepted, ordering undetected);
guard escape through join/channel/field-storage; poisoned locks;
recursive-lock mode; field-granular holds (190 races at the root).

Measured cost (exactly as budgeted):

```text
new syntax:              mutex<T> + `mutex e` + lock/unlock builtin names
new programmer states:   +0   (guards reuse Live|Consumed|Maybe)
new programmer rules:    +4   (unguarded-access, non-mutex-op,
                               guard-escape, recursive-lock)
new internal hold kind:  +1   (Hold::Sync — never source-visible)
new access kind:         +1   (Acc::Sync — internal)
new summary fact:        +1   (Effect.syncs)
new machinery:           +1   (synchronized-domains(guards))
```

The target formula held with zero new programmer-visible temporal states:

```text
Safe Shared Mutation = Capability + Synchronization Domain + Live Guard
```

## Amendment C — channels: authority transport (accepted)

Claim under test: **a channel transfers authority; it does not duplicate
authority.** The unifying view survived: a call is an immediate graph
transform, a task a suspended one, and a channel a *transported* one —
`send` cuts an authority packet out of the sender's graph, `recv`
installs it in the receiver's.

Syntax delta: `channel<T>` in type position (contextual ident, like
`atomic<T>` — no keyword) and three builtin call names: `send(ch, v)`,
`recv(ch)`, `close(ch)`; `channel()` constructs an endpoint. No
`Sender<T>`/`Receiver<T>` split — the corpus did not force endpoint
types.

**C1. send is affine transfer.** `send(ch, v)` on a non-Copy `v`
consumes it (`Op::Move`): use after send is `use-after-consume` (192),
double send is `double-consume` (194). Copy payloads and handles copy —
the sender keeps them (195, 200).
*Forced by: 192, 194, 195.*

**C2. capability transfer.** `send(ch, &mut x)` and `send(ch, r)` send
the *authority*, not the storage: `x` enters `in_transit` — the
authority's *location* moved to the channel, its state did not change
(+0 programmer-visible temporal states). Any access to `x` while in
transit is `use-after-send` (197, 211); `send` of an `&x` shared
reference travels as an ordinary (non-exclusive) packet. A packet's
alias-edge targets enter `in_transit`; its capability-edge targets do
not — `Handle = Identity` survives message passing (200, 201).
*Forced by: 196, 197, 200, 201, 211.*

**C3. recv installs authority.** `let g = recv(ch)` pops the in-flight
packet: `g` gains the packet's flags and edges — a received `&mut`
capability writes through to the claimed target (196, 208). Receiving a
packet whose target already died is `stale-handle` — the generation
check made static across the transport boundary (202). `recv` on a
channel with no recorded packets yields an opaque Live value (205).
*Forced by: 196, 198, 202, 205, 208.*

**C4. `send-escape`.** A value carrying an embedded *alias* edge
(`b.p = &x`) cannot cross the transport boundary — the promote edge
-closure rule restated for channels (203). Embedded *capability* edges
travel fine (204).
*Forced by: 203, 204.*

**C5. channel ops are synchronized access.** `send`/`recv`/`close` record
`Sync({ch})` on the endpoint — reusing the mutex round's domain hold.
Sibling sends/recvs on one channel are legal (198, 208); an ordinary
access or consume of the endpoint races them.
*Forced by: 198, 208.*

**C6. `non-channel-op`.** A channel op whose first argument does not
resolve to a `channel<T>` endpoint (directly or through a ticket)
rejects.
*Machinery rule — no dedicated scenario; fires on mis-typed programs.*

**C7. close is consume.** `close(ch)` is `Op::Close` on the ordinary
consume path: send after close is `use-after-consume` (206), double
close is `double-consume` (207). `recv` after close is also
`use-after-consume` — the endpoint is gone; drain semantics deferred.
*Forced by: 206, 207.*

**C8. guards cannot cross.** `send(ch, g)` where `g` is a lock guard is
`guard-escape` — scope-bound authority cannot travel (209).
*Forced by: 209.*

**C9. channel state is shared memory, not a hold.** At spawn, the child
sees the parent's in-flight packets and `in_transit` set; at capture its
final queue and transit map merge back into the parent (shared vars
only). A child's send therefore blocks the parent's use of the target
(211); a sibling's recv claims the packet and writes legally (208).
*Forced by: 198, 208, 211.*

Non-goals (recorded, not claimed): buffering semantics, select,
timeouts, rendezvous-vs-buffered, receive-after-close drain, endpoint
types, FIFO provenance (recv is a bag — packets carry no sender
identity), blocking/deadlock analysis on empty recv, interprocedural
send summaries beyond what `own`/param modes already cover (210 needs
none: the `own` boundary consumes the argument).

Measured cost (as budgeted):

```text
new syntax:              channel<T> + channel()/send/recv/close names
new programmer states:   +0   (in_transit is a location set, not a Temp)
new programmer rules:    +3   (use-after-send, send-escape,
                               non-channel-op)
new internal hold kinds: +0   (Sync({ch}) reuses the mutex domain kind)
new machinery:           +1   (channel-packets(in-transit))
```

The target formula held:

```text
Message Passing = Authority Transfer + Region Validity
```
