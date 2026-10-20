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
- `int mml_chan_send_bounded(c, src)` — **status-returning**:
  `MML_CHAN_SENT` = enqueued, ownership moved to the channel;
  `MML_CHAN_CLOSED` = the channel closed before admission, the
  message was NOT enqueued, and ownership stays with the caller.
- `bound` is **orthogonal to storage layout**: it applies to the
  legacy per-message layout and to every `MML_CHAN_SEG` capacity.
- `mml_chan_send` (void) is unchanged — its legacy contract
  cannot report closure. Bounded-channel callers use the status
  API; the void API on a bounded channel retains the legacy
  close-wake-enqueue behavior for compatibility only.

## Semantics

### Capacity & blocking

1. A channel's *depth* is its live queued-message count.
2. `send_bounded(c, v)`: under the channel lock, while
   `depth == bound && !closed`, the sender blocks on a
   space-available condition variable (`cv_space`). After the
   wait it checks `closed` **first**: a closed channel returns
   `MML_CHAN_CLOSED` without enqueuing — even if space freed.
3. `recv(c, dst)`: unchanged — blocks while `depth == 0 &&
   !closed`; queued messages always drain before closure is
   observed.
4. `close(c)`: sets `closed` and broadcasts **both** `cv_space`
   and `cv_data`. No thread may remain blocked after close.
5. Hard invariants, including during closure:
   `0 <= depth(c) <= bound(c)` at all times, and
   `closed(c) ⇒ no subsequent successful enqueue`. Once the
   queue is closed and drained, no further message can arrive —
   the drain guarantee is unambiguous.

### Ownership transition

6. A successful `send_bounded` copies `esz` bytes into the queue
   and returns `MML_CHAN_SENT`: ownership transfers exactly once.
7. A `MML_CHAN_CLOSED` result transfers nothing: the caller's
   value remains owned by the caller (at the eventual language
   level, a `Result`-like operation would return the original
   owned value — syntax deferred).

### Ordering

6. Enqueue order under the lock defines FIFO. A receiver observes
   a per-channel FIFO stream.
7. Wake order of multiple blocked senders is scheduler-defined;
   their enqueue order is the order they reacquire the lock.
   Ordering guarantees do not extend across concurrently blocked
   senders (same as unbounded channels).

### Wake-ups

8. A successful `recv` signals `cv_space` once (one freed slot
   admits one sender). A successful `send` signals `cv_data` once.
9. Blocked threads wake only on: signal after a matching event,
   broadcast on close, or spurious wakeup (handled by the
   condition loop).

### Ownership

10. A blocked `send_bounded` holds `src` valid until completion
    — the caller cannot observe the block except by time and
    status.
11. `recv` copies `esz` bytes into `dst`; on closed-and-empty it
    writes zeros (unchanged sentinel contract).

### Destruction

12. None — channels live for process lifetime, matching the
    existing contract.

## Invariants under test (t/bound_test.c)

I1. FIFO: single producer → single consumer preserves order at
    bound=7 over 500 messages via `send_bounded`.
I2. Depth bound: `0 <= depth <= bound` always (stats
    cross-check: msgs_peak never exceeds bound).
I3. Close wake: a blocked `send_bounded` returns
    `MML_CHAN_CLOSED`, does not enqueue, and a later drain sees
    only pre-close messages — no late enqueue after a receiver
    observed closed+empty.
I4. Close wakes a blocked receiver on an empty queue — recv
    returns (zeroed payload), no hang.
I5. Multi-producer: 2 producers × bound=8 deliver all messages.
I6. Refill: partial drain below bound unblocks senders — no
    deadlock in the wake path.
I7. Deterministic close race: blocked sender + draining receiver
    + concurrent close — the loser of the admission race always
    reports CLOSED and never enqueues.
I8. Ownership preservation: on MML_CHAN_CLOSED the caller's
    value is still valid and usable.
I9. Both layouts: every invariant holds with MML_CHAN_SEG unset
    and =64.
