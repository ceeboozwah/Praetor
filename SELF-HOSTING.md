# SELF-HOSTING.md — feasibility audit (N64)

Status: **audit complete**

The question: *what features used by the current Rust compiler cannot
yet be expressed efficiently and safely in `.lang`?* Audited by
capability against the real implementation surface — 27.7k lines of
Rust: `check.rs` 10.7k, `ir/gen.rs` 4.4k, `mono.rs` 1.6k, `parser.rs`
1.3k, `emit.rs` 1.2k, `lsp.rs` 1.1k, plus driver/frontend/support.

The Rust compiler's actual stdlib diet, measured:

```
1072 .clone()      802 String        770 format!       477 Vec<T>
 373 .iter()       291 HashMap       113 HashSet        61 Mutex
  55 fs::*          42 process::*     36 Box            23 RefCell
  11 Command::       7 OnceLock        4 BTreeMap        2 dyn-adjacent
```

## Classification

Each capability, with the verdict bucket:

- **expressible** — `.lang` + stdlib do this today, pins exist
- **painful** — expressible, but the idiomatic Rust shape must be
  restructured into something the ownership model prefers
- **stdlib gap** — the language suffices; a module/function is
  missing. Mechanical work, no design risk
- **runtime gap** — needs new `mml_*` primitives in the C runtime
- **language gap** — needs a language feature (each is examined for
  whether restructuring avoids it)
- **later** — not worth self-hosting yet

## 1. Already expressible

| Capability | Evidence |
|---|---|
| Recursive AST/IR data | enums with payloads + `box`; `match` with bind modes |
| Generics (incl. multi-param) | `Vec<T>`, `Result<T,E>`, `StrMap<V>` all instantiate today |
| Error propagation | `Result<T,E>` + `?` — the whole stdlib is built on it |
| Byte-level lexing/parsing | `str.bytes`, `slice u8`, slicing — a lexer is byte iteration |
| Diagnostics accumulation | `Vec<Diag>` + spans — plain structs in a Vec |
| Arena allocation | `alloc` tokens are allocator-parameterized — a bump arena is a *custom allocator*, no language work needed |
| Dependency graphs / topo-sort | Vec + StrMap — DAG walks, `visiting`/`done` sets are str-keyed |
| String-keyed maps | `StrMap<V>`/`OMap<V>` — **~85% of the compiler's maps are `HashMap<String,_>`/`HashSet<String>`** |
| SHA-256 | pure bit-twiddling over `slice u8` — no special support needed |
| LLVM emission | `emit.rs` writes `.ll` **as text into a String** — codegen is string-building |
| stdout/stderr | `io.print/eprint` family exists |
| Tasks/mutexes/channels | full concurrency surface exists (though a compiler barely needs it) |
| `defer` | exists — teardown-heavy affine code's pressure valve |

## 2. Expressible but painful — architecture, not features

| Pattern | The friction |
|---|---|
| `.clone()` × 1072 | Rust's ownership made cloning the escape hatch; `.lang`'s affine model makes every copy explicit. The honest answer isn't 1072 `dup()` calls — it's the **arena + index** idiom: AST/IR nodes live in an arena (`alloc`), references are `u32` indices, "cloning" is copying an index. Cheaper *and* checkable. This is a rewrite-the-data-shape cost, not a wall. |
| `.iter()` × 373 | No iterators. Cursor iteration (`next_entry`) + explicit loops cover it — verbose but mechanical. |
| `RefCell` × 23 | Interior mutability through shared refs (shared string table, `bind_modes`). `.lang` has no equivalent — restructure to an explicit `write ctx` parameter threaded through passes. Signature churn, cleaner dataflow afterward. |
| `OnceLock` lazy statics | No globals. Caches become fields on an explicit `Ctx` struct built in `main`. Painful-trivial. |
| `format!` × 770 | Covered in §3 — once `String.push_*` exists, each `format!` becomes a few appends. No varargs → printf-style is impossible; builder-style is fine. |

## 3. Stdlib gaps — mechanical, no design risk

| Missing | Needed for | Shape | Status |
|---|---|---|---|
| ~~`spawn`/`exec`+wait~~ | orchestration | `process.run` (N65B) | **done** |
| Int/struct-keyed map | `HashMap<Span,_>`, `HashSet<(usize,u32,u32)>` — the 15% of maps with non-str keys | `imap.IMap<V>` (i64 keys, fmix64; `pack2` for u32 pairs/Span) + `omap` `*_kbuf` read surface for wider packed keys | **done (N66)** |
| `String.push_i64/push_u64/push_byte` | `format!`-into-String (770 uses), `.ll` emission, diagnostics | `string.push_*` — builder-style appends; no varargs, no format grammar | **done (N66)** — `{:04x}`/`{:?}` (3 total) can stay manual |
| `sort` | dep ordering, symbol tables, deterministic output | `sort.sort_str`/`sort_i64` — insertion sort, byte-wise/numeric; no comparators exist | **done (N66)** |
| `Option<T>` | pervasive in the Rust code | `option.Option<T>` — 3-variant-minimal enum + `unwrap`/`is_some`/`is_none` | **done (N66)** |
| Path ops | `join`×105, `strip_prefix`×32, `parent`×13, `file_stem`/`file_name`×14, ext ops×8 | `path` module — decomposition returns `Option<str>` views (zero-alloc), composition returns `String` | **done (N66)** |
| ~~`readdir` binding~~ | dir discovery | `fs.read_dir` (N65A) | **done** |
| `stdin` helper | LSP reads stdio | `fs.fd_read(0,…)` works today; a named wrapper is polish | open |
| JSON writer | LSP protocol | string-building; `emit.rs` proves the pattern scales | open |

## 4. Runtime gaps — the real blockers

~~Two missing `mml_*` primitives gate the whole effort~~ — **closed by
N65** (POSIX verified on macOS; Win32 written blind alongside the rest
of `mmlrt_win32.c`, pending a real host):

| Was missing | Now |
|---|---|
| ~~process spawn~~ | `mml_proc_run` + `process.run` — exec + argv block + cwd + inherited stdio + exit status / `Err(errno)` on spawn failure. No pipes/capture yet — sufficient for the clang/curl orchestration shape (stdout/stderr passthrough). v2 adds capture only if bootstrap needs it. |
| ~~directory listing~~ | `mml_dir_*` + `fs.read_dir` → sorted `DirList` — `.`/`..` skipped, file/dir/other classification, non-UTF-8 names invisible. |
| ~~`mkdir`/`remove`~~ | `mml_mkdir`/`mml_remove` + `fs.mkdir`/`fs.remove` — landed with N65A since enumeration pins need fixture control anyway. |

Still open, work-aroundable: `stat`/`exists` (today faked via
open-failure), `rename`, `remove_dir_all` (a self-hosted driver wants it
for cache management).

## 5. Language gaps — examined for whether restructuring avoids them

| Feature | Verdict |
|---|---|
| Traits / bounded generics | **Avoidable.** All uses are conventions (Hash/Eq/Display/Debug/Clone). Push ops to callers (caller-hashed maps, `push_str` variants per type) and the need evaporates. Not a blocker. |
| Closures / fn values | **Avoidable.** Zero `dyn`, two `impl Iterator` params in the whole codebase. Visitor patterns become explicit recursion; sort becomes per-type. Reorganizing code, not missing semantics. |
| Iterators | **Avoidable.** Cursor + loops; N59A proved the idiom. |
| Interior mutability | **Avoidable** — explicit `write` threading (§2). |
| Global/lazy mutable state | **Avoidable** — `Ctx` structs. |

No language feature is a hard wall. The honest cost is *shape*:
the `.lang` compiler won't look like the Rust one transliterated —
it will look like an arena-indexed, context-threaded pipeline, which
is the architecture the ownership model actually rewards.

## 6. Not worth self-hosting yet

| Piece | Why defer |
|---|---|
| `ourc lsp` | Needs the whole pipeline + stdin + JSON; benefits from everything else landing first. Real but last. |
| `ntest` corpus runner | ~~Needs dir-walk + per-pin spawn~~ — both landed in N65; the .lang port of the runner is now unblocked. |
| `our doc`/`fmt` | Self-contained once the frontend self-hosts; no extra gaps. |
| The Windows/Linux runtime halves | Self-hosting *expands* the runtime surface the port must cover — `mml_spawn`/`mml_readdir` would be new blind-Win32 work. Sequence: audit → gaps → host verification → then bootstrap. |

## Verdict

**Self-hosting is mostly engineering.** The two hard runtime blockers
(`spawn`, `readdir`) **landed in N65** — POSIX-verified, Win32-blind.
What remains is a tractable list of stdlib additions (int-map, string formatting, sort, `Option`,
paths). **No fundamental language barrier was found** — the deepest
frictions are idiomatic (no closures/traits/iterators → a different
code shape), not expressive.

The interesting finding is directional: the `.lang` version wants
the *better* architecture anyway — arena + indices instead of
clone-heavy trees, explicit `Ctx` instead of lazy statics. The
audit suggests self-hosting would produce a *cleaner* compiler, not
just a translated one.

## Suggested sequence (not this milestone)

1. ~~**N65**: runtime gaps — `mml_spawn`, `mml_readdir`, `remove`/
   `mkdir`~~ **done** — `mml_proc_run`, `mml_dir_*`, `mml_mkdir`,
   `mml_remove` + `.lang` bindings + pins 745/746
2. ~~**N66**: stdlib fills — `I64Map`, `String.push_*`/strconv append,
   `sort_str`/`sort_i64`, `Option`, `path` module~~ **done** —
   `imap.IMap`, `omap` kbuf reads, `option.Option`, `string.push_*`,
   `path` module, `sort` module + pins 747–752
3. ~~**N67**: bootstrap architecture — stage0 stays the Rust `ourc`,
   or freeze a seed binary?~~ **decided** — `BOOTSTRAP.md`: Rust
   `ourc` is the permanent Stage-0 oracle (no seed binary yet), the
   `.lang` compiler is arena+ID-shaped (not transliterated), every
   layer gets a canonical golden-diff surface (`--dump-tokens` for
   N68's lexer is already live), self-hosting threshold = Stage-2
   corpus equivalence
4. **N68+**: port by layer — lexer → parser → loader → types/symbols
   → checker → IR → LLVM emitter → driver, each gated on its golden
   differential going green across the corpus (BOOTSTRAP.md §5–6).
   ~~**N68** lexer~~ **done** — `selfhost/lexer.lang` +
   `selfhost/dumptokens.lang`: byte-identical `--dump-tokens` vs
   Stage-0 across the corpus (827 files), edge cases, and seeded
   fuzz; `difftokens` makes it a permanent `cargo test` gate
