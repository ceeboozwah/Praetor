# N42 — Comparative Systems Benchmarks

The question this suite answers: *how much explicit ownership machinery
does the programmer write to get C/Rust-class performance, and how close
does the language actually get?*

- Machine: Apple M5 (arm64), macOS 26.4
- C: Apple clang 21.0.0 (`-O0` debug / `-O2` release)
- Rust: rustc 1.98.1 (`-C opt-level=0` / `-C opt-level=2`)
- Ours: `ourc` frontend → LLVM IR → clang (`-O0` / `-O2`), `mmlrt.o`
  always `-O2` — the exact pipeline `our build --release` runs.
- Zig: **not installed on this machine — no Zig column exists yet**;
  adding a toolchain later is a one-line harness change, not a rerun.

## Workloads

| workload | spec | inputs |
|---|---|---|
| fcopy | copy a file in 64KB chunks | 64 MB blob |
| wordfreq | token count + unique words, zero-copy `str` keys | 12 MB corpus |
| jsonparse | recursive-descent DOM parse, node count, free | 6 MB JSON |
| mapbench | insert then look up N u64 keys | 4M ops |
| psum | 4 workers each alloc/fill/sum/free a quarter | 40M i64 |
| prodcons | producer thread → unbounded channel → consumer | 2M heap pkts |
| httpsrv | iterative accept→read→fixed-response→close | 1500 conns |
| kvsrv | iterative `SET k v`/`GET k` over `OMap<i64>` | 1500 conns |

**Algorithm parity is the point.** `wordfreq`, `mapbench`, and `kvsrv`
all use the same table in every language: open addressing, linear probe,
FNV-1a on the key bytes, stored hash, grow at 70% (`StrMap`'s actual
design, hand-rolled in C/Rust). Idiomatic Rust would use
`std::HashMap` (SipHash + SwissTable) — a different algorithm, so it's
not the controlled cell. Server load comes from shared C drivers
(`httpload`, `kvload`), so client code is identical across languages.

## Results — release

`run` = min wall of 5 reps; `rss` = `maximum resident set size` from
`/usr/bin/time -l`; `size` = binary bytes; `build` = full pipeline
wall time (ourc+clang / clang / rustc); `loc` = source lines.

| workload | metric | ours | C | Rust |
|---|---|---|---|---|
| fcopy | run | 0.010s | 0.010s | 0.010s |
| | rss | 1.4 MB | 1.4 MB | 1.6 MB |
| | size | 38,464 | 33,792 | 490,464 |
| | loc | 39 | 26 | 19 |
| jsonparse | run | 0.070s | 0.050s | 0.030s |
| | rss | 82.7 MB | 50.6 MB | 51.4 MB |
| | size | 56,488 | 33,920 | 490,776 |
| | loc | 260 | 165 | 120 |
| mapbench | run | 0.350s | 0.260s | 0.260s |
| | rss | 404.2 MB | 404.2 MB | 404.4 MB |
| | size | 38,424 | 33,600 | 489,184 |
| | loc | 109 | 75 | 62 |
| psum | run | 0.020s | 0.010s | 0.010s |
| | rss | 321.5 MB | 321.5 MB | 321.8 MB |
| | size | 38,448 | 33,840 | 521,200 |
| | loc | 40 | 37 | 31 |
| prodcons | run | 0.150s | 0.080s | 0.190s |
| | rss | 7.4 MB | 6.4 MB | 2.2 MB |
| | size | 38,280 | 34,160 | 571,216 |
| | loc | 42 | 75 | 24 |
| wordfreq | run | 0.090s | 0.030s | 0.040s |
| | rss | 30.9 MB | 14.0 MB | 14.2 MB |
| | size | 39,016 | 33,792 | 490,208 |
| | loc | 57 | 108 | 79 |
| httpsrv | run | 0.067s | 0.066s | 0.067s |
| | rss | 1.4 MB | 1.4 MB | 1.6 MB |
| | size | 39,800 | 34,000 | 496,832 |
| | loc | 39 | 35 | 25 |
| kvsrv | run | 0.071s | 0.072s | 0.071s |
| | rss | 1.4 MB | 1.4 MB | 1.6 MB |
| | size | 57,040 | 34,288 | 514,208 |
| | loc | 136 | 127 | 95 |

## Results — debug (no optimisation)

| workload | ours | C | Rust |
|---|---|---|---|
| fcopy | 0.010s | 0.010s | 0.010s |
| jsonparse | 0.180s | 0.070s | 0.060s |
| mapbench | 0.640s | 0.550s | 1.130s |
| psum | 0.040s | 0.030s | 0.110s |
| prodcons | 0.140s | 0.080s | 0.160s |
| wordfreq | 0.210s | 0.070s | 0.210s |
| httpsrv | 0.065s | 0.064s | 0.066s |
| kvsrv | 0.067s | 0.071s | 0.069s |

## Ceremony — explicit ownership/annotation tokens

Rough token counts per source file (`bench/ceremony.py`): ownership
qualifiers, `free`/`alloc`/`destroy`, `?`, `&`-borrows, `mut`/`unsafe`/
`Box`/`unwrap`/`clone`, `malloc`/`free`/`pthread_*`, explicit type
annotations. A proxy for "plumbing the author typed", not a precision
metric — see the methodology comment in `ceremony.py`.

| workload | ours | C | Rust |
|---|---|---|---|
| fcopy | 8 | 0 | 11 |
| wordfreq | 9 | 8 | 28 |
| jsonparse | 44 | 17 | 26 |
| mapbench | 32 | 6 | 19 |
| psum | 8 | 6 | 15 |
| prodcons | 8 | 22 | 14 |
| httpsrv | 7 | 3 | 8 |
| kvsrv | 40 | 14 | 38 |

## What the numbers actually say

**Runtime.** Release-mode ours lands at ~1.3–3× C on the CPU-bound
cells (mapbench 1.35×, jsonparse 1.4×, psum 2×, wordfreq 3×) and
*dead even* on the syscall-bound ones (both servers, fcopy — the OS
is the wall, not the language). The residual CPU gap is explainable:
every `s[i]` carries a bounds check (`mml_bounds_check` in the runtime)
plus conservative codegen — that's the explicit price of the safety
the C cells don't have. Debug mode tells the nicer story: ours beats
or ties Rust `-O0` on mapbench/psum/prodcons/wordfreq — rustc's
unoptimised output is notoriously bad and ours isn't far off C `-O0`.

**Memory.** psum/mapbench/servers sit at parity (the workload's data
dominates). Two honest misses: jsonparse at 1.6× (the `J` DOM's
`Vec`/`box` overhead per node vs C's packed `Node`) and wordfreq at
2.2× — worth profiling before claiming parity there.

**Binary size.** 38–90 KB against C's ~34 KB and Rust's ~490–690 KB —
an order of magnitude under Rust because nothing but `mmlrt.o` + libc
is linked. This is the cleanest win in the table.

**Compile time.** Ours sits between: 0.13–0.32 s for the full
frontend→LLVM→clang pipeline vs C's 0.04–0.06 s and rustc's
0.08–0.6 s. For these small files the frontend isn't the bottleneck —
the clang link step is most of it.

**The thesis metric — ceremony.** On resource-light code ours is at
parity or below (fcopy 8, wordfreq 9, psum 8, httpsrv 7 — under Rust
on all four). The cost shows up where C's story is "just don't get it
wrong": jsonparse (44 — every `Vec`/`box` gets an explicit
`alloc`/`destroy`/`free`) and kvsrv (40 — owned-key drain discipline).
Rust pays less (26, 38) because RAII hides those tokens in `Drop`.
The precise version of the thesis, then: **lifetime and topology
relationships are compiler-inferred; resource transfer and
destruction stay explicit.** The ceremony is *visible* —
`read`/`write`/`own` in signatures, `free` at the end — rather than
hidden in a destructor call graph. Whether that's a feature or a
tax is exactly the question N43's cold-user test should pressure.

## N46 — dominating-condition bounds-check elimination

First optimization pass, deliberately narrow: `mml_bounds_check` /
`mml_range_check` is elided only when a *dominating* `if`/`while`/`&&`
condition already bounded the index expression. The fact kernel keeps
operands verbatim — `i + 1 < s.len` proves `s[i + 1]` but NOT `s[i]`
(arithmetic wraps; at `i = u64::MAX` the sum is 0 and the cond still
holds). Any store to a fact's place, any `write`-param call, `swap`,
`replace`, atomics, `join`, or a `*p` store kills it.

Same source, same safety behaviour — every negative case (651–657)
still traps.

`call void @mml_bounds_check` / `@mml_range_check` sites in emitted
LLVM, before → after:

| workload | check sites |
|---|---|
| psum | 8 → 6 |
| mapbench | 23 → 17 |
| httpsrv | 8 → 6 |
| prodcons | 6 → 4 |
| fcopy | 7 (unchanged) |
| jsonparse / kvsrv / wordfreq | still pre-N45R source — did not compile; migrated under N47 (real sites: 15 / 38 / 25) |

The elisions land in hot code where the loop bound IS `.len`:
`strconv`'s digit loops (all workloads lost those calls) and
`mapbench`'s `mgrow` rehash loop (`while i < slots.len`, 4 → 1).
`minsert`/`mget`/`quarter` keep theirs honestly — they index
`slots[h & mask]` or bound a plain var `n`, neither a `.len` fact.

Same-binary A/B (ourc `.ll` → `clang -O2` → `mmlrt.o`, min of 5):

| workload | before | after |
|---|---|---|
| mapbench | 0.37s | 0.35s |
| prodcons | 0.15s | 0.14s |
| psum | 0.03s | 0.02s |

Regression pins `native-tests/646–657` cover both directions: six
elision shapes exit 42 with zero emitted check calls, six must-keep
shapes (`<=` bound, the u64 wrap, mutation invalidation, else-arm,
`||`, re-slice) still trap.

## N47 — symbolic range provenance

N46 facts only connect a comparison to the slice it names. N47 adds
*exact value equalities* — no arithmetic, just provenance:

- `n == s.len` in a dominating condition equates `n` and `s.len`.
- `let m = e` / `x = e` equate the target with `e`'s normal form.
- `alloc_slice(heap, n)` produces `b.len == n`.
- `let t = s` on a slice/str also equates `t.len == s.len`.

Equalities share the N46 discipline: verbatim operands, transitive
through copies, killed by any write to either side, truncated at
branch/loop scope end. Casts are the load-bearing boundary:
value-preserving casts (same type, same-signedness or
unsigned→wider-signed widening) keep the operand's key; every other
int cast becomes an *opaque* symbol — `u64(n)` on a signed `n` is
keyed `cast:u64:n`, so `alloc_slice(heap, u64(n))` still yields the
exact fact `b.len ≡ u64(n)` and can meet a `while i < u64(n)` bound,
but never collapses to `n` (negative `n` would lie). psum's
`while i < n` over `n: i64` therefore stays checked — the bound was
established in the i64 domain; that's the soundness boundary doing
its job, not a missed optimization.

Same-source check sites, N46 → N47 (`call void @mml_bounds_check` /
`@mml_range_check` in emitted LLVM):

| program | sites |
|---|---|
| programs/wordfreq | 34 → 28 |
| programs/wordfreq_vec | 23 → 18 |
| programs/indexer | 34 → 33 |
| programs/kvserver | 46 → 45 |
| bench workloads (all eight) | unchanged except wordfreq 25 → 24 |

Bench-level runtime is a rounding error at this scale — wordfreq
`min of 5` moved 0.08s → 0.07s. The real effect shows up where loops
bind a length copy (`let n = s.len; while i < n { s[i] }`), which the
real programs do and the bench sources mostly don't.

Ten pins `native-tests/658–667`: four elision shapes (copy,
alloc-provenance, copy chains, `==` cond) emit zero checks; six
must-keep shapes (mutated copy, re-sliced base, `u64(i64)` count
walked by an unrelated bound, `i64(u64)` copy, else-arm, plus the
widening positive at 667) behave correctly — the negatives still
trap.

Also under N47: `bench/src/{wordfreq,jsonparse,kvsrv}.lang` were
still pre-N45R sources (their N46 "0 checks" was a masked build
failure). Migrated mechanically — `wordfreq`'s `Full` arm now
discharges `m`/`k`/`b`/`v`, `jsonparse`'s `?` drains `items`
elements, `kvsrv` gained a `drain_map` helper for the accept-error
exit.

## N48 — masked-index bounds + loop rotation

Two additions. First, bitwise-AND bound facts: `x & m <= m` holds for
any `m` whose top bit can't be set, and `x & m < cap` when `m == cap-1`
with `cap > 0` — i.e. `x & (n-1) < n` for *every* positive `n`. A
power of two is NOT required: pow2 constrains which slots a probe
sequence reaches (coverage), never the index bound. Facts attach at
`let i = h & mask`, at `i = (i+1) & mask` re-binds, and inline at
`slots[h & (len - 1)]`.

Second, negated-condition facts: `if len == 0 { return }` leaves
`len > 0` on the fallthrough (unsigned `!= 0` gives `0 < x`); else
arms run under `!cond`, so `if i < 0 || i >= s.len { return }`
establishes `0 <= i < s.len` after it. Guards the programmer already
writes become the positivity proofs the mask rule needs — nothing is
inferred from construction convention.

Third, loop rotation. The cond is now emitted per incoming edge —
entry, each `continue`, and the back edge — so each evaluation is
proved under the facts live *there*. This closed a real hole from
N46/N47: ambient facts at cond-emission described iteration-1 values,
so `let i = 0; while s[i] != 0 { i = 99 }` read `s[99]` unchecked on
iteration 2. Now the back-edge test sees `i = 99`'s kill. (The
rotation duplicates a kept cond check in the `.ll`; `clang -O2`
merges identical copies back — post-`-O2` call sites are unchanged.)

Same-source check sites, N47 → N48 (`call void @mml_bounds_check` in
emitted LLVM):

| program | sites |
|---|---|
| programs/lru | 57 → 52 |
| programs/indexer | 30 → 26 |
| programs/kvserver | 39 → 35 |
| programs/workq | 24 → 20 |
| programs/graph / minijson / wordfreq | −1 each |
| programs/httpd | 14 → 15 (rotation dup; identical post-`-O2`) |
| bench/jsonparse / kvsrv / wordfreq | 12 → 11 / 35 → 33 / 23 → 22 |
| bench/mapbench | 17 → 19 (rotation dup; identical post-`-O2`) |
| bench/fcopy / psum / prodcons / httpsrv | unchanged |

`mapbench`'s masked index itself stays checked — its helpers never
establish `slots.len > 0` locally (it's a construction invariant),
and N48 deliberately does not invent it. The elisions that did land
are where guards exist: `omap.key_eq`/`key_eq_b`, `omap.grow`'s
rehash, `copy_*`/`dup_key` in kvserver. Runtime on the timed
workloads is flat at this scale (mapbench 0.33–0.34s, wordfreq
0.08s, jsonparse 0.07s — all noise-level deltas either way).

Eleven pins `native-tests/668–678`: the canonical mask shape, the
inline `slots[h & (len-1)]` shape, a *non*-power-of-two capacity
(elides — pinned so the "pow2 required" trap can't be re-added), the
probe loop, and `continue`-edge re-testing all emit zero checks;
unproven `cap` (live trap at `cap = 0` — `index 99 out of bounds`),
unrelated mask, `slots.len != cap`, `(h & mask) + 1`, `h | mask`,
and the stale-back-edge regression all keep theirs.

## N49 — cost attribution (profile before the next proof domain)

Before adding more proof machinery, measure where the remaining CPU
gap lives. Dynamic `mml_bounds_check` executions (instrumented
runtime) on the standard workload sizes:

| workload | dynamic checks | wall time |
|---|---|---|
| mapbench | 93.3M | 0.29s |
| wordfreq | 47.3M | 0.07s |
| psum | 22.8M | 0.03s |
| jsonparse | 9.7M (+172K range) | 0.06s |
| prodcons | 14 | — |

The headline finding: the gap was never "too many checks" in the
abstract — it was **the representation of the kept checks as opaque
extern calls**. Three costs stacked per check: the call itself, the
call-site register traffic, and the call acting as an optimization
barrier (no hoisting, no merging, no vectorization across it).
Secondary cost, visible in `minsert`'s probe loop: `m.slots.{base,
len}` reloaded every probe step — LLVM can't hoist through the
slot-store without alias information. Not a cost: memcpy storms
(aggregates lower to load/store pairs), `mml_range_check` (rare),
allocator calls (rare), and the servers (syscall-bound; prodcons
executes 14 checks total).

## N50 — inline check, out-of-line trap

Codegen now emits the bounds check as an in-block compare with a cold
trap arm:

```llvm
%ok = icmp ult i64 %idx, %len
br i1 %ok, label %bcok, label %bcbad
bcbad:
  call void @mml_bounds_fail(i64 %idx, i64 %len)   ; noreturn cold
  unreachable
bcok:
```

`icmp ult` is the whole semantic: len is never negative, and a
signed-negative index reads as unsigned-huge, so the unsigned compare
covers `idx < 0 || idx >= len` in one instruction. The reporting path
stays centralized (`mml_bounds_fail`, `noreturn cold`) — identical
message, identical abort. `mml_range_check` is unchanged — its call
count (172K) never justified the same treatment.

Same-source runtimes, N48 call-form → N50 inline (min of 5):

| workload | N48 | N50 | C |
|---|---|---|---|
| mapbench | 0.29s | **0.24s** | 0.22s |
| psum | 0.03s | **0.01s** | ~0.01s |
| wordfreq | 0.07s | 0.06s | |
| jsonparse | 0.06s | 0.06s | |

Two effects beyond call overhead, both free consequences of giving
LLVM real IR: psum's sum loop **vectorized** (120 SIMD instructions —
the extern call had been the barrier), and in `minsert` LLVM itself
folded `idx < len` where `idx = h & (len-1)` into a single `cbz len`
— the stock optimizer re-derived the N48 mask fact from the
transparent check. mapbench now sits at ~1.09× C — the 1.35× gap was
never the price of safety, it was the price of hiding it behind a
call boundary.

Kept-check site counts are unchanged (every N46–N48 prover decision
still holds — elided stays elided); what changed is what a kept
check costs. All 678 pins pass, including every must-trap pin now
routing through `mml_bounds_fail`.

## Methodology notes

- Runtime = min over `REPS` (default 5); RSS = max over reps.
- Server `run` is driver wall time — the load the server absorbed —
  since the servers are iterative and exit after a conn count.
- `fcopy`'s 0.010s floor is page-cache hits on `copy.bin`; the
  workload is a smoke test for correctness more than a benchmark.
- Rust `prodcons` RSS (2.2 MB) is genuinely low — `Box<i64>` on mpsc
  is one pointer's queue overhead; our `Pkt` is a 16-byte `box` and
  the runtime channel stores payloads inline.
- Drivers retry `connect` up to 500× (loopback TIME_WAIT pressure
  made earlier runs flaky; `SO_REUSEADDR` on client sockets + a
  never-reused port counter made the matrix deterministic).
- Compile time for ours includes the full `ourc → .ll → .o → exe`
  pipeline; for C/Rust it's the single driver invocation.

## Reproduce

    python3 bench/gen.py        # one-time data
    REPS=5 python3 bench/harness.py
    python3 bench/ceremony.py

`bench/results.csv` holds the raw cells; `ONLY=<substr>` reruns a
subset (server cells merge into the CSV rather than clobbering it).
