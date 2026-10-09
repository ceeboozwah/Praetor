# PROCESS-PROPOSAL — extension delta against SPEC v0 (+Amendments A/B)

Status: **implemented** (N32) — pinned by native-tests 540–545;
spawn added in N65B (pin 746).
N31 evidence ranked this first: the only top gap that was
*unexpressible* (not merely awkward). Read-mostly by design: no
`setenv`, no signals, no shell exec. (N65B added one synchronous
spawn-and-wait — inherited stdio, no pipes/capture — needed by the
self-hosting runtime surface; still no `setenv`/signals/shell.)

## Claim under test

```text
Process state is a source of READ-ONLY foreign views and one
TERMINAL effect:

  args/env/cwd = process-lifetime or caller-buffer storage — views,
                 never copies, no hidden allocation
  exit         = a diverging call — the checker must know the path
                 ends; nothing after it needs cleanup
```

The interesting half was never the syscalls — `mml_*` shims already
proved that pattern. It is **no-return as a composable semantic
property**: `process.exit` is a library fn, and its divergence
reaches callers through the ordinary summary mechanism, so
`fatal()`/`abort()`-shaped wrappers work without compiler knowledge
of each one.

## Surface (`stdlib/process.lang`)

```text
process.env(name: str) -> Result<str, i32>
    getenv through a bounded copy of the (non-NUL) name in the rt
    shim. Ok payload is a view into process-global environ storage
    — immutable, process-lifetime. Err(1) missing; Err(2) bytes not
    valid UTF-8 (env is untrusted input — the str invariant becomes
    an error channel via the rt's non-trapping scan, not a trap);
    Err(3) name too long for the shim's 4 KiB copy buffer.

process.cwd(buf: slice mut u8) -> Result<str, i32>
    getcwd into caller storage — the alloc-free shape. `slice mut`
    IS the write claim (same as fs.read's buf); the returned str
    borrows the same storage, so it cannot outlive the buffer.
    Err(errno) on failure — ERANGE when the buffer is too small.

process.arg(i: i64) -> Result<str, i32>   — i-th argv view
process.argc() -> i64                     — argument count
process.pid() -> i32                      — getpid
process.exit(code: i32)                   — never returns

process.run(heap, exe: str, args: slice str, cwd: str) -> Result<i32, i32>   — N65B
    synchronous spawn-and-wait: argv[0] is `exe` itself, `cwd` ""
    inherits, stdout/stderr pass straight through (no pipes/capture).
    Ok(exit code) — 128+signal on signal death (POSIX) — or
    Err(errno) when the child can't start: spawn failure is never
    reported as a child exit status.
```

`arg`/`argc` instead of `args() -> slice str`: the FFI surface stays
free of `*const str` (a `str` element pointer); the out-param shape
`mml_arg(i, &mut p, &mut n)` reuses the exact pattern `env` already
needs. rt `main` stashes the {ptr,len} argv table it already builds
on its own frame — the process root, so the stash is honest
process-lifetime storage. Zero copies.

## The semantic delta — `diverges` on FnSummary

```text
unreachable()          builtin call — "control never reaches here";
                       the one primitive. Sets diverged+terminated.
fn summary.diverges    the callee's body ends terminated ⇒ the CALL
                       never returns ⇒ caller path diverged+terminated.
```

`terminated` is a second bit next to `ctx.diverged`:

- `diverged` — the path doesn't fall through (return/break/continue/
  exit all diverge). Joins already handled it.
- `terminated` — the path ended **without unwinding**: the process
  died, so scope-exit obligations (the unjoined-task and task-escape
  diagnostics in `expire_scope`) don't apply, and remaining
  statements are not checked (`exit(1); return x` cannot read an
  uninit `x` — the read is unreachable). `return` unwinds —
  `process.exit` does not. That difference is the whole reason for
  the second bit.

Join rules mirror `diverged`:

```text
if:    term = pre_term || (then_t && else_t)     — both arms end it
match: term = pre_term || (covering && all-terminated)
while c:  term = pre_term        (cond-false edge is a live exit)
while true, no exits:            continuation is DEAD —
           diverged = true, terminated = body_t
           (`while true { exit }` diverges the fn;
            `while true { return }` unwinds through it)
block: stays set once terminated
```

A fn whose body ends `terminated` gets `diverges` in its summary —
`{ mml_exit(c); unreachable() }` makes `process.exit` diverge, and
`fatal(m) { eprintln(m); process.exit(1) }` inherits it through the
ordinary call-site replay. No `noreturn` attribute, no special fn
registry — divergence is derived, not declared.

Deliberately NOT done: declaring `diverges` on cyclic/recursive SCCs
(conservative `false`), a `!` never type (the flag carries the only
information the checker needs), and cleanup-on-exit (hard exit runs
none — stated explicitly so it's a design choice, not an accident).

## LLVM

`unreachable()` emits `unreachable` — a terminator; post-call instrs
land in the dead-block machinery that already exists for post-`ret`.
`process.exit` callers emit an ordinary `call` — the callee carries
noreturn by construction (its own body seals `unreachable`); nothing
needs marking at call sites.

## Test matrix — shipped as native-tests 540–545

- `env("HOME")` → `Ok` view; missing name → `Err(1)` (540)
- `cwd(buf)` → the real directory; `arg(1)`/`argc` views (541)
- `exit(7)` → process status 7; init join ignores the exit arm (542)
- `fatal()` wrapper propagates `diverges` through the summary;
  consumed-Vec state can't weaken the survivor either (543)
- `while true { exit }` → dead continuation, diverging fn (544)
- `exit; return x` with uninit `x` → dead code not checked (545)

## Non-goals

`setenv`/`unsetenv` (would invalidate the env-view lifetime story),
signal handling, `fork`/`exec`/shell spawn, env iteration, `chdir`.
