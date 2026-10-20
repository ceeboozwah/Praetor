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
a newer segment, so no thread can still touch its slots.

## Hardening (post-review)

- `MML_CHAN_SEG` is parsed strictly (`strtol` + end-pointer):
  negative, non-numeric, and out-of-range values abort with
  `mml: MML_CHAN_SEG must be 0..4096`. Documented max: 4096 slots.
- `cap * esz` overflow is checked before `malloc`; every
  allocation (channel, segment, legacy node) is checked and
  aborts with a consistent `mml:` diagnostic.
- Legacy default path is unchanged — seg is opt-in only.

## Results (lg = 8M packets, REPS=9, interleaved rounds)

| cell | seg0 (legacy) | seg16 | seg64 | seg256 |
|---|---|---|---|---|
| prodcons (box ptr, 1P→1C) | 0.645 | 0.483 | **0.483** | 0.485 |
| prodcons64 (64B by-value) | 0.456 | 0.275 | **0.246** | 0.247 |
| prodcons_2p (2P→1C) | 1.991 | 1.337 | 1.224 | **1.176** |

Baselines: C pthread queue 0.345s · Rust `mpsc::channel` 0.973s.
(An earlier pre-hardening run measured seg64 prodcons at 0.379s;
the improvement direction is stable, the absolute medians drift
with machine load — the interleaved rounds keep comparisons
within a run fair.)

- **~25–42% runtime reduction** legacy → segmented across runs.
  Residual vs C (~1.4× this run) is a mix of extern `mml_chan_*`
  call overhead, synchronization, and bookkeeping — none of it
  yet attributed to a single cause; inlining is a separate,
  deferred experiment.
- Contended case (2P→1C) prefers the *largest* segment (256):
  allocation amortization outweighs handoff granularity.
- Queue allocations: 1/msg → ~1/N/msg.
- **Peak RSS trade-off**: segmented buffering grows peak memory
  (prodcons: 7.7MB legacy → 89MB seg64; prodcons64: 95MB → 622MB
  seg64 — 64B payloads × in-flight segments). Segments free only
  once fully drained, so a lagging consumer holds more. Bounded
  by queue depth, but a real cost to weigh before defaulting.
- A naive cross-thread freelist was measured *slower* (0.584
  vs 0.513 for matched C) — not pursued.

## Validation

- Runner enforces: every rep must pass or the cell reports
  INVALID (no timing summary); all seg variants of a workload
  must produce identical stdout digests; variants interleave in
  shuffled rounds so drift hits all variants alike.
- Per-rep peak RSS via `/usr/bin/time` and Mpkt/s throughput
  are recorded alongside medians + bootstrap CIs.
- **Full corpus under `MML_CHAN_SEG=64`: 776/776 pass** —
  FIFO, blocking-recv, close, sentinel, and spawn/ownership
  semantics preserved on the segmented path.
- Targeted lifecycle fixtures (native-tests 774–777) pass at
  seg ∈ {legacy, 1, 16, 64, 256}: cross-segment FIFO order,
  synchronized close boundary, partially-filled final segment,
  exact-capacity boundary. Windows implementation mirrors POSIX
  but has not been executed on Windows hardware.

## Runner

`REPS=9 ONLY=prodcons python3 bench/chan/run.py` — builds each
source through ourc + fresh runtime objects, runs every
MML_CHAN_SEG variant, per-rep digest, median + bootstrap CI,
peak RSS, throughput.
