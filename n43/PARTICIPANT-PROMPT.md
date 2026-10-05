# N43A — participant instructions (hand this to the agent verbatim)

---

You are evaluating a new programming language you have never used
before (file extension `.lang`). Treat this as if you downloaded a
new language and are trying to learn it yourself. This is a
controlled cold-start usability experiment.

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
- `n43/TASK.md`

## Surfaces you may use (normal user surfaces only)

- Docs: `SPEC.md`, `ERRORS.md`, `README.md`
- The toolchain: `our` and `ourc` (per SETUP.md; release binaries may
  already exist under `target/release/` — check before building)
- `ourc --explain <CODE>`
- `ourc lsp` — a JSON-RPC server over stdio; attach it via any
  generic LSP client or speak the protocol yourself
- `programs/` — example programs that ship with the repo
- `stdlib/*.lang` — the library API surface; reading it is like
  reading library docs

## Forbidden — reading these invalidates the experiment

- Do NOT read anything under `compiler/`, `native-tests/`, `bench/`,
  or `n43/` other than `SETUP.md` and `TASK.md`
- Do NOT inspect git history (`git log`, `git diff`, `git blame`,
  `git show`)
- Do NOT read `n43/DRYRUN.md` or `n43/OBSERVER.md`
- Do NOT search compiler source code for answers to errors — learn
  through the same surfaces a normal user would

## Working directory

Do all project work in a fresh directory outside the repo
(e.g. `/tmp/n43a/`). Never write inside the repo.

## Protocol

Work independently. Do not ask how the language works unless you are
completely blocked on one thing for >10 minutes — being blocked is
valid data, not failure.

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

Do NOT classify events as compiler bugs or language-design flaws
while working. Record observations only; classification happens
after the session.

## Final report

When finished (or blocked), produce:

1. the completed program (or best state reached)
2. whether each TASK.md requirement passed
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
