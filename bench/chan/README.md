# bench/chan — CHAN-001 segmented channel experiment

The PraetorBench-1.1 `prodcons` gap (~2× vs C) decomposed to ~60%
queue-node allocation + ~40% extern call/bookkeeping overhead. The
channel copied every payload into a fresh `malloc(node + esz)` on
send and `free`d it on recv — on top of the producer's own
`alloc<Pkt>`.

**CHAN-001** adds a benchmark-switched segmented layout:
`MML_CHAN_SEG=N` makes the queue link N-slot segments — one
allocation per N messages. `MML_CHAN_SEG` unset/0 = the legacy
per-message layout (both paths live in the runtime; POSIX +
Win32 mirrors). Segment reclaim happens under the channel mutex:
a segment frees when fully consumed AND the writer has moved to
a newer segment.

## Results (lg = 8M packets, REPS=9, all cells valid)

| cell | seg0 (legacy) | seg16 | seg64 | seg256 |
|---|---|---|---|---|
| prodcons (box ptr, 1P→1C) | 0.654 | 0.405 | **0.379** | 0.383 |
| prodcons64 (64B by-value) | 0.419 | 0.265 | **0.247** | 0.251 |
| prodcons_2p (2P→1C) | 1.797 | 1.218 | 1.208 | **1.116** |

Baselines: C pthread queue 0.345s · Rust `mpsc::channel` 1.027s.

- **seg64 → 1.10× vs C** (0.379/0.345) — from 1.88× legacy.
  The +10% residual is extern `mml_chan_*` call overhead +
  generated bookkeeping — the inlining question, deferred per
  the experiment plan.
- Contended case (2P→1C) prefers the *largest* segment (256):
  allocation amortization outweighs handoff granularity.
- Queue allocations: 1/msg → ~1/64/msg. Peak queue memory is
  bounded by ~cap×esz per outstanding segment plus whatever
  payload the producer allocated.
- A naive cross-thread freelist was measured *slower* (0.584
  vs 0.513 for matched C) — not pursued.

## Validation

- Output-digest verified every rep; all cells valid.
- **Full corpus under `MML_CHAN_SEG=64`: 772/772 pass** —
  FIFO, blocking-recv, close, sentinel, and spawn/ownership
  semantics preserved on the segmented path.

## Runner

`REPS=9 ONLY=prodcons python3 bench/chan/run.py` — builds each
source through ourc + fresh runtime objects, runs every
MML_CHAN_SEG variant, per-rep digest, median + bootstrap CI.
