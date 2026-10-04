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
