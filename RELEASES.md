# Release notes

Detailed notes per release line. The changelog is the terse
ledger; this file tells the story.

## 0.2.0 — the self-hosted line

**0.2.0 is the first release line where the language builds
itself.** The Stage-0 Rust compiler (`ourc`) compiles the
self-hosted source tree (`selfhost/`) into the Stage-1 `our`
binary; Stage-1 compiles the same tree into Stage-2; Stage-2 →
Stage-3 emits byte-identical LLVM IR. The bootstrap ladder is
closed and stable.

### Self-hosting & compiler

- The self-hosted pipeline is complete end to end: lexer → parser
  → loader → resolver → checker → IR → LLVM emitter → native
  driver → `clang`/link → executable — all written in the
  language itself
- Checker, IR, and LLVM parity against Stage 0 is verified across
  the full native corpus, with byte-identical diagnostics on all
  319 compile-error fixtures
- Qualified/generic type-name resolution now matches Stage 0 at
  every position (params, returns, fields, nested args, clone
  signatures) — closing the last known semantic divergence
- The large-aggregate call ABI moved to pointer/sret lowering:
  stack frames dropped from megabytes to kilobytes and the
  temporary giant-stack scaffolding is gone

### Toolchain & project workflow

- `our` is the self-hosted project driver: `new`, `check`,
  `build`, `run`, `test`, `deps`, `fmt`, `doc`, `toolchain`,
  `clean`, `rebuild` — byte-identical output to Stage 0 on every
  tested surface
- `our fmt` is trivia-preserving; `our doc` generates API
  reference from the resolver's own index
- Toolchain dispatch installs and discovers versioned siblings
  (`our-0.1.0`, `our-0.2.0`, …); a `toolchain = "0.1"` project
  re-execs its pinned sibling while `0.2` projects run natively —
  verified with real binaries side by side

### Packages & distribution

- Manifest-driven projects, path and directory-registry
  dependencies, transitive resolution, `project.lock` (format 2)
- Remote registries over HTTP/HTTPS (curl transport) — verified
  end to end including lockfile and cache byte-parity, `--offline`,
  `--locked`, warm-cache, and failure-text parity
- Embedded stdlib + runtime provision themselves to
  `$OUR_HOME/lib/<ver>/` — installed binaries need no repo
  checkout and no `OUR_STDLIB`/`OUR_RUNTIME` overrides
- The release tarball (`our-<ver>-<triple>.tar.gz`) carries both
  binaries, a dist installer, and docs — install → compile → run
  → package all verified from an isolated prefix with a foreign
  `HOME`/`OUR_HOME`

### Diagnostics & docs

- Family error codes (`E1001`–`E7002`) with `ourc --explain`;
  `ERRORS.md` is generated from the registry and CI-gated for
  freshness
- Pretty diagnostics render multi-file attribution, notes, and
  caret spans identically to Stage 0

### The `ourc`/`our` split — read this before scripting

**`our` is the primary self-hosted toolchain.** `ourc` ships in
0.2.0 as the **bootstrap/reference compiler and extended
developer utility** — it carries the LSP server, `--explain`, the
corpus runner (`ourc test`), and other extended surfaces the
self-hosted driver does not yet duplicate. This is a deliberate
release policy, not an oversight: the user-facing workflow is
fully self-hosted, and the extended `ourc` surface remains on the
bootstrap implementation until it is ported or retired.

### Version policy for 0.2.x

Per VERSIONING.md, 0.x is pin-your-toolchain:

- `0.2.x` patch releases: bug fixes and compatibility
  maintenance within the minor line
- `0.3.0` may break language, stdlib, or toolchain behavior —
  `toolchain = "0.2"` pins will dispatch or error rather than
  silently adopt it
- Projects that need reproducibility should set `toolchain =` in
  `project.toml` and commit `project.lock`
- Older siblings stay installed; `$OUR_HOME/lib/0.2.0/` does not
  disturb `lib/0.1.0/`

### Verification summary

| Evidence | Result |
|---|---|
| Native corpus | 757 PASS / 0 FAIL / 1 SKIP |
| Checker / IR / LLVM parity | full corpus vs Stage 0 |
| Compile-error diagnostics | 319/319 fixtures byte-identical |
| Stage-2 ↔ Stage-3 `.ll` | byte-identical |
| Release install round-trip | isolated prefix, foreign HOME/OUR_HOME, offline registry — pass |

### Known issues & caveats

- **Known semantic divergences from Stage 0: none currently
  known.** Any new one found is a release-blocking bug by policy.
- **Linux empirical verification: pending** (CI-gated).
- **Windows empirical verification: pending** (CI-gated). This
  release is verified on macOS arm64 only.
- `ourc` remains the Stage-0 bootstrap/reference binary (see
  above).
- Pre-1.0 compatibility is intentionally unstable across minor
  lines — pin toolchains where it matters.
- External tools: `clang`/`cc` for native codegen (plus `lld` on
  Windows); `curl` for remote registry access only.
- Nothing here is "production-ready" or "fully portable" yet —
  0.2.0 is a verified self-hosted developer release line, pending
  cross-platform evidence.

### Licensing

0.2.0 is the first release under a license: the project is
dual-licensed **MIT OR Apache-2.0** — see `LICENSE-MIT`,
`LICENSE-APACHE`, and the README note.
