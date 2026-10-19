# Release checklist — 0.2.0

The literal gate for publishing `0.2.0`. Items are checked only
with evidence on record; unchecked items block tagging. Until the
platform sections go green, `main` is in **release-candidate
stabilization**: bug fixes, test strengthening, doc corrections,
and release plumbing only — no new language features.

## Done — evidence on record

- [x] **Self-host bootstrap** — Stage-0 → Stage-1 → Stage-2 →
  Stage-3; s2↔s3 `.ll` byte-identical fixpoint
- [x] **Native corpus green** — 757 PASS / 0 FAIL / 1 SKIP
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

## Pending — Linux (N79B)

Held to the **zero-failure** expectation: every corpus fixture
must compile, run, and match on stock Ubuntu.

- [ ] Runtime compiles (`mmlrt_posix.c` + `mmlrt_common.c`)
- [ ] Smoke: `.lang` compiles, links, runs (exit-code probe)
- [ ] Full native corpus: 757/0/1 reproduced
- [ ] Self-host on Linux: s1 builds s2; s2↔s3 `.ll` parity
- [ ] Project workflow + registry legs on Linux
- [ ] Install/tarball round-trip on Linux

## Pending — Windows (N79C)

Different bar: a fixture failure is acceptable **only after
classification** — either a real compiler/runtime defect to fix,
or an explicitly unsupported POSIX-assuming test assumption to
document and skip with a marker. Zero unclassified failures.

- [ ] Runtime compiles (`mmlrt_win32.c`, `lld` link)
- [ ] Smoke: `.lang` compiles, links, runs
- [ ] Corpus: every failure classified (defect vs. documented
  platform assumption) — report-only is fine, unexplained is not
- [ ] Self-host on Windows: s1 builds s2
- [ ] Project workflow on Windows

## Publish steps — last, in order

- [ ] Final clean-checkout release build (`scripts/release.sh`)
- [ ] Fresh stranger-test install of the exact artifact
- [ ] Tag `v0.2.0`
- [ ] Publish artifacts + release notes (RELEASES.md §0.2.0)
- [ ] Announce with the evidence table; call it what it is —
  verified macOS arm64, Linux/Windows results as recorded above

## Explicitly not required for 0.2.0

- Porting `ourc`'s extended surface (lsp/`--explain`/`test <dir>`)
  to the self-hosted driver — 0.2.x ships `ourc` as the
  documented bootstrap/reference binary
- Source-compat promises across minor lines — VERSIONING.md keeps
  0.x on pin-your-toolchain
- Signed artifacts, crates.io-style registry, or CI-required
  gates beyond the existing freshness checks — post-0.2.0 work
