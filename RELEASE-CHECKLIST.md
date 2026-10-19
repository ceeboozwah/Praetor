# Release checklist — 0.2.0

The literal gate for publishing `0.2.0`. Items are checked only
with evidence on record; unchecked items block tagging. Until the
platform sections go green, `main` is in **release-candidate
stabilization**: bug fixes, test strengthening, doc corrections,
and release plumbing only — no new language features.

## Done — evidence on record

- [x] **Self-host bootstrap** — Stage-0 → Stage-1 → Stage-2 →
  Stage-3; s2↔s3 `.ll` and `.o` byte-identical fixpoint
- [x] **Native corpus green** — 759 PASS / 0 FAIL / 1 SKIP under
  both Stage 0 and Stage 1
- [x] **Stage-0 ↔ Stage-1 checker parity** — 760 fixtures,
  0 divergences, 0 crashes (`scripts/check-parity.sh`)
- [x] **No known Stage-0 semantic divergences** — N82R closed the
  last (qualified/generic type-name resolution); release-blocking
  policy for any new divergence
- [x] **Release tarball** — `our-0.2.0-<triple>.tar.gz`: `our`
  (self-hosted), `ourc` (bootstrap/reference), `install.sh`,
  `VERSION`, docs, licenses (N83-A)
- [x] **Isolated install proof** — foreign `HOME`/`OUR_HOME`, no
  `OUR_STDLIB`/`OUR_RUNTIME`, cwd outside the repo; compile/run,
  project workflow, registry+offline, resource rematerialization,
  idempotent reinstall (N83-A/N83-C)
- [x] **Licenses** — MIT OR Apache-2.0, `Copyright (c) 2025
  rb labs`, shipped in the tarball (N83-D)
- [x] **Docs/example freshness** — every ` ```lang ` block in
  LANGUAGE.md compiles; `check-langdoc.py` is CI-gated (N83-E)
- [x] **ERRORS.md freshness** — generated doc == registry, unit
  test + CI step (N83-B)
- [x] **Remote registry** — HTTPS/HTTP fetch, lockfile+cache
  parity, `--offline`/`--locked`, failure text (N82-D)
- [x] **Toolchain dispatch** — versioned siblings, real 0.1.x ↔
  0.2.0 side-by-side dispatch proven (N83-C)
- [x] **macOS arm64** — full suite verified on the dev platform

## Done — Windows (N79C, Windows 11 x86-64)

Validated by `scripts/verify-windows.ps1`: **54 pass / 0 fail /
1 skip** (optional `llvm-dwarfdump` step — tool not installed).

- [x] Runtime compiles (`mmlrt_win32.c`, MSVC-target link)
- [x] Smoke: `.lang` compiles, links, runs
- [x] Corpus: 755/0 under s1 — every earlier failure was either a
  fixed defect or a documented POSIX-only assumption
- [x] Self-host on Windows: s1 builds s2; s2↔s3 fixed point
- [x] Project workflow: `our new` → `check`/`test`/`build`/`run`
  under self-hosted `our.exe`
- [x] Canonical `\\?\` paths, recursive `mkdir_all`, external
  clang/lld argv conversion (s0 + s1), byte-preserving I/O,
  Unicode argv/env, sockets/threads/mutexes/channels
- [x] **Stage-1 checker OOB — closed.** Root cause: two
  use-after-frees on `CQ.pkts` (shallow `clone_cq` copy +
  `join_land` adopting then freeing the winner's buffer). Fixed
  in `4db090c`; `*(&mut x)` key parity in `ae776f2`. Regression:
  `native-tests/760_spawn_chan_merge.lang`, verified to crash the
  pre-fix checker.

## Deferred — Linux (N79B)

Deliberately out of scope for 0.2.0: builds may work, but the
platform is **unvalidated**. The release notes and docs must not
claim Linux support. Post-0.2.0 gate when picked up:

- [ ] Runtime compiles (`mmlrt_posix.c` + `mmlrt_common.c`)
- [ ] Smoke: `.lang` compiles, links, runs (exit-code probe)
- [ ] Full native corpus reproduced (currently 759/0/1 on macOS)
- [ ] Self-host on Linux: s1 builds s2; s2↔s3 `.ll` parity
- [ ] Project workflow + registry legs on Linux
- [ ] Install/artifact round-trip on Linux

## Publish steps — last, in order

- [ ] Final clean-checkout release build (`scripts/release.sh`) —
  macOS rehearsal done at `ae776f2`, re-run at tag commit
- [ ] Final Windows release-artifact rehearsal — the shipped
  `.zip`/installer path must be installed and exercised on
  Windows, not just the compiler itself
- [ ] Fresh stranger-test install of the exact artifact
- [ ] Tag `v0.2.0`
- [ ] Publish artifacts + release notes (RELEASES.md §0.2.0)
- [ ] Announce with the evidence table; call it what it is —
  verified macOS arm64 + Windows x86_64, Linux deferred

## Explicitly not required for 0.2.0

- Porting `ourc`'s extended surface (lsp/`--explain`/`test <dir>`)
  to the self-hosted driver — 0.2.x ships `ourc` as the
  documented bootstrap/reference binary
- Source-compat promises across minor lines — VERSIONING.md keeps
  0.x on pin-your-toolchain
- Signed artifacts, crates.io-style registry, or CI-required
  gates beyond the existing freshness checks — post-0.2.0 work
