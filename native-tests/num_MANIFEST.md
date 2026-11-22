# Numerical Correctness Suite — manifest

Purpose: when the compiler changes, this suite provides automated
evidence that Praetor still computes the right answers — not merely
that programs compile. It exists to catch silent wrong answers.

## Oracle independence

Expected values are computed by `scripts/gen_numtests.py` using
**Python arbitrary-precision integers only**. Praetor's own compiler,
runtime, and stdlib are never part of the oracle. Regenerate:

    python3 scripts/gen_numtests.py     # SEED = 20260122, deterministic

Generated files are checked in so the corpus runs without Python;
regeneration is a review step, not a build step.

## Praetor semantics the oracle models

| Feature | Modeled as | Source |
|---|---|---|
| `+ - *`, unary `-` | two's-complement wrap at width | `add/sub/mul`, no nsw/nuw |
| `<<` | wrap at width | `shl` |
| `>>` | arithmetic signed / logical unsigned | `ashr`/`lshr` |
| `/` `%` signed | truncate toward zero (NOT Python `//`) | `sdiv`/`srem` |
| `/` `%` unsigned | floor | `udiv`/`urem` |
| `~` | complement at operand width | `xor`-equivalent |
| `dst(src)` cast | `sext` signed src / `zext` unsigned src / `trunc` / same-width bit-preserve | emit.rs Cast |
| shift amount | `0..width`, else compile error (N25A) | checker |
| pure-literal trees | fold at final type, must fit it (#44/N29) | checker |
| unadopted literals | must fit `i32` (#44) | checker |

Excluded by construction: division/modulo by zero and signed
`MIN / -1` — UB at the LLVM layer, not defined by the language.

## Case classes

- **valid** — runtime result fits the type: must equal the
  mathematical answer.
- **wrap** — runtime op escapes the type: must equal the
  two's-complement wrap. Legal runtime semantics; only *literal
  folds* are range-checked.
- **compile-error** — `@compile-error` pins: unadopted literals
  outside `i32`, typed-literal overflow, fold overflow, over-width
  shifts.
- **fold-vs-runtime** — a pure-literal tree folded by the checker
  must equal the same expression computed at runtime.
- **stdout** — full-width results printed and compared byte-exact
  against `<fixture>.stdout` via the `@stdout` directive (added with
  this suite; resolves the reference next to the fixture file).

## Files

- `840`–`842` — curated permanent boundary fixtures (hand-written,
  highest-value edges, commented).
- `num_010`–`num_080` — generated matrices across all eight int
  types: arithmetic, bitwise, shifts, comparisons, casts, div/mod,
  nested expressions, fold/runtime.
- `num_090` — generated `@stdout` print oracle (+ `.stdout` ref).
- `num_100`–`num_113` — generated rejection pins.
- `num_090_stdout.stdout` — expected-output oracle.

Failure attribution: a failing check returns its 1-based index, so
`FAIL-E num_040_cmp 137!=42` names check 137 in that file.
