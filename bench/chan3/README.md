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
legacy contract unchanged.

## Invariant harness

`t/bound_test.c` runs against mmlrt.h directly (portable — only
mml_task/mml_sleep/mml_chan, `_Atomic` flags), covering:

- I1 FIFO at bound=7 over 500 msgs via send_bounded
- I2 depth ≤ bound (stats cross-check: peaks equal bounds)
- I3 close-woken blocked send returns CLOSED, no late enqueue
- I7 deterministic close race: blocked sender + drain + close —
  loser reports CLOSED regardless of drain timing
- I8 ownership preserved on CLOSED (caller retains value)
- I4 close wakes blocked receiver; I5 two producers; I6 refill
- I9 every invariant under MML_CHAN_SEG unset and =64

Local: `clang -O2 -Icompiler/runtime t/bound_test.c rt_common.o
rt_posix.o -o bound_test -lpthread && ./bound_test &&
MML_CHAN_SEG=64 ./bound_test` → `RESULT: PASS` both layouts.
CI: platform workflow runs it under both layouts in probe-linux
and probe-windows.
