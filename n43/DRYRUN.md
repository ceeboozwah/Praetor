# N43 — dry run (self-administered, /tmp scratch project)

Ran the participant path end-to-end: `our new` → `our build` →
`our run` → `our test` → `our fmt` → `ourc lsp` handshake. I know the
language, so this can't measure *cold* confusion — what it measured is
whether the documented path works at all, and it flushed out two real
defects plus a list of likely traps to watch for in the human session.

## Verdict

The path is walkable: a correct program satisfying every TASK.md
constraint (argv, file read, `Result`/`?`, map, top token, one test,
fmt clean) builds and runs. But two things are genuinely broken and
one is a structural gap, not user error.

## Defects found (real bugs, not friction)

1. **LSP member completion returns `[]` for module paths.** In a
   project buffer, `io.`, `fs.`, and `count.` (a bare sibling import)
   all completed to nothing at correct cursor positions, while hover,
   definition, and diagnostics on the same symbols all worked. Likely
   cause: import occs are named by full path (`std.io`) but
   `member_items` matches the typed chain root (`io`); `count.` failing
   too suggests a second gap in defs-key enumeration. Investigate
   `member_items` / `member_chain` in `compiler/src/driver/lsp.rs`.
2. **`our deps` accepts a self-dependency.** `wordcount = "."` in
   `[dependencies]` resolved cleanly and made `import wordcount.count`
   work. Harmless here, probably should be rejected — a package
   depending on itself is a cycle the checker doesn't flag.
3. **`tests/` cannot import `src/` modules.** `our test` resolves only
   `std.*`, declared deps, and `tests/` siblings — no `..`, no
   self-module. So "add one test" *forces* either a self-dep hack
   (used in this dry run), a lib+bin split with a path dep, or a test
   that exercises no project code. This is the single biggest
   onboarding gap: the scaffold's `tests/smoke.lang` teaches a
   pattern that doesn't scale to the user's own code.

## Friction events (in encounter order)

| event | category | outcome |
|---|---|---|
| `fs.slurp_r(...)?` in `fn main() -> i32` rejected: "propagates `Err(i32)`, but this function returns `i32"` | diagnostic taught | fixed by wrapping in `run() -> Result<i32,i32>` — correct law, but "handle the missing-file case" + `?` doesn't work from `main` is a docs moment |
| `v.destroy()` rejected: "cannot consume `v`: it is borrowed" | authority | fixed by wrapping the tokenize+drain in `if true { }` — borrows live to scope end, and *bare blocks don't exist* (`if true` is the scoping idiom). Two discoveries in one event. |
| `pub toks: i64;` on struct fields → `E7001 expected ':'` | syntax guess | diagnostic pointed at the token but not the law (no field visibility; `,` separators not `;`). Docs needed. |
| `buf[..]` passed as `slice mut` → "a range `a[lo..hi]` only names storage under `&`/`&mut`" | diagnostic taught | good message — and I still re-committed the same error in the test file minutes later. The rule doesn't stick on first contact. |
| `"lit".bytes` is `*const u8`, not `slice u8` | surprise | needs `let s: str = "lit"` first — literals are C-string globals until an annotated binding. Plausible cold trap. |
| winning `e.key` is a `str` view into `v` — can't survive `v.destroy()` | authority | copied bytes into a `[64]u8` stack buffer. Correct per the model, but a cold user will expect to return the string. |
| `result.Result<i32,i32>::Ok(0)` fully-qualified construction | ceremony | verbose; user will try `Ok(0)`/`Err(2)` bare and hit resolution errors first. |
| `m.len()` after drain prints 0 | user error | read `uniq` before `pop_entry` — task ordering trap I set for myself. |

## What went smoothly

- `our new`, `build`, `run`, `run --release`, `test`, `fmt`,
  `fmt --check`, `deps`, `clean` — all correct first try.
- Every diagnostic was *accurate*; the good ones name the law
  ("a slice is a view, not a value", "`b` borrows it here" labels).
- LSP: hover returns full signatures (`fn fs.slurp_r(heap: alloc,
  path: *const u8) -> result.Result<vec.Vec<u8>,i32>`), push
  diagnostics carry error codes, definition works. Only completion
  is broken.
- `str_from_utf8(&b[lo..i])`, `map.new`, `insert`/`get_mut`/`pop_entry`,
  `free_storage`, `destroy` — the stdlib surface needed for this task
  all exists and behaves.

## Friction scorecard (dry run)

```
Blocked by syntax ............ 1   (struct fields)
Misunderstood authority ...... 2   (borrow-at-destroy, key view)
Missing method/API ........... 0
Diagnostic solved it ......... 3   (?, range-view, borrow label)
Diagnostic made it worse ..... 1   (E7001 didn't teach field syntax)
Needed docs .................. 2   (struct decl, str literal types)
Explicit cleanup surprise .... 1   (scope-bound borrows → if true{})
LSP helped / misled .......... 3 / 1   (hover+diags+def good; completion dead)
Finished? .................... yes
Time to first green build .... ~15 min for the full task
```

## Implications for the human session

- Watch for the `?`-in-`main` moment — likely the first wall. Note
  whether they find the `run() -> Result` idiom alone.
- `tests/` + `src/` is the structural gap: the TASK asks for a test,
  and there is no clean way to test their own code. Log how they
  resolve it (or don't).
- `Vec` was relaxed out of the task (nothing needed it — forcing it
  would have been a scavenger hunt, not signal).
- The `str`-view-can't-escape moment is the ownership thesis in
  miniature. If they get bitten, that's the highest-signal event.
- Don't fix the completion bug or the self-dep acceptance before the
  human session — they're part of what we're measuring.
