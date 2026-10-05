# N45 validation — participant instructions (hand this to the agent verbatim)

You are evaluating a new programming language you have never used
before (file extension `.lang`). Treat this as if you downloaded a
new language and are trying to learn it yourself. This is a
controlled cold-start usability experiment.

One constraint of this environment: the standard library's source
is not readable here. The generated API reference (`our doc`,
below) is the library reference.

## Step 0 — the repo

The language's repository is already checked out locally at:

```
/Users/bourgeois/Desktop/Devin/memory-model-lab
```

All paths below are relative to that root. Build/install per
`n43/SETUP.md` (release binaries may already exist under
`target/release/` — check before building).

## Step 1 — read these two files first

- `n43/SETUP.md`
- `n43/TASK-A2.md`

## Surfaces you may use (normal user surfaces only)

- Docs: `SPEC.md`, `ERRORS.md`, `README.md`
- The toolchain: `our` and `ourc`
- `our doc <pkg>.<mod>` — the generated API reference.
  `our doc std` lists every stdlib module; `our doc std.<name>`
  (e.g. `our doc std.map`) shows one module's public API, and
  `our doc <your-pkg>.<mod>` shows your own modules. When you
  need to discover what stdlib modules exist, use `our doc std` —
  do not inspect the filesystem for module names.
- Do not pre-read the whole reference. Consult `our doc` only
  when you naturally need information while solving the task.
- `ourc --explain <CODE>`
- `ourc lsp` — a JSON-RPC server over stdio. A minimal client is
  provided at `n43/lsp_client.py` so you don't need to write one:
  `python3 n43/lsp_client.py <diag|hover|def|comp> <file> [line col]`
  (line/col are 0-based)

## Forbidden — reading these invalidates the experiment

- Do NOT read or search `stdlib/*.lang` — the generated `our doc`
  output is the library reference here. If you accidentally open
  or read any `stdlib/*.lang` file, stop and report the run as
  invalid rather than continuing.
- Do NOT read anything under `compiler/`, `native-tests/`, `bench/`,
  `programs/`, or `n43/` other than `SETUP.md`, `TASK-A2.md`, and
  `lsp_client.py`
- Do NOT inspect git history (`git log`, `git diff`, `git blame`,
  `git show`)
- Do NOT read `n43/DRYRUN.md`, `n43/OBSERVER.md`, or any
  `n43/N43A-*.md` records
- Do NOT search compiler source code for answers to errors — learn
  through the same surfaces a normal user would

## Working directory

Do all project work in a fresh directory outside the repo
(e.g. `/tmp/n45/`). Never write inside the repo.

## Additional experiment rules

- Do NOT use external web search, other AI models, or outside
  documentation.
- Do NOT search the repository for a completed or near-isomorphic
  contacts solution.
- Do NOT modify the compiler, stdlib, LSP, build system, or
  repository to make the task pass.
- If blocked on one issue for >10 minutes, record the block exactly
  and continue with another requirement if possible. Ask for help
  only if the entire task cannot proceed.

## Protocol

Work independently. Being blocked is valid data, not failure.

Maintain a chronological friction log as you go. For each meaningful
friction event record:

- elapsed time
- what you expected
- what you tried
- the EXACT compiler/tool response (verbatim — preserve full
  messages, including error codes)
- what you think the response means
- what you tried next
- whether that solved it
- approximate recovery time

If the same rule recurs, explicitly note whether you remembered it
and whether recovery was faster.

Record successful discoveries too when they resolve an earlier
misunderstanding — not just failures.

**Also record every use of `our doc`**: which module you doc'd,
what you were looking for, whether the output answered it, and
what (if anything) you could not find in it.

Do NOT classify events as compiler bugs or language-design flaws
while working. Record observations only; classification happens
after the session.

## Final report

When finished (or blocked), produce:

1. the completed program (or best state reached)
2. whether each TASK-A2.md requirement passed
3. the complete chronological friction log, verbatim compiler
   messages preserved
4. commands used
5. tools/docs/features you relied on
6. things you guessed correctly
7. things you guessed incorrectly
8. the three most confusing moments
9. the three most helpful diagnostics/tool features
10. whether you would voluntarily build another program in this
    language, and why
11. moments where the language/tooling successfully taught you a
    rule without outside help
12. a verdict on `our doc`: which questions it answered, which it
    didn't, and whether it could replace reading library source
13. whether module/API discovery felt natural — for each `our doc`
    use, would you have known which module to try without the
    enumeration being pointed out? Note anything you found only
    because the prompt told you a listing exists
