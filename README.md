<p align="center">
  <img src="assets/praetor.png" alt="Praetor — Authority over the machine." width="640" />
</p>

# Praetor

Praetor is a self-hosted native systems programming language. Its compiler
tracks **authority** — who may read memory, who may write it, who owns it, and
how long it lives — as a static proof obligation, checked before codegen and
enforced across function calls, data structures, and OS threads. What the
checker proves, the generated code does not have to pay for at runtime.

The toolchain is real and installed today: `our` is the self-hosted project
driver and compiler, written in Praetor and compiled by itself; `ourc` is the
Stage-0 bootstrap and reference compiler (Rust); the backend emits LLVM IR and
links native executables through clang, with a small C runtime for threads,
mutexes, and channels. Praetor ships with a standard library, a package and
project system, a formatter, generated API docs, and an LSP server.

Praetor grew out of the **Memory Model Lab** — an experiment that compared
candidate memory models against each other on an adversarial corpus before
committing to one. That research record is preserved in
[HISTORY.md](HISTORY.md); the corpus it produced still runs as a regression
suite.

## Why Praetor?

Systems languages pick a point on a trade: C gives you control and no safety;
Rust gives you safety and asks you to learn a borrow checker. Praetor tests a
different composition:

- **Authority is the primitive, not the borrow.** `read`, `write`, and `own`
  on every binding and parameter say exactly what a callee may do to your
  data — the contract is in the signature, visible at the call site, and
  enforced across the call.
- **Safety decomposes instead of compounding.** Lifetime (region), access
  (capability), and consumption (affine state) are three small machines that
  compose, rather than one large one. Each rule traces to a bug class it was
  added to catch.
- **Concurrency is authority transport, not a library.** `spawn`/`join`,
  channels, mutexes, and atomics are checked against the same authority graph
  as sequential code — a data race is a compile error, not a `Send` bound.
- **Statically proven authority and lifetime properties add no runtime
  enforcement cost.** The checker's output is plain LLVM IR and native code;
  there is no runtime borrow checker, no GC, no capability tags in memory.
  Where a property can't be proven statically (a dynamic bounds index), the
  check is explicit and visible in the emitted IR.

## Core model

Four concepts do all the work:

| Concept | Answers |
|---|---|
| **Region = Lifetime** | every allocation lives in a region; a reference may never outlive it. No lifetime parameters, no named lifetimes — region structure is lexical. |
| **Handle = Identity** | a `handle` is a stable, generation-checked name for a thing. Identity is not authority and not lifetime — a handle can name something whose region already died, and stale-at-use is caught. |
| **Capability = Authority** | `read`/`write`/`own` rights attach to every binding and reference. Authority derives downward only — nothing mints write authority from read access. |
| **Affine state** | consumables (`box`, `own` values, `join` tokens, guards) carry `Live / Consumed / MaybeConsumed`. Use-after-consume is a compile error; ordinary region memory never enters the temporal machine. |

Memory safety = region validity + access capability + minimal temporal state.
The checker implements exactly this; [SPEC.md](SPEC.md) freezes the rules, each
cited to the scenario that forced it.

## Status

| | |
|---|---|
| **Version** | `0.2.0` |
| **Implementation** | self-hosted: the compiler is written in Praetor and compiles itself |
| **macOS** | `aarch64-apple-darwin` — validated |
| **Windows** | `x86_64-pc-windows-msvc` — validated on Windows 11 |
| **Linux** | deferred — builds may work, unvalidated for 0.2.0 |

### A note on GitHub's language bar

GitHub Linguist does not yet recognize Praetor's `.lang` source files, so the
repository sidebar currently overstates Rust and omits Praetor entirely. At the
0.2.0 release snapshot, the repository contains **863 Praetor source files**
(`*.lang`) totaling **2.62 MB**, including **2.07 MB** of self-hosted compiler
source under `selfhost/`. Across the primary programming-source extensions in
the repository, the byte split is approximately **59.0% Praetor**, **37.7% Rust**,
**1.8% C**, with the remainder in PowerShell, Python, and shell scripts. Rust is
the Stage-0 bootstrap/reference implementation; the primary `our` compiler is
written in Praetor and compiles itself.

GitHub Linguist requires broad public usage before accepting a new language, so
Praetor cannot be added upstream yet. Until then, the sidebar language graph
should not be read as the implementation breakdown of the compiler.

Requires `clang` on `PATH` for `.ll` → object → link (both binaries invoke it
externally), and `curl` for remote package registries. Everything else is
embedded in the two binaries.

## Quick start

From a release archive (see [RELEASES.md](RELEASES.md)) — `.tar.gz`
on macOS, `.zip` on Windows:

```bash
tar xzf our-0.2.0-<triple>.tar.gz      # Windows: unzip the .zip
cd our-0.2.0-<triple>
./install.sh            # → ~/.local/bin/{our,ourc} (+ versioned siblings)
# Windows: powershell -ExecutionPolicy Bypass -File .\install.ps1
```

From source:

```bash
./scripts/release.sh    # builds Stage 0 + self-hosted Stage 1, packs the archive
# …then untar and ./install.sh as above
```

Then:

```bash
our new hello
cd hello
our run                 # builds and runs src/main.lang
our test                # runs the tests/ corpus
```

## Example

```lang
import std.io;
import std.vec;

fn sum(read xs: slice i32) -> i32 {
    let total: i32 = 0;
    let i: u64 = 0;
    while i < xs.len {
        total = total + xs[i];
        i = i + 1;
    }
    return total;
}

fn main() -> i32 {
    let heap = system_allocator();
    let v = vec.Vec<i32>::new(heap);
    v = v.push(20);
    v = v.push(22);
    let t = spawn {
        io.println("task ran");
    };
    join t;
    let s = sum(v.as_slice());
    v.destroy();
    if s == 42 { return 42; }
    return 1;
}
```

As a single file — no project needed, `our` compiles one `.lang` directly:

```bash
$ our main.lang && ./main
task ran
$ echo $?
42
```

(Inside a `project.toml` project, `our run` is the normal path — see
Quick start.)

The checker enforces, statically: `read xs` can inspect but never write the
caller's slice; `v.push(…)` takes `own` — the vector is consumed and returned,
so `v = v.push(…)` makes the move explicit; `spawn`/`join` moves authority
into the task and back; `v.destroy()` consumes the `Vec` — the previously
computed scalar `s` is independent of the vector's storage and remains valid.

More programs — a TCP key/value server, a JSON parser, grep, an LRU cache, a
word indexer — live in [`programs/`](programs/).

## Toolchain

Two binaries, self-contained (stdlib and runtime sources are embedded; see
[INSTALL.md](INSTALL.md)):

- **`our`** — the developer-facing driver and compiler, self-hosted (written
  in Praetor, built by Stage 0). Project commands: `new`, `check`, `build`,
  `run`, `test`, `clean`, `rebuild`, `deps`, `fmt`, `doc`, `toolchain` — plus
  `our f.lang` for single files.
- **`ourc`** — the Stage-0 bootstrap/reference compiler (Rust) and extended
  developer utility: `ourc f.lang`, `--show-ast` / `--show-ir` / `--show-llvm`
  / `--show-asm`, `ourc test <dir>` (native corpus runner), `ourc lsp`,
  `ourc fmt`, `ourc --explain <code>` for diagnostics.

The pipeline:

```
.lang source → lexer → parser → loader (imports, generics mono) →
checker (region/capability/affine proof) → our IR → LLVM IR (.ll) →
clang → .o → link (clang/lld + mmlrt.o) → native executable
```

`mmlrt` is a small C runtime (~POSIX/Win32 shims) providing threads, mutexes,
channel queues, allocation, bounds-check traps, and byte-exact I/O. The
checker is where the safety argument lives; everything downstream of it is
ordinary native code.

## Safety model

The four concepts above become concrete rules:

- **Regions** — `region r { … }` scopes allocation; `return &x` into a dead
  region rejects before codegen. `promote` re-homes a binding outward under
  edge-closure (no reference may dangle after the move).
- **Borrows and references** — `&x` / `&mut x` create edges in an alias graph;
  shared+shared may overlap, anything + mutable may not. Reborrow, field
  paths, slices, and FFI pointer coercion all ride the same matrix.
- **Capabilities** — `read`/`write`/`own` parameters are authority contracts:
  `write` may mutate caller storage, `own` takes responsibility (consuming
  the caller's value), `read` can do neither. Derivation only narrows.
- **Affine state** — `box`/`own` values, join tokens, mutex guards, and
  channel endpoints are consumables: move once, consume once, reject reuse.
  `free(x)` *is* consume — double-free is double-consume.
- **Concurrency** — `spawn` records the task's external accesses as holds over
  the `[spawn, join)` window; `send` cuts an authority packet onto the
  channel, `recv` installs it; `mutex` guards serialize access to their
  sync domain; `atomic` ops only synchronize with each other — a mixed
  atomic/plain access is a race, reported at compile time.

Checks the compiler cannot do statically (dynamic slice indexing, `str`
UTF-8 validation) become explicit runtime traps — visible in the `.ll`, never
silent.

## Self-hosting

```
Stage 0   ourc (Rust)         trusted reference compiler — permanent oracle
   │ compiles
Stage 1   our (.lang)         self-hosted compiler — this is the shipped `our`
   │ compiles
Stage 2   our (.lang)         built by Stage 1
   │ compiles
Stage 3   our (.lang)         rebuild — fixed-point / reproducibility check
```

Stage 0 stays in the tree forever as the readable, debuggable behavioral
oracle — `scripts/check-parity.sh` diffs Stage-0 and Stage-1 checker verdicts
across the whole corpus, and the `diffast`/`diffgraph`/`diffsyms`/`difftokens`
suites diff the frontends token-for-token. Stage 2 vs Stage 3 emit
**byte-identical `.ll` and `.o`** — the self-hosted compiler is a fixed point
of itself. See [BOOTSTRAP.md](BOOTSTRAP.md) and
[SELF-HOSTING.md](SELF-HOSTING.md).

## Standard library & project tooling

`stdlib/` is written in ordinary Praetor — `Vec<T>` is a struct over
`box slice T` whose `own`/`write`/`read` signatures *are* the API contract:
`push` may realloc (needs `own`), `as_slice` lends a view that pins the vector
against moves. Modules include `io`, `fs`, `path`, `process`, `net` (sockets),
`vec`, `string`, `strconv`, `sort`, `map`/`omap`, `result`/`option`, `chan`,
`sync`, `thread`, `mem`, `math`. `our doc <pkg>.<mod>` renders the public
surface of any module from source.

Projects use `project.toml` (`[project]`, `[build]`, `[dependencies]`) with
path dependencies and registry packages, transitive resolution, a
`project.lock` with content hashes, `--locked`/`--offline` builds, and
toolchain-version pinning (`[project] toolchain` selects from installed
`our-<ver>` siblings). See [PACKAGES.md](PACKAGES.md) and
[CHANNELS.md](CHANNELS.md).

## Validation

| Gate | Result |
|---|---|
| Native corpus (`ourc test`, `@exit`/`@compile-error` oracles) | 759 pass / 0 fail / 1 skip — under **both** Stage 0 and Stage 1 |
| Stage-0 ↔ Stage-1 checker parity | 760 fixtures, 0 divergences, 0 crashes |
| Frontend differentials (`diffast`/`diffgraph`/`diffsyms`/`difftokens`) | green |
| Self-host fixed point | Stage 2 ↔ Stage 3 `.ll` + `.o` byte-identical |
| Workspace tests | 71 pass / 0 fail |
| macOS arm64 release rehearsal | clean-clone archive, stranger install, byte-identical reproducible tarball |
| Windows x86_64 validator (Windows 11) | 54 pass / 0 fail / 1 skip (optional `llvm-dwarfdump` step, tool absent) |

## Performance

The checker's statically proven authority and lifetime properties require no
runtime enforcement — no GC and no runtime borrow tracking.
Measured against C and Rust on matched algorithms (`bench/`): release builds
run ~1.3–3× C on CPU-bound cells (explicit bounds checks plus conservative
codegen, stated plainly), dead even on syscall-bound cells, ~10× smaller
binaries than Rust, and debug builds beat `rustc -O0` on four of eight.
Full readout: [BENCHMARKS.md](BENCHMARKS.md).

## Design history

Praetor's memory model wasn't designed on paper — it was *selected*. The
Memory Model Lab implemented multiple candidate models — ownership, region,
capability, affine, and several hybrids — and scored them against a
211-scenario adversarial corpus with a model-independent safety oracle. Prototype F —
region ⊕ capability ⊕ minimal affine — won; Phase Three then rebuilt that
winning shape as the real checker's core. The full research record — model
descriptions, corpus results, the evidence for every rule, and the complete
milestone log from experiment to self-hosted compiler — is preserved in
[HISTORY.md](HISTORY.md). The `corpus/` suite still runs under `cargo test`.

## Documentation

| Topic | Doc |
|---|---|
| Language reference | [LANGUAGE.md](LANGUAGE.md) |
| Semantic rules (frozen v0) | [SPEC.md](SPEC.md), [SEMANTIC-COVERAGE.md](SEMANTIC-COVERAGE.md) |
| Error catalog | [ERRORS.md](ERRORS.md) |
| Installing & distribution | [INSTALL.md](INSTALL.md), [RELEASES.md](RELEASES.md), [VERSIONING.md](VERSIONING.md), [CHANNELS.md](CHANNELS.md) |
| Packages & registries | [PACKAGES.md](PACKAGES.md) |
| Bootstrap / self-hosting | [BOOTSTRAP.md](BOOTSTRAP.md), [SELF-HOSTING.md](SELF-HOSTING.md) |
| Portability | [PORTABILITY.md](PORTABILITY.md), [WINDOWS-PORT.md](WINDOWS-PORT.md) |
| Benchmarks | [BENCHMARKS.md](BENCHMARKS.md) |
| Research & milestone log | [HISTORY.md](HISTORY.md) |
| Release process | [RELEASE-CHECKLIST.md](RELEASE-CHECKLIST.md), [CHANGELOG.md](CHANGELOG.md) |

## License

Licensed under either **MIT** or **Apache-2.0**, at your option —
`license = "MIT OR Apache-2.0"`:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT License ([LICENSE-MIT](LICENSE-MIT))
