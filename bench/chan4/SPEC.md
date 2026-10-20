# CHAN-004 — Bounded Channels at the Language Level

Specification and checker analysis only. No parser, compiler, or
runtime changes are made by this document. CHAN-003 delivered the
runtime contract; this defines how Praetor source expresses it
with ownership preserved by the checker.

**Revision 4** — the epistemic model (§1), pending-state
transition table (§4), `?` mechanics (§5), conservative
`Some`/`None` analysis (§6), path-sensitive exactly-one-owner
invariant (§2), conformance matrix (§10), staged plan (§11).
Accepted architectural decisions are recorded in §12.
**Appendix A** carries the abstract-ownership proofs: the join
operator (§A.1), the opaque-payload rule (§A.2), and the
candidate-set/may-alias identity rules for unordered queue
claims (§A.3 — revised after the identity-soundness correction;
the earlier "label drift can only leak" claim was wrong for
capability edges, see §A.3.2). Registered issues: QA-006
(§8, issue #23) and QA-007 (§8.1, issue #25).

## 0. Existing machinery (what we build on)

The channel checker already models authority in flight:

- `send(ch, v)` — `packet_for` cuts a `Packet` (graph delta:
  edges, caps, `sent` targets, `auth`) out of the sender. Owned
  bindings are consumed (`cons_set` + `kill_set`); exclusive
  targets are `transit`-marked ("authority location, not a new
  state"). The packet parks on `ctx.chan_queues[endpoint]`.
  Returns `void`.
- `recv(ch)` — pops the queue front into `ctx.last_recv`; the
  enclosing `let`/`=` calls `claim_packet(key, pkt)`, which
  re-roots the travelled edges/caps at the binding and un-transits
  targets. Empty model queue → opaque value of the declared
  payload type. Returns `T`.
- `close(ch)` — `consume_place` on the endpoint; later endpoint
  ops are use-after-consume errors.
- `chan_gate` — every endpoint op records an `Auth::Sync` hold:
  sibling channel ops serialize, plain access races. The channel
  world (`chan_queues`, `transit`) clones into `spawn` children
  and merges back at spawn-end.
- `Result<T,E>` / `Option<T>` — ordinary generic enums. Payload
  authority is plain variant semantics: `Ok(b)` on a `box` moves
  it; `match` arms move payloads into their bindings; `?`
  desugars to match + early-return `Err(e)`; `ctx.tag` tracks
  known variants.
- Consumed-state merging — `ConsState::{Cons, Maybe}`: a place
  consumed on one branch of a join is `Maybe` afterwards; use is
  diagnosed.
- Runtime (CHAN-003, merged): `mml_chan_new_bounded(esz, bound)`,
  `mml_chan_send_bounded(c, src) -> SENT | CLOSED`, and
  `mml_chan_send` aborts on bounded channels as a backstop.

## 1. The epistemic model — runtime outcome vs checker pending

**`mml_chan_send_bounded` is synchronous.** When it returns, the
physical outcome is already fixed: the message is either enqueued
(`SENT`, ownership moved to the channel) or untouched in the
caller's cell (`CLOSED`, ownership never left). The emitted
`Result<void,T>` tag is already the true answer.

"Pending" is therefore **not a runtime state** — it is the
checker's epistemic superposition: after the call, the checker
cannot see which physical outcome occurred, so the authority
model must hold *both* outcomes live until the tag is observed
by a `match` (or the `?` desugar). Resolution is observation,
not an event: the Ok arm reveals "already committed," the Err
arm reveals "never committed."

Consequences:

- The packet's authority is parked — latent inside the Packet
  — in a `pending_send` entry keyed on the Result's place for
  the duration of the checker's uncertainty. Physically it is
  in exactly one place already; the model parks it with the
  Result so every path sees a consistent owner.
- The pending fact is **compile-time bookkeeping only** — a
  `map place-key → (endpoint, Packet)` in the checker context,
  same shape as `chan_queues`. Nothing at runtime knows or cares.
- Commit-to-queue at the Ok arm is a *model* operation; the
  physical enqueue happened earlier. Since the model queue is
  already an approximation (cross-task order is unknowable),
  this costs nothing — but it produces one sharp edge
  (§6.3): an intervening same-task `recv` before the match
  under-claims (safe direction).

## 2. Ownership rules and the path-sensitive exactly-one-owner invariant

**Invariant I-owner (path-sensitive).** Let `π` range over
feasible concrete execution paths and `Live(π)` over resources
live on `π`. Let `A_π(r,ℓ) ∈ {0,1}` indicate whether location
`ℓ` holds the ownership obligation for `r` on `π`, where `ℓ`
ranges over {bindings, pending Result Err-slots, channel queue
packets, recv'd bindings}:

    ∀π. ∀r ∈ Live(π).  Σ_ℓ A_π(r,ℓ) = 1

Exactly one owner **per path** — not per merged state. The
abstract (merged) checker state is a *disjunction over paths*:
mutually exclusive owners of `r` on different paths appear as
**multiple `Cert::Maybe` edges to `r`**, never as two
`Cert::Def` edges. The existing join machinery already encodes
this discipline (an edge is `Def` only when it exists with the
same kind on both incoming paths); §A.1 defines the join
operator `⊔` for every channel-world component and proves each
channel operation preserves I-owner.

The packet is a single cursor — on any one path the edge lives
in exactly one of {sender binding, pending Result's parked
packet, model queue, receiver binding} at every instant.

Transitions that preserve I-owner:

```
send(c, v) : Result<void, T>     [c : bchannel T]

  call        →  packet_for cuts v's edge; v consumed;
                 edge latent in pkt; pending_send[r] = (c, pkt).
                 I-owner: 1 edge, latent in the pending packet.

  Ok arm      →  edge moves r.Err → chan_queues[c]  (commit).
                 I-owner: 1 edge, in queue.

  Err(v') arm →  edge re-roots r.Err → v'  (claim, untransit).
                 I-owner: 1 edge, at v'.
```

```
recv(c) : Option<T>

  Some(v) arm →  queue-front packet's edges re-root → v;
                 untransit its targets.   I-owner: 1 edge, at v.
  None arm    →  no packet, no payload.   I-owner: unchanged.
```

Rules 4–7 from the runtime contract carry over verbatim:

4. `0 ≤ depth(c) ≤ bound(c)` — runtime-enforced; the checker
   does not model depth.
5. `closed(c) ⟹ send` evaluates to `Err` at runtime; the
   checker keeps both arms live regardless (it cannot see
   runtime closure — the tag is `Unknown` until observed).
6. `close(c)` consumes the endpoint; later ops are
   use-after-consume errors (existing rule).
7. `?` and `match` are the only pending-resolution sites in v1
   (§3, §4).

## 3. Language-level API (accepted — ruling §12)

```lang
let ch: bchannel Pkt = bchannel(8);      // bound: runtime arg, ≥1 validated

match send(ch, p) {                      // Result<void, Pkt>
    Ok(_)  => { }                        // p committed — authority channel-held
    Err(v) => { free(v); }               // closed — v re-binds p's authority
}

match recv(ch) {                         // Option<Pkt>
    Some(v) => { use(v); }
    None    => { break; }                // closed and drained
}

close(ch);
```

| op          | `channel T` (unchanged) | `bchannel T`        |
|-------------|-------------------------|---------------------|
| `send(c,v)` | `void`                  | `Result<void, T>`   |
| `recv(c)`   | `T`                     | `Option<T>`         |
| `close(c)`  | `void` (consume)        | `void` (consume)    |

No conversion between `channel T` and `bchannel T` — the distinct
type makes the void-send contract unreachable at the source
level; the runtime abort remains as defense-in-depth.

## 4. The pending state — full transition table

A pending fact `P = (endpoint, Packet)` is born at the `send`
call, recorded in `ctx.pending_send[r_place] = P`. The packet's
edges stay **latent inside the Packet struct** — parked exactly
like a queued packet's — so `r` is an **ordinary affine
`Result<void,T>`** for every rule except resolution. The
pending map is the authoritative record for the new diagnostics
(discard, overwrite, escape): a `pending_send` entry whose place
dies, is overwritten, or crosses a boundary unexamined is an
error — an explicit check, not a side-effect of the leak audit.

| transition                                   | rule                                                                                                      |
|----------------------------------------------|-----------------------------------------------------------------------------------------------------------|
| `let r = send(c,v)`                          | `r` binds `Result<void,T>` + pending P.                                                                    |
| `r2 = r` / `let r2 = r` (move)               | Pending re-keys to `r2`; `r` consumed — ordinary affine move.                                              |
| `match r` in `Ok` arm                        | P consumed; `pkt` pushed to `chan_queues[ep]`; `r` consumed.                                               |
| `match r` in `Err(v')` arm                   | P consumed; `claim_packet(v', pkt)` — edges re-root, `untransit`; `r` consumed.                            |
| `send(c,v)?`                                 | `?`'s own desugar match resolves P (§5) — no pending escapes.                                              |
| assignment `r = send(c,v)` over live pending `r` | **error** — overwriting an unexamined pending Result discards the possibly-undelivered packet (§4.2).    |
| `return r` / `f(r)` / `spawn`/join crossing  | **error** — pending is function-local bookkeeping in v1; resolve it first (§4.3).                          |
| `break`/`continue`/`return` with live pending `r` | **error** — `pending_send` entry abandons with the place (explicit pending-map check at scope-pop). |
| branch join                                  | **no special rule** — consumed-state merging applies unchanged (§4.1).                                     |
| loop back-edge                               | same as join: pending may persist only inside a live `r`; overwriting or abandoning it errors (§4.2).      |
| `close(c)` while `r` pending on `c`          | legal — Ok arm commits to the model queue of a consumed endpoint (harmless bookkeeping); Err arm claims normally. |

### 4.1 Branch joins — consumed-state and pending merges

Matching a pending `r` **consumes** it (the scrutinee is affine).
So on a path where `r` was matched, `r` is dead; on a path where
it wasn't, `r` is alive and still pending. The existing
`ConsState::Maybe` merge produces:

- `r` resolved on one path, untouched on the other → merged
  `Maybe` → any post-join `match r` is a "use of maybe-consumed"
  diagnostic. The checker *forces* resolution on all paths or
  none — which is exactly the desired semantics.
- `r` pending and alive on both paths → stays pending; resolved
  later as usual.
- `r` resolved on both paths → dead on both; no pending at the
  join. Committed packets on the Ok paths merged into
  `chan_queues[ep]` per the queue join (§A.1).

`ConsState::Maybe` alone does not establish authority
conservation across a join — it is only the binding-liveness
half. The conservation half is the join operator on edges,
caps, queues, transit, and pending facts defined in §A.1: the
merged state records *path-alternative* owners as `Cert::Maybe`
edges, so the abstract state can never contain two `Def`-owners
of one resource.

### 4.2 Loop interactions

- `let r = send(c,v); match r {..}` inside the body — resolved
  within the iteration; nothing reaches the back-edge pending.
- `let r = send(c,v); if cond { match r {..} }` — on the false
  path `r` survives pending; matched after the `if` or the Maybe
  merge diagnoses later use.
- `r = send(c,v)` where `r` is an outer binding already pending
  → overwrite-discard error (an unexamined send result).
- `let r = send(c,v); continue` — pending `r` abandoned at the
  iteration boundary → error.
- Loop-carried *unexamined* pending across iterations is
  impossible in well-typed code: either `r` is consumed by a
  match (resolved) or the carry is an overwrite/abandon error.

### 4.3 Pending escape — the v1 restriction

A Result carrying a pending fact may not cross a function-call,
`return`, `spawn`, or `join` boundary **while still pending**.
Rationale: the pending fact is keyed by the result's place in the
*local* checker context; a callee's world has no entry for it —
its `Ok` arm would drop the packet from the model silently (a
leak the checker can't see) and its `Err` arm would claim
correctly only by accident of the generic payload move. Rather
than widen every boundary's contract, v1 diagnoses:

> "resolve the `send` result (`match` or `?`) before it leaves
> this function"

The `?` operator is **not** an escape — it resolves first (§5).
An *ordinary* `Result<void,T>` — built in source, or produced by
resolving `?`'s Err path — carries no pending fact and crosses
freely.

## 5. `?` — precise resolution mechanics

`send(c,v)?` desugars to `match r { Ok(_) => ⟨void⟩;
Err(e) => return Err(e) }` — and that desugared match **is** the
resolution site:

- **Ok path**: pending P consumed; packet commits to
  `chan_queues[ep]`; continuation value is `void`.
- **Err path**: `e` claims the packet — `claim_packet(e, pkt)`
  re-roots its edges at `e` and `untransit`s its targets;
  `return Err(e)` re-wraps `e`'s authority as the function's
  return payload — an *ordinary* `Result<_,T>::Err`,
  pending-free.

So `?` never violates §4.3: what crosses the `return` boundary is
a resolved enum value. The undelivered payload travels home as
`Err(v)` — which is precisely why `send` returns
`Result<void,T>`: on failure the *value itself* is the error.

Constraints (all existing machinery):

- The enclosing fn must return `Result<_,T>` with the **same**
  `Err` type — `send(c, box Pkt)?` inside
  `fn -> Result<void, box Pkt>` type-checks; inside
  `fn -> Result<(), i32>` the existing `?` diagnostic fires.
- Inside `spawn` the existing rule rejects `?` outright (task
  results aren't propagation channels) — so a spawned producer
  must `match`; pending can never dangle at task-end via `?`.
- The abandon audit runs on the Err path before unwinding
  (existing N45R mechanics): other owned values held across
  `send(c,v)?` still leak-diagnose correctly.

## 6. Conservative `Some`/`None` analysis

The model queue is a **per-task partial-order approximation**:
it records packets this task has committed, in commit order.
Cross-task producers enqueue at the endpoint in program order
the checker cannot see; the merged child queues (spawn/join
machinery) extend the model conservatively.

### 6.1 Arm liveness

- `None` is **always live** — the checker can never prove the
  runtime queue non-empty (concurrent consumers may have drained
  it; closure is unobservable).
- `Some` is **always live** — the checker can never prove it
  empty (concurrent producers may have filled it outside the
  model; even an empty model queue yields `Some` an opaque `T`).

No `dead_ok`-style tag pruning applies to `Option` recv results:
`ctx.tag` is `Unknown` for channel-derived Options — the tag is
set by the runtime, not by construction the checker saw.

### 6.2 The `Some` claim rule — and the opaque-payload contract

Three cases, in increasing order of what the model can prove:

- **Ordered queue, non-empty** → pop front, `claim_packet(v,
  pkt)` verbatim, `untransit` the packet's named targets —
  identity-faithful (single-producer FIFO is provable; §A.3.1).
- **Unordered queue, non-empty** → pop front, `claim_packet`
  with **identity-unknown substitution**: `Own` edges keep real
  labels (region-immortal targets — §A.3.4); capability
  (`Read`/`Write`) and cap edges re-root to fresh `⊥` keys
  carrying the candidate set of every packet possibly queued —
  `⊥` **may-aliases** every candidate (§A.3.3): kind-level use
  allowed, named-place conflicts and region escapes fail-safe;
  named `pkt.sent` targets are *not* un-transited (§A.3.5).
- **Empty** → `v` binds an **opaque `T`** — no edges,
  no caps, no `auth`, `Prov::Unknown`. This is not "free to
  reject sometimes": it is a precise operation contract — an
  opaque owned value is *movable but never usable*:

| operation on opaque `v : T`                 | verdict | reason                                    |
|---------------------------------------------|---------|-------------------------------------------|
| move `v2 = v`, pass/store, re-send          | allow   | value-level ops consult no authority      |
| POD field read/write, destructure           | allow   | bit-level access needs no edge            |
| `free(v)` / free embedded `own` field       | reject  | discharge requires an `Own` edge          |
| deref embedded ref/box, `v.b.x`, `*v.r`     | reject  | deref requires a live edge to the referent|
| `&v.x`/`&mut v.x` (borrow interior)         | reject  | borrow requires authority over the target |
| capability payload use (`*v` on `&mut` pkt) | reject  | no `Write` edge                           |

  The soundness argument (full version §A.2): every permission
  the checker grants is edge-mediated. An opaque value carries
  zero edges, so it can satisfy zero authority requirements —
  under-approximation can only *reject* valid programs, never
  *permit* invalid ones. The physical authority still exists
  and travels with the bits when `v` is moved/re-sent; it is
  simply unmodeled. Consequence: for unmodeled authority the
  checker preserves **safety but not leak-freedom** — an opaque
  box that is never freed leaks silently; programs that must
  free recv'd owned values must keep the send where the checker
  can see it (same function, or a `spawn`ed task whose channel
  effects merge at spawn-end).

### 6.3 The self-send edge case (documented)

`r = send(c, p)` pending; `v2 = recv(c)` on the **same** task
before `match r`: physically, if the send returned `SENT`, `v2`
may receive `p`'s own authority back. The model's pending packet
isn't in `chan_queues` yet (it commits at the Ok arm), so `v2`
is modeled opaque — the model under-approximates `v2`'s
authority. Safe direction: `free(v2)`-style uses may be
rejected; nothing unsound is permitted. Programs needing the
claimed authority should resolve the send first. This is the
only observable consequence of deferred commit.

### 6.4 `None` and drain guarantees — and who may observe them

`None` establishes no permanent fact — a different endpoint or a
revived producer could refill the channel (the checker doesn't
track "closed" as endpoint state beyond `close`'s consume). The
harness-level guarantee (CHAN-003: after close + drain, no late
message arrives) is a runtime contract; the checker need only
model `None` as "no packet this time."

Note the scope rule on `close`: `close(ch)` **consumes** the
endpoint — a `recv(ch)` later in the *same* scope is a
use-after-consume error, on `bchannel` exactly as on `channel`.
The drain-after-close pattern therefore always involves a
**second party**: the consumer task holds its own binding to the
endpoint (channel endpoints are shared state across `spawn`;
each task's binding is separately tracked). The producer's
`close` consumes *its* binding; the consumer's `recv` keeps
returning `Some` while the queue drains, then `None`. The
conformance fixtures (P11) use that arrangement — never a
same-scope close-then-recv.

## 7. Misuse diagnostics

| program                                              | diagnostic                                                                    |
|------------------------------------------------------|-------------------------------------------------------------------------------|
| `send(c,v);` bare statement, any payload             | "the result of `send` on a bounded channel must be examined — `Err` means the message was not delivered" |
| `let _ = send(c,v)` or `r` dies unexamined           | same — discard of a pending Result                                            |
| `send(c,v)` result used as `Result` on `channel T`   | "`send` on `channel T` returns `void` — bounded sends need `bchannel T`"      |
| `recv(c)` bound to non-`Option` on `bchannel`        | "`recv` on `bchannel T` returns `Option<T>` — `None` means closed and drained" |
| use `v` after `send(c,v)`                            | existing consumed-use                                                         |
| pending `r` → `return`/`f(r)`/spawn/join             | §4.3                                                                          |
| pending `r` overwritten / abandoned                  | §4.2 / pending-map check at place death                                       |
| `bchannel` ↔ `channel` interchange                   | ordinary type mismatch                                                        |
| `free(v)` in `Ok` arm                                | consumed-use (authority is channel-held)                                      |
| `?` with mismatched `Err` type                       | existing `?` diagnostic                                                       |

## 8. QA-006 — registered: unbounded recv zero-fill on owned payloads

**Registered as a separate memory-safety issue — tracked as
[Praetor issue #23](https://github.com/ceeboozwah/Praetor/issues/23).
Semantics unchanged in CHAN-004.**

`recv` on a closed-and-drained `channel T` zero-fills the
destination cell. For `T` containing owned resources
(`box`, `slice`, structs embedding them), the zero bits are
*forged owned values*: the checker models the recv'd binding as a
live `T` with no edges, so `free(v)` on the forged box frees a
null-backed pointer and `v.field` dereferences through garbage —
the hazard is real today for `channel box T`, avoided only by
sentinel discipline the checker doesn't verify.

- **Status**: documented, not fixed. Unbounded `recv` keeps
  `T`/zero-fill for backward compatibility.
- **Mitigation landed by CHAN-004's design**: `bchannel` `recv`
  returns `Option<T>` — `None` carries no payload by
  construction.
- **Fix path** (deferred, its own milestone): an opt-in
  `recv_opt`/status-returning recv for `channel T`, or a
  checker-level warning on `recv` of owned-payload types into
  unconditional use. See §12 deferred items.

## 8.1 QA-007 — registered: recv claims mislabel authority under queue-order ambiguity

**Registered as a separate memory-safety issue — tracked as
[Praetor issue #25](https://github.com/ceeboozwah/Praetor/issues/25).
Surfaced by this specification's ownership proof (§A.3);
preexisting in unbounded `channel T` recv claims today.**

When the model queue's order is not provably local (spawn
merges, join-union of differing queues), `recv` may claim the
wrong packet's edges: the received binding gets a *real* edge to
the *wrong* resource. Capability payloads are affected through
two reachable failure modes — send-escape validates the labeled
target's region rather than the physical capability's (a
dangling `&mut` escapes its region — the §A.3.2 counterexample),
and a dead mislabeled edge frees the model's view of a source
while its physical cap remains in flight (capability
duplication). Owned payloads are unaffected (§A.3.2:
region-immortal targets, count-preserving discharge).

- **Status**: documented, not fixed for `channel T` — the
  ⊥-identity claim rule rejects previously-accepted programs;
  applying it to unbounded channels is a separate gated change.
- **Mitigation landed by CHAN-004's design**: `bchannel T`
  adopts the ordered/unordered claim rule (§A.3.3) from the
  start — identity-faithful where provable, unknown-identity
  where not.

## 9. Ownership walkthrough (complete trace)

Payload `box Pkt` — the owned-resource case:

```lang
let ch: bchannel box Pkt = bchannel(2);
let p = alloc<Pkt>(heap);               // (p#1 → heap#r0, Own)
match send(ch, p) {
    Ok(_)  => { }
    Err(v) => { free(v); }
}
```

1. `bchannel(2)` — `ch#1 : bchannel box Pkt`;
   `mml_chan_new_bounded(8, 2)`.
2. `alloc` — `p#1` binds `box Pkt`; edge `(p#1 → heap#r0, Own)`.
   **I-owner: 1** (at `p#1`).
3. `send(ch, p)` — `chan_gate` Sync hold; `packet_for(p)`:
   `cons_set(p#1)`, `kill_set(p#1)`, `edges_retain`; `pkt.edges
   = [("" → heap#r0, Own)]`. Runtime has *already* chosen
   SENT/CLOSED; the checker records `pending_send[r] = (ch#1,
   pkt)` — the edge latent inside `pkt`. **I-owner: 1** (latent
   in the pending packet).
4. `match r`:
   - **Ok** — commit: `chan_queues[ch#1].push(pkt)`. **I-owner:
     1** (in queue). `free(p)` or `free(v)` here: `p` consumed,
     `v` unbound — both diagnose.
   - **Err** — `claim_packet`: `("" → heap#r0, Own)` re-roots at
     `v#1`. `free(v)` discharges it. **I-owner: 1** (at `v#1`).

`?` variant — `fn produce(c: bchannel box Pkt) -> Result<void, box Pkt>`:

```lang
let p = alloc<Pkt>(heap);
send(c, p)?;          // Ok: commit, continue;  Err: return Err(pkt→e→payload)
return result.Result<void, box Pkt>::Ok(());
```

Err path: `e` claims `(→ heap#r0, Own)`; `return Err(e)` wraps
it — the caller's `Err(v)` owns the box and frees it. The
pending fact was consumed inside the callee; only a resolved
`Err` crossed the boundary.

## 10. Conformance-test matrix

Positive (each fixture header states its ownership trace):

| #    | case                                                                     | exercises                                  |
|------|--------------------------------------------------------------------------|--------------------------------------------|
| P1   | happy path POD send/recv/close, all sends matched                        | basics                                     |
| P2   | `box` payload: Ok→recv→free; Err→caller free                             | I-owner both arms                          |
| P3   | `send(c,v)?` in `fn -> Result<void, box P>`; caller frees Err payload    | `?` resolution + return-boundary legality  |
| P4   | spawn producer/consumer pair; join merges queue world                    | task boundary, Sync holds                  |
| P5   | send+match inside nested match arms inside a loop                        | nested resolution                          |
| P6   | Err arm retries on a second channel `send(c2, v)`                        | retained-value re-send                     |
| P7   | unbounded `channel T` corpus unchanged (771–777 et al.)                  | backward compatibility                     |
| P8   | `r2 = r` — moved pending Result, then match                              | pending re-key on move                     |
| P9   | `let r = send; if c { match r {..} } else { match r {..} }`              | resolution on all join paths               |
| P10  | loop: send+match per iteration; `Err` → break + free retained            | loop back-edge discipline                  |
| P11  | consumer task drains via `Some`/`None` loop while producer's own endpoint binding `close`s | status-driven consumer; endpoint consumption respected (§6.4) |
| P12  | `Some` arm on model-empty queue (cross-task producer)                    | conservative opaque claim — move/store only (§6.2) |
| P13  | self-send then same-task `recv` before match (§6.3) — **positive only for value-level uses** of the recv'd payload; any use requiring authority the checker can't establish (free/deref on the under-claimed value) belongs to N19 | under-approximation is rejection-safe      |
| P14  | `&mut` capability payload: Err arm restores the capability binding       | untransit + capability re-root             |
| P15  | two `box` payloads sent by competing producers; consumer frees both — owned-payload drift is count-preserving (counterexample-oriented control: same shape as N20, provably safe) | own-edge label honesty under ambiguity     |
| P16  | `&mut` payload recv'd on ordered queue (single producer): full-identity claim, `*v` writes, rebinding | verbatim claim path                        |
| P17  | `&mut` payload recv'd on unordered queue: `⊥` claim — `*v` writes allowed, binding dies cleanly | kind-level use under unknown identity      |
| P18  | `&mut x` legal once `MayOutstanding(x) = ∅` — endpoint drained, all `⊥` claims naming `x` dead, no pending (positive boundary of N23) | release iff no unprovable residence remains |

Negative (each documents the violated invariant):

| #    | case                                                                     | invariant violated                         |
|------|--------------------------------------------------------------------------|--------------------------------------------|
| N1   | `send(c,v);` bare statement (POD)                                        | unexamined result — undelivered message    |
| N2   | `let r = send(c,v)` dies unexamined (owned)                              | live-authority abandon                     |
| N3   | use `v` after `send(c,v)`                                                | consumed                                   |
| N4   | use `v`/`Err` binding in the wrong arm                                   | unbound/consumed                           |
| N5   | `let r: Result<..> = send(uc,v)` on `channel T`                          | void-result misuse                         |
| N6   | `let v: Pkt = recv(bc)` skipping `Option`                                | status elided                              |
| N7   | `return r` / `f(r)` / spawn-out with pending `r`                         | §4.3 escape                                |
| N8   | `bchannel`↔`channel` assignment/arg mix                                  | type mismatch, no conversion               |
| N9   | `close(c)` then `send(c,v)`                                              | use-after-consume                          |
| N10  | `free(v)` in `Ok` arm                                                    | authority channel-held, not caller-held    |
| N11  | `Some` payload used in `None` arm                                        | unbound                                    |
| N12  | send-escape / region-outlives-endpoint on `bchannel`                     | packet_for rules unchanged                 |
| N13  | send in loop without match, `r` carried to next iteration                | pending abandoned at back-edge             |
| N14  | `r = send(c,v)` overwriting a live pending `r`                           | unexamined discard via overwrite           |
| N15  | `match r` in one `if` branch only; `match r` after join                  | maybe-consumed merge                       |
| N16  | `send(c,v)?` in `fn -> Result<_, U>`, `U ≠ T`                            | `?` Err-type mismatch                      |
| N17  | `?` inside `spawn` on a pending send                                     | existing task-`?` rejection                |
| N18  | spawn body leaves pending unresolved at task-end                         | abandon at spawn boundary                  |
| N19  | `free(v)` where `v` bound from `Some` on model-empty queue               | conservative under-claim rejection (P13's dual) |
| N20  | **counterexample**: `&mut x`/`&mut y` from competing producers; `recv` → `send(c2, v)` where `R_y ⊉ R_c2` — labeled-region check must not authorize physical escape (§A.3.2) | region-bypass via mislabeled claim — `⊥` re-send rejected |
| N21  | **regression (reviewer-specified)**: unordered-queue `&mut` recv → `&mut x` where `x ∈ C(⊥ᵥ)` | may-alias conflict — a second cap on a possible original target is unprovable |
| N22  | unordered-queue `&mut` recv, then `send`/`return` of the `⊥` capability  | escape check must satisfy **all** candidates — any `R_t ⊉ R_ep` rejects |
| N23  | named source stays `transit` while its packet may remain queued, even after the `⊥` claim dies  | transit release conditions §A.3.5        |

## 11. Staged implementation plan — both stages

Rollback boundary after every stage; S1–S2 inert without S3–S4.

| stage | change                                                                                     | surface                | rollback                          |
|-------|--------------------------------------------------------------------------------------------|------------------------|-----------------------------------|
| S1    | `mml_chan_recv_bounded(c,dst)->int` + `mmlrt.h` + `gen-embed` — **landed, PR #24**           | posix + win32 runtime  | additive entry                    |
| S2    | `Ty::BChannel(Box<Ty>)`, parser (`bchannel T`, `bchannel(n)`), print/mono                    | frontend               | dead variant without S3           |
| S3    | checker: `chan_inner`/`chan_gate` on `BChannel`; send→pending `Result<void,T>`; recv→`Option<T>`; pending table + resolution in `match`/`Expr::Try`; per-endpoint `unordered` flag + `⊥`-identity claims (§A.3); §4–§7 diagnostics | `check.rs` (s0)        | rules fire only on `BChannel`     |
| S4    | lowering: `bchannel(n)`→`new_bounded`; `send`→`send_bounded`+Result build; `recv`→`recv_bounded`+Option build | `gen.rs` (s0)          | gated on `BChannel`               |
| S5    | selfhost mirror (`ir.lang` gen + checker parity), `embed.lang` regen                         | selfhost               | s2→s3 `.ll` fixpoint gate         |
| S6    | fixtures P1–P14/N1–N19, corpus both platforms, docs                                          | `native-tests/`, CI    | revert fixtures                   |

**Parity requirements (no runtime-only assumptions).** The
pending table is ordinary checker state — `map place-key →
(endpoint, Packet)` — the same data shape as `chan_queues`, so
the selfhost checker's `.lang` implementation needs no new
primitive. Both stages: identical diagnostics on every
N-fixture; identical `.ll` at s2→s3 on every P-fixture. The
checker never reads the physical tag — the epistemic model
holds both arms live regardless of what the runtime did — so
nothing in the semantics depends on runtime introspection, and
no existing ownership guarantee is weakened: `packet_for`,
`claim_packet`, `chan_gate`, transit, consumed-merge, and the
abandon audit are all reused unmodified. The only new state is
the pending map; the only new rules are its birth (send),
resolution (match/`?`), and escape/discard diagnostics.

## 12. Rulings applied and remaining decisions

Accepted by architectural review (ruling on rev-1):

- **A (accepted)** — type-directed `send`/`recv` dispatch on
  endpoint kind; no separate op names.
- **B (accepted)** — uniform error on unexamined send results,
  all payload types.
- **C (accepted)** — pending Results are function-local in v1
  (§4.3); relaxation deferred.
- **D (accepted)** — `bchannel T` type spelling; bound is a
  runtime argument, not type-level.
- **F (accepted)** — `Result<void,T>` for send, `Option<T>` for
  recv; `?` mechanics per §5.
- **G (accepted)** — bound validated at construction by the
  runtime (≥1, abort).

Deferred items (not part of this milestone):

- **E** — opt-in `recv_opt`/status recv for unbounded `channel T`
  (the QA-006 fix path). Separate milestone.
- **C-relax** — value-carried pending across call/join
  boundaries; needs the pending fact to ride inside the enum's
  authority model through interprocedural merge. Defer until
  usage demands it.

None remaining open: the design is complete pending review of
this revision.

---

# Appendix A — Abstract-ownership soundness

This appendix supplies the two proofs the rev-2 review required:
(A.1) that branch joins conserve per-path ownership, and (A.2)
that operations on opaque recv'd payloads cannot bypass
authority validation. A.3 covers packet-label drift; A.4 gives
the two minimal checker traces.

## A.1 The join operator ⊔ and path-sensitive conservation

**Abstract state.** The checker context is a tuple
`σ = ⟨E, C, K, T, Q, P, M⟩` — edges `E`, caps `C`, consumed `K`,
transit `T`, channel queues `Q`, pending sends `P`, and the
remaining binding/scope state `M` (which joins by existing
`merge_envs`/`join_*` discipline and is out of scope here).
Each `σ` conservatively represents a *set of concrete
path-states* `{σ_π}`; the concretization `γ(σ)` is the set of
per-path states the abstract one may stand for.

**Cert discipline.** Every edge/cap/kill carries
`Cert ∈ {Def, Maybe}`: `Def` = present on *all* incoming paths,
`Maybe` = present on *some*. A `Def` edge is a commitment; a
`Maybe` edge is an alternative.

**Component joins** (the first four are the existing rules —
verified against `check.rs`; the last two are new for CHAN-004):

| component | `σ₁ ⊔ σ₂` rule                                                            |
|-----------|----------------------------------------------------------------------------|
| edges `E` | union; `cert = Def` iff the same `(holder,target,kind)` edge is `Def` on both paths, else `Maybe` |
| caps `C`  | union; `Def` iff `Def` on both                                             |
| consumed `K` | `Cons` iff `Cons` on both; else `Maybe` (`join_consumed`)               |
| transit `T` | union (may-set) — authority in flight on either path may be in flight |
| queues `Q` | per endpoint: the longer queue (over-approximation by length — a packet sent on either path may be in flight) |
| pending `P` | per result place `r`: pending on both paths → **disjunction** `⟨pkt₁ | pkt₂⟩` (the two packets are path-alternatives, not co-owners — see below); pending on one path only → `r` is `Maybe`-consumed by `K`'s join anyway, so the surviving pending fact is *unreachable* — a may-fact on a maybe-dead place, harmless |

**Pending disjunction.** When `r` is pending on both paths with
packets `pkt₁` (over `v₁`) and `pkt₂` (over `v₂`), the merged
fact is the *alternation*, not the union. Resolution:

- **Ok arm**: commit *all* candidate packets to `chan_queues`
  (each is a may-packet — consistent with the queue join;
  physically exactly one path's send ran, so physically exactly
  one arrives).
- **Err arm**: `v'` claims each candidate's edge set with
  `cert = Maybe`. Two Maybe-edges to *different* resources are
  path-alternative owners; two Maybe-edges to the *same*
  resource (both sends moved the same `v`) are likewise
  alternatives. Either way each concrete path sees exactly one
  claimant — the disjunction never manufactures a `Def` pair.

**Conservation argument.** For each operation `op` and each
path `π` through it, the ownership edge targeting `r` moves
between exactly one source and one destination location:

- `send`: `edge ∈ E` → `edge ∈ pkt.P` (one move, latent in the
  packet).
- `Ok`-resolution: `pkt.P` → `pkt ∈ Q` (one move).
- `Err`-resolution: `pkt.P` → `edge ∈ E` at `v'` (one move).
- `recv`/`Some`: `pkt ∈ Q` → `edge ∈ E` at `v` (one move).
- `free`/`consume`: `edge ∈ E` → discharged (resource exits
  `Live(π)` — invariant vacuous for dead `r`).
- `⊔`: on `π` the merged facts refine to exactly the
  pre-join path-state's facts; alternatives collapse to the
  actual path's entries.

Since every `op` is edge-count-preserving on every `π`
(exactly-one-in, exactly-one-out, or a discharge that removes
`r` from `Live`), and `⊔` introduces only `Maybe`-certified
alternatives — never a second `Def` owner — I-owner holds by
induction on path length.

**What could go wrong and doesn't.** The forbidden outcome is
two `Def` `Own` edges to one `r` — which would let two holders
each pass an exclusivity check. A `Def` cert requires presence
on *both* paths with the *same* `(holder,target,kind)` — but a
resource consumed/moved on one path has no such edge there, so
the surviving edge merges `Maybe`. Q.E.D. for the join.

## A.2 Opaque recv'd payloads — the soundness rule

**Statement.** `v : T` bound from `Some` on an empty model queue
carries no edges, caps, or `auth`. The permitted operation set
is exactly the *value-level* operations of §6.2's table; every
*authority-level* operation is rejected — not as a heuristic,
but because:

> **Lemma (edge mediation).** In this checker, every permission
> to access, free, borrow, or mutate through a value is granted
> by consulting a graph edge or cap — never by the value's type
> alone. An opaque value carries zero edges, so it satisfies
> zero authority requirements. Rejection is total, not
> occasional.

**Case partition** (exhaustive over physical outcomes):

1. Physical `None` (closed+empty) → `v` doesn't exist; the
   `Some` arm is unreachable on that path — any modeling inside
   it is vacuous.
2. Physical `Some`, model queue non-empty → packet claim:
   verbatim on ordered queues; under order ambiguity the claim
   substitutes `⊥` targets for capability/cap edges (§A.3 —
   mislabeled identity is not merely a drift nuisance: it can
   bypass label-sensitive checks, so unknown claims get unknown
   identities).
3. Physical `Some`, model queue empty → opaque `v`: the physical
   authority exists but is unmodeled. `v` may be moved, stored,
   destructured, re-sent (authority travels with the bits);
   nothing authority-level is permitted.

**Consequence stated plainly:** for authority the checker cannot
see, it guarantees *safety* — no dereference, free, borrow, or
exclusivity violation can be *permitted* — but not *leak-freedom*
— an opaque owned value that is never freed leaks without
diagnostic. That is the honest trade, and it only arises for
sends outside the checker's visibility (FFI-produced channels,
opaque `channel` params); sends inside the analyzed program —
including `spawn`ed tasks, whose channel effects merge into the
parent's model at spawn-end — are always claimed with edges.

## A.3 Resource identity under queue-order ambiguity

Rev-2's claim that mislabeling "can only leak" was **wrong for
capability edges**. This section states the correct theorem,
exhibits the counterexample, and defines the conservative
claim rule the checker must implement.

### A.3.1 What the queue abstraction does and does not preserve

`chan_queues` preserves **packet contents** — every `Packet`'s
own edges/caps are recorded honestly at send time and never
mutated in flight. It does **not** preserve total order across
non-local sources:

- a branch join merges per-path queues by longer-queue union —
  packets from different paths have no provable order;
- a spawn boundary merges child endpoint queues — a child's
  sends interleave with the parent's in physical time the
  checker cannot see.

Therefore a `recv` claim is **identity-faithful** — the model
front *is* the physically-arriving packet — only when the queue
was built solely by the local task's program-order pushes and
pops. Call such a queue `ordered`; every join-union that merges
differing queues and every spawn-merge marks the endpoint
`unordered`, **sticky** (pops and later local pushes cannot
restore provable order — the arriving message's position is
unknowable).

### A.3.2 The counterexample — mislabeling defeats region checks

```lang
// Regions: R_x = R_c2 (function level); R_c1 ⊂ R_y (nested)
let c2: channel &mut i32 = channel();
{
    let y = 0;                       // R_y
    {
        let c1: channel &mut i32 = channel();  // R_c1 ⊂ R_y
        spawn { send(c1, &mut x) };  // pkt_x: Write→x; R_x ⊋ R_c1 ✓
        spawn { send(c1, &mut y) };  // pkt_y: Write→y; R_y ⊋ R_c1 ✓
        let v = recv(c1);            // model may claim pkt_x → v:(v→x)
                                     // physical v may hold &mut y
        send(c2, v);                 // escape check on →x: R_x ⊋ R_c2 → PASS
                                     // physical: &mut y escapes into c2
    }                                // c1 dies
}                                    // y dies — its cap sits in c2
// recv(c2) yields a dangling &mut — use-after-scope, checker-approved
```

Two failure modes, one root cause — the model installs a
*correct* packet's edges on the *wrong* binding:

1. **Region-check bypass** (above): send-escape validates the
   *labeled* target's region; the *physical* capability's region
   is unchecked.
2. **Capability duplication**: if `v` dies unused, its `→x`
   model edge dies — `x` appears free and `&mut x` is permitted,
   while `x`'s *physical* capability is still in flight in the
   queue. Two exclusive capabilities on one resource.
3. Symmetrically, untransiting the labeled target frees the
   *model's* view of `x` while `x`'s real cap remains
   outstanding — the same duplication, arrived at through the
   transit set instead of edge death.

Owned payloads do **not** share the hole: `box` targets are
heap-region-immortal, so no label-sensitive gate exists for
them; and free/discharge is count-preserving — each `Own` edge
is discharged once, each physical box freed once — the
rev-2 argument *was* correct for own edges and survives as the
own-edge half of the theorem.

### A.3.3 The claim rule — ordered verbatim, unordered candidate-sets

A per-endpoint `unordered` flag (§A.3.1) selects the claim. An
unordered model queue is a **may-bag**: membership is *may* —
every physical enqueue came from a modeled push so the bag
**over-approximates** the physical queue (a branch-path packet
may not physically exist), and *membership per pop* is
ambiguous — a pop departs one physical message without
identifying which candidate left. Consequence: a claim's
candidate set **cannot shrink per-pop**; it equals the union of
sent-targets over all packets ever pushed that are not provably
drained (§A.3.5 defines provable drain).

- **Ordered queue** → claim packet edges verbatim (identity
  provable).
- **Unordered queue** → **identity-unknown claim with may-alias
  semantics.** Each claimed capability edge (`Read`/`Write`
  auth) and cap edge (handle identity) re-roots to a fresh key
  `⊥ᵢ` carrying a **candidate set** `C(⊥ᵢ)` = the union of
  sent-targets over every packet possibly still queued — the
  whole may-bag **plus every unresolved `pending_send` on `ep`**
  (a pending `SENT` message is physically deliverable — the
  executable model in §A.3.10 caught this omission). `⊥ᵢ` **may-alias every `t ∈ C(⊥ᵢ)`**
  — it is *not* a distinct identity:

| gate on `v` with edge `→⊥{C}`                     | outcome   | why it is safe                                                 |
|---------------------------------------------------|-----------|----------------------------------------------------------------|
| read/write through the cap (`*v`, `v.f`)          | allow     | kind-level authorization — the physical cap is real            |
| `free(v)` on an `Own`-kind edge                   | allow     | count-preserving discharge (A.3.4)                             |
| `&mut t`, `&t`, or plain access, `t ∈ C`          | **reject**| may-alias: `v` may physically hold `t`'s cap — a second cap is unprovable |
| `&mut t` / access to `t ∉ C`                      | allow     | `v` provably cannot hold `t`'s cap                             |
| send/re-send `v` onward                           | allowed iff **every** `t ∈ C` satisfies `R_t ⊇ R_ep` | escape is provable only if all possible targets qualify |
| two claims `v₁:→⊥{C}`, `v₂:→⊥{C}`                 | no conflict | distinct physical deliveries — `⊥`-vs-`⊥` is never aliasing |
| `pkt.sent` named-target un-transit at claim       | skipped   | which source returned is unprovable — see A.3.5                |

**Why may-alias, not distinct:** if `⊥` were a fresh identity
unrelated to `x`/`y`, then `v:→⊥` live while `&mut x` is created
would permit a physically-real second exclusive cap on `x` —
the QA-007 duplication laundered through the unknown key. The
conflict query `edges_to(t)` must therefore treat any live
`⊥`-edge with `t ∈ C(⊥)` as a possible holder of `t`; equally,
`v` can never *falsely* free `x` — `x`'s authority can't die
through `v`'s death, only the possibility-v dies.

**Multiple queued capabilities.** `k` unordered claims produce
`⊥₁..⊥ₖ` — physically `k` distinct delivered messages, so the
`⊥`s never alias each other even with overlapping candidate
sets. The bag's union bounds every candidate set; membership
stays `may` until the endpoint provably drains (§A.3.5).

**Singleton promotion.** If every packet in the may-bag names
the same target `t` — `|C| = 1` — the physically delivered
capability is provably `t`'s (only bag members can physically
exist). The claim may then be installed **verbatim** as `v→t`
and `t` un-transits into ordinary ownership — identity is
proven, not assumed.

**Err-arm reclaims are exempt**: a pending packet is attached
to its `Result` by construction — identity is never in doubt;
`claim_packet` runs verbatim there regardless of queue order.

### A.3.4 The `Own`-edge assumption — stated, and rejected when it fails

Real-label retention for `Own` edges is sound **iff** no
label-sensitive gate ever applies to owned-payload targets —
concretely:

- every `Own`-edge target is a region cell whose owner is the
  immortal heap region — so the send-escape check passes
  identically for any own-label (drift is region-immaterial);
  and
- discharge is count-preserving — each `Own` edge discharged
  once, each physical box freed once (per-path I-owner, §A.1).

**Rejection clause:** if an `Own`-edge target ever resolves to
a non-immortal region (e.g., a scoped-arena allocation, should
the language grow one), that edge falls back to `⊥` under
ambiguity like a capability edge — freeing stays allowed
(count-preserving) but re-send is rejected because a
candidate's region is unprovable. The rule is: *assume nothing
about a label you cannot prove.*

### A.3.5 Named-source transit release — `MayOutstanding`

A named source `t` transit-marked by an ambiguous send has its
cap physically *somewhere*. The release question is not "how
many messages remain" — the bag's membership is `may`, so a
count can't prove absence. It is the structural query:

```
MayOutstanding(t) = U(t) ∪ L(t) ∪ P(t)
```

the set of every *unprovable* residence `t`'s capability could
still physically occupy:

- **U(t) — queued may-packets**: every packet `p` with
  `t ∈ sent(p)` on an endpoint whose bag is *not provably
  drained*. `sent(p)` includes the candidate sets of `⊥`
  edges carried inside `p` — a forwarded `⊥` keeps `t`
  outstanding through the second channel.
- **L(t) — live unknown claims**: every live `⊥ᵢ` edge with
  `t ∈ C(⊥ᵢ)`, wherever the `⊥`-carrying place has moved
  (rebinding, struct fields, aggregates — the edge moves with
  the place, so `L` follows automatically).
- **P(t) — pending sends**: every unresolved `pending_send`
  packet with `t ∈ sent(p)` — the cap may be physically queued
  (`SENT`) or latent in the `Result`'s `Err` slot.

**Provable drain.** A bag is provably empty iff
`pops_π(ep) ≥ may_pushes(ep)` on path `π`. Soundness: physical
enqueues ≤ may-pushes (the bag over-approximates — every real
send corresponds to a modeled packet); physical pops ≤ physical
enqueues (each `Some` consumes one real message). So
`pops ≥ may_pushes` forces `enqueues = may_pushes = pops` —
every may-packet was real and all departed. Per-path pop counts
join by **min** — a pop proven on only one branch is not a pop
on the merged path (under-counting is the safe direction).
Nothing weaker proves drain: `pops = may_pushes − 1` leaves one
real message, and *any* may-packet may be it — so no individual
packet ever leaves `U(t)` except via drain.

**Release rule.** `t`'s transit lifts iff
`MayOutstanding(t) = ∅`. Then `&mut t` proceeds to the ordinary
exclusive checks — which still reject if a *named* claim `v→t`
(singleton-promoted or ordered) is live. Provable residences
are ordinary ownership, not uncertainty; `MayOutstanding`
tracks only what the model cannot place.

**Soundness sketch.** `MayOutstanding(t)` over-approximates the
physical residences of `t`'s cap: a physical cap is (a) queued
→ some modeled may-packet names `t` on a non-drained endpoint —
in `U`; (b) latent in an unresolved `Result` — in `P`;
(c) claimed — a live `⊥` naming `t` (in `L`) or a named edge
(ordinary exclusivity); or (d) dead — no cap exists. Empty set
⟹ cases (c-named) and (d) only ⟹ release cannot expose a live
capability. ∎

### A.3.10 Executable model — `bench/chan4/t/mayoutstanding.py`

A dual simulation validates the release rule: a physical world
tracks every cap's true residence (binding / queue / pending /
dead); the abstract state tracks only model-visible facts.
DFS enumerates every nondeterministic choice — physical
delivery order within a may-bag, branch outcomes, the pending
send's SENT/CLOSED fork — and asserts at every reachable state:

```
abs.borrowable(t)  ⟹  no live physical cap on t
```

Scenarios: competing producers with recv/drop in both orders
(the QA-007 regression shape), a branch-join may-packet that
never physically existed (ghost packet stays restricted — the
accepted false-rejection direction), singleton promotion,
pending-send resolution in both worlds with an intervening
`recv`, and a forwarded `⊥` keeping candidates outstanding
through a second channel. Result: **102 checks / 8 paths, no
violation**; a mutant dropping `L(t)` produces 18 violations —
the harness detects the bug class it exists to catch.

Two corrections the model forced into this appendix: candidate
sets must include unresolved pending packets, and must not
shrink on the pop that provably drains the bag (the just-popped
message's own identity remains ambiguous).

The rule is intentionally coarse — a contested queue may hold
`t` restricted for the rest of its scope. That is the honest
answer: **you cannot recover `x` by name while uncertainty
about its cap's residence survives** — only *a* capability,
held as `⊥`. The common idiom still works: producer sends
`&mut x`, consumer claims into `v` (`⊥{x}`), `v` dies, the
endpoint provably drains → `x` is usable again.

### A.3.6 Pending sends also mark unordered

A live `pending_send` on endpoint `ep` may be physically queued
(the runtime chose `SENT`) but absent from the model queue — a
same-task `recv` before the `match` could physically deliver
it, defeating the ordered-queue assumption. A live pending
send therefore marks `ep` unordered for the pending's
lifetime; the mark lifts on resolution. (§6.3's under-claim
is thereby subsumed into the uniform `⊥` rule.)

### A.3.7 Consequences

- `bchannel T` adopts the rule from the start — no compatibility
  cost (new type, new semantics).
- Unbounded `channel T` has the same hole **today** — the
  counterexample needs no bounded channel. Registered as
  **QA-007** ([issue #25](https://github.com/ceeboozwah/Praetor/issues/25)):
  a preexisting soundness defect, fix proposed as the same
  `unordered`/`⊥`-may-alias rule applied to `channel` recv
  claims under a separate gated change (it rejects
  previously-accepted programs — correctly).

### A.3.8 Coverage verification — joins, spawns, multi-cap queues

| scenario | behavior under the rule |
|---|---|
| join-union of differing local queues | marks `unordered`; later claims get `⊥` over the merged bag's union — the ambiguous `if { send } else { send }` case is covered by the same rule as cross-task sends |
| spawn-merge (child queue supersedes parent view) | marks `unordered`; the parent's later claims take `⊥` over the union — producers spawned in any order are handled |
| multiple capabilities in one bag | each claim gets its own `⊥ᵢ` with the current union as candidates; `⊥ᵢ` never alias each other (distinct physical deliveries), all may-alias the named sources |
| ordered single-producer traffic | verbatim claims, named targets un-transit normally — the common producer/consumer pattern is unaffected |
| pending send outstanding on `ep` | `ep` unordered while pending (A.3.6) |

### A.3.9 Proof obligations — the four operations on `⊥`

For `v` holding `Write→⊥{C}`, each operation is shown incapable
of enabling a second conflicting capability on any `t ∈ C`:

- **create** (the claim itself): installs `v→⊥{C}`; mints no
  named authority. Any subsequent `&mut t`/`&t`/access on
  `t ∈ C` hits the may-alias conflict → rejected. `t`'s transit
  mark is *not* cleared (A.3.5). No path to a second cap.
- **move** (`v2 = v`, pass/store): the `⊥` edge moves with the
  binding, candidates intact — conflict coverage follows the
  edge; no aliasing change.
- **use** (`*v` read/write): kind-authorized; physically `v`
  holds exactly one real exclusive cap — the write is the
  capability doing what a capability does. The model permits it
  on edge *existence + kind*, which is label-insensitive and
  therefore drift-immune. Creates nothing new.
- **drop** (scope end / consume): the `⊥` edge dies — releases
  the may-alias pressure on `C`, but releases **no** transit on
  any `t ∈ C` unless `MayOutstanding(t)` is otherwise empty
  (A.3.5): the
  physically-dead cap might not have been `t`'s — only when no
  unclaimed packet can name `t` *and* no live `⊥` candidates
  contain `t` is `t` provably cap-free. Until then `&mut t`
  stays rejected.

So the only ways a fresh named `&mut t` can be minted — transit
release or a verbatim claim — are both gated on provable
physical exclusivity. ∎

## A.4 Two minimal checker traces

**Trace 1 — conditional send + branch merge.**

```lang
let ch: bchannel box P = bchannel(4);
let p = alloc<P>(heap);            // (p#1 → h, Own, Def)
match send(ch, p) {
    Ok(_)  => { }                  // π_A: pkt{p.edges = [→h]} committed to Q[ch]
    Err(v) => { free(v); }         // π_B: (v#1 → h, Own) discharged; h dead
}
```

Path `π_A`: `h` owned once — by the queued packet (latent).
`I-owner`: Σ=1.
Path `π_B`: `h` freed — `h ∉ Live(π_B)`; invariant vacuous.
Join: `Q[ch]` = longer queue = `[pkt]`; no `Def` edge to `h`
anywhere in `E` (π_A parked it, π_B discharged it) — the merged
state cannot claim `h` is caller-owned on either path.
A subsequent `recv`'s `Some` arm pops `pkt` and claims `(v→h)`
— reachable only on paths where a message physically arrived;
on `π_B` the arm is dead. Post-match join restores `Maybe`.

**Trace 2 — cross-task recv, model queue empty.**

```lang
// consumer sees a channel whose producers the checker never
// saw (e.g., endpoint passed in as an opaque parameter)
match recv(ch) {
    Some(v) => { take(v); }        // opaque box P — movable, not freeable
    None    => { }
}
```

Model queue empty → `v` binds `box P` with no edges. `take(v)`
is a value-level move: legal. `free(v)` would require an `Own`
edge — absent → rejected. Physically `v` owns a real box (some
unmodeled producer sent it); the model simply can't prove it.
If the producer *were* a `spawn`ed task in the same context, its
channel effects merge into `Q[ch]` at spawn-end — the queue is
then `unordered` (§A.3.1), so `v` claims `pkt`'s `Own` edges
verbatim (own labels are kept — A.3.4) but any capability edges
would land as `⊥`. The opaque rule applies only to sends
genuinely outside the checker's view.

**Trace 3 — unordered claim + may-alias rejection (the
regression scenario).**

```lang
let ch: bchannel &mut P = bchannel(4);
let a = spawn { send(ch, &mut x) };   // producer 1
let b = spawn { send(ch, &mut y) };   // producer 2 — spawn merges
join(a); join(b);                     //   mark ch unordered; bag = {pkt_x, pkt_y}
match recv(ch) {
    Some(v) => {
        // claim: v → ⊥{x,y} (Write). Physical v holds one of them.
        *v = P{..};                   // OK — kind-level use
        let r = &mut x;               // REJECT: x ∈ C(⊥ᵥ) — v may
                                      // physically hold &mut x;
                                      // a second cap is unprovable
    }
    None => { }
}
// later: second recv claims ⊥'{x,y} on the other packet —
// ⊥' vs ⊥ never alias. MayOutstanding(x) and MayOutstanding(y)
// stay non-empty while ⊥, ⊥', or undelivered candidates
// persist — transit releases only when the structural set
// empties (A.3.5).
```

Under rev-2/3's claim (verbatim edges), the model could install
`(v→x)` while `v` physically held `&mut y` — the A.3.2 escapes.
Under the candidate-set claim, `&mut x` is rejected while any
live `⊥` may-alias `x`; the name is released only when
`MayOutstanding(x) = ∅` proves `x`'s cap dead (A.3.5).
