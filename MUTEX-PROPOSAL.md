# MUTEX-PROPOSAL — extension delta against SPEC v0 (+Amendment A)

Status: **accepted** — survived falsification (1337/1337 expectations,
0 missed / 0 over-restrictive). Merged into `SPEC.md` as Amendment B.

## Claim under test

```text
A mutex does not grant permanent write authority.
`lock` temporarily DERIVES exclusive write authority for the guard's
lifetime; the guard's death returns it.

  static capability partition  = compiler proves exclusivity
  mutex guard                  = runtime lock enforces exclusivity
```

Same safe operation, two different reasons. The architecture extends the
existing split one step:

```text
Handle  = Identity         Capability = Authority
Mutex   = Synchronization domain    Guard = temporary authority proof
```

## Syntax delta

```text
let m: mutex<Point> = mutex p   — mutex<T> wraps/moves a value (private interior)
let m2 = mutex h                — mutex<handle<T>> protects the TICKET
let g = lock(m)                 — builtin: derives Guard, domain = m's identity
let g = lock(s.a)               — domain = field path "s.a"
let g = lock(hm)                — hm = handle m: domain resolves through
                                  the capability edge to m — same mutex,
                                  same domain
unlock(g)                       — builtin: consumes the guard (affine)
                                  and releases the domain
```

No `Mutex<T>`/`Guard<T>` type syntax is needed beyond `mutex<T>` — guards
are produced by `lock` and are scope-bound values.

## Proposed rules

**M1. `unguarded-access`** — the mutex's interior is private: direct field
read/write (`m.x`, `m.x = 1`) on a `mutex<T>` binding rejects. The only
lawful access is through a live guard.
*Forced by: 174, 175.*

**M2. `non-mutex-op`** — `lock` requires `mutex<T>` storage (resolved
through fields/handles); `unlock` requires a live guard.
*Forced by: 188.*

**M3. `guard-escape`** — guards are scope-bound authority proofs: a guard
cannot be `return`ed, and its domain is released at scope exit or
`unlock`. (Joins, field storage, and rebinding out are equally
scope-bound; v1 rejects the escape, it does not track it.)
*Forced by: 181.*

**M4. `recursive-lock`** — `lock(m)` while a live guard already holds
domain `m` in the same task rejects (self-deadlock; recursive mutexes are
not v1 semantics).
*Forced by: 183.*

Guards reuse affine consumption entirely: `unlock(g)` twice is
`double-consume`; `g.x` after `unlock(g)` is `use-after-consume`; a guard
moved into a task is `use-after-send`/`use-after-consume` as usual — NO
guard-state machine exists. *Forced by: 185, 186, 187, 182.*

## Synchronized holds

Writes through a live guard resolve through `g → m`'s capability edge
(and onward through whatever `m` contains) and record
`sync_writes[target] += domain`. At capture these become hold
`Sync(domains)` — a fourth hold kind, internal only:

| live access \ outstanding hold | Shared | Atomic | Sync(D) | Write | Transferred |
|---|---|---|---|---|---|
| read          | ok   | race | race | race | use-after-send |
| write         | race | race | race | race | use-after-send |
| atomic op     | race | ok   | race | race | use-after-send |
| sync op (dom d′) | race | race | ok iff D == {d′} | race | use-after-send |

Key property: **two writes under the same domain serialize** (same mutex);
**disjoint domains race** — `Mutex<handle<T>>` twice over the same target
rejects because the domains differ (178).

*Correction vs the pre-implementation table:* compatibility is singleton
equality, not intersection. If a task may write under *either* of two
domains (|D| > 1), a sibling under just one of them can meet the write
under the other — so a Sync hold is compatible only when it is exactly
the sibling's single domain.

Mixed-touch promotion is shared with atomics: a var touched both plainly
and under a lock in one task has an unsynchronized access in its window —
the hold degenerates to `Write`.

## Effect summaries

A callee's synchronized writes are a new summary fact:
`syncs: (param_idx, domain_suffix)` — recorded when the callee's
`sync_writes` key is a param (domain suffix is the path inside the param,
"" for the whole param). `fn update(mut m: mutex<P>)` called from two
tasks applies `sync_writes[actual] += {actual+suffix}` — the summary must
carry the synchronization fact or the corpus exposes it (191).

## Explicitly deferred (mutex non-goals)

- **Deadlock freedom.** Race safety is guaranteed; lock-ordering deadlock
  (task A: m1→m2, task B: m2→m1) is memory-safe and NOT detected. Only
  self-deadlock (`recursive-lock`) is rejected. A future lock-ordering
  frontier is separate.
- Guard escape through `join` / channel send / stored into fields.
- Poisoned/failed lock paths (no lock-failure semantics).
- Recursive/reentrant mutexes as a mode; read-write lock asymmetry
  (`lock` always derives exclusive authority in v1).
- Field-granular holds: guarded writes are root-granular like everything
  else in v0 (190 documents the bound).
- `free g` on a guard is `invalid-consume` (guards aren't heap).

## Cost budget (claimed now, measured after)

```text
new syntax:              mutex<T> + mutex/lock/unlock forms
new programmer states:   +0   (guards reuse Live|Consumed|Maybe)
new programmer rules:    +4   (unguarded-access, non-mutex-op,
                              guard-escape, recursive-lock)
new internal hold kind:  +1   (Hold::Sync — domain-tagged)
new access kind:         +1   (Acc::Sync)
new summary fact:        +1   (Effect.syncs)
new machinery:           sync-domain tracking, guard resolution
```

The over-budget item to watch: +4 rules vs the "+1 or +2" ideal. Each is
a distinct misuse kind; if falsification shows `guard-escape` or
`recursive-lock` folding cleanly into existing kinds, the count drops.

## Outcome (measured after implementation)

```text
corpus:                 191 scenarios (172–191 added), 1337/1337 @expect
F:                      103 caught / 0 missed / 0 over
data-race coverage:     21/21   permission-violation: 9/9   invalid-use: 3/3
rules:                  affine 12 -> 16,  F 19 -> 23
machinery:              +1 synchronized-domains(guards)

new syntax:             mutex<T> type + `mutex e` wrapper + lock/unlock names
new programmer states:  +0      (guards reuse Live|Consumed|Maybe — as claimed)
new programmer rules:   +4      (unguarded-access, non-mutex-op,
                                 guard-escape, recursive-lock — as budgeted)
new internal hold kind: +1      (Hold::Sync(domains) — never source-visible)
new access kind:        +1      (Acc::Sync — internal)
new summary fact:       +1      (Effect.syncs)
```

Budget: met exactly. Guards needed **zero** new temporal states — `unlock`
is `Op::Unlock` on the same consume path; scope exit releases the domain
through the ordinary `pop_scope` machinery.

One ownership-model note: `unlock(g)` moves the guard in the ownership
baseline too — 186's double-unlock is caught twice (`use-after-move` +
`double-consume`). Consistent: the guard is a consumed resource in both
readings.
