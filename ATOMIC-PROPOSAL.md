# ATOMIC-PROPOSAL — extension delta against SPEC v0

Status: **proposal**. Not merged into `SPEC.md`. Becomes a spec amendment
only if its corpus (scenarios 151+) survives falsification with
0 missed / 0 over-restrictive.

## Claim under test

```text
Atomicity is an operation-level synchronization permission,
not a general escape from capability rules.
```

```text
atomic storage + atomic operation   ⇒ synchronized access
atomic storage + ordinary access    ⇒ normal capability/race rules
non-atomic storage + atomic op      ⇒ rejected
```

## Syntax delta

```text
let c: atomic<u64> = 0            — atomic storage (Copy iff inner is Copy)
struct S { cnt: atomic<u64> }     — atomic field inside a struct
atomic_load(x)                    — builtin ops (not library fns)
atomic_store(x, v)
atomic_add(x, v)
compare_exchange(x, old, new)     — returns bool; both arms one access
```

Ops resolve through the existing graphs: `atomic_add(h, 1)` where `h` is a
handle reaches `h`'s capability-edge target; `atomic_add(*a, 1)` reaches
`a`'s alias target; `atomic_add(s.cnt, 1)` names an atomic field.

## Proposed rules

**A1. `non-atomic-op`** — an atomic operation on storage that is not
`atomic<T>` is rejected (plain vars, non-atomic fields).
*Forced by: 158, 166.*

**A2. atomic ops are mutually compatible across task boundaries** — a
child's atomic access records hold `Atomic` (compiler-internal); a parent's
atomic access to the same storage is legal. Two sibling tasks doing atomic
ops are legal.
*Forced by: 151, 152, 156, 159, 162, 165, 168, 170.*

**A3. mixed access is still a race** — ordinary read or write vs an
outstanding `Atomic` hold, or an atomic op vs an outstanding `Shared`/`Write`
hold, is `data-race`. Reuses R14; no new diagnostic kind.
*Forced by: 153, 154, 155, 160, 163, 167, 169.*

## Compatibility matrix (task window)

| live access \ outstanding hold | Shared | Atomic | Write | Transferred |
|---|---|---|---|---|
| read          | ok   | race | race | use-after-send |
| write         | race | race | race | use-after-send |
| atomic op     | race | ok   | race | use-after-send |

Sibling-vs-sibling: Shared×Shared and Atomic×Atomic are the only legal
pairs (all other combinations reject at the second `spawn`).

## Cost budget (claimed now, measured after)

```text
new syntax:              atomic<T> type + 4 builtin op names
new programmer states:   +0   (Live|Consumed|Maybe only)
new programmer rules:    +1   (non-atomic-op; races reuse data-race)
new internal hold kind:  +1   (Hold::Atomic — not programmer-visible)
new machinery:           atomic-op resolution over alias/cap edges
```

## Explicitly deferred

- Memory ordering. This round freezes at *"atomic operations are
  race-safe"*; relaxed/acquire/release/acq_rel/seq_cst is a separate
  frontier with its own corpus.
- Ordering guarantees between an atomic op and surrounding ordinary
  accesses (fences/happens-before) — unmodelled.
- `atomic<non-scalar>` representation, lock-freedom, wait-freedom.
- RMW semantics beyond "one atomic access" (no ABA, no weak-CAS paths).
