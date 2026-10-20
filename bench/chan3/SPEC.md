# CHAN-003 — Bounded Channel Specification (experimental)

Status: **specification + experimental runtime**. No language or
checker changes. The surface is two runtime entry points; the
invariant harness exercises them directly against mmlrt.h on
both platforms. Language-level syntax (e.g. `channel(T, n)` or a
distinct bounded type) is a separate, later decision.

## Motivation

CHAN-002 established that the segmented queue's memory growth is
legitimate backlog: a faster producer deepens an *unbounded*
queue. Backpressure is a different concern from storage layout —
it belongs to an explicitly bounded channel, not to accidental
allocation latency. This spec defines that mechanism
independently, per the unbounded/bounded separation principle.

## Definitions

- `mml_chan *mml_chan_new(size_t esz)` — unbounded (unchanged).
- `mml_chan *mml_chan_new_bounded(size_t esz, size_t bound)` —
  bounded; `bound ≥ 1` is the maximum number of queued messages.
  `bound == 0` aborts with `mml: channel bound must be >= 1`
  (rendezvous is deferred; v1 bounds are ≥1).
- `bound` is **orthogonal to storage layout**: it applies to the
  legacy per-message layout and to every `MML_CHAN_SEG` capacity.

## Semantics

### Capacity & blocking

1. A channel's *depth* is its live queued-message count.
2. `send(c, v)`: under the channel lock, while `depth == bound
   && !closed`, the sender blocks on a space-available condition
   variable (`cv_space`). On wake with `depth < bound` it
   enqueues; on wake with `closed` it enqueues anyway (see §4).
3. `recv(c, dst)`: unchanged — blocks while `depth == 0 &&
   !closed`; queued messages always drain before closure is
   observed.
4. `close(c)`: sets `closed` and broadcasts **both** `cv_space`
   and `cv_data`. No thread may remain blocked after close.
5. A close-woken `send` still enqueues its message exactly once.
   Consequence: depth may transiently exceed `bound` by at most
   the number of senders blocked at close. Rationale: `send`'s
   signature is `void` and ownership of `v` is consumed — there
   is no channel for returning the value or reporting failure.
   "Queued and drainable" is the only loss-free outcome.

### Ordering

6. Enqueue order under the lock defines FIFO. A receiver observes
   a per-channel FIFO stream.
7. Wake order of multiple blocked senders is scheduler-defined;
   their enqueue order is the order they reacquire the lock.
   Ordering guarantees do not extend across concurrently blocked
   senders (same as unbounded channels).

### Wake-ups

8. A successful `recv` signals `cv_space` once (one freed slot
   admits one sender). A `send` signals `cv_data` once.
9. Blocked threads wake only on: signal after a matching event,
   broadcast on close, or spurious wakeup (handled by the
   condition loop).

### Ownership

10. `send` copies `esz` bytes out of `src` under the lock; the
    caller's obligation ends at copy completion (same as
    unbounded). A blocked `send` holds `src` valid until
    completion — the caller cannot observe the block except by
    time.
11. `recv` copies `esz` bytes into `dst`; on closed-and-empty it
    writes zeros (unchanged sentinel contract).

### Destruction

12. None — channels live for process lifetime, matching the
    existing contract.

## Invariants under test (t/bound_test.c)

I1. FIFO: single producer → single consumer preserves order at
    bound=7 over 500 messages.
I2. Depth bound: while any consumer lags, depth never exceeds
    bound (observed via a producer-progress counter and stats).
I3. Close wakes a blocked sender exactly once — the send
    completes, the sender does not hang, and the message is
    drainable.
I4. Close wakes a blocked receiver on an empty queue — recv
    returns (zeroed payload), no hang.
I5. Multi-producer: 2 producers × bound=8 deliver all messages.
I6. Refill: partial drain below bound unblocks exactly the
    senders needed — never more than available space.
I7. Both layouts: every invariant holds with MML_CHAN_SEG unset
    and =64.
