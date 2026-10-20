# bench/chan2 — CHAN-002 channel memory attribution

CHAN-001 showed the segmented queue's peak RSS growing ~6–8× vs
legacy (prodcons64: 95MB → 622MB). CHAN-002 measures *why* — live
queued messages, allocated slot/byte peaks, and occupancy under
controlled producer/consumer rates — without changing queue
semantics.

## Instrumentation

`MML_CHAN_STATS=1` enables per-channel counters in both runtimes
(POSIX + Win32), mutated under the channel mutex, reported at
exit via `atexit`:

- `msgs_peak` — deepest live queue (messages in flight)
- `slots_peak` — max allocated payload slots (segments or nodes)
- `bytes_peak` — max allocated queue bytes
- `occ` — avg occupancy (Σmsgs/Σslots per op): retained-capacity
  fraction is `1 - occ`

Zero cost when off (`stats` flag set once at `mml_chan_new`).

## Results — mem_pc.lang, 64B by-value payloads, n=200k

| rate | seg | msgs_peak | slots_peak | bytes_peak | occ | RSS |
|---|---|---|---|---|---|---|
| matched | 0 | 30,714 | 30,714 | 2.2MB | 1.00 | 3.7MB |
| matched | 16 | 93,070 | 93,088 | 6.1MB | 1.00 | 9.0MB |
| matched | 64 | 66,084 | 66,176 | 4.3MB | 0.998 | 6.6MB |
| matched | 256 | 110,752 | 111,104 | 7.1MB | 0.995 | 10.4MB |
| cons-slow1 | 0 | 199,143 | 199,143 | 14.3MB | 1.00 | 16.7MB |
| cons-slow1 | 64 | 199,840 | 199,936 | 12.9MB | 0.999 | 17.0MB |
| cons-slow1 | 256 | 199,909 | 200,192 | 12.8MB | 0.998 | 17.7MB |
| prod-slow5 | 0 | 248 | 248 | 17.9KB | 1.00 | 1.4MB |
| prod-slow5 | 64 | 192 | 256 | 16.5KB | 0.338 | 1.4MB |
| prod-slow5 | 256 | 128 | 512 | 32.8KB | 0.113 | 1.4MB |

n=8M matched (the CHAN-001 workload shape):

| seg | msgs_peak | bytes_peak | occ | RSS |
|---|---|---|---|---|
| 0 | 1,770,091 | 127MB | 1.000 | 138MB |
| 64 | 2,844,123 | 183MB | 1.000 | 224MB |
| 256 | 3,902,078 | 250MB | 1.000 | 320MB |

## Attribution

**The memory increase is legitimate queue backlog, not retained
segment capacity.**

1. `occ ≈ 1.000` in every backlog regime — allocated slots track
   live messages almost exactly. Retained capacity is bounded by
   ~2 segments (the partially-drained rseg + partially-filled
   wseg), i.e. `≤ 2·cap·esz` — 32KB worst observed at cap=256.
2. `msgs_peak × esz ≈ bytes_peak ≈ RSS − baseline` — the RSS
   delta is fully explained by deeper in-flight queues.
3. The deeper backlog is *caused by* the optimization: segmented
   send is faster, so the producer outruns the consumer sooner
   and more messages legitimately accumulate. Legacy throttles
   the producer via per-message malloc — an accidental
   backpressure, not a designed bound.
4. Worst-case retention (shallow queue, prod-slow5, seg256):
   occ=0.11 but only ~32KB — padding, not a leak.

Implication for a future default: the memory trade-off is
inherent to *unbounded* channels with a faster producer — a
bounded/capacity-limited channel would address it at the
language level, not the storage layout. Segment size affects
retained waste only at `O(cap)` scale.

## Runner

`python3 bench/chan2/run.py` — builds mem_pc.lang with fresh
runtime objects, runs seg ∈ {0,16,64,256} × rate configs with
MML_CHAN_STATS=1 + `/usr/bin/time` RSS. N env (default 200000).
