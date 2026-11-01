# Debug-Codegen Investigation — alloca-heavy lowering

## Baseline
Debug cells: `ourc --show-llvm | clang -O0` + mmlrt `-O2` (same
runtime as release). Comparisons: C `-O0`, C++ `-O0`, Rust
`opt-level=0` — equivalent debug configurations.

## Attribution (mapbench, -O0)
- ours `mget`: 190 insns / 77 ldr+str; C `map_get`: 75 / 32
  → **2.5× instructions, 2.4× memory ops** in the hot lookup
- Controlled C microbenchmark (same probe work, -O0): field
  reload-through-pointer (alloca pattern) vs cached locals:
  **+33% wall** — confirms the stack-traffic tax is real and
  measurable, not just static count
- Runtime evidence matches: mapbench lg debug ours 2.545s vs C
  1.845s (138%); the gap shrinks to ~102–108% at -O2 — the tax
  lives exactly where -O0 can't rescue stack traffic

## Mechanism
Frontend lowers every local + aggregate field through `alloca`,
so every `b[i]`/`m.slots[i]` at -O0 reloads the slice's ptr+len
through the stack per access. Bounds checks add ~1 branch/access.

## Risk note
`mem2reg`-style promotion at -O0 recovers the traffic but locals
leave the frame → debugger variable inspection degrades. Any
production fix needs the debug-info question answered first
(guaranteed-inspectable locals vs promoted temporaries split).

## Status
Attribution only; no implementation proposed yet. Next gate:
quantify debuggability cost of promoting provably-temporary
allocas (loop counters, spill slots) while keeping user-named
locals in the frame.
