# CHANNELS.md — release channels & toolchain selection (N63)

Status: **implemented** — `compiler/tests/toolchain.rs` pins every
rule here: satisfied pins run without dispatch, unsatisfiable pins
hard-fail (never fall back, never download), `OUR_TOOLCHAIN` beats
the manifest, the sibling pool is a filename convention, and a
mislabeled `our-<ver>` binary is caught by the loop guard rather
than looping forever.

How does a user move between installed toolchain versions without
silently changing project behavior? The philosophy is the same one
the package system already encodes: **explicit state transitions,
reproducibility first, convenience second.**

Invariants, up front:

- **no silent auto-upgrade** — nothing ever replaces a binary you
  invoked
- **no silent project retargeting** — a project's toolchain choice
  comes from its own `project.toml` or an explicit env override
- **no implicit network access** — the toolchain never downloads
  itself; `curl` exists for the package registry only (PACKAGES.md
  §11), and toolchain acquisition is a deliberate `install.sh` /
  tarball act by the user
- **dispatch never writes** — selecting a toolchain is read-only
  process routing

## 1. Channels are distribution labels, not tool state

A *channel* is a named stream of releases — today exactly one
exists:

- **`stable`** — every tagged release. In 0.x all releases are
  stable-tagged; VERSIONING.md already governs what 0.x means.

Toolchain binaries are **channel-blind**: `our --version` reports a
version, never a channel. Channel membership lives in release
metadata (the git tag, the tarball name
`our-<ver>-<triple>.tar.gz`), nowhere in the tools. A `nightly`
channel is deferred — when it arrives it will be a separate release
stream, still just versioned tarballs. No manifest key, index
entry, or resolution rule may ever reference a channel name:
packages and projects constrain *versions*, period.

## 2. Side-by-side installation is the norm

`scripts/install.sh` installs each toolchain twice:

```
<prefix>/bin/our            ← the "current" copy (last installed wins)
<prefix>/bin/our-<x.y.z>    ← the versioned sibling — never removed
                                by installing something else
<prefix>/bin/ourc           same pair for the single-file compiler
<prefix>/bin/ourc-<x.y.z>
```

Installing 0.2.0 does not remove `our-0.1.0`. A manually installed
tarball joins the pool the same way: **any executable named
`our-<x.y.z>` sitting beside the `our` you invoked is a dispatch
candidate.** The pool is a filename convention, not a database —
`ls` is the registry.

`$OUR_HOME/lib/<ver>/` stays per-version (INSTALL.md), so each
dispatched binary materializes only its own resources
(VERSIONING.md §5).

## 3. Projects can request a toolchain — `[project] toolchain`

The name N62 reserved now carries semantics:

```toml
[project]
toolchain = "0.2"        # caret req — any 0.2.x
toolchain = "=0.2.1"     # or an exact pin
```

Same req grammar as dependencies (`semver::Req`: `=`, `^`, bare).
Evaluation order, first match wins:

1. `OUR_TOOLCHAIN` env var — explicit operator override beats the
   manifest (useful for testing a project against another toolchain)
2. `[project] toolchain` — the manifest's own request
3. no request → the invoked binary runs, whatever version it is

When a req exists and the **running** binary satisfies it: proceed,
zero cost — the manifest was already being read.

When it doesn't: scan the directory containing the invoked binary
for `our-<x.y.z>` siblings, pick the **highest** satisfying version,
and **re-exec** it with the identical argv and environment plus
`OUR_DISPATCHED=<ver>`.

When no sibling satisfies: hard error — never fall back, never
download:

```
our: project.toml requires toolchain `^0.2` — running 0.1.0,
     installed: 0.1.0
     install a 0.2.x toolchain (install.sh / release tarball)
     or adjust the pin
```

## 4. Dispatch cannot loop — mislabeled binaries are caught

A versioned filename is a claim; the binary's real version is the
truth. The re-exec sets `OUR_DISPATCHED=<ver>`; a binary that starts
with that var set and *still* fails the req stops immediately:

```
our: dispatched to `our-0.1.9` but it reports 0.1.0 — a sibling's
     filename does not match its toolchain; fix the install
```

No infinite re-exec chains, ever. Dispatch happens at manifest load
in `cwd_project()`, so every project-bound command —
`check`/`build`/`run`/`test`/`deps`/`clean`/`rebuild`/`fmt` — honors
the pin identically. `our doc` honors it when a project is present
but still works outside one (`our doc std.io` needs no project).
`our new` and single-file `ourc` invocations never dispatch — they
carry no project pin context. (`ourc` the binary is always exactly
what you invoked; projects pin through `our`.)

Dispatch precedes all resource materialization, so pre-materializing
resources "for" a project is a non-question by construction.

## 5. Upgrades and downgrades are the same act

**Upgrade** = install a newer version. `install.sh` drops the new
versioned sibling and updates the `our`/`ourc` copies. Projects
without a pin pick up the new version on the next invocation;
pinned projects keep routing to their pin.

**Downgrade** = install the older version again (its binaries
become `our`/`ourc` once more), or just invoke `our-<oldver>`
directly — the dispatcher pool doesn't care which copy sits at the
unversioned name.

**Uninstall** = delete the binaries. `$OUR_HOME` remains a pure
cache — nothing there is required state.

## 6. What resolution is allowed to see

Package resolution gates on **versions** — the registry index
`requires` field (PACKAGES.md §10) compares against the *selected*
toolchain's version after dispatch. Channel names, install order,
and `OUR_HOME` contents are invisible to resolution: two machines
with the same toolchain version and the same lockfile resolve
identically regardless of how the binaries got there.

## 7. Deferred

- `our self-update` / `our install <ver>` — acquisition commands;
  when they exist they will still be explicit invocations, never
  implicit
- `nightly` channel and per-channel install slots
- channel index files / remote release metadata — needs a real
  distribution endpoint first
- `ourc` project-mode dispatch (today it is deliberately
  project-blind)
- signed releases / checksum verification of tarballs
