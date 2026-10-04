# CHANNEL-PROPOSAL — extension delta against SPEC v0 (+Amendments A, B)

Status: **accepted**. The corpus (scenarios 192–211) held all recorded
expectations with 0 missed unsafe / 0 over-restrictive under
`region+cap+affine`; merged into `SPEC.md` as Amendment C.

## Claim under test

```text
A channel transfers authority; it does not duplicate authority.

sender owns x
    ↓ send(ch, x)         sender state(x) = Consumed / authority in transit
    ↓
channel carries a suspended authority packet
    ↓ recv(ch)
receiver gains fresh Live authority
```

The unifying view under test:

```text
call     = immediate graph transform      G → G'
task     = suspended graph transform      (applied at join)
channel  = transported graph transform    (applied across tasks)
```

## Syntax delta

```text
let ch: channel<File> = channel()   — channel<T> in type position (contextual
                                      ident, like atomic<T>); channel() builtin
send(ch, x)                         — builtin: transfers payload authority
let g = recv(ch)                    — builtin: receives a packet
close(ch)                           — builtin: consumes the channel endpoint
```

No keywords: `channel`/`send`/`recv`/`close` are call names. No `Sender<T>`/
`Receiver<T>` split — endpoint duplication is not forced by the corpus yet.

## Proposed semantics

**C1. send is affine transfer.** `send(ch, v)` on a non-Copy `v` consumes it
(`Op::Move` path) — `use v` after send is `use-after-consume`; `send(ch, v)`
twice is `double-consume`. Copy payloads read plainly. A payload carrying an
embedded *alias* edge (`b.ptr = &x`) is `send-escape` — the promote closure
rule applied at the transport boundary. Embedded *capability* edges
(`b.ptr = handle x`) travel fine — they can go stale, never dangle.

**C2. capability transfer.** `send(ch, &mut x)` / `send(ch, r)` sends the
*authority*, not the storage: `x` enters `in_transit` — authority's location
moves to the channel. Any access to `x` while in transit is
`use-after-send`. The sending ref binding is consumed. This is the topology
change:

```text
authority edge:  sender → x     becomes    channel → x    becomes   g → x
```

The state machine never gains a state — only the edge's holder changes.

**C3. recv installs authority.** `let g = recv(ch)` pops the in-flight packet:
`g` gains the packet's edges/flags (a received handle is a handle; a received
`&mut` edge gives write-through). Receiving a packet whose target already
died is `stale-handle`. `recv` on an empty channel yields an opaque Live
value (v1 makes no claim about the payload).

**C4. close is consume.** `close(ch)` is `Op::Close` on the ordinary consume
path: `send` after close is `use-after-consume`, double close is
`double-consume`. Since `close` consumes the endpoint, `recv` after close is
also `use-after-consume` in v1 — drain semantics deferred entirely
(stricter than the draft's "accepted"; the endpoint is gone).

**C5. channel ops are synchronized domain access.** `send`/`recv`/`close`
record `Sync({ch})` on the channel — the same runtime-serialized hold
kind mutexes introduced. Two tasks sending/receiving on one channel do
not race on the endpoint.

**C6. `in_transit` is global authority location.** A child's send merges the
sent target into the parent's `in_transit` at capture (authority left the
task world); a receiver's `recv` releases it in that env. A sibling writing
the target without receiving is `use-after-send`.

**C7. Guards cannot cross the boundary.** `send(ch, g)` where `g` is a lock
guard is `guard-escape` — same rule as `return g`.

## Non-goals (explicit)

- buffering semantics, select, timeouts, rendezvous-vs-buffered
- receive-after-close payload guarantees
- `Sender`/`Receiver` endpoint types
- provenance tracking of WHO sent which packet (recv is a bag, not a
  matched FIFO, statically)
- deadlock/blocking analysis on empty recv

## Cost budget (frozen before implementation)

```text
new syntax:              channel<T> + channel()/send/recv/close builtin names
new programmer states:   +0 target  (in_transit is a location set, not a Temp)
new general rules:       +1–3       (non-channel-op, send-escape; close/…
                                     reuse existing consume kinds)
new hold kinds:          +0 target  (Sync(ch) reuses the mutex hold kind)
new machinery:           chan_queues (packets), in_transit, pending_recv
reuse:                   affine consume, region edge closure, cap edges,
                         task holds, Sync domains, stale-handle
```

Target formula:

```text
Message Passing = Authority Transfer + Region Validity
```

## Measured outcome

```text
new syntax:              channel<T> (contextual ident, type position)
                         + channel()/send/recv/close builtin names — as
                         budgeted; zero new tokens, zero new keywords
new programmer states:   +0 — in_transit is an authority-location set,
                         not a Temp; Live|Consumed|Maybe unchanged
new general rules:       +3 — non-channel-op, send-escape, use-after-send
new hold kinds:          +0 — channel ops reuse Sync({ch}) from the mutex
                         round (the endpoint is a synchronization domain)
new machinery:           chan_queues (Packet graph deltas), in_transit,
                         pending_recv, claim_packet — the transported
                         graph transform
reuse confirmed:         affine consume (send/close/double-close),
                         alias-edge closure (send-escape), cap edges
                         (handles travel as identity), task holds +
                         Sync domains (endpoint ops don't race), dead-cap
                         tracking (stale ticket at recv), task spawn
                         merge (channel state is shared memory)
```

Result: **F = 115 caught / 0 missed / 0 over-restrictive** across 211
scenarios; 1477/1477 recorded expectations held. The unification survived:
a channel really is a transported graph transform, and `in_transit` needed
to be topology (a set), not a state.
