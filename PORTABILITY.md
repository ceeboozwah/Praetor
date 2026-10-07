# Portability audit (N58A)

Inventory of every assumption that binds the toolchain to macOS /
POSIX / the Apple ABI. Classified, not yet fixed. Columns: does it
work on each platform today, what's the classification, what's the
action.

**Classification key**

- `portable C` — ISO C, works everywhere
- `POSIX` — works on macOS + Linux, not Windows
- `Apple-only` — breaks or misbehaves off Apple platforms
- `Windows-incompat` — needs a real abstraction, not a shim
- `LLVM-target` — handled by clang, but per-target
- `tooling` — test/dev-only concern, not shipped semantics

## 1. Runtime — `compiler/runtime/mmlrt.c` (434 lines)

| Surface | macOS | Linux | Windows | Class | Action |
|---|---|---|---|---|---|
| malloc / free / memcpy / memset / strlen | yes | yes | yes | portable C | none |
| abort / fprintf / stderr traps | yes | yes | yes (exit 3, not SIGABRT) | portable C | none (exit-code note below) |
| `errno` | yes | yes | yes (but Winsock uses `WSAGetLastError`) | portable C | net layer needs a per-OS error channel |
| `pthread_create`/`join`/`mutex`/`cond` | yes | yes | no | POSIX | split rt: `mmlrt_posix.c` + `mmlrt_win32.c` (threads, `CRITICAL_SECTION`, `CONDITION_VARIABLE`) |
| `open`/`read`/`write`/`close` (via `mml_*` shims) | yes | yes | partial — `_open`/`_read`/`_write`/`_close` exist | POSIX | same shims work; must always pass `O_BINARY` (default is `O_TEXT`: `\n`→`\r\n` mangles files) |
| sockets (`socket`/`bind`/`connect`/`listen`/`accept`/`setsockopt`) | yes | yes | partial — Winsock API is near-identical but needs `WSAStartup`, `SOCKET` type, `closesocket`, different errno | POSIX | route all calls through `mml_` shims (mostly done); per-OS init + error read |
| `open(2)` variadic + mode (`mml_open_mode`) | yes | yes | n/a — `_open` is not variadic | POSIX | shim survives; mode/flag VALUES differ per OS (see std.fs) |
| `getenv` | yes | yes | yes (ANSI, not UTF-8) | POSIX | Windows needs `GetEnvironmentVariableW` + UTF-16→8 for non-ASCII values |
| `getcwd` | yes | yes | `_getcwd` | POSIX | name difference only |
| `_exit` | yes | yes | `_exit` exists | POSIX | none |
| argv → `slice str` (UTF-8 validation) | yes | yes | broken — narrow `argv` is ANSI, not UTF-8 | POSIX | Windows needs `wmain`/`GetCommandLineW` + UTF-16→UTF-8, else non-ASCII args trap or garble |
| `main` + stack VLA for argv array | yes | yes | **no — MSVC doesn't support VLAs** | portable C99 | `malloc` the array on Windows (or cap argv) |
| `__attribute__((noreturn))` | yes | yes | no — `__declspec(noreturn)` | GCC-ism | macro: `MML_NORETURN` |
| `ssize_t` | yes | yes | no | POSIX | use `ptrdiff_t`/`intptr_t` in shims |
| `char payload[]` (flex array) | yes | yes | yes | portable C99 | none |

## 2. Compiler driver — `compiler/src/driver/pipeline.rs`, `main.rs`

| Surface | macOS | Linux | Windows | Class | Action |
|---|---|---|---|---|---|
| `target_triple()` | yes | yes (x86_64 + aarch64 already mapped) | **absent — falls through to `unknown-unknown-unknown`** | LLVM-target | add `x86_64-pc-windows-msvc` / `-gnu` |
| `cc()` toolchain resolution | `xcrun -f clang` | `clang`/`cc`/`gcc` probe | `clang.exe` needed (LLVM or VS Build Tools); `clang-cl` won't accept our gcc-style flags | tooling | probe `clang` on PATH; Windows needs MSVC env (`LIB`/`INCLUDE`) or `--target=` |
| `cc_args()` SDK root | `xcrun --show-sdk-path` | none needed | none needed | Apple-only | already gated on `cfg!(macos)` |
| link flags | none | `-lpthread` (already conditional) | `ws2_32` for sockets; CRT default via clang | POSIX | add per-OS lib list |
| `.ll` → object | clang | clang | clang (emits COFF for msvc target) | LLVM-target | none — same driver |
| `mmlrt.o` build (`cc -O2 -c`) | yes | yes | needs the rt split first | tooling | none once rt compiles |
| binary naming `build/<name>` | yes | yes | **produces `name.exe` — `join(name)` won't find it** | tooling | append `.exe` on Windows |
| `--show-asm` (`clang -S -x ir`) | yes | yes | yes | LLVM-target | none |
| exit-status semantics (`o.status.code().unwrap_or(-1)`) | SIGABRT → `-1` | SIGABRT → `-1` | abort → exit **3** | POSIX | pins asserting `-1` are Unix-semantic; gate or translate |

## 3. stdlib — `stdlib/*.lang`

| Surface | macOS | Linux | Windows | Class | Action |
|---|---|---|---|---|---|
| `mem`/`vec`/`omap`/`map`/`string`/`strconv`/`math`/`result` — memcpy/memset/memmove | yes | yes | yes | portable C | none |
| `sync`/`chan` — built-in `mutex`/`channel`/`spawn` → `mml_*` rt | yes | yes | no (pthreads) | POSIX | rides the rt split |
| `fs` — `open`/`read`/`write`/`close` via `mml_` shims | yes | yes | partial | POSIX | — |
| `fs` — **flag constants are caller-supplied numerics** (`0x601` macOS / `0x241` Linux; module doc admits it) | works | works if user passes Linux values | works if user passes Windows values | **user-facing portability hole** | export named constants (`fs.O_WRONLY` etc.) per platform — needs either target-conditional stdlib or runtime constant queries |
| `io` — bare `extern fn write(fd…)` | yes | yes | no (`_write`) | POSIX | route through `mml_write` like fs already does |
| `net` — **`sockaddr_in` byte layout writes BSD `sin_len` at `sa[0]`** | correct | **broken — Linux `sockaddr_in` has no `sin_len`; family lands at byte 1 → `sin_family` reads as 0x0200** | broken (also no `sin_len`) | **Apple-only — live bug** | move sockaddr construction into the rt (a `mml_sockaddr_v4` shim) or make the layout per-platform |
| `net` — AF_INET=2, SOCK_STREAM=1 | yes | yes (same values) | AF_INET=2 yes; SOCK_STREAM=1 yes | POSIX | fine on both, but keep as rt-provided constants anyway |
| `process` — `mml_env_get`/`getcwd`/`exit`/`arg`/`argc` | yes | yes | partial (rt work) | POSIX | rt split covers most |
| `process` — `extern fn getpid` | yes | yes | `_getpid` (CRT) / `GetCurrentProcessId` | POSIX | `mml_getpid` shim |
| `thread` — `extern fn sched_yield`/`usleep` | yes | yes | `SwitchToThread`/`Sleep` | POSIX | `mml_yield`/`mml_sleep_us` shims |
| `str`/`slice` `{ptr,i64}` repr, `i64` lens | yes | yes | yes (64-bit) | LLVM-target | portable on 64-bit; 32-bit would need a revisit (out of scope) |

## 4. Tests & tooling

| Surface | macOS | Linux | Windows | Class | Action |
|---|---|---|---|---|---|
| ntest workdir / `our` scratch dirs | `temp_dir()` | same | same | portable | none |
| `cargo test` helpers (`doc.rs`, `project.rs`) | `temp_dir()` | same | same | portable | none |
| `tests/lsp.rs` — `file:///tmp/…` URIs | yes | yes | no | tooling | path-agnostic URIs or per-OS |
| `@exit -1` pins (bounds/shift/range traps → `abort()`) | SIGABRT → -1 | same | exit 3 | POSIX | harness maps signal-kill; on Windows map 3 → -1 or gate pins |
| lldb/gdb debugging | yes | yes | partial (LLDB reads limited PDB/CodeView; native debug is WinDbg/cdb) | LLVM-target | `-g` emits CodeView for `-msvc` targets automatically; doc note |
| `bench/*.py`, `programs/` | yes | yes | tooling-only | tooling | not part of the shipped surface |

## Headline findings

1. **`net.lang` is silently Apple-only today.** `sockaddr_v4` writes
   `sin_family` at `sa[1]` (the BSD form, where `sa[0]` is `sin_len`).
   On Linux `sockaddr_in` has no `sin_len` — `sin_family` is the u16 at
   `sa[0..2]` little-endian, so the BSD layout encodes `0x0200`, not
   `AF_INET`. Compiles fine, `bind`/`connect` fail. This is the only
   *latent correctness bug* the audit found; everything else fails
   loudly or not at all.

2. **`fs` flag constants are a user-facing portability hole, not just
   internal.** The module documents `0x241 Linux / 0x601 macOS` but
   exports no names — callers must hardcode their host's values.
   `O_BINARY` (required on Windows to get untranslated I/O) doesn't
   exist in the vocabulary at all.

3. **The rt boundary is already well-positioned.** Every POSIX call
   except `write` (std.io), `getpid`, `sched_yield`, `usleep` already
   goes through an `mml_` shim. Windows work is one backend file plus
   four small shim moves — the boundary discipline held.

4. **Pipeline was already built for Linux.** `target_triple()`,
   `-lpthread`, and the clang probe all handle it. Linux support is
   closer to "verify" than "port".

## What N58B (Linux) must actually touch

- `stdlib/net.lang` — fix `sockaddr_v4` layout for Linux
  (`sin_family` u16 LE at byte 0; ideally move into `mmlrt` as
  `mml_sockaddr_v4(fill, ip, port)` so the C side owns the struct).
- Verify: `mmlrt.c` compiles under glibc (all POSIX calls present),
  `our`/`ourc` build+run, the full pin corpus, DWARF under gdb/lldb,
  task/mutex/channel runtime behavior, `fs` flags passed as Linux
  values, `getpid`/`sched_yield`/`usleep` under glibc.
- Everything else listed for Linux is already `yes`.

## What N58C (Windows) must design

- `mmlrt` split: `mmlrt_posix.c` (current) + `mmlrt_win32.c`
  (Win32 threads/`CRITICAL_SECTION`/`CONDITION_VARIABLE`, Winsock2 +
  `WSAStartup`, `_open`/`_read`/`_write`/`_close` + `O_BINARY`,
  `wmain`/`GetCommandLineW` argv, `GetEnvironmentVariableW`,
  `Sleep`/`SwitchToThread`, `_getpid`).
- Compiler: Windows triple, `.exe` naming, `ws2_32` lib, CodeView
  debug info, MSVC-env-aware `cc()` resolution.
- Language-level: `fs` flag constants per platform; `O_BINARY`
  semantics; argv/env UTF-8 bridging; exit-status mapping in the pin
  harness.
