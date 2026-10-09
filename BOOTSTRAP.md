# Bootstrap architecture (N67)

This document freezes how `ourc` gets rewritten in `.lang`: the stage
ladder, the threshold that counts as "self-hosted", the comparison
surfaces, and the architectural rules for the new implementation.
Decisions here follow the N64 audit (`SELF-HOSTING.md`) — evidence,
not ambition.

## 1. Stage ladder

```text
Stage 0   Rust ourc          trusted reference compiler — permanent oracle
                │
                │  compiles
                ▼
Stage 1   ourc (.lang)       self-hosted compiler, built by Stage 0
                │
                │  compiles
                ▼
Stage 2   ourc (.lang)       self-hosted compiler, built by Stage 1
                │
                │  compiles
                ▼
Stage 3   ourc (.lang)       optional rebuild — reproducibility checks
```

**The Rust implementation is Stage 0 and stays.** It is readable,
testable, debuggable, and is the behavioral specification of the
language. Do NOT replace it with a frozen seed binary until the
`.lang` compiler already self-hosts — seed/provenance is a
supply-chain question, deferred to a later milestone (hashes of
official bootstrap binaries, reproducible chain). Sidelining the
oracle early would throw away the best differential tool the project
has.

## 2. Self-hosting threshold

The compiler counts as self-hosting when **all** hold:

1. Stage 1 successfully compiles its own source tree → Stage 2.
2. Stage 2 passes the complete corpus (every native pin, plus the
   compiler's own source as input).
3. Stage 1 and Stage 2 outputs are **behaviorally equivalent** —
   same emitted IR/object behavior on the corpus.

Not byte-identical binaries. LLVM/toolchain nondeterminism makes
that unnecessarily strict; byte reproducibility is a later
milestone.

## 3. Architecture rule — no transliteration

The `.lang` compiler is designed for this language's ownership
model, not transliterated from Rust. The N64 audit's directional
finding is a rule here:

- **Arenas + IDs from day one.** `FileId`, `TokenId`, `AstId`,
  `TyId`, `SymId`, `IrId` are `u32` (or `i64`) indices into
  arena-owned tables. `Expr{lhs: AstId}` — never `Expr{lhs: box}`.
  IDs are POD: copyable, cheap, stable under arena growth.
- **One `Ctx` struct owns the storage.** `sources`, `strings`,
  `tokens`, `ast`, `types`, `symbols`, `diagnostics` tables; passes
  take `read ctx` / `write ctx` explicitly. No `RefCell`/`OnceLock`
  conveniences to emulate — the authority graph is the interface.
- **No structural clones.** The Rust compiler's 1072 `.clone()`s
  are ownership friction; indexed arenas eliminate the need.
- **String-building is the emitter style** — `.ll` text is produced
  with `string.push_*` builders, mirroring `emit.rs`.
- **Errors are `Result`/`Diag` values** accumulated into the ctx —
  no panics, no exceptions, mirroring the existing `Vec<Diag>` flow.

Practical consequence: a self-hosted pass should land *simpler* than
the Rust equivalent wherever ownership friction was the complexity.

## 4. Canonical comparison surfaces

Every ported layer gets a falsifiable differential: both
implementations emit a canonical text dump for the same input, and
the outputs are diffed byte-for-byte. Surfaces are defined *before*
the port, in dependency order:

| Layer | Surface | Producer |
|---|---|---|
| lexer | `T <line> <col> <kind> <text> [decoded]` per token; `C`/`B` trivia lines; `!` per lex error | `ourc --dump-tokens` (N67) |
| parser | canonical AST dump (S-expr-ish, stable field order) | `--show-ast`-style flag, canonicalized |
| loader | module graph dump (resolved edges, order) | N70 tool |
| checker | diagnostics + semantic fact dump | `--show-*`-style flag |
| own IR | own-IR text | `--show-ir` |
| emitter | normalized LLVM IR | `--show-llvm` (normalized) |
| runtime | exit code + stdout + stderr | ntest runner |

Format rules shared by every surface:

- Text output only, LF newlines, no timestamps/paths/host details.
- Field order and spelling are part of the contract — a compiler
  change that reorders fields is a break in the oracle, not noise.
- Errors dump as `! <line> <col> <msg>` records; a layer that
  partially fails still dumps what it produced plus its errors.

The token-dump format is specified exactly in
`compiler/src/frontend/lexer.rs::dump_tokens`.

## 5. Differential corpus

The lexer (and every later layer) runs against the full real corpus,
not synthetic cases:

- all `native-tests/*.lang` pins (750+ — includes malformed `@exit`
  cases, so error paths are covered)
- all `stdlib/*.lang` sources
- all `programs/*.lang`, `bench/src/*.lang`
- all `selfhost/*.lang` sources — the compiler lexes itself
- fixture/template sources under `tests/` that are `.lang`
- UTF-8, comment/trivia, and `//`-at-EOF edge inputs

The token surface's harness is `compiler/tests/difftokens.rs`:
it builds `selfhost/dumptokens.lang` through the Stage-0 pipeline,
then byte-diffs every corpus file + synthetic edges + a seeded
fuzz batch inside `cargo test`.

## 6. Port order

```text
N68  source storage + lexer + token-diff harness    [done]
N69  parser (+ canonical AST dump)
N70  module loader / source graph
N71  type representation + symbol resolution
N72  checker / authority graph
N73  own IR
N74  LLVM text emitter
N75  driver / project / toolchain glue
N76  first Stage-1 compiler (self-compiles)
N77  self-build + corpus equivalence (Stage-2 threshold)
```

Numbering will move; the dependency direction won't. Each step is
gated on its golden differential going green across the corpus —
never discover divergence a layer late.

## 7. Preservation policy

Both implementations persist. Even after Stage 2 is green, the Rust
compiler remains the reference oracle and CI gate; the `.lang`
compiler earns primary status gradually. Two independently
structured implementations agreeing on an adversarial corpus is
evidence, not redundancy. Transition ordering: `.lang` becomes
primary → Rust becomes reference/legacy → *then* a frozen-seed
bootstrap chain can be defined if provenance demands it.
