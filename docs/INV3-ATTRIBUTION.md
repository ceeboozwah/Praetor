# Investigation 3 — Generated-Code Attribution (closed, no implementation)

Scope: psum, mapbench, prodcons — release mode. Evidence: canonical
PraetorBench-1.3 rows, emitted IR (`--show-llvm`), final `clang -O2`
asm, per-rep distributions from `bench/out/20261031-104933/raw.csv`.

## Findings

### psum — environmental, not codegen (CLOSED)
Per-rep distribution (lg release) is bimodal in **every** language:
~0.04 s fast cluster vs ~0.1–0.24 s slow cluster. Identical minimums
across ours/c/cxx/rs ⇒ generated code equivalent; median spread is
mode-split luck (page-fault first-touch + thread scheduling on
multi-MB per-thread mallocs). ours' vectorized fill+sum loops
(`add.2d`, `ldp`/`stp` q-regs) match C's; `alloc_slice` = plain
malloc (no zero-fill). No defect.

### mapbench — redundant-load hypothesis REFUTED at -O2
Raw IR shows `slots.ptr`/`len` re-loaded ~4× per probe iteration,
but final asm hoists all of them (mget probe loop: `madd`+`ldr`+
`cmp` only; minsert likewise). Residual release gap (2–8%) is
bimodal noise + rare semantic struct moves (`replace` on rehash).
No release-codegen inefficiency established.

### prodcons — semantic cost, not codegen
ours pays 2 alloc/free pairs per message (payload `box` + queue
node); C fuses into one `Node` malloc. The payload alloc is
ownership-required; the extra node is the unbounded-channel layout
(runtime design, O1 didn't cover unbounded). Not a compiler defect.

## Verdict
No demonstrated general release-codegen inefficiency. Optimization
work should not proceed on this evidence.
