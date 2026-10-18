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
| parser | canonical AST dump (S-expr, node kind + child order + spans + literal spellings; `!` per lex/parse error) | `ourc --dump-ast` (N69) |
| loader | module graph dump: `(module-graph (root "<path>" (dep "<qual>")…) (mod "<qual>" "<src>" (dep …)…)…)` — postorder load order, `<std>`/entry-relative path spellings, `! <file> <line> <col> <msg>` diags (`-` for root-attributed) | `ourc --dump-modules` (N70) |
| resolver | symbol/resolution dump: `(module-symbols (mod "<qual>" "<src>" (dep …) <decls> <bodies> <clones>)… (root …) <errs>)` — qualified decl tables, `(bind …)`/`(use …)`/`(member …)`/`(tyuse …)`/`(variant …)`/`(armbind …)` resolution records, generic `(clone-*)` materializations, `(err …)` resolution diags | `ourc --dump-symbols` (N71) |
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
fuzz batch inside `cargo test`. `diffast.rs` does the same for the
parser surface, `diffgraph.rs` for the module-graph surface — the
graph gate additionally builds multi-file fixture trees
(cycles, qualifier collisions, missing/package/`std` imports,
`module`-decl mismatches, dep parse/lex/read failures, diamonds)
since single-file corpus coverage can't reach the loader's error
lattice — and `diffsyms.rs` for the resolver surface, with
fixtures for cross-module generics, private-visibility diags,
desugared method calls, enum match arms, and qualifier collisions.

## 6. Port order

```text
N68  source storage + lexer + token-diff harness    [done]
N69  parser (+ canonical AST dump)                  [done]
N70  module loader / source graph                   [done]
N71  type representation + symbol resolution        [done]
N72  checker / authority graph                      [done]
N73  own IR                                         [done]
N74  LLVM text emitter                              [done]
N75  driver / project / toolchain glue              [done]
N76  first Stage-1 compiler (self-compiles)         [done]
N77  self-build + corpus equivalence (Stage-2 threshold)  [done]
```

### Status — self-hosting threshold met (N77)

- **Stage 1 → Stage 2:** `target/our selfhost/our.lang -o our2`
  completes (~4 min). Stage 2 is a working native compiler —
  verified compiling and linking runnable `.lang` programs.
- **Stage 2 corpus:** 751 PASS / 1 SKIP across all 752 native
  fixtures (`scripts/s1test-one.sh` with `OUR=<stage2>`; the SKIP is
  `739_defer_res_survives_arm`, whose bare `// @fail` is not an
  ntest directive — same as Stage 1).
- **Equivalence:** Stage 1 and Stage 2 emit **byte-identical LLVM
  IR and object code** for the entire selfhost source tree
  (`our2.ll` ≡ `our3.ll`, ~12MB; `our2.o` ≡ `our3.o`). Linked
  executables differ only in 49 bytes of linker UUID metadata.
  Identical stderr and exit codes on fixture compiles, including
  compile-error diagnostics.
- **Known engineering debt:** selfhost binaries' `own`-aggregate
  frames reach ~3.2MB per `g_expr` call; tasks now reserve 512MB
  stacks (N76-B) to carry self-compile-depth recursion. Frame-size
  reduction is a post-selfhost codegen milestone.

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

## 8. Port discipline (learned in N72)

These rules are architecture, not style — each was learned from a
concrete failure while porting the checker:

- **Frame budget.** Stage-1 codegen lays a function's locals out in
  one stack frame, so a large match arm inside a *recursive* checker
  function (expr/stmt walkers) makes every recursive call cost
  hundreds of KB — observed frames: `expr_tv_w` ~1 MB, `ety_at`
  ~390 KB, `expr_spawn` ~1 MB. Rule: when a recursive checker
  function gains an arm with large snapshots, temp vecs, or further
  recursive calls, extract the arm into a named helper *up front*,
  not after the next stack-guard crash.

- **Scripted edits.** Before any scripted replacement: assert the
  anchor count is exactly 1 *inside the target function's range*
  (not just the file); save the function range; after the edit,
  diff the whole function inventory against HEAD. A bad mechanical
  edit costs more time than a semantic bug.

- **Range convention.** AST payload fields are `(offset, len)` in
  some places and `(start, end)` in others (e.g. spawn bodies are
  `e.b` = offset, `e.c` = len; `if` lists are
  `[nt, then.., haselse, else..]`). Never mix the two in raw `u32`
  pairs — name locals `*_off`/`*_len` vs `*_lo`/`*_hi` accordingly.

- **Name-ID discipline.** `e.a` is a *parser* NameId; semantic
  helpers only accept resolver nids after `resolve.intern_pname`.
  Same trap class: operand fields (`Unary.e.b`, `AddrOf.e.b`,
  `AddrOf.e.a` is the mut flag).

- **Side-channel lifetime.** `has_lock`/`has_mutex`/`has_recv`/
  `has_spawn` are set during init-expr eval and consumed at the
  binding site of the *same* statement; anything left over dies at
  the next `check_stmt` entry (Stage-0's `ctx.last_* = None` tail).

- **Arena lifetime ≥ object lifetime.** Stage-0's semantic objects
  own their data (`Packet.edges`, TaskEff fields); Stage-1 stores
  them as offset ranges into shared arenas. If such an object can
  outlive a snapshot/restore boundary (task fork, branch snap),
  the arena it indexes must also survive that boundary —
  `pk_edges`/`pk_caps`/`pk_sent` are shared append-only for the
  whole run, not per-task arenas. Relevant again for defers,
  arm summaries, and nested-match ranges.

- **Scratch registers are ABI.** `res`/`res2`/`res3`/`res4` are
  caller-visible state: a helper that sets `c.res` on any path
  (including early returns) defines the caller's next read.
  Preserve needed values *before* calling a consuming helper, and
  reset `res` to NONE on early exits whose contract is "no result"
  (`owner_of_key` leaked `key_root`'s res → phantom send-escape;
  `claim_packet` left a key in res → bogus guard-domain gate).
