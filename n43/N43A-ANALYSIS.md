# N43A — cold-agent usability study, formal analysis

Three sessions: DRYRUN (self), N43A-1, N43A-2 (Claude Code).
Classification axes — kind (BUG / ERGONOMIC GAP / LEARNING COST) ×
failing layer (diagnostic / documentation / language-design /
tooling); ranked by severity × frequency × recovery difficulty.

## What the study established

**The core ownership model is externally learnable.** A cold agent
inferred the lexical-borrow rule from one diagnostic, discovered
the owned/borrowed-key distinction from the public API, completed
explicit drain lifecycles without friction, and finished a
resource-bearing program with no implementation access. Both runs
answered *yes* to "would you build another program."

**The highest-severity defects are outside the semantics.** The
worst findings live in the project model, the dep resolver, and
tooling/documentation — not the ownership architecture. That is
the result this stage of the project wants: the kernel holds; the
shell has holes.

## Cross-run comparison

| signal | dry run | A-1 | A-2 |
|---|---|---|---|
| task completed | yes | yes | yes |
| near-isomorphic example | n/a | yes | no |
| compiled first try | no | yes | no |
| owned-key discovery | forced | bypassed | stdlib (pre-code) |
| compiler-taught borrow arc | yes | bypassed | yes (~3 min) |
| `tests/`→`src/` gap | found | bypassed | forced, confirmed |
| self-dep exploit | found | not seen | taught by E5002 hint |
| explicit drain/cleanup | yes | limited | yes |
| local-hover BUG | — | found | — |
| member-completion BUG | found | — | not exercised |
| retention | — | not measured | not measured |

## Ranked findings

Severity × frequency × recovery difficulty; only after this ranking
should any fix be scheduled.

1. **Project test/import model — BUG (tooling), HIGH.**
   `tests/` cannot import `src/`; the only workaround is a
   self-dependency the resolver should reject. Severity: a project
   has no coherent way to test its own code. Frequency: every
   project with tests. Recovery: ~8 min and requires exploiting a
   second bug. *The E5002 hint actively teaches the exploit — the
   two bugs form a chain.* This is the clear #1.
2. **Self-dependency accepted — BUG (tooling), HIGH.**
   `name = "."` resolves; a package can depend on itself. Not just
   cosmetic: it's the only escape hatch for #1, so fixing #2 alone
   worsens #1. Fix together: test-visible project modules *and*
   self-dep rejection.
3. **Missing `E####` codes on CLI diagnostics — probable BUG
   (diagnostics), MEDIUM.** LSP emits `E3001`; identical CLI errors
   carry none. Breaks `--explain` discoverability. Needs
   reproduction — cheap to verify, likely a coverage gap in the CLI
   formatting path.
4. **LSP hover `null` on local bindings — BUG (tooling), MEDIUM.**
   Confirmed in A-1 record: `let v` → `null` while call-site hover
   works. Locals exist in the index (completion lists them) — a
   narrow resolution gap, not wholesale indexing failure.
5. **LSP member completion returns `[]` — BUG (tooling), MEDIUM.**
   `io.`/`fs.`/`count.` all empty (dry run). Likely `std.io`-vs-`io`
   occ-name mismatch plus enumeration gap. Not exercised by A-2.
6. **`*u8` → `*const u8` conversion — ERGONOMIC GAP/BUG,
   MEDIUM.** No discoverable spelling; user worked around via
   `path.bytes.ptr`. If no conversion exists, it's a language gap;
   if it exists, it's undiscoverable. Either way, ~8 min of pure
   friction.
7. **Docs split: semantics documented, syntax/API not —
   ERGONOMIC GAP (documentation), MEDIUM.** SPEC/ERRORS cover the
   model; every participant learned syntax/API from `stdlib/`
   *source*. That worked, but "read the library implementation" is
   not a manual.
8. **Formatter generic-wrap output — ERGONOMIC GAP (tooling),
   LOW.** `result.Result < omap.OMap<...>, i32 >` splits ugly;
   N39 invariants (token-equal, compiles, idempotent) hold.
9. **Bare-block scoping absent — ERGONOMIC GAP
   (language-design), LOW-MEDIUM.** A-2 used the helper-fn route
   successfully; the dry run hit it. Watch — promote if human
   sessions want `{ }` for borrow-shortening.
10. **Ceremony items — LOW.** `result.Result<..>::Ok(0)`
    qualification, `"lit".bytes` two-step, `&mut` range syntax
    (good diagnostic, repeated once in dry run).

## Retention

Not measured in either run — neither participant re-committed a
rule violation. Do not infer a result; a human N43B session is the
more likely place to see repeat encounters.

## What this implies for milestone selection (not decided)

- The ranked list says the next milestone is **project/tooling
  correctness**, not language semantics: findings 1–5 are all
  tooling/project-system bugs, and #1+#2 are coupled and must ship
  together.
- N44 (bounds-check elimination) remains the right *performance*
  milestone; the study argues for a correctness/usability milestone
  *first* — the performance gap is invisible to someone who can't
  test their own project.
- The docs gap (#7) suggests a small syntax/API reference would
  have outsized onboarding value relative to its cost.
- Nothing in the study argues for changing the ownership model.
  The one semantic event that could have (lexical borrows) was a
  teaching success.

## Frozen known-issue ledger

| issue | status | source |
|---|---|---|
| `tests/`→`src/` unresolvable | **FIXED (N44T)** — own modules import as `<pkg>.<mod>` | dry run, A-2 |
| self-dep accepted | **FIXED (N44T)** — rejected, message names the real mechanism | dry run, A-2 (exploited) |
| CLI diagnostics missing E-codes | **FIXED (N44T)** — `our check`/`fmt` use `render_all` | A-2 |
| LSP hover null on locals | **FIXED (N44T)** — occ keyed on name span, not `let` | A-1, reproduced |
| LSP member completion `[]` | **FIXED (N44T)** — module match on last path segment | dry run |
| `*u8`→`*const u8` conversion | **FIXED (N44T)** — `*mut T`→`*const T` implicit weakening | A-2 |
| fmt generic wrap | open — cosmetic | A-2 |
| SPEC has no syntax/API ref | **FIXED (N45)** — `our doc <pkg>.<mod>` renders pub APIs + `///` docs from source; stdlib prose promoted | A-2 |
