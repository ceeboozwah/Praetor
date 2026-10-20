# bench/bce — Bounds-Check Elimination & Safety-Cost Suite (0.2)

The question that makes Praetor's architecture interesting: **does the
checker+proof system reach zero-cost codegen** — do N46/N47 safety
facts remove enough `mml_bounds_fail` sites that generated code
matches unchecked C, and do ownership/authority constructs carry any
runtime cost at all?

## Method

Each experiment is a Praetor source + C twin(s) doing identical work
with **matched memory-access order** (reverse scans get a reverse C
twin; `slice_mk` gets both an unchecked and an explicitly-checked C
twin to separate construction cost from enforcement cost).

**Three-stage check pipeline** per experiment (`stages.json`):
- `frontend.ll` — `call void @mml_bounds_fail` sites OURC emits
- `opt.ll` — sites surviving `clang -O2 -emit-llvm` on that IR
- disasm — `bl`/`call`/`jmp` instructions *targeting*
  `mml_bounds_fail`, counted per function (symbol defs and
  literal-pool references don't count)

This separates *Praetor's* proof-driven elision from *LLVM's* own.
All three artifacts ship in the run dir for independent inspection.

**Validity** (same model as PraetorBench 1.1): every measured rep must
exit 0 and produce the experiment's shared output digest; any failure
or mismatch invalidates the cell. Stats: `statistics.median` + seeded
bootstrap 95% CI; ratio CI over resampled medians.

**Controls**: `alloc_loop`'s C twin escapes each allocation through a
`volatile` sink — otherwise `clang -O2` eliminates the entire
malloc/free cycle and the ratio measures *nothing* (an earlier
"allocator is 24x" figure was exactly that artifact: real-vs-absent).
The runner asserts the C binary actually contains malloc/free call
sites before accepting the cell.

## Experiments

| experiment | construct | question |
|---|---|---|
| `scan_guarded` | `while i < s.len { s[i] }` | loop-guard elision (N46) |
| `scan_eq` | `let n = s.len; while i < n` | equality substitution (N47) |
| `scan_opaque` | `while i < n`, n from argv | unprovable bound — check retained |
| `scan_rev` | `s[len-1-i]` under `i < len` | arithmetic index, reverse order |
| `scan_hot_g` / `scan_hot_k` | L2-resident 8K slice, hot loop | elided vs kept at cache-resident size |
| `idx_guarded` / `idx_bare` | masked random access ± guard | conditional indexing, isolated |
| `slice_mk` | `&b[0..k]` per iter (+checked-C control) | slice-formation cost |
| `auth_pass` | `heap: alloc` through 3-deep chain | authority = runtime cost? |
| `own_move` | `box slice` moved through calls | ownership transfer cost |
| `alloc_loop` | `alloc_slice`+`free` per iter | allocator dimension, isolated |

## Findings (Apple Silicon, lg scale, REPS=9)

| exp | fe→opt→asm sites | ours/C [95% CI] | reading |
|---|---|---|---|
| scan_guarded | 0→0→0 | 0.992 [0.967–1.011] | N46 elision confirmed in IR |
| scan_eq | 0→0→0 | 1.039 [0.997–1.079] | N47 equality substitution works |
| scan_opaque | 1→1→1 | 0.964 [0.881–1.096] | check kept, bandwidth-hidden |
| scan_rev | 1→0→0 | 1.011 [0.978–1.036] | LLVM re-derives elision anyway |
| scan_hot_g | 0→0→0 | 1.013 [0.986–1.167] | elided → NEON vectorized |
| scan_hot_k | 1→0→0 | 0.988 [0.910–1.130] | **check kept in .ll, gone in asm** — LLVM hoists it |
| idx_bare | 1→1→1 | 1.002 [0.986–1.017] | check survives to asm, hides under L3 latency |
| idx_guarded | 0→0→0 | 1.006 [0.992–1.070] | guard elides |
| slice_mk | — | **1.257 [1.215–1.313]** | real cost; checked-C control = 0.989 → it's slice *construction*, not enforcement |
| auth_pass | — | 1.115 [0.952–1.249] | `alloc` lowers to a real `ptr` param; inlined at -O2 |
| own_move | — | 1.195 [1.002–1.380] | move ≈ free once alloc is out of the loop |
| alloc_loop | — | **1.016 [0.978–1.062]** | allocator ≈ parity — WITH escape control (prior 24× was a C-elimination artifact) |

**Where the suite stands**: source-level N46/N47 elision is real and
verified in emitted IR; retained per-element checks are near-free on
this hardware because LLVM re-derives them or the CPU hides them
under memory latency. The measurable residual safety cost is **slice
formation** (+26%, construction not enforcement — checked-C control
≈1.0). The allocator is at parity once the C twin is forced to
actually allocate. Next depth (BCE-0.3): data-dependent indexing LLVM
can't re-derive, and ABI-boundary authority cost.

Artifacts: `bench/bce/out/<ts>/{raw.csv,stages.json}`.
