# INSTALL.md — toolchain distribution contract (N61)

Status: **implemented** — `compiler/tests/install.rs` pins the
stranger test (§7): isolated HOME/OUR_HOME, dev overrides removed,
both binaries exercised end to end.

The question this answers: *can someone without the repository clone
install the toolchain, create a project, build it, and use the
package system?* This document freezes what an installation is
before any mechanism is built.

## 1. The artifacts are two self-contained binaries

The toolchain ships as **two executables**:

- `our` — the **self-hosted** project driver and compiler:
  new/check/build/run/test/fmt/doc/deps, and `our f.lang` for
  single files
- `ourc` — the bootstrap/reference compiler and extended
  developer utility: `ourc f.lang`, `ourc lsp`, `ourc test <dir>`,
  `ourc --explain`, `ourc fmt`. In 0.2.x `ourc` remains the
  Stage-0 reference binary — `our` is the primary self-hosted
  toolchain (RELEASES.md).

Both link the `ourc` library, so both carry the same embedded
resources and the same version.

Each binary must work when the pair is the *only* thing on the
machine. Therefore the resources the compiler needs —
`stdlib/*.lang` and `compiler/runtime/*.{c,h}` — are **embedded at
build time** (a `build.rs`-generated table, still zero crate deps)
and materialized to disk on first use. Nothing is read from
`CARGO_MANIFEST_DIR` at runtime: a compile-time checkout path is not
an install location, and silently works-until-you-delete-the-clone
is the worst failure mode.

## 2. Resource resolution

The stdlib root resolves in this order (runtime sources identically):

1. `OUR_STDLIB` / `OUR_RUNTIME` env vars — dev and debugging
   override. Point at a checkout or a patched copy.
2. `$OUR_HOME/lib/<tool-version>/stdlib` (and `.../runtime`) —
   materialized from the embedded bytes on first use, per file,
   write-if-different. `OUR_HOME` defaults to `~/.our`.

Materialized files are real files — diagnostics, `our doc`, and the
LSP reference real paths, and a user can read the stdlib source they
are compiling against. Deleting `$OUR_HOME/lib` is always safe: it
re-materializes on the next invocation. The `<tool-version>`
directory level keeps two installed toolchain versions from
fighting over one tree.

The dev loop is unchanged: `cargo test` sets `OUR_STDLIB`/
`OUR_RUNTIME` to the repo paths, so the 744-pin corpus reads live
sources with no materialization step in the way.

## 3. Version identity

`our --version` / `ourc --version` print `<CARGO_PKG_VERSION>` — one
source of truth. The registry's `requires` toolchain gate already
compares against the same value (N60B), so an installed tool's
version is what package resolution sees.

## 4. Install methods

- **Scripted (from a clone)**: `scripts/install.sh [prefix]` →
  Stage-0 build + Stage-1 self-compile → `<prefix>/{our,ourc}`
  (default `~/.local/bin`). This is the same `our` the release
  archive ships: the self-hosted Stage-1 driver.
- **Release archive**: `scripts/release.sh` →
  `our-<ver>-<triple>.tar.gz` on POSIX, `our-<ver>-<triple>.zip`
  on Windows. Both carry `our`, `ourc`, docs, and licenses.
  POSIX: untar, run `install.sh`. Windows: unzip, run
  `powershell -ExecutionPolicy Bypass -File .\install.ps1` (or
  `install.sh` under Git Bash) — both install
  `~/.local/bin/{our,ourc}(.exe)` plus versioned siblings.
- **`cargo install --path compiler`** — installs the **Stage-0**
  `ourc` *and the Rust `our` dev driver* to `~/.cargo/bin`.
  Supported, but note the `our` it installs is the Rust
  bootstrap-side driver, not the self-hosted Stage-1 binary the
  release artifact ships. For the release-exact pair, prefer
  `scripts/install.sh` or the archive.

## 5. Host requirements

`our` shells out for codegen-adjacent work; an install needs:

- `clang` — compiles the emitted `.ll` and the runtime `.c` files
  (`cc` fallback), plus `lld` on Windows (`-fuse-ld=lld`)
- `curl` — remote registry transport only (N60C); path/directory
  registries and offline builds never touch it
- `git` is *not* required — no shell-outs to it anywhere

## 6. Post-install filesystem layout

```
~/.our/                         ($OUR_HOME)
  registry/<name>/<ver>/        package cache (N60B)
  lib/<tool-version>/
    stdlib/*.lang               materialized from embedded bytes
    runtime/mmlrt*.{c,h}        materialized from embedded bytes
```

Everything under `$OUR_HOME` is a cache — deletable, rebuildable,
never edited by hand. Nothing else is written outside the project
directory (`build/`, `project.lock`).

## 7. The stranger test — acceptance criterion

N61 is done when this passes, driven by a test that:

1. installs to a throwaway prefix (release or debug `our` binary)
2. runs with `HOME`/`OUR_HOME` isolated, `OUR_STDLIB`/`OUR_RUNTIME`
   unset, cwd outside the repo — the materialization path is
   exercised, not bypassed
3. `our new hello` → `our test` → `our run` works, and `ourc` compiles
   a bare `.lang` file
4. a registry dep (fixture index + `OUR_REGISTRY`) resolves,
   verifies, caches, builds — then `--offline` rebuilds with the
   registry deleted

Codified in `compiler/tests/install.rs`.

## 8. Deferred

- version bump policy / release channels / signed artifacts
- platform packages (brew formula, scoop manifest, distro packages)
- `our self-update`, uninstall command, man pages, shell completions
- stdlib *override* packages — `OUR_STDLIB` is a debugging seam, not
  a support surface
