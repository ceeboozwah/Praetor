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
| `slice_len` | `&b[0..k]` per iter, `t.len` read only | formation alone (no index) |
| `slice_prov` | `if k <= b.len` guard + `t[k-1]` | source-level range elision |
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
| slice_mk | — | **1.257 [1.215–1.313]** | real cost; checked-C control = 0.989 → construction, not enforcement (pre-fix; see BCE-0.4) |
| auth_pass | — | 1.115 [0.952–1.249] | `alloc` lowers to a real `ptr` param; inlined at -O2 |
| own_move | — | 1.195 [1.002–1.380] | move ≈ free once alloc is out of the loop |
| alloc_loop | — | **1.016 [0.978–1.062]** | allocator ≈ parity — WITH escape control (prior 24× was a C-elimination artifact) |

## BCE-0.3 — checker-only proofs & ABI boundaries (PR #15)

| experiment | construct | question |
|---|---|---|
| `idx_mask` | `b[h & (cap-1)]`, `b.len ≡ cap` literal | N48 mask rule: frontend elision LLVM *can't* re-derive (runtime len field) |
| `ptrchase` | `cur = s[cur]` pointer chase | check ON the load dependency chain — worst case |
| `auth_abi` | `alloc` through a non-inlinable callee | authority cost at a real ABI boundary (+no-param C twin) |

### Findings (Apple Silicon, lg, REPS=9 — all cells valid)

- **`idx_mask` — the differentiator exists, now isolated.**
  `b.len ≡ cap` (literal `alloc_slice`) + `h & (cap-1)` elides the
  check in the frontend (`mml_main` carries only the `args[1]` site);
  no check appears at any stage.
  **`idx_mask_np` — matched-IR control**: same program, but `cap`
  arrives via argv (opaque to checker AND optimizer) and the mask is
  the literal `4194303` — N48 can't fire, the check is retained at
  **every** stage (fe=6, opt=3, asm=3). Same code, same runtime work;
  the only difference is the proof. So the proof system is
  responsible for exactly one absent `mml_bounds_fail` site in final
  machine code. Runtime is ~1.0× either way (the surviving check was
  latency-hidden) — the advantage shows in *code*, not yet cycles.
- **Facts don't cross call boundaries**: `probe(b: box slice)` moved
  into a callee kept the check — the `b.len ≡ cap` efact is local to
  the allocating function. Documented limitation.
- **`ptrchase` 0.999 [0.993–1.009]** — even a check serialised on a
  pointer-chase dependency chain costs ~0: the predicted-taken branch
  resolves in parallel with the load's latency. The per-fn asm is
  preserved for branch/load placement inspection.
- **`auth_abi` 1.018 [1.008–1.025]** — `alloc` through a verified
  non-inlined call (`bl work` present in disasm; callee is a 16-arm
  dispatch LLVM won't inline) vs `void *ctx` twin. `np/ctx` = 1.009 —
  one extra argument register costs ~1% at worst. C twins carry
  `__attribute__((noinline))` + a volatile ctx store so the parameter
  must materialize.

**Standing result**: every safety construct measured so far is at or
near C parity once the checker or LLVM discharges it; the two real
costs found are slice *formation* (+26%, construction path — root
cause found and fixed, see below) and cases where neither layer can
prove safety *and* the check matters — which we have not yet produced
on this hardware.

## BCE-0.4 — slice-formation root cause & fix

The +26% `slice_mk` delta decomposed cleanly:

| variant | retained check | ours/C |
|---|---|---|
| `slice_len` (formation + `t.len` only) | range check | 1.007 |
| `slice_prov` (`if k <= b.len` elides range; `t[k-1]` kept) | index check | 1.013 |
| `slice_mk` (both retained) | range + index | 1.24–1.29 |

Neither check alone costs anything — the *call* did. Disassembly of
the hot loop showed `bl _mml_range_check` **per iteration**: slice
formation lowered to an opaque runtime call, while `s[i]`'s bounds
check got the N50 treatment (fused `icmp`+branch, cold `noreturn`
trap — "an extern call here would block vectorization and hoisting
of everything around it"). Slice formation never got that lowering.

**Fix** (`compiler/src/ir/gen.rs`, `selfhost/ir.lang`): `&b[lo..hi]`
now emits inline `hi <u lo || hi >u len` — unsigned makes `lo < 0`
and `lo > hi` collapse into the first compare — branching to a cold
arm that still calls `mml_range_check` (same trap message, same
failure semantics).

**Result**: `slice_mk` → **1.034 [0.996–1.067]** — at C parity.
Corpus 769/769; stage-1→2 bootstrap fixpoint verified (identical
emitted LLVM IR). The remaining BCE story: retained checks are fused
compare+branches that LLVM hoists, folds, or hides — and the checker's
facts (N46–N48) remove them earlier still.

Artifacts: `bench/bce/out/<ts>/{raw.csv,stages.json}`.
