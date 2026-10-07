# Windows Port — Design Contract (N58C-1)

This document defines *exactly* what the Windows port means before any
code is written. It follows from the N58A audit (`PORTABILITY.md`) and
N58B's boundary rule:

> **OS-native structs, constants, and calling quirks terminate at the
> runtime boundary.** Language and stdlib code never see `HANDLE`,
> `SOCKET`, `SOCKET_ERROR`, `DWORD`, UTF-16, or `\r\n`.

Status: **contract — nothing here is implemented.**

## Axioms

1. `str` is valid UTF-8 everywhere, on every platform. Bytes that can't
   be UTF-8 cannot become `str`. (Already enforced: `main` validates
   argv via `mml_str_validate` and traps.)
2. File I/O is byte-preserving. Text translation (`O_TEXT`) is never
   applied unless a future explicit API asks for it.
3. `path` arguments at the language level are UTF-8 byte strings with
   `/` separators. Windows APIs accept `/` universally; `\` in a path
   also works (it's the same byte Windows treats as a separator), but
   stdlib docs recommend `/` for source portability.
4. Trap = abnormal termination, observable as such. The mechanism may
   differ per platform; the contract is "did not exit normally."
5. A program that compiles on Windows should behave identically to
   POSIX for everything the language semantics define, and the
   differences (below) must be enumerated — not discovered.

## 1. Target triple and toolchain

**Decision: `x86_64-pc-windows-msvc` is the primary target.**

- `clang` already compiles `.ll` for any LLVM target — the emit path
  only changes `target_triple()` and the link step.
- Linking uses `lld-link` (LLVM's own linker, ships with clang) plus
  the Windows SDK import libraries: `ucrt.lib`, `kernel32.lib`,
  `ws2_32.lib`. On a machine with VS Build Tools these are in the SDK;
  `cc()` resolution gains a branch that locates `lld-link` and adds
  `-Wl`/`/libpath` wiring, or drives `clang -fuse-ld=lld` so clang
  resolves the SDK itself.
- `x86_64-pc-windows-gnu` (MinGW) is *not* the contract target: it
  drags in a POSIX-emulation runtime (winpthreads) that paper-ports the
  boundary instead of honoring it. It remains available as a bootstrap
  if a test machine lacks the MSVC toolchain, but a green MinGW run
  does not close N58C.

Open risk: SDK discovery (`vswhere.exe`/registry) is the fiddliest
part of toolchain bring-up. Budgeted as a known-unknown, not a design
question.

## 2. Runtime file split

`mmlrt.c` becomes three files, selected by the driver from the target
triple:

```
compiler/runtime/mmlrt.h        — shared prototypes + mml_str_t layout
compiler/runtime/mmlrt_posix.c  — today's file, renamed
compiler/runtime/mmlrt_win32.c  — Win32 implementation
```

No `#ifdef _WIN32` spaghetti inside functions; each file implements
the same `mml_*` surface whole. The compiler picks the source by
`target_triple()`, the same place `-lpthread` is already conditional.

## 3. Entry point: UTF-8 argv/env — *the semantic decision*

POSIX `main` hands bytes straight through and validates UTF-8. Windows
`main`/`argv` are ANSI — codepage-dependent, lossy, wrong by
construction. The contract:

- Windows entry is `wmain(int, wchar_t**)` (or `WinMain` +
  `GetCommandLineW`/`CommandLineToArgvW` for robust quoting).
- The runtime converts each arg UTF-16→UTF-8 (`WideCharToMultiByte`,
  `CP_UTF8`, `WC_ERR_INVALID_CHARS`).
- **Unpaired surrogates trap at startup**, same as invalid POSIX bytes
  trap today. Loud failure at the boundary; no WTF-8, no U+FFFD lossy
  mode. A program can never observe a `str` that isn't valid UTF-8.
- `mml_env_get` → `GetEnvironmentVariableW` + UTF-16→UTF-8 on the
  value; name converts UTF-8→UTF-16. Unpaired surrogates in a value →
  the `Err`/trap path, per key — never a poisoned `str`.
- `mml_getcwd` → `_wgetcwd` + convert.

This keeps the invariant identical on both platforms: the runtime is
where encoding is reconciled, exactly once, at the edge.

## 4. Path encoding

Every `mml_*` fs entry point takes `*const u8` UTF-8. On Windows the
shim does `MultiByteToWideChar` into a stack buffer (`MAX_PATH*4` or
heap on overflow) and calls the `_w*` API (`_wopen`, `_wstat`, …).
UTF-8→UTF-16 cannot fail on valid input; invalid input means the
caller violated `str`'s contract already.

## 5. File I/O

- `mml_open_flags`: map `MML_OF_*` → `_O_RDONLY`/`_O_WRONLY`/`_O_RDWR`
  `|` `_O_CREAT`/`_O_TRUNC`/`_O_APPEND`, **always `| _O_BINARY`**
  (axiom 2), then `_wopen` on the converted path. `mode` maps 0666-ish;
  Windows ignores permission bits on most filesystems — documented.
- `mml_read`/`mml_write`/`mml_close` → `_read`/`_write`/`_close`.
  `_write` takes `unsigned int` — clamp chunks at `UINT_MAX`; the
  `slice` contract ("wrote ≤ n") is already legal for partial writes.
- CRT fds are `int` — the `i32` fd type stays.

## 6. Sockets — the one real API change

Two facts break the POSIX model:

1. `SOCKET` is `UINT_PTR` — 64-bit. `std.net`'s `fd: i32` cannot hold
   it.
2. `SOCKET` is **not** a CRT fd — `net.recv`/`net.send`/`net.sock_close`
   currently alias `fs.fd_read`/`fs.fd_write`/`fs.fd_close`, which on
   Windows route to `_read`/`_write`/`_close` and would fail.

Contract:

- **Socket handle type widens to `i64`** in `std.net` — `net.socket()`
  returns `i64`; `bind`/`connect`/`listen`/`accept`/`send`/`recv`/
  `close` take `i64`. Source-visible, honest, and portable (POSIX fds
  fit trivially). fs fds stay `i32`; a socket is not a file.
- Net I/O moves off the fs wrappers to dedicated shims:
  `mml_sock_read`/`mml_sock_write`/`mml_sock_close` → POSIX `read`/
  `write`/`close`; Win32 `recv`/`send`/`closesocket`.
- **WSAStartup is lazy**, run once inside the first socket call via
  `InitOnceExecuteOnce`; `WSACleanup` via `atexit`. No API surface —
  a user should never see Winsock.
- `mml_errno` on Windows reads `WSAGetLastError()` for socket paths —
  the shim layer keeps a translation table for the dozen codes std.net
  exposes (EACCES/EADDRINUSE/ECONNREFUSED/…). Raw errno values are
  already documented host-specific, so `Err(e)` carries the mapped
  POSIX-style code for uniformity.

## 7. Threads and synchronization

`std.thread`/`std.sync`/channels map to pure Win32 (no POSIX emulation):

| POSIX today | Win32 |
|---|---|
| `pthread_create` + trampoline | `CreateThread` (or `_beginthreadex`) |
| `pthread_join` | `WaitForSingleObject` + `CloseHandle` |
| `pthread_mutex_t` | `SRWLOCK` (exclusive) — smaller, faster than `CRITICAL_SECTION` |
| `pthread_cond_t` | `CONDITION_VARIABLE` (`SleepConditionVariableSRW`) |
| `sched_yield` → `mml_yield` | `SwitchToThread` |
| `usleep` → `mml_sleep_us` | `Sleep(us/1000)` — granularity note below |
| `getpid` → `mml_getpid` | `GetCurrentProcessId` |

Semantics preserved: `mml_chan_recv` already waits in a `while` loop —
`CONDITION_VARIABLE` spurious wakeups handled identically. Sleep
granularity: `Sleep` is millisecond-granular; `mml_sleep_us` rounds up
(`(us+999)/1000`, 0→`Sleep(0)` yield). Sleep is a hint in the contract,
not a timer.

## 8. Process and exit semantics

- `mml_exit` → `exit(code)`. Observable difference, documented: POSIX
  callers see `code & 0xFF`; Windows callers see the full u32. The
  language contract is "exit code is host-reported" — the 741 pins
  only assert values 0–255, which agree.
- **Trap pins** (`@exit -1`): on POSIX, `unwrap_or(-1)` catches a
  signal; on Windows `abort()` returns code **3** and SEH faults return
  the exception code (e.g. `0xC0000005`). Contract: ntest treats
  `-1` as "abnormal termination" — on Windows that means `code == 3`
  or `code < 0` (high-bit exception codes reinterpret negative). The
  pin file needs no annotation; `-1` means "died badly" everywhere.
- `abort()` behavior: `_set_abort_behavior(0, _WRITE_ABORT_MSG)` at
  startup to suppress the WER dialog in CI.
- `mml_getpid` → `GetCurrentProcessId` (i32 fits).

## 9. Debug info

**Decision: keep DWARF emission (`-g`) unchanged for N58C-2.**

gdb and lldb-mi on Windows read DWARF; the metadata path already works.
CodeView/PDB (`clang -gcodeview`) is a later option for VS/WinDbg
interoperation — noted, not required. A `.lang`-source debugging
experience that works under gdb on Windows satisfies the milestone.

## 10. Naming and linker surface

- `target_triple()` gains `windows-msvc`; driver selects
  `mmlrt_win32.c` and links `ws2_32` (+ `ucrt`, `kernel32` via the
  clang driver's defaults).
- Output naming: `ourc -o name` → `name.exe` appended when missing on
  Windows; object files `.obj` under MSVC conventions (cosmetic —
  clang accepts `.o`, keep `.o` for code simplicity).
- `our build`/`run` path joins unchanged; `Path` handles separators.

## 11. Test adaptations

- **Pins**: zero `native-tests/*.lang` files hardcode `/tmp` — the
  corpus is already path-clean. `@exit -1` handled per §8. No
  per-OS pin annotations needed at all — a genuinely clean result.
- **`compiler/tests/lsp.rs`**: `file:///tmp/…` URIs are path strings in
  JSON — they never touch the filesystem, but switch to
  `std::env::temp_dir()` anyway for cleanliness.
- **`programs/indexer.lang`**: hardcodes `/tmp/n41idx_*` fixtures —
  gains a `TEMP`/`TMPDIR` lookup (or fixed relative paths) when the
  Windows run begins. Deferred to N58C-3.
- **`scripts/verify-linux.sh`**: gets a `verify-windows.ps1` sibling —
  same phases, PowerShell, `Test-NetConnection`/`curl.exe` for the
  socket servers.

## 12. Explicit non-goals for N58C-2

- Win32 GUI/console-subsystem split (`/SUBSYSTEM:`) — console only.
- `mmap`/`VirtualAlloc` — `system_allocator` stays `malloc`-backed
  (ucrt provides it identically).
- Async sockets, IPv6, `fork`-style process spawning — not in the API.
- PDB generation, WinDbg/VS debugger integration.
- Path normalization, UNC paths, drive-letter semantics beyond
  "UTF-8 in, `_w*` API out".

## Open risks (ranked)

1. **Toolchain discovery** — SDK/lib paths vary; mitigate by driving
   `clang` (not `lld-link` directly) so the driver resolves the SDK.
2. **ws2_32 link + Winsock quirks** — `closesocket`, `WSAE*` codes,
   `SD_BOTH` shutdown semantics differ subtly; covered by net pins.
3. **`str` contract at argv** — first platform where "host bytes" and
   "language bytes" genuinely differ; contract decided (wide→UTF-8,
   strict trap).
