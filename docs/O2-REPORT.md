# O2 Investigation — Waiter-Aware Signal Elision

Status: **closed without merge** — demonstrated mechanism, not an
approved production optimization. Evidence-only record.

Raw repetitions + variant sources: `bench/o2-evidence/`
(408 measured reps across `matrix.csv` + `deep.csv`; reference
implementations `variant-b-instrumented.c`, `variant-c-elision.c`,
`variant-c-win32-draft.c` — draft, incomplete wiring).

## Variants

All built from identical `mmlrt_posix.c` at O1 merge `cae2468`,
same `-O2` flags, same workload binary, same host:

- **A** — O1 baseline: fixed ring, unconditional `pthread_cond_signal`
- **B** — A + waiter-registration counters (`n_cv_waiters` /
  `n_space_waiters`, ++/−− around `pthread_cond_wait` under `c->mu`),
  signals still unconditional
- **C** — B + elision: `if (n_cv_waiters) signal(&cv)` /
  `if (n_space_waiters) signal(&cv_space)`; `close()` broadcasts
  remain unconditional

## The synchronization invariant (why counters, not a flag)

Safe elision requires: *whenever a thread's wait predicate is made
true, if a thread could be blocked on that predicate's CV, the CV
must be signaled.* The count check and the predicate write must
happen under the same lock — here `c->mu` — so no thread can
transition into the wait between the predicate change and the
registration check: entering the wait requires holding `c->mu`
first (the caller does), so either (a) the waiter registered before
the signaler acquired the lock → count > 0 → signal sent, or
(b) the waiter hasn't reached the wait → it will re-test the
already-updated predicate and never block.

A **single boolean "waiter present" flag is not safe with multiple
waiters**: with N blocked threads, a broadcast/point wake releases
one; that thread clears the flag on exit while another remains
blocked — subsequent eligible signals see flag==0 and get elided →
lost wakeup on the still-blocked waiter. Tracking the *count*
handles N waiters correctly. (Broadcast-every-time avoids this but
defeats the point.) This is evidence, not a formal proof over all
interleavings — the claim rests on the shared-mutex registration
protocol, exercised by TSan + 26/26 contract checks.

## Measurements — 8M-item lg release (interleaved, indep-sample
## bootstrap, 20K resamples, median-diff CIs)

Deep pass (10 reps/cell; cap-1: 5 reps, sm+md):

| cap,scale | A_med | B_med | C_med | C−A 95% CI | C−B 95% CI |
|---|---|---|---|---|---|
| 1,sm | 1.559 | 1.616 | 1.562 | [−0.046,+0.148] | [−0.364,+0.087] |
| 1,md | 5.939 | 6.749 | 6.311 | [−0.135,+0.821] | [−0.962,+0.091] |
| 16,sm | 0.127 | 0.125 | 0.129 | [−0.004,+0.006] | [−0.003,+0.010] |
| 16,md | 0.516 | 0.504 | 0.518 | [−0.015,+0.027] | [−0.001,+0.043] |
| 16,lg | 2.014 | 1.978 | 2.016 | [−0.091,+0.167] | [−0.052,+0.192] |
| 256,sm | 0.048 | 0.047 | 0.048 | [−0.002,+0.002] | [−0.002,+0.002] |
| 256,md | 0.182 | 0.184 | 0.183 | [−0.005,+0.005] | [−0.006,+0.004] |
| 256,lg | 0.716 | 0.710 | 0.706 | [−0.029,+0.014] | [−0.017,+0.020] |
| 4096,sm | 0.035 | 0.036 | 0.033 | [−0.004,−0.001] | [−0.005,−0.001] |
| 4096,md | 0.132 | 0.140 | 0.123 | [−0.013,−0.001] | [−0.020,−0.009] |
| 4096,lg | 0.498 | 0.532 | 0.487 | [−0.031,+0.003] | [−0.069,−0.019] |

Scales used sm=500K / md=2M / lg=8M items (controlled experiment
sizes, not canonical suite scales). Exploratory 3-rep sweep in
`matrix.csv` (108 reps) showed the same pattern with wider CIs.

## Findings

- **Elision works (B−vs−C):** CIs exclude 0 at cap-4096 on every
  scale — ~4–13% pure signal-elision benefit.
- **Tracking costs (B−vs−A):** the two counters + branches cost
  ~3–7% at cap-4096 (B 0.532 vs A 0.498 lg) — about half the win.
- **Net O2 vs O1 (the merge question):** ~2–6% at cap-4096;
  reliable at sm/md, **marginal at lg (CI touches 0)**; neutral at
  caps 1/16/256.
- **cap-16 regression:** unaffected (+0.002, CI spans 0) — O2 does
  not recover it; honestly reported, not tuned-for.
- **No statistically detectable regression** in tested cells — the
  CIs include 0, which bounds the effect size within measurement
  resolution but does not prove absence of regression.

## Signal accounting (cv_space side, MML_CHAN_STATS=1)

| cap | attempts | sent | elided |
|---|---|---|---|
| 4096 | ~8.0M | 6,291,438 | 1,708,563 (21%) |
| 16 | ~8.0M | 7,999,406 | 595 (0.007%) |

Mechanism confirmed: in the contention-bound regime a space-waiter
is almost always registered → elision saves nothing but pays the
check; at high capacity ~1 in 5 space signals is safely skipped.
*Limitation:* the `cv` data-notification side was not separately
wired in the stats path (aggregate matched the space side;
cv attempts ≈ 8M, direction: consumers rarely wait at high cap so
most were likely elided — unquantified).

## Safety validation (POSIX, variant C)

- `s4/ring_selftest` — **26/26**, incl. per-identity MPMC
  exactly-once (80K msg), close races, drain-after-close, rejection,
  wraparound, esz 0/1/4/8/big, fallback
- ASan + UBSan — all modes clean
- TSan — wrap/mpmc/close clean
- **Windows: incomplete** — `variant-c-win32-draft.c` has counters
  declared but wait-loop wiring unfinished; never compiled. Any
  future revival must mirror the POSIX protocol identically and
  pass `corpus-windows`.

## Verdict

O2 is a demonstrated optimization mechanism — elision removes
`pthread_cond_signal` call-site work and, where libc forwards it,
the `ulock_wake` syscall (skipping a call does not itself prove a
kernel operation would have occurred; the timing evidence shows a
real benefit either way) — but the **net benefit over O1
is ~2–6%, confined to cap-4096 and marginal at the largest scale**,
in exchange for shared mutable state and a correctness invariant in
the hottest lock path of a safety-sensitive primitive. Rejected for
merge by the benefit-vs-complexity criterion; the investigation
closes here. If a future workload makes cap-4096 contention the
dominant cost, `variant-c-elision.c` is the tested reference.
