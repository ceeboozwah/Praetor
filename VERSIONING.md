# VERSIONING.md — version & compatibility contract (N62)

Status: **implemented** — `compiler/tests/versioning.rs` pins every
rule here: the format gate, version identity, the coexistence
invariant, the reserved `toolchain` name, forward-compat unknown
keys, and prerelease rejection.

What does `0.1.0` promise? This document freezes how toolchain,
package, lockfile, stdlib, and runtime versions interact — and what
an upgrade may and may not do to you.

## 1. One version, one release unit

There is exactly one version number: **`CARGO_PKG_VERSION`** in
`compiler/Cargo.toml`. It names the *toolchain* — the compiler, the
stdlib, the runtime, `our`, and `ourc` — as a single release unit.
Stdlib and runtime are not independently versioned; they are
embedded in the binaries and cannot drift from them (INSTALL.md §2).

`our --version` and `ourc --version` print it. `build/build.info`
records it per artifact. The registry `requires` gate compares
against it. One number, everywhere.

## 2. What 0.x promises — and what it doesn't

**0.x is pin-your-toolchain.** Within 0.x, minor/patch bumps may
change language semantics, diagnostics, stdlib APIs, and codegen.
The honest commitment is not stability — it is *legibility*:

- the language has no source-level version marker; the corpus
  (`native-tests/`, 744 pins) is the de facto spec
- programs may need edits across toolchain versions; the compiler
  does not promise to accept old source
- a `project.lock` written by toolchain A *is* honored by toolchain
  B within the same lockfile format version (§4)

This deliberately under-promises. When the language stabilizes, 1.0
adopts real semver promises; that is a future contract, not this
one.

## 3. Versions are `x.y.z` only — no prereleases

Toolchain versions, package versions, and `requires` reqs are
exactly `MAJOR.MINOR.PATCH` (reqs: `=x.y.z`, `^x.y.z`, bare). There
are **no prereleases** (`-rc1`, `-alpha`) and no build metadata
(`+sha`) in 0.x — `semver::parse_ver` rejects them today and that is
the contract. Nightly-style channels, when they arrive (N63), are
*channel names*, not version strings.

## 4. Lockfile format versioning

`project.lock` gains an explicit `format = 2` in `[lock]`:

- `format` absent → treated as the current format (2) — the field
  did not exist when 2 was written, so all existing files are 2
- `format` **newer** than understood → **hard error** naming the
  file and the toolchain floor it implies. A newer lockfile is never
  silently misread.
- `format` **older** than understood → treated as stale: re-resolved
  and rewritten, exactly like a missing lockfile (and still an error
  under `--locked`, which never rewrites)

The `tool = "ourc <ver>"` line stays informational — audit metadata,
not a gate. Version *skew* is fine; format *drift* is not.

## 5. Side-by-side versions coexist — and never touch each other

`$OUR_HOME/lib/<ver>/` is per-version (INSTALL.md). The invariant,
now pinned:

> **A toolchain may create or rewrite files only under its own
> `$OUR_HOME/lib/<own-ver>/`. Anything under another version's
> directory is read-never, write-never.**

The dep cache `$OUR_HOME/registry/` is deliberately *shared* across
versions: entries are content-addressed by SHA-256, and the
canonical tree hash is version-independent, so two toolchains can
safely draw from one cache. A toolchain version that changed the
hash algorithm would need a new cache layout — a documented format
break when it happens, not a silent collision.

## 6. Package `requires` semantics (restated, frozen)

An index entry's `requires` is a req against the **toolchain**
version — the only thing a package can constrain, since stdlib and
runtime are the toolchain. An entry whose `requires` excludes the
running toolchain is *invisible to selection* (N60B): resolution
picks the highest satisfying *and* compatible version. This is a
filter, not an error — an app can still resolve if an older
compatible package version exists.

Projects may not declare a toolchain requirement in 0.x; that field
is reserved (`toolchain` in `[project]` is a recognized-reserved
name) for when it has honest semantics.

## 7. Manifest/index forward compatibility

Unknown keys in `project.toml` and index entries are **ignored** —
the forward-compat seam (a newer manifest carrying fields an old
tool doesn't know must still resolve). Values must still parse in
the TOML subset (string / integer / bool / dep-table) — unknown
*keys* are free, malformed *syntax* is not. This is a deliberate
trade-off: misspelled known keys are also silently ignored.
Accepted cost in 0.x; a strict-mode lint is the fix when it
matters.

## 8. What an upgrade is

Upgrading = replacing the binaries. The new toolchain materializes
its own `lib/<new-ver>/`, shares the registry cache, and honors or
regenerates the lockfile per §4. There is no in-place migration and
no hidden global state. Downgrading is symmetric — old binaries and
old `lib/<old-ver>/` trees still work untouched.

## 9. Deferred

- 1.0 stability promises (language/source compat, deprecation policy)
- release channels (`stable`/`nightly`) as installable streams — N63
- `our self-update` / upgrade tooling
- `[project] toolchain = "req"` enforcement — reserved name, no
  semantics yet
- lockfile v3 triggers: hash-algorithm change, cache format change —
  will be documented here when introduced
- stdlib/runtime independent versioning — the coupling is the point
  in 0.x; split only if the language outlives toolchain cadence
