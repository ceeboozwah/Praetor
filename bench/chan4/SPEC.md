# CHAN-004 — Bounded Channels at the Language Level

Specification and checker analysis only. No parser, compiler, or
runtime changes are made by this document. CHAN-003 delivered the
runtime contract; this defines how Praetor source expresses it
with ownership preserved by the checker.

**Revision 2** — incorporates the architectural review ruling:
type-directed ops, distinct `bchannel T`, uniform rejection of
ignored send results, function-local pending in v1,
`Result<void,T>` / `Option<T>`, runtime-validated capacity — all
accepted (§12). This revision adds the epistemic model (§1), the
full pending-state transition table (§4), the precise `?`
mechanics (§5), conservative `Some`/`None` analysis (§6), the
exactly-one-owner invariant (§2), the expanded conformance
matrix (§10), and registers the unbounded zero-fill hazard as
QA-006 (§8).

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

- The packet's authority is parked on the Result value for the
  duration of the checker's uncertainty — physically it is in
  exactly one place already; the model keeps it under the
  Result's `Err` payload slot so every path sees a consistent
  owner.
- The pending fact is **compile-time bookkeeping only** — a
  `map place-key → (endpoint, Packet)` in the checker context,
  same shape as `chan_queues`. Nothing at runtime knows or cares.
- Commit-to-queue at the Ok arm is a *model* operation; the
  physical enqueue happened earlier. Since the model queue is
  already an approximation (cross-task order is unknowable),
  this costs nothing — but it produces one sharp edge
  (§6.3): an intervening same-task `recv` before the match
  under-claims (safe direction).

## 2. Ownership rules and the exactly-one-owner invariant

**Invariant I-owner.** For every live owned resource `R` (a
region target reachable by `Own` authority): the merged checker
state on every path contains **exactly one** live `Own`-kind
edge targeting `R`. The packet is a single cursor — the edge
lives in exactly one of {sender binding, pending Result's Err
slot, model queue, receiver binding} at every instant.

Transitions that preserve I-owner:

```
send(c, v) : Result<void, T>     [c : bchannel T]

  call        →  packet_for cuts v's edge; v consumed;
                 edge parked under r.Err (pending fact on r).
                 I-owner: 1 edge, under r.

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
call, attached to the Result value's place. The packet's edges
are installed under `r`'s `Err` payload slot, so `r` is an
**ordinary affine `Result<void,T>`** for every rule except
resolution — that is what makes the lifecycle small.

| transition                                   | rule                                                                                                      |
|----------------------------------------------|-----------------------------------------------------------------------------------------------------------|
| `let r = send(c,v)`                          | `r` binds `Result<void,T>` + pending P.                                                                    |
| `r2 = r` / `let r2 = r` (move)               | Pending re-keys to `r2`; `r` consumed — ordinary affine move.                                              |
| `match r` in `Ok` arm                        | P consumed; `pkt` pushed to `chan_queues[ep]`; `r` consumed.                                               |
| `match r` in `Err(v')` arm                   | P consumed; `claim_packet(v', pkt)` — edges re-root, `untransit`; `r` consumed.                            |
| `send(c,v)?`                                 | `?`'s own desugar match resolves P (§5) — no pending escapes.                                              |
| assignment `r = send(c,v)` over live pending `r` | **error** — overwriting an unexamined pending Result discards the possibly-undelivered packet (§4.2).    |
| `return r` / `f(r)` / `spawn`/join crossing  | **error** — pending is function-local bookkeeping in v1; resolve it first (§4.3).                          |
| `break`/`continue`/`return` with live pending `r` | **error** — abandon audit: `r`'s Err payload holds live `Own` authority (existing machinery).           |
| branch join                                  | **no special rule** — consumed-state merging applies unchanged (§4.1).                                     |
| loop back-edge                               | same as join: pending may persist only inside a live `r`; overwriting or abandoning it errors (§4.2).      |
| `close(c)` while `r` pending on `c`          | legal — Ok arm commits to the model queue of a consumed endpoint (harmless bookkeeping); Err arm claims normally. |

### 4.1 Branch joins — why no special rule exists

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
  `chan_queues[ep]` per existing queue-merge (a send that ran on
  either path may physically be in the queue).

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
- **Err path**: `e` claims the packet by ordinary enum-payload
  move (edges re-root at `e`, `untransit`); `return Err(e)`
  re-wraps `e`'s authority as the function's return payload —
  an *ordinary* `Result<_,T>::Err`, pending-free.

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

### 6.2 The `Some` claim rule

- Model queue non-empty → pop front, `claim_packet(v, pkt)`,
  `untransit` targets (existing).
- Model queue empty → `v` binds an **opaque `T`** — no edges,
  `Prov::Unknown`/payload default. Under-approximation is the
  safe direction: a program that needs the recv'd authority's
  edges (e.g., `free(v)` on a claimed `box`) gets them when the
  checker saw the send; otherwise it may be conservatively
  rejected — never unsafely permitted.
- Interchangeability: packets of payload `T` carry equivalent
  authority *shapes*; whichever physical message arrives, its
  delta installs identically. The checker may pair a recv with a
  different packet than the runtime would — I-owner still holds
  because each packet is a complete, disjoint edge set.

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

### 6.4 `None` and drain guarantees

`None` establishes no permanent fact — a different endpoint or a
revived producer could refill the channel (the checker doesn't
track "closed" as endpoint state beyond `close`'s consume). The
harness-level guarantee (CHAN-003: after close + drain, no late
message arrives) is a runtime contract; the checker need only
model `None` as "no packet this time."

## 7. Misuse diagnostics

| program                                              | diagnostic                                                                    |
|------------------------------------------------------|-------------------------------------------------------------------------------|
| `send(c,v);` bare statement, any payload             | "the result of `send` on a bounded channel must be examined — `Err` means the message was not delivered" |
| `let _ = send(c,v)` or `r` dies unexamined           | same — discard of a pending Result                                            |
| `send(c,v)` result used as `Result` on `channel T`   | "`send` on `channel T` returns `void` — bounded sends need `bchannel T`"      |
| `recv(c)` bound to non-`Option` on `bchannel`        | "`recv` on `bchannel T` returns `Option<T>` — `None` means closed and drained" |
| use `v` after `send(c,v)`                            | existing consumed-use                                                         |
| pending `r` → `return`/`f(r)`/spawn/join             | §4.3                                                                          |
| pending `r` overwritten / abandoned                  | §4.2 / existing abandon audit                                                 |
| `bchannel` ↔ `channel` interchange                   | ordinary type mismatch                                                        |
| `free(v)` in `Ok` arm                                | consumed-use (authority is channel-held)                                      |
| `?` with mismatched `Err` type                       | existing `?` diagnostic                                                       |

## 8. QA-006 — registered: unbounded recv zero-fill on owned payloads

**Registered as a separate memory-safety issue (QA-006);
semantics unchanged in CHAN-004.**

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
   SENT/CLOSED; the checker parks P on `r` — edge installed under
   `r.Err`. **I-owner: 1** (under `r`).
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
| P11  | recv drain loop `Some`/`None`; last `None` after `close`                 | status-driven consumer                     |
| P12  | `Some` arm on model-empty queue (cross-task producer)                    | conservative opaque claim                  |
| P13  | self-send then same-task `recv` before match (documented §6.3)           | under-approximation is rejection-safe      |
| P14  | `&mut` capability payload: Err arm restores the capability binding       | untransit + capability re-root             |

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

## 11. Staged implementation plan — both stages

Rollback boundary after every stage; S1–S2 inert without S3–S4.

| stage | change                                                                                     | surface                | rollback                          |
|-------|--------------------------------------------------------------------------------------------|------------------------|-----------------------------------|
| S1    | `mml_chan_recv_bounded(c,dst)->int` + `mmlrt.h` + `gen-embed`                                | posix + win32 runtime  | additive entry                    |
| S2    | `Ty::BChannel(Box<Ty>)`, parser (`bchannel T`, `bchannel(n)`), print/mono                    | frontend               | dead variant without S3           |
| S3    | checker: `chan_inner`/`chan_gate` on `BChannel`; send→pending `Result<void,T>`; recv→`Option<T>`; pending table + resolution in `match`/`Expr::Try`; §4–§7 diagnostics | `check.rs` (s0)        | rules fire only on `BChannel`     |
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
