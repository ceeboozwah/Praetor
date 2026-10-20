# bench/chan3 — CHAN-003 bounded channel (experimental)

Spec: `SPEC.md`. Runtime: `mml_chan_new_bounded(esz, bound)` +
`mml_chan_send_bounded(c, src)` in both `mmlrt_posix.c` and
`mmlrt_win32.c` — `bound ≥ 1` = max queued messages, orthogonal
to `MML_CHAN_SEG` storage layout. No language/checker changes.

Semantics: `send_bounded` blocks at depth==bound, then checks
`closed` first — returns `MML_CHAN_SENT` (enqueued, ownership
moved) or `MML_CHAN_CLOSED` (not enqueued, caller keeps the
value). Hard invariants: `0 ≤ depth ≤ bound` always, and closed
channels admit no further enqueues — once closed and drained,
no late message can arrive. The void `mml_chan_send` keeps its
legacy contract on unbounded channels; on a bounded channel it
aborts with `mml: bounded channel requires mml_chan_send_bounded`
— the invariants are enforced, not documented-and-bypassable.

## Invariant harness

`t/bound_test.c` runs against mmlrt.h directly (portable — only
mml_task/mml_sleep/mml_chan, `_Atomic` flags), covering:

- I1 FIFO at bound=7 over 500 msgs via send_bounded
- I2 depth ≤ bound (stats cross-check: peaks equal bounds)
- I3 close-woken blocked send returns CLOSED, no late enqueue
- I7a close-then-drain: blocked sender always reports CLOSED
- I7b drain-then-close: SENT (drainable) or CLOSED (no late
  message) — each verified consistent
- I8 ownership preserved on CLOSED (caller retains value)
- I9 void mml_chan_send on a bounded channel aborts with the
  documented diagnostic (`bound_test misuse` asserts it)
- I4 close wakes blocked receiver; I5 two producers; I6 refill
- I10 every invariant under MML_CHAN_SEG unset and =64

Local: `clang -O2 -Icompiler/runtime t/bound_test.c rt_common.o
rt_posix.o -o bound_test -lpthread && ./bound_test &&
MML_CHAN_SEG=64 ./bound_test` → `RESULT: PASS` both layouts.
CI: platform workflow runs it under both layouts in probe-linux
and probe-windows.
