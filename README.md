# Memory Model Lab (`mml`)

> Phase Two of the systems-language project (§44, §59): *before* committing to a
> memory model, build several and compare them experimentally on identical
> programs.

This is not the language. It is an **experimental workbench** for the single
most important architectural decision the language will make: *how does it
achieve memory safety at compile time, and what does that cost the programmer?*

The seeding document asks (§10, §51):

> Can deterministic memory safety be achieved with a model that requires less
> lifetime reasoning from the programmer than Rust?

The lab now goes one step further and asks the sharper question that the first
round of results raised:

> Can we combine the strongest guarantees while keeping the programmer-visible
> model materially simpler than Rust?

To answer it honestly, the lab scores every model against a **model-independent
safety oracle**, so a model that *misses* a real bug is *reported as missing
it* — never hidden by tuning the tests to the implementation.

---

## The core idea: one language, many models, one oracle

Every model consumes the **same** parsed program and the **same** annotation
vocabulary, and each is scored against the same ground-truth `@safety` verdict.

| Source annotation | ownership            | region              | capability          |
|-------------------|----------------------|---------------------|---------------------|
| param `own x`     | value is *moved* in  | (ignored)           | `own<T>` full rights|
| param `ref x`     | shared `&` borrow    | (ignored)           | `read<T>`           |
| param `mut x`     | exclusive `&mut`     | (ignored)           | `write<T>`          |
| `region r { .. }` | (ignored)            | opens alloc scope   | (ignored)           |
| `new(r) T`        | heap value           | allocate in `r`     | owned value         |
| `new T`           | heap value           | heap (outlives all) | owned value         |
| `free e`          | consumes `e`         | (ignored)           | (ignored)           |
| `&e` / `&mut e`   | borrow               | ref into same region| derive read/write   |

Each corpus file declares its truth and each model's recorded behaviour:

```
// @desc   one-line description
// @class  region-escape                     (bug class under test)
// @safety reject reference outlives region   (GROUND TRUTH — the oracle)
// @expect ownership  accept                  (recorded behaviour, a regression lock)
// @expect region     reject region-escape
// @expect capability accept
// @expect region+capability reject region-escape
```

`@safety` is authored from first principles per scenario and is **never edited to
flatter a model**. The gap between `@safety` and a model's verdict is the finding:

- oracle **reject**, model reject → **caught**
- oracle **reject**, model accept → **MISSED (unsound)** — a real bug slips through
- oracle **accept**, model reject → **over-restrictive** (false positive)
- oracle **accept**, model accept → correct accept

---

## The five models

1. **`ownership`** — simplified Rust. Move timeline + borrows. Catches
   use-after-move/free, double-free, aliasing. Cost: the programmer must reason
   about *when* values move and *how long* borrows live.
2. **`region`** — escape analysis. Allocations belong to a lexical `region`;
   the only rule is *a reference must not outlive its region*. No moves, no
   borrows — the lightest mental model, the narrowest guarantee.
3. **`capability`** — read/write/own rights on every binding and reference.
   Catches permission violations and aliasing. Describes *what you may do*, not
   *when things move*.
4. **`region+capability`** (Prototype D) — the experiment: **lifetime safety
   (region) ⊕ access safety (capability)**, implemented as the union of the two
   independent analyses. Tests the hypothesis that

   > Memory Safety = Lifetime Safety + Access Safety

   can be answered by two simple mechanisms instead of one complex one.
5. **`region+ownership`** (Prototype E) — the alternative: **lifetime safety
   (region) ⊕ move/borrow timeline (ownership)**. Same composition style as D;
   the head-to-head tells us whether temporal safety must come from ownership's
   full machinery, or whether a *smaller* temporal state suffices.
6. **`affine`** — a control model: **minimal temporal state alone**
   (`Live / Consumed / MaybeConsumed` on consumables only). Registered
   standalone so the hybrids' temporal coverage is *attributable*, not muddled.
7. **`region+cap+affine`** (Prototype F) — the sharpened hypothesis:

   > Memory Safety = Region Validity + Access Capability + Minimal Temporal State

   Ordinary memory never enters the temporal machine; only consumables carry
   the `Live → Consumed` state. Its one real machinery cost is path-sensitivity:
   branch/loop joins merge state pointwise, and loops re-check the body under
   the merged state so `while c { free f }` is caught as a second-iteration
   double-consume.

### New primitives added for the expressiveness corpus

Two persistence primitives were added to the shared language so that cross-region
and long-lived relationships are *expressible* rather than silently unsafe:

- **`promote e`** — re-home `e` so it outlives every region. Lifetime analysis
  treats the result as region-level-0 (never dangling). Tests "can an object be
  transferred from a short region to a long one without lifetime bookkeeping?"
- **`handle e`** — a stable, generation-checked handle, *not* a lifetime-tracked
  pointer. Lifetime analysis treats it as level-0 too. Tests whether
  `reference safety ≠ pointer lifetime tracking`.

Both are deliberately cheap to add now (one AST node) and deliberately *tested*
by the corpus rather than assumed good.

> These checkers are **honest simplifications**, not verified analyses:
> lexical (not non-lexical) borrow scopes in ownership/capability, and
> path-sensitivity only where the profile column says `psens` (affine, F).
> Limitations are documented at the top of each model source file.
> Their imprecision is itself a measured result (see scenarios 23, 73, 76).

---

## Results (211 scenarios, all locked as regression tests)

Run `cargo run -- test corpus`. The corpus is ten groups:

- **Scenarios 01–30** — the *safety* suite (bug classes the models must catch).
- **Scenarios 31–45** — the *expressiveness* suite (graphs, promotion, handles).
- **Scenarios 46–59** — the *anti-cheat* suite for Prototype F (temporal edge
  cases: branch/loop consumes, consume-through-alias, resource reopen, etc.).
- **Scenarios 60–70** — *cross-region graph surgery*: directional edges,
  promote of cycle members/subtree roots/back-pointers, graph merge via
  handles vs raw refs, loop-grown graphs, conditional edges.
- **Scenarios 71–84** — *alias-lifetime precision*: alias death by scope exit,
  rebind, field overwrite, container destruction, branch/loop merges, nested
  containers, handles-as-non-aliases.
- **Scenarios 85–96** — *interprocedural summaries*: returned/stored/killed
  aliases across calls, conditional alias creation, nested-call composition,
  resource moves, mutual recursion, return-alias precision.
- **Scenarios 97–112** — *recursive fixed points*: direct/mutual/3-fn cycles,
  effects flowing around back-edges, conditional vs definite kills, cyclic
  returns, region/handle recursion, acyclic callers above SCCs.
- **Scenarios 113–136** — *concurrency*: `spawn`/`join`, shared reads, write
  races, ownership transfer ("send" is implicit capture), authority returned
  through `join`, alias kills across the boundary, region-vs-task lifetime,
  nested/conditional/loop spawns, handle-mediated shared access.
- **Scenarios 137–150** — *handle authority*: identity-vs-authority
  separation (handle = which object; capability = what you may do), handle
  races, copied/returned/stored/recursed handles, stale-ticket detection,
  retargeting, region-death, free-while-handled precision.
- **Scenarios 151–171** — *atomic synchronization*: `atomic<T>` storage +
  builtin ops (`atomic_load`/`store`/`add`, `compare_exchange`). Mixed
  atomic/ordinary access races; atomic ops synchronize with each other;
  ops on non-atomic storage reject. Memory ordering deferred.
- **Scenarios 172–191** — *mutex synchronization*: `mutex<T>` + `lock`/
  `unlock`. Guards derive temporary synchronized authority; same-domain
  writers serialize, disjoint domains and unguarded access race; guards
  are ordinary affine resources (double-unlock = `double-consume`);
  summaries carry the lock domain across calls. Deadlock-freedom deferred.
- **Scenarios 192–211** — *channel authority transport*: `channel<T>` +
  `send`/`recv`/`close`. A channel is a transported graph transform:
  `send` cuts an authority packet (owned value consumed; `&mut` target's
  authority enters `in_transit`; handles travel as identity), `recv`
  installs it on the receiver. Edge closure rejects ref-carrying payloads
  (`send-escape`); guards cannot cross; endpoint ops are `Sync({ch})`.
  Buffering/select/endpoint-types deferred.

### Safety soundness vs. the oracle

| model              | caught | MISSED | over-restrictive | correct-accept |
|--------------------|:------:|:------:|:----------------:|:--------------:|
| ownership          | 33     | 82     | 6                | 90             |
| region             | 14     | 101    | 0                | 96             |
| capability         | 9      | 106    | 0                | 96             |
| affine             | 94     | 21     | 0                | 96             |
| region+capability  | 23     | 92     | 0                | 96             |
| region+ownership   | 47     | 68     | 6                | 90             |
| **region+cap+affine** | **115** | **0** | **0**           | **96**         |

### Bug-class coverage (caught / total unsafe cases)

| class                      | own | reg | cap | aff | r+c | r+o | r+c+a |
|----------------------------|:---:|:---:|:---:|:---:|:---:|:---:|:-----:|
| region-escape              | 0/17|12/17| 0/17| 5/17|12/17|12/17| 17/17 |
| data-race                  | 2/24| 0/24| 0/24|24/24| 0/24| 2/24|**24/24**|
| mutable-alias              | 4/4 | 0/4 | 4/4 | 0/4 | 4/4 | 4/4 | 4/4   |
| permission-violation       | 0/10| 0/10| 5/10| 6/10| 5/10| 0/10| 10/10 |
| use-after-free             |20/44| 0/44| 0/44|44/44| 0/44|20/44| 44/44 |
| double-free                | 6/6 | 0/6 | 0/6 | 6/6 | 0/6 | 6/6 | 6/6   |
| move-after-use             | 1/1 | 0/1 | 0/1 | 1/1 | 0/1 | 1/1 | 1/1   |
| dangling-edge-after-promote| 0/3 | 2/3 | 0/3 | 2/3 | 2/3 | 2/3 | **3/3**|
| invalid-free               | 0/2 | 0/2 | 0/2 | 2/2 | 0/2 | 0/2 | 2/2   |
| invalid-consume            | 0/1 | 0/1 | 0/1 | 1/1 | 0/1 | 0/1 | 1/1   |
| invalid-use                | 0/3 | 0/3 | 0/3 | 3/3 | 0/3 | 0/3 | **3/3**|

### Burden: annotations, state space, checker machinery

| model              | annots | share | B_S | res-st | temp | mv-rl | rules | psens |
|--------------------|:------:|:-----:|:---:|:------:|:----:|:-----:|:-----:|:-----:|
| region             | 175    | 1.4%  | 2   | 0      | 0    | 0     | **3** | no    |
| capability         | 183    | 1.5%  | 3   | 0      | 0    | 1     | 4     | no    |
| affine             | 246    | 2.0%  | 3   | 0      | 3    | 1     | 18    | yes   |
| region+capability  | 358    | 2.9%  | 5   | 0      | 0    | 1     | 7     | no    |
| ownership          | 274    | 2.2%  | 6   | 0      | 1    | 4     | 3     | no    |
| region+ownership   | 449    | 3.6%  | 8   | 0      | 1    | 4     | 6     | no    |
| **region+cap+affine** | 604 | 4.9%  | 8   | **3**  | 3    | **1** | **25**| yes   |

> `B_S` = programmer-visible states on ordinary memory; `res-st` = extra states
> only consumables pay. `rules` = distinct reject-rules (diagnostic kinds) — the
> **rule-growth** metric: each frontier that forces new machinery shows here.
> Graph surgery cost the region model **+1 rule** (promote edge-closure);
> alias-lifetime precision, interprocedural summaries, AND recursive fixed
> points each cost **+0 rules and +0 states** — all compiler machinery.
> Concurrency cost affine **+4 rules** (`data-race`, `use-after-send`,
> `task-escapes-scope`, `invalid-join`) — the first real rule growth since
> the temporal states — and the handle-authority round added **+1**
> (`stale-handle`). The atomic round added **+1** (`non-atomic-op`) and the
> mutex round **+4** (`unguarded-access`, `non-mutex-op`, `guard-escape`,
> `recursive-lock`), and the channel round **+2** (`send-escape`,
> `non-channel-op` — `use-after-send` was already a kind). Still **+0
> programmer-visible states** across all of it — mutex guards and
> in-transit authority run on the same Live|Consumed|Maybe machine.

Compiler-internal machinery (honest accounting — precision bought inside the
compiler still costs compiler complexity):

| model              | internal machinery |
|--------------------|--------------------|
| ownership          | borrow-counts |
| region             | edge-levels (`points_to`/`edges_to`) |
| capability         | borrow-counts |
| affine             | path-merge, alias-edge-graph, definite-kills, effect-summaries, scc-fixpoint, capability-partition(holds), join-merge, task-escape-check, capability-edges(identity), stale-handle-tracking, atomic-op-resolution, synchronized-domains(guards), channel-packets(in-transit) |
| region+capability  | edge-levels, borrow-counts |
| region+ownership   | edge-levels, borrow-counts |
| **region+cap+affine** | edge-levels, borrow-counts, path-merge, alias-edge-graph, definite-kills, effect-summaries, scc-fixpoint, capability-partition(holds), join-merge, task-escape-check, capability-edges(identity), stale-handle-tracking, atomic-op-resolution, synchronized-domains(guards), channel-packets(in-transit) |

### The decisive three-way

|                       | D (region+cap) | E (region+own) | F (region+cap+affine) |
|-----------------------|:---:|:---:|:---:|
| spatial safety        | strong | strong | strong |
| access safety         | strong | **none** | strong |
| temporal safety       | **none** | strong | strong |
| caught / missed / over| 23/38/0 | 34/27/**3** | **61/0/0** |
| states on memory      | 5 | 8 | **5** (+3 consumable-only) |
| move/borrow rules     | 1 | 4 | **1** |
| path-sensitive        | no | no | **yes** |
| rules                 | 7 | 6 | 13* |

\* F's 13 rules are mostly *guards on one operation* (the 6 affine kinds are all
consume-checks); the count is diagnostic-kind granularity, not machinery size.

---

## What the evidence says

### From the safety suite (01–30)

**1. The concerns really are separable — up to a point.** Region+Capability is
the union of two simple, orthogonal analyses, yet it **caught every
region-escape, every aliasing, and every permission violation with zero
over-restriction**. That is strong support for
`Memory Safety = Lifetime + Access` *as far as spatial safety goes*.

**2. The clean gap is temporal safety.** Prototype D missed exactly the
`use-after-free` / `double-free` / `move-after-use` cases — because *neither*
component tracks a value's move/free timeline. Only `ownership` does.

**3. Ownership's imprecision is real and measurable.** Scenario 23 (free on both
branches, never used again — safe) is **rejected** by `ownership` as a false
double-free, because the checker is not path-sensitive. `region+ownership`
inherits that over-restriction — it pays ownership's ergonomic cost without
gaining permission-violation coverage (0/4).

### From the expressiveness suite (31–45) — the more important result

**4. Regions make graphs cheap — even the nasty ones.** Doubly-linked lists,
A→B→C→A cycles, parent/child back-pointers, and shared-owner DAGs all *just
work* inside a single region, under **every** model. This is the strongest
argument yet for a region-first design: structures that require reference
counting or unsafe/arena hacks in Rust are free here.

**5. But pure regions cannot police *access*.** Scenario 36 (two live `&mut`
into a shared graph node) is a region MISS — the graph never dangles, so the
region model has nothing to say, even though two live writers is a real bug.
**Regions answer "is it alive?" — not "who may touch it?".** They need a
companion access mechanism; both hybrids supply one.

**6. `promote` eliminates cross-region lifetime reasoning.** The same transfer
that is a dangling escape (scenario 38) becomes legal when written `promote`
(37, 39). The programmer says *"this value moves out"* once, and the compiler's
region proof handles the rest — no lifetime parameter, no borrow annotation.
This is direct evidence that a `promote`/`transfer` primitive could remove a
large class of lifetime bookkeeping.

**7. `handle` decouples relationships from pointer lifetimes.** A handle stored
in a persistent structure across a region boundary (40) is safe where the raw
reference (41) is an escape — because the handle isn't a lifetime-tracked
pointer, it's a generation-checked lookup. This supports the hypothesis that
**reference safety ≠ pointer lifetime tracking**, and long-lived relationships
may not need to be ordinary references at all.

**8. The temporal gap is *specifically about resources*, not memory.** With
regions doing collective reclamation, plain memory needs no `free` at all — the
use-after-free/double-free scenarios only exist because we *modelled a resource
consume* as `free`. Once memory is region-owned, the temporal state that's left
is exactly a resource `{Live, Consumed}` — motivating **Prototype F**:
region + capability + a *minimal affine* consume operation, rather than full
ownership.

### From the anti-cheat suite (46–59) — Prototype F, falsified where it could be

**9. `MaybeConsumed` is the price of admission.** Temporal safety needed exactly
one more state than planned: `merge(Live, Consumed) = MaybeConsumed`. With only
two states, `if c { free f }` would leave a binding either wrongly Live
(unsound) or wrongly Consumed (over-restrictive). The third state is *forced by
control flow*, not by the memory model — still cheap: 3 temporal states vs
ownership's 6-state timeline on every binding.

**10. Loops need a back-edge merge, and it's enough.** `while c { free f }`
double-consumes on iteration 2 — caught by re-checking the body under the
merged (post-join) state. No general fixpoint iteration was needed; two passes
suffice because the state space is finite and monotone.

**11. F catches temporal classes E and D both miss.** Freeing region memory
(46), consuming through an alias (54), a stored-field alias outliving a consume
(51), and an alias escaping via a `-> &T` callee (59) are all caught by affine
rules that ownership's borrow counts and capability's access sets don't model.
These fell out of *"a consume requires the only handle"* — a rule that belongs
to the temporal layer, not to ownership.

**12. ~~F's one over-restriction~~ → *fixed* by the alias graph (see below).**
Scenario 58 was the only F false positive: an alias stored in a struct dies
when the struct is reassigned, but the birth-only alias *count* couldn't see
it. Now resolved by edge-death tracking — F is at **0 over-restrictions**.

**13. The hypothesis held.** `Safety = Region + Capability + Affine` produced
the strongest model tested: maximum coverage, zero unsound misses, and (after
the alias round) zero over-restrictions, with temporal state that applies to
*resources only*. Ordinary memory never carries `Live/Consumed/Maybe` — it's
pure region.

### From the graph-surgery suite (60–70) — attacking F's strongest assumption

**14. Cross-region edges are *directional*, and regions already know the rule.**
A short-lived region's node may safely point into a longer-lived region (60 —
accepted by every model); the reverse edge dangles and every region-based model
rejects it (61). The rule is purely local: *an edge may only point toward a
longer-or-equal lifetime*. No global reachability analysis needed.

**15. Shallow `promote` was genuinely unsound for non-leaf objects — and the
corpus forced a new rule.** `promote` re-homed the *binding* but left the
object's *stored edges* pointing into a dead region. Before the fix, 62/63/68
were universal misses. The invented mechanism is exactly what the corpus
demanded: **promote requires edge-closure** — `promote x` is legal only if `x`
stores no references into still-dying regions (`x.edges_to == 0`). Cost: one
new per-variable counter (`edges_to`), one new diagnostic kind. No transitive
traversal, no graph reachability — the region model already maintained
`points_to` per binding, so the check stayed local.

**16. The two mechanisms split the dangling-edge class — and it's a clean
split.** The region rule catches *outgoing* edges (62 cycle member, 63 subtree
root: the promoted node still references dying memory). The affine
consume-while-aliased guard catches *incoming* edges (68 back-pointer: a live
alias of `p` means `p` cannot be moved/consumed). Neither model covers the
other's half — F needs both. This is evidence the decomposition is real:
"lifetime of what I point at" and "what points at me" are different questions
answered by different layers.

**17. The "raw refs are local, handles are persistent" rule held perfectly.**
Merging graphs via handles across sibling regions (64) and a handle surviving
its region's destruction (66) are both accepted — safe by generation-check
design. The identical merges via raw refs (65) are rejected. A persistent
struct can hold handles to region objects *without* lifetime coupling — this is
the strongest evidence yet that reference safety does not require pointer
lifetime tracking.

**18. Graph surgery inside control flow is free.** Loop-grown chains (69) and
conditional edges (70) work without any new machinery — the models' existing
path handling suffices. Not every hard case is a new rule; the corpus must keep
distinguishing "needs machinery" from "already handled."

**19. Rule growth stayed bounded.** The whole graph-surgery frontier cost the
region model **+1 rule** (promote edge-closure). F's rule count is now 13
diagnostic kinds — but six are guards on a single operation (`consume`), and
zero new *states* were added. The complexity did not drift toward ownership.

### From the alias-lifetime suite (71–84) — killing F's last over-restriction

The attack: **alias birth was tracked, alias death was not.** A counter
(`aliases(x) += 1` on creation, released only at the holder's scope exit)
over-restricted every scenario where an alias dies by *rebind*, *field
overwrite*, or *container death* rather than lexical scope exit.

The fix models aliases as an **edge graph**, not a counter:

    let a = &x            -> edge (a,  -,  x)
    h.r = &x              -> edge (h, "r", x)
    let a = grab(&x)      -> edge (a,  -,  x)   (ref-returning call)

    edge dies when: holder's scope ends, holder is rebound, the field is
    overwritten, or the target is consumed/redeclared.

**20. Alias death eliminated 8 over-rejections with ZERO new programmer-visible
rules and ZERO new states.** Scenario 58 (the known false positive) plus the
new corpus's rebind/overwrite/container-death cases (72, 73, 74, 76, 78, 79, 80)
all became correct accepts — a counter can't express any of them, an edge
graph gets all of them for free. Precision gain per language rule:

    P = false rejections eliminated / new programmer-visible rules = 8 / 0

The complexity moved *into the compiler* (the `machinery` column records it),
which is exactly where this project wants complexity to live.

**21. Alias merging is union, and it stays internal.** After
`if c { a = &x } else { a = &y }`, the merged alias set is *both* edges — a
compiler-internal "possible alias" fact (75 is correctly rejected). This is NOT
a programmer-visible `MaybeAlias` state:

    programmer-visible states:   Live | Consumed | MaybeConsumed
    compiler-internal facts:     definite-edge | possible-edge | no-edge

Not every analysis state needs to become a language state.

**22. The corpus caught the same birth-only bug in the OTHER models.** Fixing
the affine alias graph exposed parallel imprecision elsewhere, and each got
the same honest treatment:

- `ownership` never releases a borrow on *rebind* (`a = &x; a = &y; free x`
  still counts x as borrowed → over-rejects 73, 76) and treated `handle x`
  as a move (over-rejected handle-then-free, now read-only).
- `capability` conflated *rebinding a variable* with *writing through its
  capability* — `a = &y` where `a: &T` was wrongly rejected as
  write-through-read. Rebind now re-derives the capability instead.

**23. Handles confirm "not a lifetime alias."** Storing `handle x` in a field
(81) or binding (84) creates no alias edge — generation-checked lookup means a
stale handle *fails safely*, so `free x` is legal where the raw `&x` (51)
blocks it. The handle/reference distinction now holds on the temporal axis
too, not just the region axis.

**24. F is now clean on the intra-function corpus:** the remaining frontier
was whether alias facts survive function boundaries — answered next.

### From the interprocedural suite (85–96) — a call is a graph transformation

Each function is analyzed once (callee-first, topologically) and captured as a
compact **Effect summary**:

    kills:            param paths overwritten  (edge death)
    creates:          param->param edges live at exit  (edge birth)
    consumes/maybe:   param temporal-state transitions
    returns-alias:    params the return value may alias

Callers apply the summary to their own graph — `G_before -> G_after` — without
re-analyzing bodies. `mml effects <file>` prints them (compiler-derived, never
programmer-written).

**25. Summaries compose for free.** `a_pass -> b_pass -> c_store` propagates
`creates: h.ptr -> x` identically at each level (93) — composition fell out of
applying summaries *during* summary computation, no fixpoint needed for
non-cyclic graphs.

**26. Edge death must cross boundaries too.** `connect(h,x); clear(h); free x`
accepts because `clear`'s summary carries `kills: h.ptr` (87). Birth-only
summaries would over-restrict forever — the alias graph's death rules are as
interprocedural as its birth rules.

**27. Temporal state crosses boundaries through `own`, not summaries.**
Conditional consumption inside a callee (91) needs no `MaybeConsumed` channel
— an `own` parameter removes caller authority unconditionally. The hard case
the user predicted simply doesn't arise under F's decomposition.

**28. Recursion currently relies on the conservative fallback.** Mutual
recursion (95) defeats single-pass summaries; the ref-return heuristic still
catches it. A fixpoint over cyclic call graphs is the honest next step — and
the summary stays tiny meanwhile (max facts/function observed: 3).

**29. Summaries are MORE precise than the heuristic they replace.** `pick(&x,
&y) -> return y` yields `returns-alias: param 1` only (96) — the heuristic
("any borrowed arg") would over-restrict `free x`. Interprocedural analysis
bought precision, not just coverage.

**30. Programmer-visible complexity unchanged.** +0 rules, +0 states across
the whole suite — everything went into `machinery` (effect-summaries). The
P metric keeps paying off:
`false rejections eliminated / new programmer-visible rules`.

### From the recursion suite (97–112) — summaries are a monotone fixed point

The call graph is decomposed into strongly-connected components (Tarjan),
processed callee-first. Acyclic SCCs analyze once; recursive SCCs iterate
until a full pass adds no facts:

    seed cyclic summaries
        ↓
    pass: re-analyze every member, union-absorb new facts
        ↓
    grew? ──yes──▶ repeat        ──no──▶ converged; keep this pass's diags

**31. Kills needed a definite/possible split — and the corpus proved it was
unsound without one.** A kill recorded flatly became "definite" even inside
`if`/`while` bodies: `maybe_clear` summarized as `kills: h.ptr` let a caller
free `x` while `h.ptr` might still point at it (105, 110 — both unsound
misses before the fix). Now pending kills intersect at `if` joins (a kill is
definite only if *every* path did it) and are discarded from `while` bodies
(the zero-iteration path keeps the edge). Each fact class has its own tiny
lattice: edges are *may*, kills are *must*.

**32. The corpus found the one case single-pass ordering can't solve.** When
DFS emits `b_skip` before `a_set` and `b_skip`'s only effect arrives via the
back-edge to `a_set`, a single pass leaves `b_skip`'s summary empty (111 —
unsound miss) or loses a definite kill (112 — over-restriction). Both fixed
by iteration; interestingly the iteration count stays tiny.

**33. Convergence is fast and summaries stay small — measured, not assumed.**
Across the recursion suite: SCCs ≤ 3 functions, ≤ 3 passes to converge
(`facts/pass` e.g. `[2,1,0]` for 111, `[3,0]` for the 3-fn cycle), largest
summary = 3 facts. `mml effects <file>` now prints SCC count, largest SCC,
passes, facts-added-per-pass, seeded returns, and summary sizes — the
convergence metrics are observable, not hidden.

**34. Cyclic `returns-alias` needs a seed, not a fixpoint.** A return-chain
through itself (`return a(x)` ↔ `return b(x)`) has least fixpoint `∅` — the
functions literally never return — yet safety requires treating the result
as possibly-aliased (95, 103). We seed cyclic `ret_alias` with the borrowed-
arg bound (the conservative top for that one fact class); everything else
grows honestly from bottom. Finding: **the least fixpoint of a may-analysis
under-approximates never-returning cycles** — seeding is the honest
acknowledgment that termination isn't proven.

**35. The user's "nastiest case" doesn't exist under F.** `A(x){if c{B(x)}}
B(x){if c{consume x}else{A(x)}}` should need a `maybe-consumes` channel
through the cycle — but `own` is a move at the boundary, so caller authority
is gone unconditionally (101). Temporal state recursion collapsed to zero
machinery; the summary fields for it sit inert on ref/mut params (which
can't be consumed anyway).

**36. Programmer-visible cost of full recursive interprocedural analysis:
still zero.** +0 rules, +0 states — the round added two lines to `machinery`
(`definite-kills`, `scc-fixpoint`) and an `AnalysisStats` readout.

### From the concurrency suite (113–136) — the falsifiable question held

The question: **can capability transfer + region validity make data races
unrepresentable without a full ownership model on ordinary memory?** The
first corpus round answered *mostly yes*. Syntax is minimal — `let t =
spawn { ... }` and `join t` / `let r = join t`; there is no `send` keyword:
a child touching a parent's `own` var *is* the transfer.

**37. Sequential baseline could not see any of it.** Run before
implementation, the inline-checker baseline missed all 8 concurrency-specific
behaviours: both write races (114, 115), write-through-alias race (125),
nested-spawn race (128), task-outlives-region (122), unjoined task (136),
kill-not-visible-until-join (127), and task-returned alias (131). Every one
needed real task semantics.

**38. A task is a deferred graph transform.** `spawn` analyses the body in a
forked env and records a `TaskResult` — holds, definite kills, surviving
edges, exit states, return value. `join` applies it, exactly like a call's
effect summary but *suspended in time*. `G_parent → G_parent - transferred`
at spawn; `G_parent ← G_child exit` at join. Sequential consistency is
restored at the join boundary: post-join writes are legal again (132).

**39. Race freedom = capability partition, not a new type system.** The
child's touches become holds — `Shared` (reads), `Write` (writes + kills +
creates + mut-args), `Transferred` (consumed/moved vars) — propagated
transitively along alias edges so `a.fd = 1` through `a = &mut x` holds `x`
exclusively (125). Parent access during the window: read-vs-Shared is the
only legal overlap; anything else is `data-race`; touching a `Transferred`
var is `use-after-send`. No new types — the existing capability
read/write/own trichotomy became the concurrency partition.

**40. `lifetime(region) >= lifetime(task)` is lexical.** A var held by an
unjoined task cannot go out of scope — the region/scope *is* the lifetime
bound (122, 136). No lifetime inequality language exists or is needed.
Shadowing or rebinding a held var fires the same diagnostic.

**41. Conditional joins merge as may-holds.** `if c { join t }` keeps the
task's holds on the else path → the parent's next use is `use-after-send`
(135). A Maybe'd handle downgrades applied effects (Consumed→Maybe, kills
withheld, edges still union) — the same merge lattice the fixpoint round
established, reused.

**42. Kills across the boundary are time-shifted.** `a = &y` inside a task
is definite in the child but only *visible* to the parent at join — freeing
`x` in the window must reject (127); after join the kill applies and
`free x` accepts (126). Alias death is still structural; `join` is simply
the newest edge-death site.

**43. Nested tasks propagate transitively — the corpus caught the leak.**
Grandchild holds count as the child's touches *at spawn time*, not at the
child's exit: 128 (grandchild writes, child joins it early) initially missed
because capture only looked at still-live nested tasks. Fixed by folding
each spawn's holds into the spawning env's read/write sets — the window is
the child's whole body.

**44. Handles were the sanctioned unchecked boundary — then it got
closed.** Two handles to `x` wrote concurrently (134): accepted at first
*by design* — generation-checked tickets, the Mutex-equivalent escape
hatch. The handle-authority round (137–150) then closed it: handles became
capability edges, the race became representable, and 134's oracle flipped
from accept to reject. The corpus evolving to falsify a prior "by design"
decision is the discipline working.

**45. Cost: +4 rules, +0 states.** `data-race`, `use-after-send`,
`task-escapes-scope`, `invalid-join` are the first programmer-visible rules
added since the temporal states — honest growth for a new domain. States,
annotations, and the source model are unchanged; machinery gained
`capability-partition(holds)`, `join-merge`, `task-escape-check`.

### From the handle-authority suite (137–150) — the escape hatch closed

The question: **can handles stay the persistent cross-region/cross-task
reference without becoming the language's `unsafe`?** The answer that
landed: `Handle = Identity`, `Capability = Authority` — a handle answers
"which object?" while `read`/`write`/`own` still answer "may I?". No
`read_handle`/`write_handle`/`own_handle` type family needed.

**46. Six baseline misses, all expected.** Handle write race (138), handle
read+write race (139), flipped-134, nested-spawn-through-handle (149), and
stale tickets (142, 143) — all invisible when handles carried no edges.

**47. A capability edge is an alias edge minus the pin.** `h = handle x`
records `h -> x` in a parallel `cap_edges` set — identical birth (create,
copy, store, join-return), identical death (scope, rebind, overwrite,
consume, call), identical task-hold propagation — but `alias_count` ignores
it, so `free x` stays legal with a live ticket (150: precision preserved).
The handle *use* gets the authority check, not the handle's existence.

**48. Staleness became a static error where provable.** When a capability
edge's target dies (consume, scope exit, rebind, shadow), `(holder, path)`
lands in `dead_caps`; a later dereference reports `stale-handle` — the
runtime generation check, promoted to a compile error wherever the graph
can prove it (142, 143).

**49. Write-through resolution is precise about slots vs referents.**
`h.fd = 1` on a handle writes `x` while only *reading* `h` — so `let g = h`
(copying a ticket) never races a child's write-through. Path crossing:
`b.slot.fd` writes through the `(b,"slot")` edge; `b.slot = v` overwrites
the slot and kills the edge. The rule fires wherever the written path
crosses a live edge prefix.

**50. Cost: +1 rule, +0 states.** `stale-handle` is the only new visible
rule; machinery gained `capability-edges(identity)` and
`stale-handle-tracking`. Remaining gap, recorded honestly: function
summaries can't yet express capability-edge *creation* (a callee storing
a handle into a param's field isn't a summary fact) — such stores are
conservatively alias-tracked today.

### Standing hypothesis after the handle-authority round

    Memory Safety = Region Validity + Access Capability + Minimal Temporal State
                  + Structural Alias Graph (compiler-internal)

    where:
      Region Validity   = edges only point toward longer lifetimes,
                          promote requires edge-closure (x.edges_to == 0)
      Access Capability = read/write/own, exclusivity, no escalation;
                          rebinding a variable is NOT writing through its
                          capability
      Temporal State    = {Live, Consumed, MaybeConsumed} on consumables ONLY,
                          merged pointwise at control-flow joins
      Alias Tracking    = edge graph with explicit birth AND death,
                          propagated across calls by per-function effect
                          summaries; cyclic calls converge via monotone
                          SCC fixpoint (a call is a graph transformation)
      Task Authority    = capability partition at spawn — shared reads may
                          overlap, writes are exclusive, own transfers;
                          join applies the task's exit world as a deferred
                          graph transform (a task is a suspended call);
                          scope end before join is a lifetime violation
      Handle            = identity edge (capability edge): propagates
                          holds/reach like an alias but never pins its
                          target; dead targets make the ticket provably
                          stale — Handle says which, Capability says may

    PROGRAMMER-VISIBLE:  regions, read/write/own, promote, handle,
                         Live | Consumed | MaybeConsumed,
                         spawn, join, atomic<T> + atomic ops
    COMPILER-INTERNAL:   region levels, edge closure, definite/possible alias
                         edges, edge birth/death, path merges, effect
                         summaries, SCC decomposition, fixpoint iteration,
                         task holds (Shared|Atomic|Write|Transferred),
                         join merge, task-lifetime checks, capability edges,
                         stale-target tracking, atomic-op resolution

### From the atomic suite (151–171) — synchronization as an operation, not an escape

The falsification target: **atomicity is an operation-level synchronization
permission, not a general escape from capability rules.** `atomic<T>`
storage + an atomic op → synchronized access; `atomic<T>` storage +
ordinary access → normal capability/race rules; an atomic op on non-atomic
storage → rejected. The proposal lived in `ATOMIC-PROPOSAL.md` against
frozen `SPEC.md` v0, survived falsification, and merged back as an
amendment — the corpus-then-spec protocol now governs the project.

**51. The baseline missed exactly the mixed-access half.** Sequential
reasoning reads `atomic_add(x, 1)`'s arguments as plain reads → `Shared`
holds, which coincidentally catches child-atomic vs parent-*write* (153,
155, 160's shape) but accepts child-atomic vs parent-*read* (154), sibling
mixed access (169), ops on non-atomic storage (158, 166), and
handle-mediated atomic access entirely (160).

**52. `Hold::Atomic` — one new internal hold kind, zero new source
concepts.** The task-partition gained a fourth member: `Shared | Atomic |
Write | Transferred`. Compatibility is a crisp square: `Atomic`×`Atomic`
legal; `Atomic` vs any ordinary access → `data-race`; `Shared`/`Write`
vs an incoming atomic op → `data-race`. A var touched both plainly and
atomically inside one task degenerates to `Write` — an unsynchronized
access exists in its window, so nothing may race it.

**53. Atomic ops resolve through both graphs.** `atomic_add(h, 1)` reaches
`h`'s capability-edge target; `atomic_add(s.cnt, 1)` names an atomic
*field* (struct-decls now record which fields are `atomic<T>`); a stale
ticket reports `stale-handle`, not a race. Copies of a ticket synchronize
the same target (170). The op is the permission — the storage's declared
type is the precondition.

**54. Cost: +1 rule, +0 states, +1 machinery item.** `non-atomic-op` is the
only new diagnostic kind — every atomic race reuses `data-race`. Affine
stands at 12 rules, F at 19; data-race coverage 16/16. Root-granular task
holds are the recorded precision bound: 167's `s.cnt` atomic + `s.data`
write races at the root — conservative, not unsound. Memory ordering
(relaxed/acquire/release/seq_cst) is explicitly deferred — this round
froze at "atomic ops are race-safe."

### From the mutex suite (172–191) — runtime-serialized exclusivity

The falsification target: **a mutex does not grant permanent write
authority — `lock` derives temporary exclusive authority for the guard's
lifetime.** Static capability partitioning proves exclusivity at compile
time; a mutex guard gets it from the runtime lock — two different reasons
the same write is safe. `MUTEX-PROPOSAL.md` froze the budget first; the
claim survived and merged as `SPEC.md` Amendment B.

**55. `Hold::Sync(domain)` — the fifth internal hold kind.** Writes
through a live guard resolve through the guard's capability edge into the
mutex's closure and record `sync_writes[target] += domain`. At task
capture they become `Sync(domains)` — compatible *only* with the same
singleton domain (two tasks locking `m` serialize — 172, 173, 179, 189);
disjoint domains race (178 — two mutexes over one object); every ordinary
access races (176, 177). The compat rule is singleton-equality, not
intersection: a task that may write under either of two locks can meet a
sibling under the other.

**56. Guards needed ZERO new temporal states.** `unlock(g)` is `Op::Unlock`
on the existing consume path — double-unlock is `double-consume` (186),
write-after-unlock is `use-after-consume` (187), a captured guard is
`use-after-send` for the parent (182). Domain release rides the ordinary
`pop_scope`/rebind machinery (180). The mutex is not a transparent
wrapper: `m.x` bypasses the lock → `unguarded-access` (174, 175).

**57. The synchronization domain is identity, and it must resolve.**
`lock(s.a)` gives domain `s.a`; `lock(hm)` follows `hm`'s capability edge
to `m` — the same mutex, the same domain (189). `lock(m)` twice in one
scope is `recursive-lock` (183 — self-deadlock, the only deadlock rule v1
claims; cross-task ordering deadlock is an explicit non-goal, 184
accepted). Effect summaries gained fact class `syncs: (param, suffix)` —
`update(m)`'s guarded writes replay as synchronized on the actual (191);
a `mut` arg covered by a sync fact is not an unsynchronized write.

**58. Cost: +4 rules, +0 states, +1 hold kind, +1 summary fact, +1
machinery item.** `unguarded-access`, `non-mutex-op`, `guard-escape`,
`recursive-lock` are the new diagnostics; every race reuses `data-race`.
Affine 12 → 16 rules, F 19 → 23; data-race coverage 21/21. The headline
formula landed exactly:

```text
Safe Shared Mutation = Capability + Synchronization Domain + Live Guard
```

with `Guard` = an ordinary affine value — no second ownership model.

### From the channel suite (192–211) — transported authority

The falsification target: **a channel transfers authority; it does not
duplicate authority.** If true, message passing maps onto the existing
affine move + graph machinery instead of needing a concurrency
ownership subsystem.

**59. `send` is affine transfer, not a new consume kind.** `send(ch, p)`
runs `Op::Move` on the payload: use-after-send is `use-after-consume`
(192), double-send is `double-consume` (194), `close`/`double-close`
reuse `use-after-consume`/`double-consume` on the endpoint itself
(206–207). Copy payloads and handle tickets copy — the sender keeps
them (195, 200). Zero new temporal states: the Live|Consumed|Maybe
machine ate the whole transfer rule.

**60. `in_transit` is topology, not a state.** `send(ch, &mut x)` cuts
the capability edge `sender → x` and records `x`'s authority as
channel-held; `recv` installs it on the receiver (196). Any access to
`x` while parked is `use-after-send` (197) — including a sibling task
that writes `x` without receiving (211). No `InTransit` variant was
added to `Temp`: the authority's *location* lives in a set, its *state*
stays Live.

**61. Channel state is shared memory — merging is immediate.** At
`spawn` the child sees the parent's in-flight packets and transit set;
at capture the child's final queue *supersedes* (it contains the
snapshot it started from) and its transit map becomes the parent's
(shared names only). That makes the authority graph single-owner across
task boundaries: task1 `send(ch, &mut x)` → task2 `recv` + write is
legal (208), while task2 writing `x` unmediated is `use-after-send`
(211). Endpoint ops are `Sync({ch})` — the mutex round's domain hold
reused — so concurrent sends/recvs never race on `ch` itself.

**62. Edge closure restated for transport.** Sending a value that still
embeds an *alias* edge (`b.p = &x`) rejects as `send-escape` — the
promote closure rule at the channel boundary (203). An embedded
*capability* edge travels fine (204) — `Handle = Identity` survives
message passing unchanged: a received handle writes through to the
target only if the receiver races it like any other access (201), and a
packet whose target died in flight delivers a provable `stale-handle`
at `recv` (202). Guards cannot cross (209) — scope-bound authority is
not transportable authority.

**63. Cost: +2 new rules, +0 states, +0 hold kinds.** `send-escape` and
`non-channel-op` are new; `use-after-send` was already a kind (task
transfer); `Sync({ch})` reuses the mutex domain hold. Affine 16 → 18
rules, F 23 → 25; data-race coverage 24/24. The unification held:

```text
call    = immediate graph transform
spawn   = suspended graph transform   (applied at join)
channel = transported graph transform (applied across tasks)
mutex   = temporarily derived synchronized authority
atomic  = operation-scoped synchronized authority

Message Passing = Authority Transfer + Region Validity
```

**not** `full ownership + borrowing + explicit lifetimes` — and deliberately
**not** borrow *lifetimes*: alias death is structural (scope / rebind /
overwrite / consume / call-effect), not a lifetime the programmer ever names.
Interprocedural lifetime reasoning now emerges from graph effects and
fixed-point propagation — no lifetime parameters anywhere.

---

## Usage

```bash
cargo run -- models                    # list the models
cargo run -- test corpus               # run corpus; print soundness + coverage + burden
cargo run -- check corpus/22_branch_free_then_use.mml   # per-model verdicts + diagnostics
cargo run -- metrics corpus/17_mut_alias_across_call.mml
cargo run -- ast corpus/01_safe_basic.mml
cargo test                             # lock every recorded @expect verdict
cargo test --workspace                 # semantic corpus + native suite

# Phase Three — the native compiler:
cargo run -p ourc -- hello.lang        # → hello.ll, hello.o, hello
./hello; echo $?                       # → 42
cargo run -p ourc -- f.lang --show-ast | --show-ir | --show-llvm | --show-asm
cargo run -p ourc -- test native-tests # the codegen corpus
```

Adding a scenario: drop a `.mml` in `corpus/` with `@desc` / `@class` /
`@safety` (the oracle) and one `@expect <model>` line per model.

---

## Architecture

```
src/
  lexer.rs        hand-written tokenizer
  ast.rs          shared AST + annotation vocabulary
  parser.rs       recursive-descent parser
  check.rs        Diagnostic + MemoryModel trait (the plug-in point)
  models/
    ownership.rs   move + borrow checker
    region.rs      region escape analysis (points_to / edges_to, promote
                   edge-closure)
    capability.rs  read/write/own permission checker
    affine.rs      minimal temporal checker + alias edge graph + effect summaries
                   (Tarjan SCCs + monotone fixpoint for cyclic calls)
    hybrid.rs      Prototypes D (region ⊕ cap), E (region ⊕ own),
                   F (region ⊕ cap ⊕ affine)
  metrics.rs      annotation counts + per-model burden / token share
  harness.rs      @desc/@class/@safety/@expect parsing + oracle classification
  main.rs         CLI + corpus regression test
corpus/           211 scenarios (safety + expressiveness + anti-cheat +
                  graph-surgery + alias-lifetime), each with a safety oracle
compiler/         the `ourc` crate — the native compiler (Phase Three)
  src/frontend/   .lang lexer, AST, recursive-descent parser
  src/semantics/  typed-AST check (machine ints, scopes, arity)
  src/ir/         our own IR: SSA temps + local memory slots, blocks/terms
  src/backend_llvm/  textual .ll emission — the only backend boundary
  src/driver/     ourc CLI, pipeline, native test runner
native-tests/     the codegen corpus: @exit / @compile-error per .lang
```

Adding a model is: implement `MemoryModel`, register in `models::all()`.

---

## Where this goes next (the project plan)

```
DONE  3 base models + Prototype D, 30 safety scenarios, oracle-scored
DONE  Expressiveness corpus: graphs, cycles, promote/transfer, handles, resources
DONE  Prototype E: Region + Ownership   -> head-to-head vs D recorded
DONE  Prototype F: Region + Capability + minimal Affine
      -> 32/32 caught, 0 unsound miss, 1 over-restriction; MaybeConsumed found
DONE  Cross-region graph surgery: edge-direction rule, promote edge-closure
      (+1 rule), handle-bridge hypothesis confirmed, incoming vs outgoing
      edge coverage split cleanly across region/affine
DONE  Alias-lifetime precision: alias edge graph with death rules
      (scope/rebind/field-overwrite/consume) — F's last over-restriction
      eliminated with +0 programmer-visible rules, +0 new states.
      Same bug found & fixed in capability (rebind != write-through) and
      ownership (handle-as-move; rebind still over-restricts borrow counts)
DONE  Interprocedural aliasing: per-function effect summaries
      (kills/creates/consumes/returns-alias), callee-first topological
      analysis, compositional. `mml effects <file>` exposes the summaries.
DONE  Recursive/fixed-point summaries: Tarjan SCCs + monotone union
      iteration; kills made definite-only (intersect at joins, dropped from
      loop bodies) fixing two unsound misses; cyclic ret_alias seeded with
      the borrowed-arg bound. F = 61/0/0 across 112 scenarios, still
      +0 rules / +0 states. `mml effects` reports convergence metrics
      (SCC sizes, passes, facts/pass) — worst observed: 3 passes, 3 facts.
DONE  Concurrency v1: spawn/join with no `send` keyword — capture IS the
      transfer. A task is a deferred graph transform (TaskResult applied
      at join); holds partition as Shared|Write|Transferred over alias
      edges; scope-end-before-join is a lifetime violation. F = 73/0/0
      across 136 scenarios, data-race 5/5; cost was +4 rules, +0 states.
DONE  Handle authority: handle = identity (capability edge), authority =
      capability. The last race escape hatch closed — data-race coverage
      9/9; stale-handle detection static where provable (+1 rule, +0
      states). F = 79/0/0 across 150 scenarios.
DONE  v0 semantic freeze: SPEC.md — 18 rules, each cited to the scenarios
      that forced it; the corpus-then-spec protocol now governs evolution
      (frozen baseline + proposal + corpus + falsification -> amendment).
DONE  Atomic synchronization: atomic<T> storage + atomic_load/store/add,
      compare_exchange. Atomicity is an operation-level permission:
      ops synchronize with each other, mixed access races, ops on
      non-atomic storage reject (+1 rule non-atomic-op, +0 states,
      +1 internal hold kind). F = 91/0/0 across 171 scenarios,
      data-race 16/16. Memory ordering deferred.
DONE  Mutex synchronization: mutex<T> + lock/unlock. The claim held —
      Safe Shared Mutation = Capability + Sync Domain + Live Guard,
      with +0 programmer states: guards are ordinary affine bindings
      carrying a domain-tagged internal hold (Hold::Sync). Same-domain
      writers serialize, disjoint domains and unguarded access race;
      summaries carry the domain across calls (+4 rules, +1 hold kind,
      +1 summary fact). F = 103/0/0 across 191 scenarios,
      data-race 21/21. Deadlock-freedom is a recorded non-goal.
DONE  Channels: channel<T> + send/recv/close. The claim held — a
      channel is a transported graph transform: send cuts an authority
      packet, recv installs it. in_transit is topology (a set), not a
      temporal state; endpoint ops reuse Sync domains; send-escape
      restates edge closure. +2 rules, +0 states, +0 hold kinds.
      F = 115/0/0 across 211 scenarios, data-race 24/24.
      Buffering/select/endpoint-types/drain are recorded non-goals.

Phase Three — native compiler bring-up:

DONE  N1: hello.lang → parse → typecheck → our IR → LLVM IR → .o →
      link → ./hello → exit 42. The `ourc` crate: frontend / semantics /
      ir / backend_llvm / driver; locals are memory slots, temps are SSA
      values (mem2reg owns phis); every stage leaves a readable artifact
      (--show-ast/ir/llvm/asm). Language surface: ints u8..u64/i8..i64,
      bool, let/assign, fns + forward calls, if/else, while, return.
DONE  native-tests/: codegen oracles (@exit, @compile-error) — caught
      a real bug on first run (params never stored to slots).
      cargo test --workspace runs both corpora.
DONE  N2 aggregates: structs, field/index paths, arrays — places are
      first-class in our IR (root slot + path), GEP exists only at the
      LLVM boundary.
DONE  N3 regions: region {} frames + &T/&mut T + provenance per value;
      `return &x` dies before LLVM ("reference escapes region").
DONE  N4 alias topology: structural (holder,target,kind,cert) edges —
      birth at &/copy/call/param, death at scope/rebind/overwrite/move;
      Shared+Shared ok, everything+Mutable rejects; Maybe-cert on joins.
DONE  N5 capabilities: `read`/`write`/`own` param modifiers — place
      aliases into caller storage with authority contracts; derivation
      law enforced (no Read→Write escalation, reborrow-move for Write,
      Own un-forgeable); deref resolution follows live edges
      ((*r).f -> x.f), `&*m` freeze-for-free.
DONE  N6 promotion + handles: `promote x` moves a binding's owner one
      region outward under edge closure (dangling-edge-after-promote);
      `handle p` creates a second, parallel identity graph — identity ≠
      authority ≠ lifetime, stale-at-use. Promoted bindings percolate
      scopes; conditional promotes merge to the deeper owner.
DONE  N7 affine consumables: `consume p` / `consume p.f` / `f(own x)`
      enter the temporal machine (Live→Consumed; joins →MaybeConsumed;
      reassign resurrects). Field-level keys consume `p.f` without
      touching `p.b`. Consume-while-aliased rejected; consume through a
      handle rejected (identity ≠ authority); consumed targets stale
      their handles. Ordinary region memory never enters the map.
DONE  N8 effect summaries: callee-first analysis derives a
      param-path-keyed diff — creates/kills alias and cap edges on
      `$argN[.path]`, retargeted through call-site arg bases. May/must
      asymmetry preserved (conditional kills downgrade, not remove);
      returned aliases/handles translate; own-param effects stay
      private; summaries compose transitively. Cyclic fns fall back to
      arg-union — never worse than before. Zero new syntax.
      120/120 native tests.
DONE  N9 spawn/join: `let t = spawn { .. }` analyzes the body on a
      forked ctx, then records its external accesses as REAL hold edges
      `t -> place` — the borrow matrix guards the [spawn, join) window
      unchanged (Shared+Shared ok, Write conflicts, own-transfer blocks).
      `join t` replays the suspended kills→creates→caps→affine diff and
      consumes the token (join-once via N7). Task-region escape and
      task-local ref returns reject; handles preserve stale-at-use
      identity. (Codegen was sequential inlining until N13 made spawn a
      real OS thread — the discipline was static first.) New surface:
      `spawn { }`, `join`, `task T`.
      138/138 native tests.
  ↓
DONE  N10 atomics: `atomic T` storage class + builtin ops
      (atomic_load/store/add, compare_exchange) — real `load atomic`/
      `atomicrmw`/`cmpxchg` instructions, seq_cst. Atomic holds tolerate
      only Atomic overlap; every unsynchronized touch races — parent
      AND sibling windows. Ops resolve through handles (`h` names the
      referent), refs, fields, and capability params. Plain sequential
      access stays legal. New surface: `atomic T` + 4 builtin names.
      155/155 native tests.
  ↓
DONE  N11 mutex domains: `mutex T` + `lock(m)`/`unlock(g)` — the guard
      is an ordinary affine value (double-unlock, use-after-unlock,
      guard-escape all reuse N7/N3). `Auth::Sync` is the fifth hold
      kind; the domain rides in the edge target (`t -> m#e`), so
      Sync×same-domain serializes while Sync×plain and
      Sync×foreign-domain race. `mutex h` binds the domain to the
      handle's REFERENT (`doms[m] = c#e`) — handle-mediated locking
      resolves identity, not the token. Interior access without a live
      guard rejects at every accessor; `read`-cap mutex params cannot
      lock. FnSummary.syncs replays `$argN` domain claims at call
      sites — `update(m)` from two tasks serializes correctly.
      Path-precise (mutex field protects only its payload). Deadlock
      freedom is honest non-goal; recursive same-context lock rejects.
      (Codegen shipped the guard as a ptr to protected storage; N13
      gave it a real lock word.) New surface: `mutex T`,
      `mutex e`, `lock`, `unlock`, `guard T`. 175/175 native tests.
  ↓
DONE  N12 channels: `channel T` + `channel()` + `send`/`recv`/`close` —
      a channel transports authority, it does not duplicate it. `send`
      cuts a `Packet` (the edge/cap delta) onto the endpoint's queue;
      `recv` installs the delta at the binding; `close` is N7
      endpoint-consume. `send(ch, &mut x)` parks x's authority in the
      `transit` set — topology, not a state — so parent AND sibling
      access is use-after-send until a `recv` claims it; the sender's
      momentary derivation hold is removed so the receiver's claim is
      race-free. Handles transport identity only (sender unaffected,
      receiver resolves the same root — stale-at-use if it died in
      flight). Embedded `&` fields reject `send-escape`; embedded
      handles travel; guards never cross (`guard-escape`). Endpoint ops
      record `Sync` holds on the endpoint — channel ops serialize,
      plain touches race. Queue/transit state merges at spawn-end (the
      channel world is shared), which makes sibling handoff work.
      Typed channels carry the packet shape: `channel &mut P` /
      `channel handle P` / `channel P`; unannotated `channel()` is
      opaque, send-only. Deliberate divergences from the lab: value
      sends MOVE (never copy — "transport, not duplicate"), recv'd
      `&mut` packets bind as ordinary `&mut` locals, packet payloads
      declare at the endpoint type. Known limit: callee `send` on a
      `write ch` param writes the shared cell at runtime but queue
      contents stay callee-local — `$arg`-rooted packets don't replay
      caller-side yet. (Codegen shipped the endpoint as a ptr cell —
      single-shot transport; N13 gave it a real synchronized queue.)
      New surface: `channel T`,
      `channel()`, `send`, `recv`, `close`. 196/196 native tests.
  ↓
DONE  N13 real parallel backend: `spawn` lowers to `pthread_create`
      through `mmlrt` (the tiny C runtime boundary — tasks/mutexes/
      channels), `join` to `pthread_join`. `Expr::Spawn` synthesizes
      `__task_N(ptr env)`: free-var analysis picks captures, the env is
      a caller-frame `[K+2 x ptr]` array — [0]=task*, [1]=result-cell
      ptr, [2+]=captures (storage by address, ptr-shaped values by
      content). Task tokens are env pointers; `join` waits on the OS
      thread then loads the result cell. `mutex T` becomes a runtime
      object (`pthread_mutex_t` + data ptr — `mutex h` wraps the
      referent, `mutex v` carries the payload inline); guards carry
      the object pointer and reach payload through `mml_mutex_data`;
      `unlock` AND scope-exit release both emit real unlocks (incl.
      task-fn terminal blocks — the deadlock the sequential backend
      masked). Channels become `mml_chan` (mutex + condvar + unbounded
      queue + closed flag): send copies the declared payload's bytes,
      recv blocks while empty-and-open, close broadcasts — so sibling
      handoff is a real cross-thread rendezvous. Every alloca hoists
      to the entry block (loop-invariant frame growth was a real
      stack-overflow bug at 50k iterations). 210/210 native tests now
      execute on real OS threads — incl. 100k×2 atomic increments,
      30k×2 mutex-locked increments, 200-message channel streams,
      50-round ping-pong, nested spawn, spawn/join loops, and a
      flag-rendezvous that proves simultaneity. ThreadSanitizer-clean
      on the authority-transport, overlap, and stream tests.
      Semantics unchanged — one checker-visible fix only: `send`
      materializes the packet cell at the channel's declared `T`.
  ↓
DONE  N14 C FFI: `extern fn name(params) -> ty;` — C-ABI declares
      against libc (or anything linked). `*T` / `*const T` raw pointer
      types carry the authority contract: `&mut x`/`&x`/`handle x`/`&mut`
      bindings coerce into them, and the referent takes a transient
      Write/Read claim for the call's duration — so `f(&mut x, &mut x)`
      self-conflicts, an FFI write inside `spawn` races the parent's
      reads, a shared `&x` can't fill a writable `*T`, and a string
      literal is always `*const`. `*void` is C's `void*`. `"lit"`
      literals become private null-terminated globals (`@.str.N`).
      Params/returns are scalars and pointers only — aggregates and
      language carriers reject at the declaration; raw pointers are
      opaque inside the language (no deref). The honest guarantee:
      the compiler enforces the declared FFI contract; the foreign
      implementation's correctness is outside our proof — a C callee
      can always violate its declared signature internally.
      226/226 native tests.
  ↓
DONE  N15A slices: `&a[lo..hi]` → `slice T` (Read span claim),
      `&mut a[lo..hi]` → `slice mut T` (Write claim), `&s[lo..hi]`
      re-slices. Repr is {ptr, len} — one LLVM aggregate for all
      element types and mutabilities; mutability is semantic, not
      physical. `s[i]` loads ptr+len and calls `mml_bounds_check`
      (trap on miss — closes the dynamic-index gap N2 carried);
      `&b[lo..hi]` calls `mml_range_check` for `0<=lo<=hi<=limit`,
      skipped when all-constant (the checker proved those). `.len`
      is u64 metadata — never a place; `.ptr` is the FFI extraction
      path (`*const T` on shared slices, `*T` on `slice mut`) and
      carries the whole span's claim into extern calls.
      `slice(p, n)` is the trusted foreign-storage assertion — the
      slice is checked, the foreign buffer's validity is not.
      Authority is the ordinary edge set (`s#e -> a#e`): read-only
      slice writes reject via the same derivation matrix as `*r`,
      same-root slices conservatively overlap (no range reasoning
      yet — sound first), `slice mut` is affine, and slices live in
      struct fields, params, returns, and task captures.
      250/250 native tests.
  ↓
DONE  N15B strings: `str` is an immutable byte-view — same
      {ptr, len} repr as slices, zero new memory machinery. Literals
      bind static read-only storage with `len` excluding the physical
      NUL (kept for C interop); `let s: str = "lit"` allocates
      nothing. `.len` is BYTE count — `"é".len == 2`; `.bytes` is a
      computed read-only `slice u8` (zero-cost reinterpret, never a
      place), so byte indexing is explicit `s.bytes[i]` and `s[i]`
      refuses to hide which indexing it means. `==`/`!=` compare
      contents via `mml_str_eq`, not addresses. `str_from_utf8(b)` is
      the only bytes→str path — runtime validation (trap on invalid)
      plus a Read claim that freezes the source span while the view
      lives; `cstr(s)` is explicit and allocation-free, restricted to
      literal-provenance views known NUL-terminated (arbitrary str
      rejects — no hidden alloc; `to_cstr(alloc, s)` waits for the
      allocator milestone). `str` composes through the same graph:
      params, returns, struct fields, task captures, channel
      transport. `&mut` through a read-only view now rejects at the
      type level — edgeless storage (literal strs, foreign slices)
      can't mint write authority.
      276/276 native tests.
  ↓
DONE  N16A explicit allocators: `system_allocator()` mints an
      `alloc` token (a shareable capability — the site states WHO
      allocates, NULL is the system heap); `alloc<T>(a)` returns a
      `box T` and `alloc_slice<T>(a, n)` a `box slice T` — owning
      values whose binding key IS the heap root. `free(x)` is not
      a new operation: it IS `consume_place`, so double free is
      double consume, use-after-free is use-after-consume, a live
      `&*p`/`&b[..]` edge blocks the Own claim, `handle` targets go
      stale, task holds pin the root until join, and `box`/`box
      slice` move through `own` params, `let` moves, channels, and
      task captures on the existing machinery alone. At FFI, `b.ptr`
      carries the span's Write claim and `&mut *p` the object's —
      raw `box` args reject at `*T` params (no unchecked pointer
      escapes), and a user-declared `free` shadows the builtin for
      malloc'd C memory. The graph probe settled cleanly: internal
      self-edges (`p.next = &*p`) die with the root at free; refs
      INTO shorter-lived foreign storage reject at assignment via
      the ordinary region-escape rule — no closure walk required.
      Runtime: `mml_alloc`/`mml_alloc_slice`/`mml_free` — thin
      malloc wrappers, NULL-return traps, visible in the LLVM.
      312/312 native tests.
  ↓
DONE  N16B library types in plain language — `stdlib/vec_i32.lang`,
      `stdlib/string.lang` compile with ZERO container magic. `Vec`
      is `struct { buf: box slice i32, len, cap, heap }` and its
      API is literally the authority split: `vec_push(own v)` may
      realloc (`memcpy`+`free`+rebind the root inside), `vec_pop`
      needs only `write`, `vec_as_slice`/`vec_as_mut` mint ordinary
      views, `vec_free(own v)` destroys. The payoff: `v = vec_push
      (v, x)` while `s = vec_as_slice(v)` is live rejects "cannot
      consume v: it is borrowed" — iterator invalidation is the
      borrow rule, not a container rule; `v.len` stays readable
      under the view (path-precise). `String` the same: `as_str`
      borrows the buffer so `push_str` while viewed rejects;
      `string_eq` composes read-params into content equality;
      `to_cstr` allocates visibly and `c.buf.ptr` crosses to C.
      One checker fix surfaced: the `.ptr` FFI claim now exempts
      `path_holders` — a cap-param's own derivation edge looked
      like a foreign borrow into its own subtree.
      322/322 native tests.
  ↓
DONE  N17 modules/imports — real multi-file programs. A file says
      `import lib.math17;` (resolves `lib/math17.lang` relative to
      the importer; `import std.vec_i32` roots at `stdlib/`), calls
      qualified names `math17.add(x)`, and only sees `pub` members.
      The mechanism is load-time qualification: the loader renames
      every module-local symbol to `m.f`/`m.T` before checking, so
      the checker, IR, and LLVM emit see one uniform qualified
      namespace — `@vec_i32.vec_push`, `%vec_i32.Vec` in the .ll.
      The semantic boundary is the point: each module is checked
      alone against its dependencies' `ModuleInterface` (exported
      sigs + struct layouts + FnSummaries + a private-name set for
      diagnostics), dependency-first on a DAG — cyclic imports,
      missing files, duplicate qualifiers, and `module x;`/filename
      mismatches are all diagnostics with file attribution. The
      proofs: `store.attach(h, x)` blocks `consume x` from the
      IMPORTED summary alone; `vec_push(v)` while a `vec_as_slice`
      view lives rejects across the boundary; `own`/`write`/`read`
      contracts, returned slices, stale handles, task holds, mutex
      domains, and channel packets all replay identically — the
      corpus confirms local and cross-module produce the same
      errors. Visibility is minimal: non-`pub` is module-private
      ("private" not "undefined" at call sites), pub sigs may not
      expose private struct types, imports don't re-export, and
      there's no init code, no wildcards, no package manager. The
      stdlib files became real modules: `import std.vec_i32` gives
      the whole Vec API including reallocating `own` push.
      344/344 native tests.
  ↓
DONE  N18 definite initialization — `let x: T` declares storage;
      the checker proves a value exists before any operation needs
      it. A third lattice rides beside `consumed` and the capability
      graph: absent → `Cert::Def` on assignment, joined over branch
      arms (Def+Def = Def, anything else = Maybe), and joined
      conservatively against the pre-loop state since bodies may
      run zero times. Keys are the same `place_id` paths the edge
      maps use, so precision is path-level: `p.a = 1` initializes
      `p#e.a` while `p.b` stays uninit, and a struct built
      field-by-field passes whole-value use once every leaf covers.
      Reads of uninit storage reject at every value-use site —
      expression reads, `consume`, `read`/`own`/channel/spawn args —
      while `&mut x` and `write` params stay legal because they can
      be HOW a caller's storage gets initialized: the callee's
      `$arg`-rooted stores export through `FnSummary.init_paths`
      and replay caller-side like alias edges. Control flow follows
      reachability: a diverged arm (`return`) contributes nothing
      to the join, so `if c { x = 1 } else { return 0 } return x`
      accepts. Init and affine state stay orthogonal — `alloc →
      free → alloc` moves Init+Live → Init+Consumed → Init+Live;
      the maps answer different questions and never merge.
      383/383 native tests.
  ↓
DONE  N19 monomorphized generics — `struct Vec<T>`/`fn f<T>`,
      explicit `f<i32>`/`Vec<i32>` instantiation, `sizeof<T>`. The
      pipeline is the same shape as N17: an AST pass between load and
      check clones each requested template with `T -> concrete`,
      names the clone canonically (`vec.Vec<i32>`), and lands it in
      the REQUESTER's program — "as if the requester wrote this code
      with these types" — so arg layouts resolve locally, requester
      types work as args, and clone bodies may only call what the
      requester can import. Fixpoint instantiation covers nested
      `Vec<Vec<i32>>` and generic-fn-calling-generic-fn; identical
      clones from two requesters dedupe at module merge. The
      falsifiable claim held: generics parameterize TYPES, never
      safety semantics — `Vec<i32>` view-invalidation, `Vec<box>`
      affine moves, `Vec<handle>` identity, `Vec<str>` views,
      `Vec<mutex>` storage rules, and `use-after-consume` through
      `id<box>` all behave exactly like their concrete counterparts.
      stdlib gained `std.vec` (`Vec<T>`, `vec_new/push/pop/
      as_slice/as_mut/free` — all generic). No traits, constraints,
      specialization, or inference — explicit instantiation only.
      409/409 native tests.
  ↓
DONE  N20 inference and methods — ergonomics without new semantics.
      Generic-argument inference fills `id(42)`/`push(v, x)` from
      concrete argument positions: a per-fn local-type map feeds a
      structural `T`-unifier over `Vec<T>`, nested `Vec<Vec<T>>`,
      and struct-literal args; multiple occurrences must agree
      (`pair(1, "x")` rejects "T inferred as both i32 and str"),
      unconstrained params reject (`cannot infer type argument T`),
      and explicit `<T>` stays available. No global constraint
      solver, no return-context inference — evidence comes from
      arguments alone. Method syntax desugars before checking:
      `v.push(x)` resolves `v`'s type, rewrites to the module
      function `vec.push(v, x)`, and `Vec<i32>::new(heap)` rewrites
      to `vec.new<i32>(heap)` — the receiver slot is an ordinary
      first param whose `read`/`write`/`own` annotation IS the
      method's authority signature. The falsifiable checks all
      hold: `v = v.push(x)` still requires explicit reassignment
      (own cost visible), `s = v.as_slice(); v = v.push(1)` still
      rejects "cannot consume `v`: it is borrowed", `Vec<box>`
      affine and `Vec<handle>` identity semantics survive the
      sugar, and `a.b(...)` disambiguates variable-receiver from
      module-path by scope lookup. One real checker bug surfaced:
      StructLit field exprs were evaluated twice (once for typing,
      once for provs) — harmless until an `own`-arg call sat in a
      field slot, where it double-consumed. No vtables, no traits,
      no dispatch machinery — methods are spelled-out calls.
      424/424 native tests.
  ↓
DONE  N21A project tooling — `our` is the developer-facing driver
      (`ourc` stays the compiler): `our new hello` scaffolds
      `project.toml` + `src/main.lang` + `tests/`, `our check` runs
      parse → load → monomorphize → check with NO codegen (the fast
      feedback command — the front half is now a shared
      `frontend_path` so check and build can never drift), `our
      build` writes `build/<name>` with the `.ll`/`.o` artifacts
      beside it, `our run` builds and execs (`--` forwards args),
      `our test` runs the `tests/` corpus with the same
      `@exit`/`@compile-error` directives as the compiler suite.
      The manifest is a deliberately minimal TOML subset
      (`[project] name/version`, `[build] entry`) parsed by a
      zero-dependency reader — every command resolves the project
      by walking up for `project.toml`, so they work from any
      subdirectory. Dependency machinery landed in N38.
      424/424 native tests.
DONE  N21B thin stdlib — `std.io` (POSIX write + in-language digit
      formatting), `std.mem` (libc mem* over `*void`), `std.math`
      (i32 helpers — the language's default int), `std.sync`
      (generic atomic ops over the builtins; `lock`/`unlock` stay
      syntax — a guard can't cross a call boundary and that's the
      model working), `std.chan` (open/put/take/shut over
      send/recv/close), `std.thread` (sched wrappers — task tokens
      are region-bound so `spawn`/`join` stay syntax), `std.fs`
      (fd_open/read/write/close + read_all/slurp over
      `read(fd,&mut buf,n)` claims). The milestone surfaced four
      real compiler bugs, fixed at the root:
        · extern call returns were welding the argument's claim
          onto the result binding (`let n = read(fd,&mut b,_))`
          installed `n -> b : Write`) — `caps_of`/`refs_of` now
          return nothing for externs; the C return is opaque.
        · builtin call names outranked module declarations in both
          the checker AND ir/gen — `extern fn close` lost to the
          channel builtin and codegen emitted `mml_chan_close` on
          an fd (segfault). `local_fns` now scopes shadowing: a
          module's own decls beat builtins, imports never do.
        · `*void`/`*const void` only accepted `*void` — `*T`/`*const T`
          now weaken through `coerce` (C's rule: adding const is
          free, dropping it never happens).
        · an inferred `let` binding never registered generic
          instantiations — `let v = fs.slurp(..)` left `vec.Vec<u8>`
          un-instantiated and `v.len` void. `wants_ty` now scans
          inferred binding types.
      Also learned: atomic params must be `read` — `atomic T` is a
      storage class; by value it copies the cell and the RMW lands
      on the copy. And `our`'s sibling rule stands: touching a
      returned generic's fields means importing its module —
      interfaces expose only direct imports' layouts.
      432/432 native tests.
DONE  N22 tagged unions — `enum Maybe<T> { None, Some(T) }` decls,
      `Maybe<i32>::Some(42)` / `Maybe<i32>::None` construction
      (parens optional on nullary variants), and
      `match x { Some(v) => .., None => .. }` with by-value
      extraction only. The payload is an ordinary place subtree:
      `m.Some` rides the existing init/affine/edge/cap machinery —
      no enum-specific ownership rules anywhere. The only new state
      is one map: `tags` records the active variant per root
      (Cert = one known variant, Maybe = merge of differing
      branches, absent = uninitialized) — `if{Some}else{None}`
      joins to init=Def + tag=Maybe, distinct from uninitialized.
      Layout is explicit `{i64 tag, [N x i8] payload}`, registered
      as a synthetic struct so IR/LLVM need no new machinery; match
      lowers to a tag compare + arm blocks + a join block.
      The falsifiable results all held:
        · reading `m.Some` while the tag is unknown or known-other
          rejects ("which variant is active?")
        · `match` requires a Cert-init scrutinee and exhaustiveness
          over every variant; unknown variants reject
        · `Some(v)` moves the payload out of its path — reuse or a
          second extraction is ordinary use-after-move
        · `match m` on a `read` param may inspect the tag, but
          `Some(v)` can't move the payload through a borrow;
          `own m` extracts cleanly — the capability lattice is the
          whole story
        · `Maybe<i32>`/`Maybe<box>`/`Maybe<handle>`/`Maybe<str>`/
          `Maybe<Vec>`/`Maybe<mutex>`/`channel Maybe<box>`/
          `Maybe<Maybe<i32>>` all instantiate and check under
          requester visibility — generic enums ride `Gen::E`
          alongside `Gen::S`
        · private enums reject as "private to its module";
          associated constructors follow struct visibility
      451/451 native tests.
DONE  N23 error propagation — `std.result.Result<T,E>` is an
      ordinary `pub enum Result<T,E> { Ok(T), Err(E) }`; the ONLY
      compiler feature is `expr?`, which desugars semantically to
      `match expr { Ok(v) => v, Err(e) => return Err(e) }`. The
      checker forces the scrutinee's tag to `Err`, CHECKS the Err
      arm's payload move (a `read` borrow can't move a box out on
      either path), discards that arm's state, then forces `Ok` and
      performs the real payload move — the continuation is just the
      Ok arm of a match. `Result` is structural, not blessed: any
      enum with exactly `Ok`/`Err` payloads qualifies. The rules:
        · `expr?` requires an `Ok`/`Err` enum scrutinee
        · the fn must return `Result<_, E>` with the SAME E —
          no conversion
        · `?` inside `spawn` rejects (a task's result is not a
          propagation channel)
        · Err payloads are affine — `Result<T, box E>` moves the
          box into the early return, no leak
        · `let x: T; if c { x = get()?; }` still joins
          Maybe-uninitialized; `if c {} else {}` joins Def
        · codegen is a mid-expression tag compare: Err block
          rebuilds the fn's Result + `ret`, Ok block continues
          yielding the payload place
      470/470 native tests.
  ↓
DONE  N24A rvalue-place lowering — `frontend/lower.rs` runs after
      mono and before check, rewriting every non-place base or arg
      into a `let $rvN = ..` temp so the checker AND codegen see only
      named-root places. `f().x`, `v.as_slice()[0]`, `recv(c).v`,
      `(*mk_box()).a`, `get()?.v` all work; the eval-exactly-once
      invariant holds by construction (one let-init = one eval in
      check, IR, and machine code). Statement lifetime: `$rv` temps
      die at the end of their consuming statement (C++ temporary
      semantics), so `let s = mkv().as_slice()` correctly rejects —
      the borrow outlives its temp. Rules:
        · `write`-param args are NOT hoisted — a write into a dying
          temp can never be observed, so `f(write x+1)` and
          `mkv().pop()` still reject "takes a place" (`read`/`own`
          borrow or move the temp — C++'s const-ref/rvalue-ref line)
        · bare assign/consume/promote targets never materialize —
          `f() = 5` stays "assignment target is not a place"
        · `&`/`handle`/`mutex`/`spawn` args keep dedicated semantics
        · `while` conds re-evaluate: temps hoist before the loop AND
          re-assign at each body end (`while tick(c).a < 3` runs
          tick once per check)
      483/483 native tests.
DONE  N38 project/package experience — `[dependencies] lib = "../lib"`
      declares a local path package; the dep name becomes a module
      root like `std`, so `import lib.util` resolves
      `<dep>/src/util.lang` (module qualifiers stay file stems —
      `util.f`, same law as `std.map` → `map.`). Resolution is
      transitive (a dep's own `[dependencies]` resolve from its own
      root; a dep can even `import lib.util` by its own package
      name), name-sorted, and strict: missing dir/manifest/src,
      name mismatch, two roots claiming one name, and cycles are
      each named errors. `project.lock` records the resolved set
      (path + FNV-1a content hash per dep). `build/.fingerprint`
      hashes tool version + profile + manifest + every `.lang`
      under root (minus `build/`,`tests/`) and dep roots — a
      second `our build` prints `fresh`; `build/build.info`
      records tool/triple/profile. `our build --release` (or
      `[build] profile = "release"`) compiles `.ll`→`.o` and links
      with `-O2`; `our clean`/`our rebuild`/`our deps` round out
      the commands; `our test` builds the tests/ corpus with dep
      roots so test files can `import` packages. Unresolvable
      `pkg.` imports name what's missing ("`zed` is not a
      declared package"); the LSP resolves the enclosing
      project's deps per document so hover/definition/completion
      jump into package sources. `tests/project.rs` drives the
      real binary through a two-package workspace.
      604/604 native tests; 11 unit + 4 integration.
DONE  N39 trivia-preserving lexer + `our fmt` — the lexer now
      carries a lossless stream: every `Token` has `text` (exact
      source spelling — `0xFF`, `"a\x41"`, `1_000` never respelled)
      and `leading` trivia (`//` comments with a trailing/standalone
      flag, blank-line counts). The parser ignores both — zero
      semantic churn (N39A/B). `driver/fmt.rs` (N39C) is a
      token-stream formatter, NOT an AST printer: 4-space indent,
      one statement per line, spaces around binary ops, no space
      before `(`/index `[`, `{`-vs-literal-brace by the parser's own
      lookahead (`{` + `Ident :` = literal, else block; `struct`/
      `enum` decls exempt), `<...>` vs `<` comparison by a balanced
      type-token scan, `<<`/`>>` reglued when source-adjacent,
      `{ }`→`{}` for empty blocks, comments preserved exactly once
      (trailing comments keep their line), blank runs capped at 1,
      `} else {` stays together. `ourc fmt <file>` prints;
      `our fmt [--check]` rewrites `src/`+`tests/` in place (CI
      mode exits 1 on drift). Pins: 8 unit tests + `tests/fmt.rs`
      over every repo `.lang` — formatted output produces the
      IDENTICAL token stream, is idempotent, and keeps every
      comment's exact text once; unlexable files are refused.
      604/604 native tests; 19 unit + 4 integration + fmt corpus.
DONE  N40 resource ergonomics: aggregate moves + `replace`/`swap` —
      the milestone started as API polish and surfaced a real
      soundness hole: `let old = w` on an aggregate SHALLOW-COPIED
      interior authority, so `w.n`/`old.n` owned the same box
      (double-free) and `h.r`/`old.r` held two `&mut`s to one target.
      `consume_owned_src` now moves any place whose subtree carries
      live own-authority or edges/caps (let/assign/arg/field/payload/
      yield/return — every implicit copy site), while a rel-keyed
      `LitPkt` (`collect_lit`/`push_lit`) re-roots the moved value's
      edges, caps, provs, dead-marks, lens, tags, and NUL-safety
      under the destination via `record_lit_fields`. Pure-data
      aggregates still copy freely.
      `replace(place, v)` is the atomic exchange: the old value moves
      OUT to the result while `v` moves IN — legal through a `write`
      borrow where extraction would leave a hole, and no overwrite-
      law violation since nothing is lost. Its root refs/caps ride a
      span-keyed `replace_outs` channel so `refs_of`/`caps_of` answer
      for the OLD value, not arg1's fallback union. `swap(a, b)`
      exchanges two places' whole packets — edges, caps, dead marks,
      tags, cstr-safety — at matched types, rejecting overlap; a
      `[]`-collapsed self-pair (`swap(v[i], v[j])`) is the index-
      family swap, fact-invariant by construction. Statement-position
      drains keep unclaimed packets off the next `let` (phantom
      authority is the unsound direction). Codegen is two loads +
      two stores — registers, no hidden allocation.
      Stdlib: `Vec::swap_remove` (no memmove, works for `box`), and
      `StrMap::set` — the only map write needing `write` not `own`,
      since the atomic exchange never leaves a slot hole.
      623/623 native tests incl. pins 605–623.
DONE  N41 real-program hardening II — a hostile composition pass,
      not new features: can ~2400 lines of resource-heavy systems
      code break the ownership layer? Answer: two compiler fixes,
      zero semantic holes. `stdlib/omap.lang` adds `OMap<V>` — the
      `HashMap<String,V>` to `StrMap`'s `HashMap<&str,V>` — because
      runtime-built keys have no stable home for a `str` view: each
      entry owns its key bytes (`box slice u8`), `str` args stay
      transient lookup views, rehash migrates `OEntry{kbuf,hash,v}`
      whole, drain is explicit `pop_entry` + `free`. Six programs:
      `swapstress` (300 random swap/replace ops on affine values,
      invariants checked after every op), `graph` (`Vec<Node>` with
      nested `Vec` adjacency, `swap_remove` + index fixups + BFS),
      `workq` (`channel<box Job>` fan-out, worker tasks, results
      merged into an `OMap` under runtime keys — collatz verified),
      `indexer` (multi-file word index, `OMap<Vec<u64>>` extract-
      restore mutation; also runs on real argv: 940 words across
      the repo), `lru` (`OMap` + intrusive list + eviction, owned
      keys freed one-for-one), `kvserver` (TCP KV store, `mutex
      Store{OMap<box slice u8>}` shared by two accept-workers,
      self-test clients on loopback). The laws the pass pinned:
      views never carry own-authority — nested moves go through
      `write`-param helpers (container + index, interior path
      formed inside); guards give field-level access, so guarded
      maps extract to a local via `replace(g.m, filler)`, run
      own-level calls, swap back, unlock — the slot is never
      observed empty. Two real compiler fixes composition forced:
      mono `ty_of` types `replace` as the place's type (a method
      call on its result resolves) and `lock` as `Guard(inner)`
      with `field_ty` unwrapping guard fields — both pinned in 626.
      Match-binder views stay deferred: `self.slots[i].Full.v` is
      ugly but correct, and that beats changing match preemptively.
      626/626 native tests incl. pins 624–626.
DONE  N42 borrowed match binders + comparative benchmark suite —
      the binder fix first: N40's whole-subtree move had made
      `Full(e)` under a `read` scrutinee unwritable (the synthesized
      `let e = scrut.V` can't own through a borrow), and minijson
      regressed silently because programs/ aren't pins. Now the bind
      is authority-shaped: POD copies, affine under own/write moves,
      affine under read arrives as `&T` — a view of the live payload,
      consume through it still rejects E1001. Mode decided by
      `place_auth`, recorded via a span-keyed ARM_BINDS side channel,
      folded per-module into gen. Pin 627; corpus 627/627.
      Then `bench/`: 8 workloads x {ours, C, Rust} x {debug, release}
      with matched algorithms (same FNV-1a open-addressing table in
      all three for wordfreq/mapbench/kvsrv) and shared C drivers for
      the servers. Headline: release ours is ~1.3-3x C on CPU-bound
      cells (the bounds-check + conservative-codegen tax, stated
      plainly), dead even on syscall-bound cells, ~10x smaller
      binaries than Rust, and debug-mode ours beats rustc -O0 on
      four of eight. Ceremony counts (`bench/ceremony.py`): ours is
      under Rust on the resource-light cells and above C/Rust only
      where C's contract is "just don't get it wrong" — the explicit
      free/alloc discipline is the visible cost. Full readout and
      methodology in BENCHMARKS.md. Zig column pending (no toolchain
      on this machine).
  ↓
THEN  N24B real-program hardening — 5–10 medium programs (grep,
      wordcount, file_copy, parallel_sum, producer_consumer, JSON
      parser, small HTTP server) to expose borrow-lifetime
      annoyances, stdlib holes, and error-message quality before a
      v0.1 preview.
  ↓
STEP  Per-model annotation-burden study using model-specific rewrites
  ↓
STEP  Invent from surviving failures — whatever F can't express or rejects
      wrongly is where the language's novelty actually lives
  ↓
THEN  select a candidate model — and only then compare generated code
      against C/Rust/C++ on tiny kernels
```

Deliberately **not** done yet, and deliberately cheap to keep changing:
no LLVM, no final syntax, no real compiler, no generics. The hybrids
are **not** our language — they're experiments designed to expose what our
language must invent. F is the first candidate that looks like it might
*become* a memory model: it closed the temporal gap without ownership's
machinery and exposed the memory/resource distinction as a first-class idea.
But the corpus is intra-function only — the interprocedural and concurrent
cases are where a real design would still be tested hardest.
