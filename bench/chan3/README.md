# bench/chan3 — CHAN-003 bounded channel (experimental)

Spec: `SPEC.md`. Runtime: `mml_chan_new_bounded(esz, bound)` in
both `mmlrt_posix.c` and `mmlrt_win32.c` — `bound ≥ 1` = max
queued messages, orthogonal to `MML_CHAN_SEG` storage layout.
No language/checker changes; this is an experimental runtime
surface pending a syntax decision.

Semantics: `send` blocks at depth==bound until a `recv` frees a
slot or `close` broadcasts. Close-woken senders still enqueue
(send is void — the value can't be returned), so depth may
transiently reach bound + #blocked-senders. FIFO and the
closed-drain contract are unchanged.

## Invariant harness

`t/bound_test.c` runs against mmlrt.h directly (portable — only
mml_task/mml_sleep/mml_chan), covering:

- I1 FIFO at bound=7 over 500 msgs
- I2 depth ≤ bound (stats cross-check: peaks equal bounds)
- I3 close wakes a blocked sender; message drainable
- I4 close wakes a blocked receiver (zeroed payload)
- I5 two producers share bound=8
- I6 refill unblocks senders, no deadlock

Local: `clang -O2 -Icompiler/runtime t/bound_test.c rt_common.o
rt_posix.o -o bound_test -lpthread && ./bound_test &&
MML_CHAN_SEG=64 ./bound_test` → `RESULT: PASS` both layouts.
CI: the platform workflow compiles and runs it under both
layouts in probe-linux and probe-windows.
