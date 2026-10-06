# N51 — Authority → LLVM alias guarantees (`noalias`)

Status: **N51B — emitted** for the proven subset (`362e5be` N51A
proof, N51B emission). Measurement: see BENCHMARKS.md.

## The question

N49's disassembly showed LLVM reloading `m.slots.{base,len}` inside
`minsert`'s probe loop on every iteration: a store through the slice
buffer can, absent alias information, alias `m`'s fields. C gets this
for free (no alias contract needed to be wrong); Rust gets it from
`&mut` → `noalias`. Our `write`/`own`/`&mut`/`slice mut` authority
model *should* imply the same guarantee — but `noalias` is a specific
LLVM claim with specific semantics, and mapping our authority law onto
it is a proof obligation, not a flag flip.

Falsifiable question: **does the language's exclusive-authority model
imply every LLVM alias guarantee we would claim with `noalias`, for
the exact lifetime and memory reachable through that parameter?**

## What LLVM `noalias` actually claims

On a parameter `ptr noalias %p`, LLVM asserts: during the call's
dynamic extent, memory accessed through pointers **based on** `%p`
(derived via GEP/bitcast/etc. from the argument value) is not accessed
through pointers **not based on** `%p`.

Two consequences that shape everything below:

1. **Scope is the call.** Aliases that exist before or after are
   irrelevant — only *accesses during the callee's execution* count.
   A stale pointer that nobody dereferences is harmless.
2. **"Based on" is provenance, not reachability.** A pointer *loaded
   from* `m`'s storage (`m.slots.base`) is a fresh provenance — stores
   through it are NOT `%m`-based accesses. So `noalias` on `write m`
   guards exactly `m`'s own storage — which is precisely what lets
   LLVM hoist `m.slots.len` across `m.slots[i] = v` (the store is
   through the loaded buffer pointer, not through `%m`).

This means the proof obligation is narrow and structural:

> **No pointer that can address `m`'s own storage may be accessed
> during the call unless it is derived from the callee's `%m`.**

Caller-side borrows that outlive the call end are irrelevant; the
danger is anything *usable inside* the callee — other parameters,
task-visible storage, globals, or pointers reachable from `m`'s own
fields.

## Authority → alias mapping

| Language form | Lowers to | `noalias` claim | Verdict |
|---|---|---|---|
| `write m: T` | `ptr` param | exclusive to `m`'s storage for the call | **yes — under conditions (below)** |
| `own m: T` (aggregate) | by-value `%T` | n/a — fresh SSA value is trivially unique | unnecessary |
| `own b: box T` | `ptr` param | exclusive to the heap cell | yes — same conditions |
| `read m: T` | `ptr` param | shared — other readers legal | **never** (`readonly` is a different axis) |
| `s: slice mut T` | `{ptr,i64}` by value | the *buffer range* — not expressible as param `noalias` | `alias.scope` metadata territory, later |
| `&mut T` param | `ptr` param | exclusive to referent | same conditions as `write` |
| `handle T` | `ptr` param | **identity only** — see holes | never; and its *presence* poisons |
| `*T` param | `ptr` param | extern contract only | never; presence poisons |

## The three layers

### A. Root exclusivity — can another live edge reach `m`'s storage?

Empirically verified rejections (the checker already enforces these):

| Attempt | Result |
|---|---|
| `let r = &mut m.x; f(write m); *r` | reject E1001 — `m` already borrowed |
| `let r = &m.x; f(write m); *r` | reject E1001 — shared borrow blocks too |
| `let r = &mut m; f(write m)` | reject E1001 |
| `f(m, m)` — two `write` params | reject E1001 |
| `f(write m, &mut m.x)` | reject E1001 |
| `f(write m, &m.x)` | reject E1001 |
| `let r = &m.x; f(own m)` | reject E1001 |
| `let s = &mut m.arr[..]` (inline array field) + `f(write m)` | reject E1001 |
| unjoined `spawn` task touching `m` + `f(write m)` | reject E1002 |

Accepted (correctly — all `%m`-derived or non-`m`-storage):

| Shape | Result | Why safe for `noalias` |
|---|---|---|
| `let a = &mut m.x; let b = &mut m.y` | accept | disjoint subplace borrows; both `%m`-derived |
| inside `g(write m)`: `let r = &mut m.x; h(write *r)` | accept | reborrow — `%m`-derived chain |
| `let s = m.field_slice` (`slice`, read view) + `f(write m)` | accept | `s` points at the *buffer*, not `m`'s storage — its accesses aren't `%m`-based but also never touch `m`'s storage |

### B. Reachability — can memory *reachable through* `m` alias?

Under LLVM's provenance rule, only `m`'s own storage is claimed; a
reachable heap buffer (`box slice` contents) is a fresh provenance and
its accesses never count against `noalias(%m)`. Interior storage
(inline `arr: [N]T` fields, nested struct fields) IS `m`-storage —
covered above (f1 rejects).

### C. Dynamic scope — can a non-`%m` base reach `m`'s storage *during* the call?

Carriers capable of producing such a pointer:

| Carrier | Verdict |
|---|---|
| `&T` / `&mut T` params | checker rejects aliasing args (A) — covered |
| `slice mut`/`slice`/`str` params | same — borrow args, covered |
| unjoined `spawn` tasks | reject E1002 — covered |
| mutable globals | **none exist** — module level has no mutable bindings |
| reborrows | `%m`-derived — covered by definition |
| **`handle T`** | **HOLE** — see below |
| **`*T` raw pointers** | **HOLE** — see below |
| **`*T`/`handle` fields inside `T`** | **HOLE** — see below |

## The holes (the reason N51A exists)

### H1. `handle` is an unchecked provenance carrier — and it dereferences

```lang
fn touch(write m: M, h: handle M) -> i64 { (*h).x = 9; return m.x; }
// caller: let h = handle m; touch(m, h)   — COMPILES
```

A live `handle` neither consumes nor borrows authority (identity
token), so it coexists with `write m` — yet `(*h).x` inside the callee
writes `m`'s storage through non-`%m` provenance. With `noalias` on
`%m`, this is UB. Handles can also be *stored in fields*
(`w.h = handle w.m` compiles), so the hazard reaches inside aggregates
too.

### H2. `*T` fields can point at their own container

```lang
struct M { x: i64, p: *i64 }
m.p = idp(&mut m.x);   // extern returns the coerced borrow — compiles
```

Raw pointers are opaque (no in-language deref), so the checker
correctly ignores them — but `*T` field values have foreign
provenance, and an extern can stuff `m`'s own address into `m.p`. A
callee doing `memset(m.p, ...)` while writing `m.x` through `%m`
violates `noalias`.

### H3. `*T` params coexist with `write` params unchecked

```lang
fn touch2(write m: M, q: *i32) -> i32 { m.x = 1; wr(q, 9); return m.x; }
// caller: let q = stash(&mut m.x); touch2(m, q)   — compiles
```

`q` is opaque — no edge, no check. If the callee forwards it to any
extern that writes, `m`'s storage is touched through a foreign
provenance mid-call.

### H4. (flag, not a hole for `noalias`) `slice mut` field reads duplicate write authority

`let s = v.buf` where `buf: slice mut i64` leaves BOTH `v.buf[i]=` and
`s[i]=` live and writable — while `let t = s` on a `slice mut`
*binding* moves (E1001). Field reads copy the view; binding moves it.
Not a `noalias` violation (the buffer isn't `v`'s storage), but a
language-level exclusivity inconsistency worth deciding on its own.

## The N51B eligibility rule (as shipped)

`noalias` is emitted on a parameter `p: T` of function `f` iff all of:

1. **Authority**: `p` is `write` (place-alias → `ptr`) or `own` on a
   pointer-typed value (`box`). `read` never qualifies; `own` on a
   by-value aggregate needs nothing (a fresh SSA value is trivially
   unique). `&mut` params are deliberately out of the first pass.
2. **Self-scan**: `T` transitively contains no `*T`, `handle`, `&`,
   `slice`, or `str` — the loaded-pointer-view carriers (see
   refinement below). Owned `box`/`box slice` recurse into contents;
   mutex/guard/task/channel tokens address runtime heap objects but
   their payload types recurse too.
3. **Sibling-scan**: no *other* parameter's type transitively
   contains `*T` or `handle`. Checked carriers (`&`, `slice`) are
   rejected at the call site when they'd alias `p`, so a surviving
   sibling provably aims elsewhere — the sibling scan stays narrower
   than the self scan *on purpose*.

### Refinement discovered at emission time (N51B)

The N51A carrier set (`*T`, `handle`) is necessary but not sufficient
for the self scan. `slice`/`str`/`ref` fields are loaded-pointer
values of foreign provenance, and a `slice` field can be populated
with `slice(q, n)` where `q` is an extern-minted pointer that was
once `m`-derived — a `let q = ext(&mut m.x); m.s = slice(q, 4)` shape
executed at a *previous* call site, invisible to this call's checks.
The `*T` need not persist in `T`'s layout to leave the hazard behind.
In-language paths are closed — `m.s = &mut m.arr[..]` and direct
`&mut`→`*T` coercion both reject — but FFI-minted `*T` flows are
opaque, so view fields poison. Consequence: `StrMap`'s `key: str`
correctly costs `insert`/`get_mut` their `noalias`; `mapbench`'s
`Map` (u64 entries) is clean and gets it.

Rule 2 is the one that bites real code: `f(write m, q: *i32)` loses
`noalias`. That's honest — the alternative is trusting an opaque
pointer.

Not required: per-place escape tracking on the *caller* side. Caller
locals can't execute during the call; anything reachable through
`m`'s own fields is covered by rule 3; anything else live in the
caller was rejected by the borrow rules (A) or can't access `m`'s
storage during the call window.

## What this buys, concretely

`minsert(write m: Map)` — `Map`'s fields are `box slice` + scalars +
`alloc`: no `*T`/`handle` transitively; siblings are `i64`s. Eligible
→ `m.slots.{base,len}` hoist out of the probe loop. Measured in N49
via a hand-edited `.ll`: the per-probe cost drops from
`ldp`+`sub`+`and`+call per access to `and`+`madd`+`ldr`+`cbz` —
mapbench 0.24 → 0.23s stacked on N50 (and N50 already lets LLVM fold
the check itself to `cbz len`).

## Pins

- 679–690 (N51A): authority verdicts — rejections as
  `@compile-error`, legal shapes as `@exit 42`.
- 691–699 (N51B): emission verdicts via new `@ir-has`/`@ir-lacks`
  ntest directives — `ptr noalias` present on clean `write`/`own box`
  signatures, absent for `*T`/`handle`/slice-view fields, carrier
  siblings, `read` params, and nested (`struct`-in-`struct`)
  carriers.
