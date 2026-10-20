# CHAN-004 — Bounded Channels at the Language Level

Specification and checker analysis only. No parser, compiler, or
runtime changes are made by this document. CHAN-003 delivered the
runtime contract; this defines how Praetor source expresses it
with ownership preserved by the checker.

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
  known variants and makes dead arms explicit.
- Runtime (CHAN-003, merged): `mml_chan_new_bounded(esz, bound)`,
  `mml_chan_send_bounded(c, src) -> MML_CHAN_SENT | MML_CHAN_CLOSED`,
  and `mml_chan_send` aborts on bounded channels as a backstop.

The key observation for this design: **a `MML_CHAN_CLOSED` return
leaves the caller's materialized cell untouched** — the value
never moves. Returning ownership on failure needs no copy-back,
no allocation, and no new ownership machinery: the packet simply
isn't committed, and the checker's packet-cut is re-installed at
the `Err` payload binding.

## 1. Ownership transition rules (formal)

Let `c : bchannel T`, `v` a value of compatible payload type.

```
send(c, v) : Result<void, T>

  match send(c, v) {
      Ok(_)   →  pkt(v) ∈ chan_queues(c)      [authority channel-held;
                                               exclusive targets in transit]
      Err(v') →  auth(v') = auth(v)           [authority returns to caller;
                                               v is dead; v' is live]
  }
```

Invariants (identical to the runtime contract):

1. `Ok` transfers ownership **exactly once** — `v`'s binding is
   consumed at the call; the packet commits to the endpoint's
   queue at `Ok`-arm resolution.
2. `Err` transfers ownership **zero times** — the packet never
   commits; its travelled authority re-roots at the `Err`
   payload binding (`claim_packet`-equivalent).
3. Between the call and result resolution, the packet is
   **pending**: `v` is consumed and `v`'s exclusive targets stay
   transit-marked, so neither the sender nor any sibling can touch
   the value's authority. The pending packet is carried *by the
   Result value itself* — it is value-carried authority, resolved
   wherever the Result is examined.
4. `0 ≤ depth(c) ≤ bound(c)` at all times — runtime-enforced
   (CHAN-003); the checker does not model depth.
5. `closed(c) ⟹ send(c,·)` evaluates to `Err` at runtime; the
   checker treats both arms as live (it cannot see runtime
   closure — conservative both-ways checking, same as `Result`
   scrutinees with unknown tag).

```
recv(c) : Option<T>          (on bchannel T)

  Some(v)  →  pkt claimed: queue-front packet installs at v      [T's authority at v]
  None     →  closed ∧ empty: no packet, no payload              [nothing moved]
```

6. `None` means the endpoint is closed AND drained — a
   `close`-and-drain consumer loop terminates with no authority
   outstanding and **no zero-initialized payload**. (See §5: this
   answers the owned-payload zero-fill hazard.)

7. `close(c)` is unchanged: consumes the endpoint. `send`/`recv`
   on a closed endpoint remain use-after-consume diagnostics.

## 2. Language-level API proposal

New type `bchannel T` (spelling under review — see §9.D):

```lang
// Construction — bound is a runtime value ≥ 1 (runtime validates).
let ch: bchannel Pkt = bchannel(8);

// Ops — same names, dispatched on the endpoint's type:
match send(ch, p) {
    Ok(_)  => { /* p committed; its authority is channel-held */ }
    Err(v) => { /* closed: v re-binds p's authority — free(v), retry, or stash */ }
}

match recv(ch) {
    Some(v) => { /* v claims the next packet */ }
    None    => { /* closed and drained */ }
}

close(ch);   // unchanged — consumes the endpoint
```

Type-dispatch table:

| op          | `channel T` (unchanged) | `bchannel T`        |
|-------------|-------------------------|---------------------|
| `send(c,v)` | `void`                  | `Result<void, T>`   |
| `recv(c)`   | `T`                     | `Option<T>`         |
| `close(c)`  | `void` (consume)        | `void` (consume)    |

Why type-directed dispatch rather than new op names: `bchannel T`
is a *distinct type* — a `channel T` value cannot flow where a
`bchannel T` is expected and vice versa (no conversion, no
subtyping). There is no reachable path from a `bchannel`
endpoint to the void-send contract, which is exactly what the
CHAN-003 reviewer required ("cannot accidentally invoke the
existing void-send"). The runtime `mml_chan_send` abort remains
as defense-in-depth below the language. Distinct names
(`send_bounded`/`recv_bounded`) are the documented alternative —
§9.A.

## 3. Checker rules

### 3.1 `send` on `bchannel T`

1. `chan_gate` runs unchanged (endpoint live, untransited, Sync
   hold, foreign-borrow race checks).
2. `packet_for(v)` cuts the packet **unchanged** — `v` consumed,
   exclusive targets `transit`-marked, send-escape and
   region-outlives-endpoint checks apply verbatim.
3. The packet is **not** pushed to `chan_queues`. It parks on the
   result: `ctx.pending_send[result_key] = (endpoint, pkt)`.
   Physically the packet's authority is installed under the
   result place's `Err` payload — so the `Result<void,T>` is an
   ordinary affine enum value for every rule that isn't
   resolution.
4. Expression type: `Result<void, T>` (`Ty::Struct` of the
   canonical monomorphized name — same as any std.result value).

### 3.2 Resolution of a pending send

A `Result<void,T>` carrying a pending packet is resolved by
exactly one of:

| consumer                          | Ok path                        | Err path                              |
|-----------------------------------|--------------------------------|---------------------------------------|
| `match r { Ok, Err(v') }`        | commit pkt → `chan_queues[c]`  | ordinary payload move claims pkt at `v'` (edges re-rooted, `untransit`) |
| `send(c,v)?` (in `fn -> Result<_,T>`) | commit; continue with `void` | early-returns `Err(pkt)` — the packet rides as the caller-visible `Err` payload |
| `let r = send(c,v)` then `match r` | deferred to the match          | deferred to the match                 |

v1 restriction: a **pending** send Result may not cross a
function-call, `return`, `spawn`, or `join` boundary unresolved
— the pending fact is function-local bookkeeping and would be
invisible to a callee's checker world. Diagnostic: "resolve the
`send` result (`match` or `?`) before it leaves this function."
A `Result<void,T>` with an *already-resolved* pending fact
(i.e., an ordinary one built in source) is unrestricted. — §9.C

### 3.3 Unresolved pending — the discard rules

- `send(ch, v);` as a bare statement, `let _ = send(...)`, or
  letting `r` die unexamined: **error** for all payload types —
  "the result of `send` on a bounded channel must be examined —
  an `Err` result means the message was not delivered."
  Rationale: for owned payloads this is a live-authority abandon
  (the existing leak audit would fire anyway); for POD payloads
  it is a silently-undelivered message — a contract violation,
  not a warning. Uniform rule, no payload-type dispatch. — §9.B
- Early `return`/divergence while a pending `r` is in scope: the
  existing abandon audit fires via the pending packet's edges
  installed under `r` — no new machinery.

### 3.4 `recv` on `bchannel T`

1. `chan_gate` unchanged.
2. Pops the model queue front (same as unbounded `recv`) — but
   installs the packet under the result place's `Some` payload
   path rather than at the result root. `match` then performs
   the ordinary enum-payload move to `v`. This reuses
   `claim_packet` mechanics and needs **no new authority
   machinery**: a discarded `Option<box _>` is an ordinary
   affine leak; `None` carries nothing.
3. Empty model queue → `Some` payload is an opaque value of `T`
   (unchanged conservative rule).
4. Expression type: `Option<T>`.

### 3.5 `close` on `bchannel T`

Unchanged — `consume_place`. All post-close op diagnostics
already exist.

### 3.6 Misuse diagnostics (new)

| program                                        | diagnostic                                              |
|------------------------------------------------|---------------------------------------------------------|
| `send(ch, v)` where `ch: channel T`, result used as `Result` | "`send` on `channel T` returns `void` — bounded sends need `bchannel T`" |
| `recv(ch)` bound to non-`Option` on `bchannel` | "`recv` on `bchannel T` returns `Option<T>` — `None` means closed and drained" |
| `send`/`recv` result discarded (`bchannel`)    | §3.3                                                    |
| pending `Result` crosses call/return/spawn/join| §3.2                                                    |
| use of `v` after `send(ch, v)` before result resolves | existing consumed-use diagnostic fires (`v` is consumed at the call) |
| `bchannel`/`channel` used interchangeably      | ordinary type mismatch — no conversion                  |

## 4. Ownership walkthrough (complete trace)

Payload `box Pkt` — the owned-resource case that motivated the
status types:

```lang
let ch: bchannel box Pkt = bchannel(2);
let p = alloc<Pkt>(heap);               // edges: (p#1 → heap#r0, Own)
match send(ch, p) {
    Ok(_)  => { }
    Err(v) => { free(v); }
}
```

Trace:

1. `bchannel(2)` — `ch#1` binds `bchannel box Pkt`; runtime
   `mml_chan_new_bounded(8, 2)`.
2. `alloc` — `p#1` binds `box Pkt`, edge `(p#1 → heap#r0, Own)`.
3. `send(ch, p)`:
   - `chan_gate(ch)`: Sync hold on `ch#1`; `ch` live, untransited.
   - `packet_for(p)`: `cons_set(p#1)`, `kill_set(p#1)`,
     `edges_retain` removes `(p#1 → …)`; `pkt = {ty: box Pkt,
     edges: [("" → heap#r0, Own)]}`.
   - Packet parked: `pending_send[r#0] = (ch#1, pkt)`; result
     `r : Result<void, box Pkt>` holds the authority under its
     `Err` slot.
4. `match r`:
   - **Ok arm**: `chan_queues[ch#1].push(pkt)` — box authority is
     now channel-held. `p` is gone; `heap#r0` reachable only via
     a later `recv` claim. `free(p)` here would be a
     consumed-use error.
   - **Err arm**: payload move re-roots `("" → heap#r0, Own)` at
     `v#1` — `v` owns the box exactly as `p` did. `free(v)`
     discharges it. Double-free impossible: `p` is dead, `v` is
     the sole owner; there is exactly one live `Own` edge to
     `heap#r0` in the whole graph on every path.

At codegen: `mml_chan_send_bounded(ch, &cell)`; `Ok` → tag
`Result.Ok`; `CLOSED` → tag `Result.Err`, payload = `load(cell)`
(the pointer bits — the cell is dead stack storage; one owner).

## 5. `recv` on closed+empty — the zero-fill question

Required finding: **yes, bounded `recv` must return an explicit
status**. The unbounded contract zero-fills `dst` on
closed-and-empty. For `T` containing owned resources
(`box`, `slice`, any struct embedding them), a zero-filled value
is a *forged* owned value — freeing it frees garbage; using it
dereferences null/near-null. Today that hazard is real for
`channel box T` and only avoided by sentinel discipline the
checker doesn't verify.

- `bchannel T`: `recv` returns `Option<T>` — `None` is
  unambiguous. Requires one additive runtime entry:
  `mml_chan_recv_bounded(c, dst) -> 0 | 1` (0 = message copied,
  1 = closed-and-empty, `dst` untouched). POSIX + Win32, ~15
  lines each, additive.
- `channel T` (unbounded): `recv` keeps `T`/zero-fill —
  backward compatible. The owned-payload zero-fill hazard is a
  **pre-existing unbounded limitation**, documented here; an
  opt-in `recv_opt`/status form for unbounded channels is
  deferred as its own milestone (§9.E) rather than silently
  changed.

## 6. Failure-mode analysis

| scenario                                   | source level                              | runtime level                            |
|--------------------------------------------|-------------------------------------------|------------------------------------------|
| send on full bounded channel, open         | `Ok` (blocks then admits)                 | waits on space CV, enqueue               |
| send racing close                          | `Ok` or `Err` — both arms live in checker | `SENT`/`CLOSED` under lock               |
| send on closed channel                     | `Err(v)` — caller retains                 | `CLOSED`, cell untouched                 |
| recv on open non-empty                     | `Some(v)`                                 | dequeue + signal space                   |
| recv racing close                          | `Some` or `None`                          | drains until closed-and-empty            |
| recv on closed, drained                    | `None`                                    | return 1, dst untouched                  |
| sender ignores result                      | checker error (§3.3)                      | n/a                                      |
| value used after `send` before match       | consumed-use error                        | n/a (authority model)                    |
| wrong op for endpoint kind                 | type/arity diagnostics (§3.6)             | `mml_chan_send` abort = last line        |
| `close` while senders blocked              | n/a (compile can't see)                   | all wake; every blocked send → `CLOSED`  |
| `close` while receivers blocked            | n/a                                       | all wake → `None` after drain            |
| bound < 1 at `bchannel(n)`                 | — (not statically required)               | runtime validates ≥1, aborts             |

The one residual gap: a `Some(v)`/`Err(v)` claim assumes the
runtime kept the contract; the checker's model and the runtime's
behavior are verified together by the CHAN-003 harness plus the
new conformance fixtures — there is no cross-verification gap
because the status types make every runtime outcome visible to
the checker world.

## 7. Conformance-test matrix

Positive (must compile; ownership trace explained in the
fixture header):

- **P1** send/recv/close happy path, POD payload; every send
  matched; recv drained via `Some`/`None` loop.
- **P2** `box` payload: `Ok` arm → `recv` claims → `free`;
  `Err` arm → `free(v)` on the caller side. One owner on every
  path.
- **P3** `send(c,v)?` inside `fn -> Result<void, box Pkt>` —
  Err propagation returns the *payload itself* to the caller,
  who frees it.
- **P4** producer/consumer `spawn` pair over `bchannel` —
  Sync-hold serialization and queue merge across join.
- **P5** nested match: `send` inside `match` arm inside a loop;
  per-iteration pending resolution.
- **P6** `Err` arm retries: `match send` → `Err(v)` →
  `match send(c2, v)` — re-send the retained value (re-cut at
  each call).
- **P7** `channel T` unbounded program unchanged — regression
  that existing corpus (771–777 et al.) still passes.

Negative (each documents its violated invariant):

- **N1** `send(ch, v);` bare statement (POD) → undelivered
  message hidden: "result must be examined."
- **N2** `let r = send(ch, p); /* no match */` (owned) →
  live-authority abandon / unexamined result.
- **N3** use `p` after `send(ch, p)` → consumed.
- **N4** `match send { Ok(_) => use(v) }` — Err binding named in
  Ok arm / use `v` in the wrong arm → unbound/consumed.
- **N5** `let x: Result<..> = send(uc, v)` on `channel T` →
  void-result misuse.
- **N6** `let v: Pkt = recv(bc)` skipping `Option` → type error.
- **N7** pending `Result` `return`ed / passed to `f(r)` /
  spawned-out unresolved → boundary restriction.
- **N8** `bchannel`↔`channel` assignment/argument mix → type
  mismatch, no conversion.
- **N9** `close(ch)` then `send(ch, v)` → use-after-consume.
- **N10** `free(v)` in `Ok` arm after send → consumed-use
  (authority is channel-held, not caller-held).
- **N11** `recv` `Some` payload used in `None` arm → unbound.
- **N12** send-escape / region-outlives-endpoint rules on
  `bchannel` identical to `channel` (reuse of `packet_for`
  checks, e.g. sending `&mut` target dying before endpoint).

## 8. Staged implementation plan

Rollback boundary after every stage; stages 1–2 are inert
without 3–4, so partial lands are safe.

| stage | change                                                                    | surface                          | rollback                                  |
|-------|---------------------------------------------------------------------------|----------------------------------|-------------------------------------------|
| S1    | `mml_chan_recv_bounded(c,dst)->int` + `mmlrt.h` + `gen-embed`               | posix + win32 runtime            | additive entry; revert file               |
| S2    | `Ty::BChannel(Box<Ty>)`, parser (`bchannel T`, `bchannel(n)`), print/mono   | frontend                          | dead variant without S3                   |
| S3    | checker: `chan_gate`/`chan_inner` generalize to `BChannel`; send → pending `Result<void,T>`; recv → `Option<T>`; resolution in `match`/`Expr::Try`; §3.3/§3.6 diagnostics | `check.rs` (s0)                   | new rules fire only on `BChannel` type    |
| S4    | lowering: `bchannel(n)`→`mml_chan_new_bounded`; `send`→`send_bounded`+Result build; `recv`→`recv_bounded`+Option build | `gen.rs` (s0)                     | gated on `BChannel` type                  |
| S5    | selfhost mirror: `selfhost/ir.lang` (+ checker parity), `embed.lang` regen | selfhost                          | s2→s3 `.ll` fixpoint gate                 |
| S6    | fixtures P1–P7/N1–N12, corpus wiring both platforms, docs                  | `native-tests/`, CI               | revert fixtures                           |

Semantic parity requirement: s0 and selfhost must emit identical
diagnostics on every N-fixture and identical `.ll` at s2→s3 for
every P-fixture.

Estimated new checker surface: the pending-packet table +
resolution hooks; `packet_for`, `claim_packet`, `chan_gate`,
transit, and the enum machinery are all reused.

## 9. Unresolved decisions — for architectural review

- **A. Op naming.** Recommended: type-directed `send`/`recv`
  (dispatch on endpoint kind; a `bchannel` can't reach the void
  contract). Alternative: distinct `send_bounded`/`recv_bounded`
  builtins — more visible at call sites, more vocabulary,
  identical expressive power. The reviewer's "separately checked
  construction and send operation" is satisfied either way; the
  question is ergonomics vs. explicitness.
- **B. POD discard.** Recommended: uniform error — the send
  contract is delivery; an unexamined result hides failure.
  Alternative: error only for owned payloads (leak), warning for
  POD. Uniformity argued for simplicity and honesty.
- **C. Pending escape.** v1 restricts unresolved pending Results
  to function-local resolution. Relaxing it needs the pending
  fact to be value-carried across call/join boundaries —
  feasible (it is authority, like any enum payload) but widens
  every boundary's contract. Defer until usage demands it.
- **D. Type spelling.** `bchannel T` proposed (consistent with
  `bstr`-style compact builtins); alternatives `bounded channel T`
  (two-word modifier) or `channel T, n` with bound-in-type.
  Bound-in-type was rejected: capacity is a runtime quantity;
  two endpoints of different bounds should unify.
- **E. Unbounded `recv` Option form.** The zero-fill hazard on
  `channel box T` is pre-existing; adding `recv_opt` for
  unbounded channels is a separate, additive milestone —
  deliberately not bundled here.
- **F. `?` on `Result<void,T>`.** `?` propagates `Err(v)` —
  the *payload* — so the enclosing fn must return
  `Result<_,T>`. Mechanically sound (delivery failure carries
  the undelivered value home); semantically unusual enough to
  confirm. A dedicated `enum Deliver<T> { Sent, Closed(T) }`
  would be self-documenting but loses `?` unless the desugar
  generalizes to variant names.
- **G. Bound value.** Runtime argument (validated ≥1) proposed.
  Compile-time-constant bounds would allow static checks but add
  a type-level or const-eval requirement for no measurable
  benefit — the runtime guard already exists.
