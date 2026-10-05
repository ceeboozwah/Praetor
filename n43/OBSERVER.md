# N43 — Observer guide + friction log

## Rules

- Hand the participant `SETUP.md` + `TASK.md`. Nothing else.
- Do not coach unless they're *completely* blocked (>10 min, one thing).
  Even then: answer the literal question, don't teach the model.
- Don't fix anything live. Note it, keep going.
- Time the session loosely; a finished binary isn't required.

## What to capture live

Every friction event gets one line:

| time | event | category | outcome |
|---|---|---|---|
| e.g. 4m | wrote `fn main(args: [str])` | syntax guess | diagnostic fixed it |

Per event, also note the recovery arc — it's worth more than a
pass/fail:

- what they expected to happen
- what they tried
- compiler/editor response
- whether they understood it
- what they tried next
- time-to-recovery

Categories: `syntax guess` / `authority` / `missing method` /
`diagnostic taught` / `diagnostic confused` / `docs needed` /
`explicit cleanup` / `lsp` / `build tool` / `surprise`

## Triage after the session

Classify every event into exactly one bucket — without this split,
every unfamiliar rule reads like a defect:

- **BUG** — expected behavior is broken (e.g. dead member completion,
  `tests/` can't reach `src/`, self-dep accepted).
- **ERGONOMIC GAP** — behavior is correct but unnecessarily hard
  (bare blocks missing, `Result<..>::Ok` ceremony, str-literal
  two-step).
- **LEARNING COST** — coherent rule the user simply hadn't
  internalized yet (`?` from `main`, `&mut` on ranges, scope-bound
  borrows).

Rank fixes by **severity × frequency × recovery difficulty** — a
one-time "oh, okay" is nothing like something that interrupts every
session. Do not fix everything they disliked.

Within each event, also name the failing layer — two LEARNING COST
events can need very different fixes:

- **diagnostic failure** — the rule held but the message didn't teach
- **documentation failure** — the answer existed nowhere reachable
- **language-design failure** — the rule itself fought the user
- **tooling failure** — compiler/LSP/`our` behavior was wrong or absent

Borrowed `str` keys → diagnostic → `OMap` discovery is successful
language teaching. The same rule + ten minutes of "the map is
corrupt" is a good rule with a failing explanation path.

Also note:
- which docs they opened, and whether the answer was in there
- anything they compared mentally to Rust/C/Zig/Go
- any moment of "oh, that's actually nice"
- whether they independently discover the owned-key container: the
  sequence `StrMap` → borrow error → docs/diagnostic → `OMap`
  clicking is a big usability win; "maps are broken" is a
  naming/docs problem
- whether they want a bare block scope (`{ let view = ... }`) to end
  a borrow, or accept `if true { }`
- **rule retention**: when the same mistake repeats (`&mut` ranges,
  borrow lifetimes), record second-occurrence recovery time vs the
  first. `first: 3m10s, second: 15s` is a positive result — a normal
  learning curve, not a bad rule.

## The friction log template

```
Blocked by syntax ............ ___
Misunderstood authority ...... ___
Missing method/API ........... ___
Diagnostic solved it ......... ___
Diagnostic made it worse ..... ___
Needed docs .................. ___
Explicit cleanup surprise .... ___
LSP helped / misled .......... ___ / ___
Finished? .................... ___
Time to first green build .... ___
```

## Post-session questions (5 min)

1. What did you expect `own`/`read`/`write` to mean before reading?
2. What did you write first that the compiler rejected — was it right to?
3. What did you look for that doesn't exist?
4. What felt faster/slower than the same task in a language you know?
5. What's the one thing you'd delete from this language?
