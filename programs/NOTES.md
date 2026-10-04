# N24B — real-program hardening log

Semantics frozen unless a soundness bug appears. Awkwardness is
classified before proposing changes: stdlib gap / ergonomics /
sugar / diagnostic / lowering / module friction.

## Program 1: fcopy.lang — file copier (~55 LOC)

Chunks a file through a 4096-byte stack buffer via raw externs.
Verified: 50000 bytes copied byte-identical.

### Metrics
- explicit auth annotations (own/read/write on params): 0
- forced workarounds: 4 (F1, F4, F5, literal-CString discovery)
- compile time: <0.5s; binary: native via ourc
- runtime: 50000 bytes in 13 chunk reads, instant
- authority-model strictness: 1 hit (F1)

### Findings

- **F1 — `slice mut` by-value param holds a lasting borrow.** 
  `fd_read(fd, &mut arr[0..4096], n)` leaves `arr` borrowed after
  the call returns; a second call or `arr[0]=0` errors
  "cannot mutate while borrowed". The extern `*void` claim is
  transient per-call — which is what scratch-buffer semantics
  actually need. Question for later: should a by-value slice-mut
  arg's borrow be call-scoped like `write` capability is?
  Classification: semantics (do NOT touch now) / workaround:
  use externs directly.

- **F4 — literal inference is shallow.** `return -1` in a
  `-> i64` fn rejects (expected i64, found i32); `let e: i64 = -1`
  also rejects — the annotation reaches a bare literal init but
  not through `Unary`. Fn-arg position DOES propagate
  (`write(..., u64(n))`, `read(..., 4096)` to u64 params).
  Classification: diagnostics/inference ergonomics — candidate
  for a small want-propagation fix. Workaround: `i64(0) - 1`.

- **F5 — variadic extern mis-marshals trailing args.**
  `open(2)` is variadic; through the fixed-arity extern decl its
  `mode` arg landed as 0, producing a mode-0000 file. Same latent
  bug exists in `fs.fd_open` — O_CREAT calls through it create
  mode-0000 files. Classification: FFI semantics gap — real bug,
  worth a variadic-annotation or stdlib fix. Workaround:
  non-variadic `creat()`.

- **Nice surprise:** str literals coerce to `*const u8` at extern
  boundaries — `open("/path", 0, 0)` works with no CString dance.

### Gaps (stdlib/language)
- G1: no argv access — paths are compiled-in literals.
- G2: no `break`/`continue` — while-loop control flow is flag-based.
- G3: `fs.slurp` swallows open errors into an empty Vec — can't
  distinguish empty file from failure. Should return Result.
- G4: no int casts until today — `u64(x)`/`i32(x)` builtins added
  for N24B (checker-validated, emits real sext/zext/trunc). Not a
  semantic change — a missing builtin.

## Program 2: ugrep.lang — grep-like searcher (~95 LOC)

Slurps file into `Vec<u8>`, line-by-line substring scan of
`str.bytes`, prints `lineno: line` via raw write(2).
Verified: 254 matches == `grep -c`, correct line numbers.

### Metrics
- annotations: 3 `read` params (buf passed read-only, honest)
- workarounds: 3 (W1–W3 below)
- authority strictness: 0 — `read` params + transient extern
  claims stayed out of the way entirely. The checker never
  complained about a correct program.

### Findings
- **W1 — `Vec` is not indexable.** `v[i]` must be
  `v.as_slice()[i]`. Honest (slicing is the borrow point) but
  verbose in scan loops; each `v.as_slice()[i]` in a loop is one
  extra call expr per element. Classification: ergonomics —
  `Index` impl on Vec or a `v[i]`-desugar candidate.
- **W2 — `str` has no `.len`.** Length is `s.bytes.len`.
  Ergonomics: `str.len` field or method would be the obvious
  spelling.
- **W3 — module-qualified type names.** Params must spell
  `vec.Vec<u8>` — `Vec<u8>` alone is "unknown struct" even with
  `import std.vec`. Module-boundary naming is strict by design
  ("only a naming boundary") — but the diagnostic could suggest
  the qualified name. Friction: minor.
- **W4 — extern `write` dup:** `io` already declares it;
  redeclaring in user code = "duplicate function". Error is
  accurate but the fix (just call it — externs are global) wasn't
  obvious from the message.

### Non-bug
- stderr/stdout interleave: `print_i64` writes fd1, `eprint` fd2 —
  piping one fd makes the report look truncated. Program-side UX
  choice, no compiler issue.

## Program 3: psum.lang — parallel map-reduce (~60 LOC)

One shared [10000]i32, 4 OS threads each sum a quarter through
read-only task capture into `atomic i64`. Verified 49995000.
- annotations: 1 `read` (sum_range's array param)
- workarounds: 0 — shared-read capture just worked
- **Strong positive:** Shared+Shared capture of one array into 4
  concurrent tasks needed no ceremony. This is the "checker stays
  out of the way" evidence N24B wants.
- Definite-init note: array element-family must be seeded
  (`data[0]=0`) before a fill loop, per the N18 design. One extra
  line, documented in corpus tests.

## Program 4: prodcons.lang — producer-consumer (~60 LOC)

Producer heap-allocates `box Msg` per message, `send` transports
Own through `channel box Msg`, consumer frees each. Sentinel
terminates. Verified 2800.
- workarounds: 2
- **W5 — `bool == bool` unsupported.** `while done == false`
  errors "arithmetic on non-integers: bool and bool" — a *comparison*
  diagnosed as *arithmetic*, misleading phrasing. Used i32 flag.
  Classification: diagnostics + missing builtin compare.
- **W6 — `box T` vs `&mut T` channel payload.** `alloc` returns
  `box T` — the error was precise ("expected &mut Msg, found box
  Msg"), fix immediate. Good diagnostic.
- **Nice:** `heap` (the alloc capability) captures into a task
  env — producer allocs on its own thread, owner-transport works.

## Program 5: echo.lang — TCP echo server (~75 LOC)

Raw socket/bind/listen/accept/read/write FFI, sockaddr_in built
byte-wise in a [16]u8. Verified: real TCP round trip, 22 bytes.
- annotations: 0
- workarounds: 2
- **F7 — no hex literals.** `sa[2] = 0x90` lexes as `0` + ident
  `x90`. Byte-level FFI constants in decimal are error-prone —
  I swapped port bytes (wrote 144,31 for 31,144) and only caught
  it via connection-refused + lsof. Classification: lexer sugar —
  high value for a systems language (register masks, ports, flags).
- **G5 — no byte-order helpers.** htons/ntohs spelled by hand as
  raw bytes. stdlib gap: `std.net` htons/htonl wrappers.
- **G6 — sockaddr marshalling by hand.** A `std.net` module would
  own sockaddr_in layout + socket externs. Currently every user
  rebuilds the 16-byte array. Obvious stdlib gap, not semantics.

## Program 6: cfgparse.lang — config parser (~125 LOC)

`key = value` lines, comments, blanks. `Result<Entry,i32>` per
line, `match` dispatch, Vec<Entry> collection, offset-based keys
(no string copies). Verified: 4 entries parsed, comments skipped,
malformed line -> exit 1.
- annotations: 2 `read`
- workarounds: 5
- **W7 — Vec.push is consuming.** `v.push(x)` must be
  `v = v.push(x)` — `own self -> Vec<T>`. Honest ownership but the
  reassign idiom needs to be discoverable; the infer error sent me
  the wrong direction first.
- **W8 — method-call generic inference fails on match bindings.**
  `entries.push(e)` inside `match` can't infer T
  ("argument type not statically known"); the assoc spelling
  `vec.Vec<Entry>::push(entries, e)` works. Classification:
  inference gap.
- **u64/i64 friction is constant.** Vec.len/slice len are u64,
  print/io wants i64, array indices accept i64 — a real program
  sprinkles `i64()` casts at every boundary. Metric: 6 casts in
  ~125 lines. Widening casts are noise the compiler could infer.

## Program 7: wordfreq.lang — word frequency (~140 LOC)

Linear-map Vec<Entry{offsets,cnt}>, `slice mut` selection sort,
raw byte tokenizer. Verified against `tr|sort|uniq -c|sort -rn`:
identical unique count (2909) and identical top-6 frequencies.
- annotations: 2 `read`
- workarounds: 4
- **F8 — no `&&`/`||` at all.** Every compound condition is nested
  if/else or flag chains — the tokenizer clause needed a 3-deep
  else-nest. `!` DOES work; bool `==` doesn't ("arithmetic on
  non-integers: bool and bool" — wrong diagnostic too).
- **F9 — no bare `{}` lexical blocks.** `let m = map.as_mut()`
  borrows to scope end; `map.destroy()` then fails. `if true {}`
  is the only borrow-bounding idiom. Ugly but works.
- `map.as_mut()` inside a counting loop while `map.as_slice()`
  read-borrows in the same loop: sequential borrows composed
  cleanly — checker stayed out of the way where aliasing was
  genuinely exclusive.

## Program 8: hexdump/ — two-module CLI tool (~110 LOC total)

`hexutil.lang` (byte->hex writers) + `main.lang` (fd loop).
Verified byte-exact hex vs `hexdump -C` (spacing differs).
- **F1 generalized — CHECKER BUG CANDIDATE.** `slice mut` passed
  to ANY fn param mode (by-value, `write`-borrowed) leaves a
  lasting borrow on backing storage — the callee's claim never
  releases. `write`-param on a whole ARRAY releases correctly,
  so the bug is specific to slice-mut authority bookkeeping.
  Over-restrictive (safe direction) but makes slice-mut helper
  APIs essentially unusable — every write must route through the
  single long-lived view. Workaround: pass the array itself under
  `write`, or keep one &mut view and write through it everywhere.
- **F10 — no bitwise ops.** No `<<`/`>>`/`&`(binary)/`|`/`^`/`~`.
  `&` lexes only as address-of. Hex extraction via `%16`/`/16` +
  a reverse pass. For a systems language this is the highest-value
  pure-surface gap found so far — flag ops, register pokes,
  protocol fields all want it.
- Local modules worked frictionlessly: `import hexutil` +
  `hexutil.f()` resolution, module boundary purely naming as
  designed.

## Cross-program metrics summary

| program    | LOC | read/write/own | workarounds | compiles | verified |
|------------|-----|----------------|-------------|----------|----------|
| fcopy      | ~55 | 0              | 4           | yes      | byte-identical copy |
| ugrep      | ~95 | 3 read         | 3           | yes      | 254 == grep -c |
| psum       | ~60 | 1 read         | 0           | yes      | exact sum, 4 OS threads |
| prodcons   | ~60 | 0              | 2           | yes      | exact total, ownership transferred |
| echo       | ~75 | 0              | 2           | yes      | real TCP round trip |
| cfgparse   | ~125| 2 read         | 5           | yes      | entries + error path |
| wordfreq   | ~140| 2 read         | 4           | yes      | exact vs unix pipeline |
| hexdump    | ~110| 1 read + write params | 2    | yes      | byte-exact hex |

All compile in <1s. Zero unsafe escapes needed anywhere.

## Bug candidates for review (not fixed — semantics frozen)

- **B1:** slice mut param borrow never releases (F1 generalized).
  Check the summary-edge teardown for slice-mut-typed params vs
  array/struct write params, which release correctly.
- **B2:** method-call T-inference doesn't see match bindings.

## Prioritized gap list (by evidence)

1. bitwise ops (`<<` `>>` `&` `|` `^` `~`) — needed by hex/FFI work
2. hex/oct/bin int literals — same domain
3. `&&`/`||` — the else-nesting tax is the top line-count inflator
4. `break`/`continue` — flag-based exits are workable but noisy
5. literal inference through `return`/`Unary` — `return -1` rejects
6. argv access — currently zero way to take user input parameters
7. `str.len`, Vec indexing sugar, `std.net` module
8. fs.slurp -> Result (can't distinguish empty vs error)
9. variadic extern annotation — the open(2) mode bug is a real FFI
   hazard; either support `...` or warn on known-variadic decls
10. implicit widening i64<->u64 where lossless — the cast noise is
    the most frequent micro-friction across all 8 programs

## The verdict on authority strictness

The "too strict" count is low: ~3 hits across 8 programs (F1 is a
bug candidate, not intentional strictness). The shared-read
parallel sum and the channel ownership transfer — the two places
where Rust would make you fight — both went through with zero
ceremony. The checker stayed out of the way on correct code in
roughly 95% of cases; the exceptions were the slice-mut param bug
and missing surface sugar, not the core model.

---

## N24C — real-program hardening fixes (all three landed)

### B1 — FIXED. Two distinct bugs, one symptom class.

1. **Phantom borrow on call results** (the fcopy/hexdump failure):
   `refs_of`'s Call arm fell through to the conservative arg-union
   when the callee had a summary with EMPTY ret_edges. Every
   `x = f(slice_arg, ..)` bound a phantom `<local> -> span` Write
   edge — the local "held" a borrow it never asked for. Fix: a
   summary with empty ret_edges now PROVES no ref return (returns
   None); the arg-union only runs for truly unchecked calls.
   Test 485.

2. **Permanent reborrow-kill on `write` param places**: the
   derivation-source kill only swept `edges` and never restored —
   `f(write *m)` left `*m` dead forever. Now the source's edges AND
   caps park for the call window (self-conflict still enforced —
   `f(write m, write m)` rejects) and restore after the summary
   replay. `write` can't consume authority, so restore is
   unconditional when a summary exists; no-summary stays
   conservative. Test 486.

   Note: `a` staying borrowed while `let m = &mut a[..]` lives is
   CORRECT (not a bug) — the borrow is m's own live claim. The
   bug was the *call* extending it past m's actual death.

### FFI variadic — FIXED via shim (deferred full variadic FFI).

`mml_open_mode(p, flags, mode)` in mmlrt.c makes the real
variadic `open(2)` call; `fs.fd_open`/`slurp` route through it.
Verified: O_CREAT now produces mode 0644 (was 0000). Variadic FFI
syntax (`...`, default promotions, varargs ABI) deferred — needs
its own milestone.

### B2 — FIXED. Match-arm binding types for generic inference.

mono's match scan bound payload locals only for `Ty::Struct`
scruts; instantiated generics arrive as `App("result.Result",
[..])` so the binding silently skipped. Now resolves the enum
decl by base name and substitutes tparams->args into the payload
ty. `v.push(e)` / `take(e)` inside `match` infer correctly.
Test 487; cfgparse uses the natural method form.

---

## N25A — bitwise operators (shipped)

`& | ^ ~ << >>` landed end-to-end. Deliberately boring rules:

- integer operands only, same type, result = operand type
- `>>` arithmetic on signed, logical on unsigned (no C UB)
- `x << n`: constant n >= width is a compile error; dynamic n
  traps via `mml_shift_check` ("shift amount 70 out of range
  (0..64)") — no C-style masking
- `~x` lowers to `x ^ -1`
- precedence is C-shaped: `| > ^ > & > ==`, shifts between
  comparison and additive

`>>`/`<<` never lex as single tokens — `Vec<Vec<i32>>` needs the
pair split in type position, so the parser merges two `>`/`>`
tokens only at the shift precedence level. Verified with a
nested-generic program doing shifts (test 491).

Tests: 488 (all ops, signed+unsigned >>), 489 (const OOB
reject), 490 (bool reject), 491 (nested generics + shifts).
Corpus: 491/491.

### New findings during N25A

- **FIXED — lexer panic on huge decimal literals**: the digit
  accumulator `n * 10 + v` overflowed i64 → panic instead of a
  diagnostic. Now `checked_*` → "integer literal too large".
- **Confirmed — `want` doesn't propagate into Bin/Unary**:
  `let x: i64 = 10 & 12` and `let x: i64 = -1` both fail literal
  coercion (operands default i32). Corpus convention is typed
  bindings (`let a: i64 = 10; a & b`). This is the literal-
  inference polish item — next milestone candidate.

## N25B — hex/binary literals (shipped)

`0x`/`0X`, `0b`/`0B`, `_` separators in all radices
(`0xff_ff_00_80`, `0b1111_0000`, `1_000_000`).

- `_` must sit between digits — leading/trailing is a lex error
- digits outside the radix error (`0b102` → "invalid digit `2`
  in binary literal"), not a silent token split
- value bound is i64::MAX for now — `0xffff_ffff_ffff_ffff`
  rejects "too large". Top-bit u64 masks (`0x8000_…`) need
  `~0 << 63` style until `Expr::Int` can carry u64 bit-patterns
  through coerce. Noted as polish, not blocking.

Tests: 492 (values + separators + bitwise mix), 493 (bad radix
digit), 494 (edge `_`). Corpus 494/494.

## N25C — short-circuit `&&` / `||` (shipped)

`a && b` lowers to `if a { b } else { false }`; `a || b` to
`if a { true } else { b }` — a named bool slot, one store per
arm, load at the merge. No phi; the generated code shows the
control flow exactly.

- bool operands only (`!` discipline — no int-to-bool)
- RHS runs at most once, only when the LHS demands it —
  `false && tick()` provably doesn't call, `true || recv()`
  provably doesn't consume
- checker mirrors the `if`-arm merge at expr level: the RHS's
  edges/caps/consumed/kills join as Maybe against the state
  where it never ran. `false && sink(own p)` → `p` is
  maybe-consumed → later use rejects, identical to
  `if c { free(p) }`.
- precedence: `||` loosest, then `&&`, then `|` — C-shaped.

Tests: 495 (exactly-once + both skip paths), 496 (conditional
consume → maybe-consumed use rejects). Corpus 496/496.

## N25D — `break` / `continue` (shipped)

Loop-exit semantics done the careful way:

- `break`/`continue` snapshot the full path-sensitive state at
  their site (edges, caps, consumed, init, kills, path provs,
  tag, transit, chan queues) after expiring every binding
  deeper than the loop's scope — the same structural death `}`
  performs, including task-escape and unjoined-join diagnostics.
- Post-loop state = join(zero-trip, all break paths, all
  continue paths, fall-through end) — the `if`-arm cert
  discipline folded N-wise. `continue` snapshots join too: a
  continue-path effect can exit on a later iteration.
- `while true` special case: a literal-true cond has no
  zero-trip arm — `while true { x = 40; break }` leaves `x`
  DEF-init, not Maybe.
- `break` outside a loop rejects; break/continue can't tunnel
  through `spawn` (task bodies get their own ctx.loops).
- Guards: a `let g = lock(m)` inside the loop releases at the
  break jump — verified by re-locking after (would deadlock).
  The subtle bug: `open_guards.remove` at break poisoned the
  scope-exit unlock for every NON-break iteration — `contains`
  leaves the set so each path emits exactly one unlock.

Tests: 497 (control + nesting), 498 (def-init on break-only
exit), 499 (one-path init rejects), 500 (outside loop), 501
(guard release, both paths), 502 (consume on break path →
maybe-consumed). Corpus 502/502.

## N25-validation rewrite pass (measured before/after)

Every program that carried an N25 workaround got rewritten and
re-verified externally. LOC is wc -l on the checked-in file.

| program | before | after | workarounds removed |
|---|---|---|---|
| hexdump/hexutil | 44 | 33 | `%16` `/16` + 8-byte temp + reverse loop → `& 0xF`, `>>` |
| hexdump/main | 66 | 77 | +11 LOC: format bugs fixed — byte-8 gap, ASCII pad, EOF offset line. Now **byte-exact vs `hexdump -C`** (before, it only looked similar). |
| echo | 71 | 73 | decimal port bytes `31,144` → `0x1f,0x90` — self-spelling htons(8080) |
| cfgparse | 119 | 111 | 2 flag-scans → `break`; blank/comment i32 flags → `i > lo && b[lo] != '#'`; `d>=48&&d<=57` |
| wordfreq | 125 | 111 | `is_ws` ladder → `||` expr; `closing` flags → `i==len \|\| is_ws(b[i])` (**short-circuit is load-bearing**: b[v.len] would trap); `eq`/`found`/`k>=10` flags → `break` |
| ugrep | 78 | 79 | `ok` flag → canonical `j == needle.len` strstr idiom |
| prodcons | 51 | 51 | `done` i32 flag → `while true` + sentinel `break`; `free(g)` before break (maybe-consumed × loop-exit works — the checker forced the deref into the else arm, which is correct) |
| fcopy | 62 | 54 | raw externs → `fs.fd_*` wrappers now that B1 is fixed (inline `&mut buf[..]` in a loop no longer pins); `creat` hack → `fd_open(…, 0x1\|0x200\|0x400, 0x1a4)` |
| psum | 47 | 47 | none — already clean; nothing honest to rewrite |

663 → 636 total, but the number that matters is zero remaining
workaround comments claiming "the language can't say this."

### Verification

- hexdump: `diff` vs `hexdump -C` — 0 lines, aligned AND partial
- echo: real TCP round trip on :8080, 19 bytes echoed
- cfgparse: 4 entries + malformed line 7 caught (exit 1), same as before
- wordfreq: top-10 identical to `tr -s ' \t\r\n'` pipeline, 2909 unique
- ugrep: 254 = `grep -c the`
- fcopy: byte-exact 50KB copy, mode -rw-r--r-- through real flag path
- prodcons: 2800 = 100*(0+..+7) through channel + sentinel break

### Bugs found during the rewrite pass

1. **hexdump was never actually byte-exact** — N24B compared only
   the hex region. Missing the mid-line gap, ASCII-column pad,
   and the final EOF offset line. Fixed (+11 LOC, honest cost).
2. **prodcons `free(g)` before `break` needed an `else`** — the
   checker correctly rejected `(*g).val` after a maybe-consumed
   join. That's the semantic model *helping*, not friction.
   No compiler bugs filed from this pass.

### Semantic-friction verdict

The rewritten code reads as the code you would have written
first: `& 0xF` nibbles, `0x1f90` port bytes, `j == len` strstr,
sentinel `break`. The only rewrites worth doing were the ones
the language previously *forced* us to avoid. `&&`/`||` were
applied conservatively (wordfreq's `is_ws`, cfgparse's line
filter) — branchy spots that were clearer as branches stayed
branches (psum untouched).

### Fresh-evidence gap ranking (post-rewrite)

1. **literal-context propagation** — `let x: i64 = 10 & 12`,
   `f(0xff)` under `fn f(u32)` still need casts; `want` doesn't
   flow through `Bin`/`Unary` operands. Hit during N25A tests.
2. **argv / process args** — every program hardcodes paths
   (`/tmp/n24b_src.txt`) and needles (`"the"`). Biggest real-CLI gap.
3. **`std.net`** — echo spells sockaddr bytes by hand. AF_INET,
   htons, sockaddr_in deserve names, not comments.
4. **numeric inference polish** — `i64(bool)` rejection is right,
   but literal-under-expectation typing is the same fix as #1.
5. **enum/match ergonomics** — B2 fixed method calls on bindings;
   match-as-expression (value-yielding) is the remaining wart.

## N26 — bidirectional literal typing (shipped)

`want: Option<&Ty>` now threads through `expr_tv_go`: context
types literals, it never coerces concrete operands. gen's
`infer` already had this discipline — the checker was the gap.

- `Expr::Int` leaf: adopts `want` (unatomic'd) after a range
  check — `let x: u8 = 256` now says "literal 256 does not fit
  u8" instead of a generic mismatch.
- `Bin`: int-preserving ops (`+ - * / % & | ^ << >>`) forward
  `want` to operands. Shifts forward to the LHS only — the
  amount stays an ordinary integer. Cmp/`&&`/`||` yield bool:
  nothing forwarded (peer typing already covers literals).
- `Unary`: `~` forwards any int `want`; `-` forwards SIGNED
  `want` only — `let x: u8 = -1` still rejects rather than
  wrapping to 255. `-LIT` range-checks the magnitude against
  `-min`, so `-128` is a valid i8 and `-0x80000000` a valid i32.
- Boundaries: `let` annotation, `x = e` target, `return`,
  task return, fn/extern args, cast arg, struct fields, enum
  payloads all supply `want`.
- Peer-operand propagation kept, now guarded by
  `== INT_DEFAULT`: a literal that adopted `want` is typed, not
  default — this is what makes `0xff << 32` work while
  `u8v << 3` still resolves to u8.
- lower: `arg_needs_temp` exempts pure literal trees
  (`is_const_expr`) — hoisting `0xff << 3` into `let $rv`
  severed the context AND served no place-bookkeeping purpose.
- Not coercion: `u32 + u64` still rejects; `i64` cast still
  required for widening. Bonus: `u8(300)` now rejects at the
  leaf instead of silently truncating.

Tests: 503 (full matrix incl. struct fields, atomic, peers),
504-508 (range/sign/param/cast rejections), 509 (no numeric
coercion). Corpus 509/509.
