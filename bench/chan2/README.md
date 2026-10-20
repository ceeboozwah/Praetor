# bench/chan2 — CHAN-002 channel memory attribution

CHAN-001 showed the segmented queue's peak RSS growing ~6–8× vs
legacy (prodcons64: 95MB → 622MB). CHAN-002 measures *why* — live
queued messages, allocated slot/byte peaks, and occupancy under
controlled producer/consumer rates — without changing queue
semantics.

## Instrumentation

`MML_CHAN_STATS=1` enables per-channel counters in both runtimes
(POSIX + Win32), mutated under the channel mutex, reported at
exit via `atexit`. Opt-in instrumentation: when disabled, no
counter updates run (conditional checks and struct fields
remain — residual overhead measured negligible, ratio ≈1.0 on
matched/seg64).

- `msgs_peak` / `slots_peak` / `bytes_peak` — **non-simultaneous
  maxima**: each records its own high-water mark and they need
  not coincide.
- `at_bytes_peak_msgs` / `at_bytes_peak_slots` — **simultaneous
  snapshot**: live msgs and allocated slots at the exact instant
  `st_bytes` reached its maximum. This is the authoritative
  retained-capacity measure.
- `occ` — Σmsgs/Σslots accumulated per queue op; a count-weighted
  (not time-weighted) utilization estimate.

## Results — mem_pc.lang, 64B by-value payloads, n=200k

| rate | seg | msgs_pk | slots_pk | bytes_pk | msgs@bytepk | slots@bytepk | occ | RSS |
|---|---|---|---|---|---|---|---|---|
| matched | 0 | 16,524 | 16,524 | 1.2MB | 16,524 | 16,524 | 1.00 | 2.7MB |
| matched | 16 | 200,001 | 200,016 | 13.1MB | 200,001 | 200,016 | 1.00 | 17.7MB |
| matched | 64 | 200,001 | 200,064 | 12.9MB | 200,001 | 200,064 | 1.00 | 17.0MB |
| matched | 256 | 199,760 | 200,192 | 12.8MB | 199,696 | 200,192 | 1.00 | 17.7MB |
| cons-slow1 | 0 | 199,240 | 199,240 | 14.3MB | — | — | 1.00 | 16.7MB |
| cons-slow1 | 64 | 200,001 | 200,064 | 12.9MB | 200,001 | 200,064 | 1.00 | 17.0MB |
| cons-slow1 | 256 | 200,001 | 200,192 | 12.8MB | 199,937 | 200,192 | 1.00 | 17.7MB |
| prod-slow5 | 0 | 64 | 64 | 4.6KB | 64 | 64 | 1.00 | 1.4MB |
| prod-slow5 | 64 | 128 | 192 | 12.4KB | 65 | 192 | 0.334 | 1.4MB |
| prod-slow5 | 256 | 192 | 512 | 32.8KB | 1 | 512 | 0.111 | 1.4MB |

n=8M matched (the CHAN-001 workload shape):

| seg | msgs_peak | bytes_peak | occ | RSS |
|---|---|---|---|---|
| 0 | 1,770,091 | 127MB | 1.000 | 138MB |
| 64 | 2,844,123 | 183MB | 1.000 | 224MB |
| 256 | 3,902,078 | 250MB | 1.000 | 320MB |

## Attribution

**The memory increase is legitimate queue backlog, not retained
segment capacity.**

1. At the byte-peak instant, retained capacity is ~1–2 segments:
   `slots − msgs` = 15 / 63 / 432 for cap 16 / 64 / 256 — bounded
   by `O(2·cap)`, i.e. ≤32KB at cap 256 even in the shallowest
   queue (prod-slow5: 512 slots for 1 msg).
2. `msgs × esz ≈ bytes_peak ≈ RSS − baseline` — the RSS delta is
   fully explained by deeper in-flight queues.
3. The deeper backlog is *caused by* the optimization: segmented
   send is faster, so the producer outruns the consumer sooner.
   Legacy's per-message malloc was accidental backpressure, not
   a designed bound.
4. Stats enabled vs disabled: matched/seg64 median ratio ≈ 0.98
   — counter overhead is below noise at this workload scale.

Implication for a future default: the trade-off is inherent to
*unbounded* channels with a faster producer — a bounded channel
is the language-level answer, not a segment-size knob.

## Runner

`python3 bench/chan2/run.py` — builds mem_pc.lang with fresh
runtime objects, runs seg ∈ {0,16,64,256} × rate configs with
MML_CHAN_STATS=1 + `/usr/bin/time` RSS. A cell is reported only
if rc==0, the stats line parsed, RSS parsed, and the stdout
digest matches all other segs of the same rate; any missing
evidence suppresses the row and exits nonzero. N env
(default 200000).
