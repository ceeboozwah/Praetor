# Changelog

All notable changes to the `our` language and toolchain. Versions
are `x.y.z` only; the compatibility contract is VERSIONING.md —
0.x is pin-your-toolchain.

## Unreleased

### Security

- **Remote registry path traversal (QA-001)** — a malicious
  `files[]` relpath (`../x`, absolute, drive/UNC) steered package
  writes outside the `$OUR_HOME/registry` scratch tree; the hash
  check only ran after all writes, so the rejection couldn't undo
  the escape. `files[]` entries are now validated against the
  canonical component grammar before the first fetch, and the tree
  hash is verified on the scratch tree before promotion — in both
  Stage 0 and Stage 1.

### Fixed

- Runtime allocator operand validation (QA-002) —
  `mml_alloc_slice` computed `size * count` before checking it, so
  the overflow guard inspected the result of undefined behavior;
  a negative `size` passed the division check and silently
  allocated one byte; `mml_alloc` cast a negative size to a
  `SIZE_MAX` request. Both functions now reject negative operands
  and out-of-range products before multiplying, and bound the
  result to `size_t`. Zero-size allocations still succeed.
  Regression: `compiler/tests/runtime_alloc.rs` boundary battery.

- **Soundness** — branch-local bound/equality facts could escape their
  control-flow scope: a fact-kill inside a `if`/`while` arm compacted
  the fact tables, so a later push slid below the saved truncate mark
  and leaked into the sibling path. A leaked `i ≡ 0` could convince
  bounds-check elimination to drop a required `mml_range_check` and
  forge an out-of-bounds slice. Both Stage 0 and Stage 1 kills now
  tombstone in place — fact vectors are append-only under a live
  scope mark. (PLEX-0003; fixtures 765-770)
- Stage-1 `gb_le`/`gb_lt` const-representative scans took the last
  match while Stage 0's `find()` took the first; the order only
  diverged under the (now-fixed) fact leak. First-match restored;
  whole-compiler `our.ll` is byte-identical across stages.
  (PLEX-0002; fixtures 761-764)

## 0.2.0

First self-hosted release line: the compiler, project toolchain,
package machinery, and distribution are all built by the language
itself.

### Added

- Fully self-hosted native compiler pipeline — Stage-0 (`ourc`)
  compiles `selfhost/` into Stage-1 `our`; Stage-1 → Stage-2 →
  Stage-3 with a byte-identical `.ll` fixpoint
- Self-hosted project/package toolchain — `our new`, `check`,
  `build`, `run`, `test`, `deps`, `fmt`, `doc`, `toolchain`,
  `clean`, `rebuild` with full Stage-0 command parity
- Remote registry transport (HTTP/HTTPS via curl), directory
  registries, lockfile + offline/`--locked` package cache under
  `$OUR_HOME/registry`
- Standalone embedded stdlib/runtime provisioning — binaries
  materialize resources to `$OUR_HOME/lib/<ver>/` with no
  repository or dev overrides required
- Pretty diagnostic renderer — carets, notes, multi-file
  attribution, short-format fallback (all 319 compile-error
  fixtures byte-identical to Stage 0)
- `our fmt` (trivia-preserving) and `our doc` (generated API
  reference) on the self-hosted driver
- Versioned side-by-side toolchain dispatch — `our-<x.y.z>`
  siblings, `toolchain =` manifest pins, `OUR_TOOLCHAIN`
- Installable release archives — `.tar.gz` on POSIX, `.zip` on
  Windows + dist installers (`install.sh`; `install.ps1` on
  Windows) → prefix, idempotent
- `toolv` — a single version source inside the self-hosted tree,
  mirroring `CARGO_PKG_VERSION`

### Changed

- Large-aggregate call ABI moved to pointer/sret lowering — frame
  sizes from megabytes to kilobytes; the giant-stack scaffolding
  workaround is retired
- Toolchain resource identity is `0.2.0` — a new
  `$OUR_HOME/lib/0.2.0/` materializes beside existing `0.1.x`
  installs; siblings are never removed
- Package/toolchain compatibility follows 0.x minor isolation:
  `toolchain = "0.1"` projects dispatch to a `0.1.x` sibling;
  registry `requires` filters against the running minor line

### Fixed

- Qualified/generic type-name resolution parity — unknown App
  bases (params, fields, `let` annotations, nested args, enum/fn
  clone signatures) now produce Stage-0's `unknown struct`
  diagnostic instead of reaching LLVM
- Span-keyed checker→IR side-table collisions (nested scrutinee
  corruption)
- Project-path NUL termination — heap-luck `ENOTDIR` phantom
  diagnostics in project mode
- Remote fetch argv double-NUL and stderr capture parity
- Build/run diagnostic wrapping parity with Stage 0
- Deterministic diagnostic and IR ordering across cloned decls
- Numerous checker, IR, LLVM, package, registry, and bootstrap
  parity fixes across the self-hosting milestones

### Verification

- Native corpus: **759 PASS / 0 FAIL / 1 SKIP** under both Stage 0
  and Stage 1
- Stage-0 ↔ Stage-1 checker parity: **760 fixtures, 0
  divergences, 0 crashes**
- Frontend differentials (diffast/diffgraph/diffsyms/difftokens):
  green
- Stage-2 ↔ Stage-3 `.ll` + `.o`: **byte-identical** fixpoint
- Windows x86_64 validator (Windows 11): **54 pass / 0 fail /
  1 skip**
- Known semantic divergences from Stage 0: **none currently
  known**

### Known limitations

- Linux deferred — builds may work but the platform is
  unvalidated for 0.2.0 and is not a supported target
- Windows release-artifact install path is rehearsed separately
  before tagging (compiler/runtime evidence already complete)
- `ourc` in the release artifact remains the Stage-0
  bootstrap/reference binary — see RELEASES.md
- `clang`/`cc` required for native codegen; `curl` required for
  remote registry access; `lld` additionally on Windows
