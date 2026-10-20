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
- disasm — call sites to `mml_bounds_fail` in the final binary

This separates *Praetor's* proof-driven elision from *LLVM's* own.

**Validity** (same model as PraetorBench 1.1): every measured rep must
exit 0 and produce the experiment's shared output digest; any failure
or mismatch invalidates the cell. Stats: `statistics.median` + seeded
bootstrap 95% CI; ratio CI over resampled medians.

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

| exp | fe→opt→asm sites | ours/C | reading |
|---|---|---|---|
| scan_guarded | 0→0 | ~1.00 | N46 elision confirmed in IR |
| scan_eq | 0→0 | ~1.00 | N47 equality substitution works |
| scan_opaque | 1→1 (scan) | ~1.00 | check kept, bandwidth-hidden |
| scan_rev | 1→0 (scan) | 1.002 [0.984–1.025] | LLVM re-derives elision anyway |
| scan_hot_g | 0→0 | ~1.00 | elided → NEON vectorized |
| scan_hot_k | 1→0 (scan) | ~1.00 | **check kept in .ll, gone in asm** — LLVM hoists it |
| idx_* | 0/1 | ~1.01 | masked access hides check under L3 latency |
| slice_mk | — | **1.29** | real cost — but checked-C control is only 1.03, so the +26% is slice *construction*, not enforcement |
| auth_pass | — | ~1.1 | `alloc` lowers to a real `ptr` param; inlined at -O2 |
| own_move | — | ~1.09 | move ≈ free once alloc is out of the loop |
| alloc_loop | — | **~24×** | `alloc_slice`+`free` vs malloc+free — the real cost center |

**Where the suite stands**: source-level N46/N47 elision is real and
verified in emitted IR; retained per-element checks are near-free on
this hardware because LLVM re-derives or the CPU hides them. The
measurable safety costs are **slice formation** (construction, not the
check) and the **allocator**. Next depth: data-dependent indexing LLVM
can't re-derive (e.g., indexed loads inside structures), and
noinline/ABI-boundary authority costs.

Artifacts: `bench/bce/out/<ts>/{raw.csv,stages.json}`.
