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
| `fs` — flag constants were caller-supplied numerics (`0x601` macOS / `0x241` Linux) | **fixed (N58B)**: `fs.open` takes portable `o_*` bits; `mml_open_flags` translates | same | `O_BINARY` implicit | POSIX | resolved |
| `io` — bare `extern fn write(fd…)` | **fixed (N58B)**: routes through `mml_write` | same | same | POSIX | resolved |
| `net` — **`sockaddr_in` byte layout wrote BSD `sin_len` at `sa[0]`** | **fixed (N58B)**: `mml_sockaddr_v4` in the rt builds the host's struct | same | same | **Apple-only — was a live bug** | resolved |
| `net` — AF_INET=2, SOCK_STREAM=1 | yes | yes (same values) | AF_INET=2 yes; SOCK_STREAM=1 yes | POSIX | fine on both, but keep as rt-provided constants anyway |
| `process` — `mml_env_get`/`getcwd`/`exit`/`arg`/`argc` | yes | yes | partial (rt work) | POSIX | rt split covers most |
| `process` — `extern fn getpid` | **fixed (N58B)**: `mml_getpid` | same | `_getpid`/`GetCurrentProcessId` | POSIX | resolved |
| `thread` — `extern fn sched_yield`/`usleep` | **fixed (N58B)**: `mml_yield`/`mml_sleep_us` | same | `SwitchToThread`/`Sleep` | POSIX | resolved |
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

1. **`net.lang` was silently Apple-only** — fixed in N58B by
   `mml_sockaddr_v4`. `sockaddr_v4` had written `sin_family` at
   `sa[1]` (the BSD form, where `sa[0]` is `sin_len`); on Linux
   `sin_family` is the u16 at `sa[0..2]` little-endian, so the BSD
   layout encoded `0x0210`, not `AF_INET`. The runtime now owns the
   struct — the architectural rule is "OS-native structs terminate
   at the runtime boundary."

2. **`fs` flag constants were a user-facing portability hole** —
   fixed in N58B. `fs.open` now takes the portable `o_*` bit
   vocabulary (`o_read`/`o_write`/`o_create`/`o_trunc`/`o_append`)
   and `mml_open_flags` translates to host `O_*` values. `fd_open`
   survives as the documented raw escape hatch; `O_BINARY` is
   implicit on Windows.

3. **The rt boundary held.** After N58B every OS call in the stdlib
   crosses an `mml_` name — the four direct externs (`write`,
   `getpid`, `sched_yield`, `usleep`) were moved behind shims.

4. **Pipeline was already built for Linux.** `target_triple()`,
   `-lpthread`, and the clang probe all handle it. Linux support is
   closer to "verify" than "port".

## What N58B (Linux) must actually verify

The code changes landed already (`mml_sockaddr_v4`, `mml_open_flags`
+ `fs.o_*`, and `mml_getpid`/`mml_yield`/`mml_sleep_us`/`mml_write`
for the last direct externs). Remaining is host verification — the
disciplined runbook is `scripts/verify-linux.sh`; run it from the
repo root on a Linux x86_64 host. It covers:

- `mmlrt.c` compiles under glibc (all calls are POSIX — expected yes)
- `our`/`ourc` build+run on x86_64-linux
- the full pin corpus on Linux (741 as of N58B)
- all `programs/` binaries, especially `echo`/`httpd`/`kvserver`
  (the sockaddr path)
- `our new` → `check`/`test`/`build`/`run` project-tool flow
- DWARF under gdb or lldb (non-blocking — presence of `.debug_info`)
- task/mutex/channel behavior under real parallelism

When the script runs green on a real host, record the platform
facts it prints — distro/version, arch, glibc, the resolved C driver
(`pipeline.rs` probes `xcrun`/`clang`/`cc`/`gcc`), and its version —
in the closing report. "Linux works" without those is too broad.

Status: macOS implementation verified; Linux source compatibility
prepared but not yet empirically verified on Linux.

## What N58C (Windows) must design

The full design contract lives in `WINDOWS-PORT.md` (N58C-1). Headline
decisions:

- Target `x86_64-pc-windows-msvc`; `mmlrt` splits into
  `mmlrt_posix.c` + `mmlrt_win32.c` behind a shared header — no
  `#ifdef` spaghetti, the driver picks the file by triple.
- argv/env enter as UTF-16 via `wmain`/`GetCommandLineW` and convert
  to UTF-8 at the boundary; unpaired surrogates trap — the `str`
  invariant survives.
- Sockets are the one API change: `SOCKET` is 64-bit and not a CRT
  fd, so `std.net` handles widen to `i64` and socket I/O gets its own
  `mml_sock_*` shims instead of aliasing `fs` fd ops.
- `@exit -1` means "abnormal termination" — POSIX signal or Windows
  code 3/exception — so the pin corpus needs no per-OS annotations.
