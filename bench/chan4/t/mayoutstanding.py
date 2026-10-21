#!/usr/bin/env python3
"""CHAN-004 — executable model of the A.3.3/A.3.5 identity rules.

Dual simulation. A PHYSICAL world tracks ground truth: where each
resource's exclusive capability actually resides (binding, channel
queue, pending Result, dead). An ABSTRACT checker state tracks only
what the spec's model can know: may-bags per endpoint, ⊥ claims
with candidate sets, pending packets, transit marks.

INDEPENDENCE RULE: abstract operations never read physical state.
The only places physical truth touches the abstract are (a) arm
feasibility — whether recv's Some arm or the Result's Ok/Err arm
executes at all is a runtime fact — and (b) the oracle assertion
comparing the two worlds. Claimed identities come from abstract
packets; on ordered queues the model additionally ASSERTS the
abstract front agrees with the physical pop — verification, not
a source of truth.

Safety invariant, checked at every reachable state:

    abs.borrowable(t)  ⟹  no live physical capability on t

Scenarios:
  S1  two spawned producers; recv/drop in both orders (the QA-007
      regression shape: unknown &mut claim, then &mut a candidate)
  S2  branch-join ghost packet — the pushed packet may not exist
      physically; plus the |C|=1 singleton-promotion control
  S3  pending bounded send — the SENT/CLOSED fork at send, the
      Ok/Err resolution FORK on separate branches, recv attempted
      both before and after resolution
  S4  forwarded ⊥ — a claimed unknown cap re-sent on a second
      channel keeps its candidates outstanding through the move
  S5  branch-joined disjoint producers — per-path packets merge
      into one may-bag; document the resulting permanent
      restriction (the honest coarse cost)
  S6  candidate completeness through a binding move — ⊥ edge
      re-keys with C preserved

Runs 4 passes: clean + U/L/P mutants. The clean pass must produce
zero violations; every mutant must produce at least one — a
mutant passing silently means the harness has a coverage hole.
Required-path table: any required scenario/outcome completing
zero paths fails the run.
"""
import sys
from dataclasses import dataclass, field

ALL_RES = ('x', 'y')
MUTANT = None          # 'u' | 'l' | 'p' — drop that component

CHECKS = [0]
FAIL = []


def check(cond, what, ctx):
    CHECKS[0] += 1
    if not cond:
        FAIL.append(f"VIOLATION: {what}  [{ctx}]")


# ---------------- physical world (ground truth) ----------------

@dataclass
class Phys:
    caps: dict = field(default_factory=dict)     # res -> 'dead' | ('bind',v) | ('chan',ep) | ('pend',k)
    queues: dict = field(default_factory=dict)   # ep -> [res,...]
    outcomes: dict = field(default_factory=dict) # pending id -> 'SENT'|'CLOSED'

    def alive(self, t):
        return t in self.caps and self.caps[t] != 'dead'

    def clone(self):
        return Phys(dict(self.caps),
                    {e: list(q) for e, q in self.queues.items()},
                    dict(self.outcomes))


# ---------------- abstract checker state ----------------

@dataclass
class Abs:
    pushes: dict = field(default_factory=dict)      # ep -> may-push count
    pops: dict = field(default_factory=dict)        # ep -> Some-arm count
    bags: dict = field(default_factory=dict)        # ep -> [frozenset sent-names]
    unordered: set = field(default_factory=set)     # endpoints with may-bags
    bots: dict = field(default_factory=dict)        # binding -> frozenset C
    named: dict = field(default_factory=dict)       # res -> binding
    pending: list = field(default_factory=list)     # [(pid, ep, frozenset)]

    def drained(self, ep):
        # pops ≥ may-pushes proves drain BECAUSE the bag
        # over-approximates: physical enqueues ≤ may-pushes and
        # pops ≤ physical enqueues ⇒ all real messages departed.
        return self.pops.get(ep, 0) >= self.pushes.get(ep, 0)

    def candidates(self, ep):
        # §A.3.3: union over every may-packet on ep PLUS every
        # unresolved pending packet on ep (a pending SENT message
        # is physically deliverable). Never shrinks per-pop.
        names = set()
        for p in self.bags.get(ep, []):
            names |= set(p)
        for _, e, p in self.pending:
            if e == ep:
                names |= set(p)
        return names

    def may_outstanding(self, t):
        """MayOutstanding(t) = U(t) ∪ L(t) ∪ P(t) — §A.3.5."""
        u = MUTANT != 'u' and any(
            t in p for ep, bag in self.bags.items()
            for p in bag if not self.drained(ep))
        l = MUTANT != 'l' and any(
            t in C for C in self.bots.values())
        p = MUTANT != 'p' and any(
            t in pk for _, _, pk in self.pending)
        return bool(u or l or p)

    def borrowable(self, t):
        return t not in self.named and not self.may_outstanding(t)

    def clone(self):
        return Abs(dict(self.pushes), dict(self.pops),
                   {e: list(b) for e, b in self.bags.items()},
                   set(self.unordered),
                   {b: set(c) for b, c in self.bots.items()},
                   dict(self.named), list(self.pending))


def invariant(phys, abs_, ctx):
    for t in ALL_RES:
        check(not (abs_.borrowable(t) and phys.alive(t)),
              f"borrowable({t}) while physical cap alive at "
              f"{phys.caps.get(t)}", ctx)


# ---------------- operations ----------------
# Return False when infeasible on this physical world (a dead
# arm), pruning the subtree. Abstract state is updated ONLY from
# abstract facts — never from physical contents.

def op_send(phys, abs_, ep, res, ctx):
    """send(ep, &mut res): mint cap, physical enqueue + may-push."""
    phys.caps[res] = ('chan', ep)
    phys.queues.setdefault(ep, []).append(res)
    abs_.bags.setdefault(ep, []).append(frozenset({res}))
    abs_.pushes[ep] = abs_.pushes.get(ep, 0) + 1
    invariant(phys, abs_, ctx)
    return True


def _do_claim(phys, abs_, ep, vname, ctx):
    """Shared Some-arm effects: physical pop + abstract claim.
    The claimed IDENTITY comes from the abstract packet, never
    the physical pop."""
    res = phys.queues[ep].pop(0)            # physical bookkeeping
    phys.caps[res] = ('bind', vname)
    abs_.pops[ep] = abs_.pops.get(ep, 0) + 1
    if ep in abs_.unordered:
        C = abs_.candidates(ep)
        # The may-bag is NOT popped — a departing physical message
        # can't be identified, so every may-packet's membership
        # survives until the endpoint provably drains (pops ≥
        # may-pushes). Only the pops counter advances.
        if len(C) == 1:
            abs_.named[next(iter(C))] = vname     # singleton promotion
        else:
            abs_.bots[vname] = C
    else:
        front = abs_.bags[ep].pop(0)        # abstract front packet
        check(res in front,
              f"ordered claim: abstract front {set(front)} disagrees "
              f"with physical delivery {res}", ctx)
        for t in front:                     # identity from the packet
            abs_.named[t] = vname


def op_claim(phys, abs_, ep, vname, ctx):
    """recv(ep) asserting the Some arm — prunes if the physical
    queue is empty (coverage table catches unexpected prunes)."""
    if not phys.queues.get(ep):
        return False
    _do_claim(phys, abs_, ep, vname, ctx)
    invariant(phys, abs_, ctx)
    return True


def op_recv(phys, abs_, ep, vname, ctx):
    """recv(ep) honest Some-or-None: claims iff a message exists,
    continues either way (the None arm is a live path too)."""
    if phys.queues.get(ep):
        _do_claim(phys, abs_, ep, vname, ctx)
    invariant(phys, abs_, ctx)
    return True


def op_drop(phys, abs_, vname, ctx):
    """Binding dies: the cap it physically held dies; its abstract
    edge (⊥ or named) leaves the state."""
    for t, c in phys.caps.items():
        if c == ('bind', vname):
            phys.caps[t] = 'dead'
    abs_.bots.pop(vname, None)
    for t, b in list(abs_.named.items()):
        if b == vname:
            del abs_.named[t]
    invariant(phys, abs_, ctx)
    return True


def op_move(phys, abs_, v1, v2, ctx):
    """v2 = v1 — move: phys binding re-keys; ⊥ edge follows with
    C intact (candidates must survive moves — §A.3.5 L)."""
    for t, c in phys.caps.items():
        if c == ('bind', v1):
            phys.caps[t] = ('bind', v2)
    if v1 in abs_.bots:
        abs_.bots[v2] = abs_.bots.pop(v1)
    for t, b in abs_.named.items():
        if b == v1:
            abs_.named[t] = v2
    invariant(phys, abs_, ctx)
    return True


def op_borrow_attempt(phys, abs_, t, ctx):
    """&mut t attempt. borrowable(t) ⟹ abstract ALLOWS — soundness
    requires no live physical cap on t."""
    allowed = abs_.borrowable(t)
    check(not (allowed and phys.alive(t)),
          f"&mut {t} ALLOWED while physical cap alive at "
          f"{phys.caps.get(t)}", ctx)
    return True


def op_expect_blocked(phys, abs_, t, ctx):
    """Coverage assertion: this point MUST find t restricted."""
    check(not abs_.borrowable(t),
          f"expected {t} restricted but it was RELEASED", ctx)
    return True


def fork_resolve(ep, res, pid, vname):
    """Result resolution: Ok and Err are mutually exclusive — each
    is a separate branch, feasible only on its own physical world."""
    pkt = frozenset({res})

    def ok_phys(ph): pass
    def ok_abs(ab):
        ab.pending.remove((pid, ep, pkt))
        ab.bags.setdefault(ep, []).append(pkt)
        ab.pushes[ep] = ab.pushes.get(ep, 0) + 1

    def err_phys(ph):
        ph.caps[res] = ('bind', vname)
    def err_abs(ab):
        ab.pending.remove((pid, ep, pkt))
        ab.named[res] = vname           # verbatim reclaim (A.3.3)

    def feasible(ph, want):
        return ph.outcomes.get(pid) == want
    return ('FORK2', [
        (ok_phys, ok_abs, 'Ok', lambda ph: feasible(ph, 'SENT')),
        (err_phys, err_abs, f'Err({vname})',
         lambda ph: feasible(ph, 'CLOSED')),
    ])


def fork_pending_send(ep, res, pid):
    """The bounded send's physical outcome — chosen synchronously
    by the runtime, invisible to the checker — forks two worlds."""
    def sent(ph):
        ph.caps[res] = ('chan', ep)
        ph.queues.setdefault(ep, []).append(res)
        ph.outcomes[pid] = 'SENT'
    def closed(ph):
        ph.caps[res] = ('pend', pid)
        ph.outcomes[pid] = 'CLOSED'
    def mk_abs(ab):
        ab.pending.append((pid, ep, frozenset({res})))
        ab.unordered.add(ep)            # live pending → unordered (A.3.6)
    return ('FORK2', [
        (sent, mk_abs, 'SENT', lambda ph: True),
        (closed, mk_abs, 'CLOSED', lambda ph: True),
    ])


def op_forward(phys, abs_, ep2, vname, ctx):
    """Re-send a ⊥-carrying binding on ep2: cap moves channels;
    the carried packet keeps ALL candidates outstanding (U)."""
    for t, c in phys.caps.items():
        if c == ('bind', vname):
            phys.caps[t] = ('chan', ep2)
            phys.queues.setdefault(ep2, []).append(t)
    C = abs_.bots.pop(vname, frozenset())
    abs_.bags.setdefault(ep2, []).append(frozenset(C))
    abs_.pushes[ep2] = abs_.pushes.get(ep2, 0) + 1
    abs_.unordered.add(ep2)
    check(C == abs_.candidates(ep2),
          f"forwarded packet lost candidates: {C} vs "
          f"{abs_.candidates(ep2)}", ctx)
    invariant(phys, abs_, ctx)
    return True


def op_joinmerge(phys, abs_, ep, extra_pkts, extra_pushes,
                 extra_pops, ctx):
    """Branch join: union the other path's may-pushes into ep's
    bag (longer-queue union), pops join by min — marks unordered."""
    abs_.bags.setdefault(ep, []).extend(extra_pkts)
    abs_.pushes[ep] = abs_.pushes.get(ep, 0) + extra_pushes
    abs_.pops[ep] = min(abs_.pops.get(ep, 0), extra_pops)
    abs_.unordered.add(ep)
    invariant(phys, abs_, ctx)
    return True


OPS = {
    'send': op_send, 'claim': op_claim, 'recv': op_recv,
    'drop': op_drop, 'move': op_move, 'try': op_borrow_attempt,
    'blocked': op_expect_blocked, 'fwd': op_forward,
    'joinmerge': op_joinmerge,
}


# ---------------- scenario driver ----------------

LEAVES = {}


def run(label, phys, abs_, script):
    """DFS. FORK2 items expand (phys_fn, abs_fn, tag, feasible)
    alternatives — infeasible alternatives prune. Completed paths
    are counted per scenario label AND per outcome tag."""
    def rec(i, ph, ab, trace):
        if i == len(script):
            LEAVES.setdefault(label, []).append(trace)
            return
        op = script[i]
        if op[0] == 'FORK2':
            for pfn, afn, tag, feas in op[1]:
                ph2, ab2 = ph.clone(), ab.clone()
                if not feas(ph2):
                    continue
                pfn(ph2); afn(ab2)
                rec(i + 1, ph2, ab2, f"{trace}/{tag}")
            return
        if OPS[op[0]](ph, ab, *op[1:], trace):
            rec(i + 1, ph, ab, trace)
    rec(0, phys, abs_, label)


def make_phys(order):
    p = Phys()
    for r in order:
        p.caps[r] = ('chan', 'ch')
    p.queues['ch'] = list(order)
    return p


# ---------------- suite ----------------

def suite():
    # --- S1: two spawned producers — the regression shape --------
    for order in [('x', 'y'), ('y', 'x')]:
        bag = {'ch': [frozenset('x'), frozenset('y')]}
        run(f"S1-order{order}", make_phys(order),
            Abs(bags=bag, pushes={'ch': 2}, unordered={'ch'}), [
                ('claim', 'ch', 'v1'),
                ('blocked', 'x'), ('blocked', 'y'),
                ('claim', 'ch', 'v2'),
                ('blocked', 'x'), ('blocked', 'y'),
                ('drop', 'v1'),
                ('blocked', 'x'),            # ⊥' still names x
                ('drop', 'v2'),
                ('try', 'x'), ('try', 'y'),  # drained + dead → free
            ])
        run(f"S1b-order{order}", make_phys(order),
            Abs(bags={'ch': [frozenset('x'), frozenset('y')]},
                pushes={'ch': 2}, unordered={'ch'}), [
                ('claim', 'ch', 'v1'),
                ('drop', 'v1'),
                ('blocked', 'x'), ('blocked', 'y'),  # bag not drained
                ('claim', 'ch', 'v2'),
                ('drop', 'v2'),
                ('try', 'x'), ('try', 'y'),
            ])

    # --- S2: branch-join ghost + singleton promotion -------------
    # The pushed packet may not physically exist: recv can only
    # take the None arm (claim prunes). x stays restricted — the
    # ghost can never be proven absent. REQUIRED coverage: the
    # path completes AND the restriction is still held at the end.
    run("S2-ghost", Phys(queues={'ch': []}),
        Abs(bags={'ch': [frozenset('x')]}, pushes={'ch': 1},
            unordered={'ch'}), [
            ('recv', 'ch', 'v'),           # None arm — no message
            ('blocked', 'x'),              # ghost can never be
        ])                                 # proven absent
    run("S2-singleton", make_phys(('x',)),
        Abs(bags={'ch': [frozenset('x')]}, pushes={'ch': 1},
            unordered={'ch'}), [
            ('claim', 'ch', 'v'),          # |C|=1 → named v→x
            ('blocked', 'x'),
            ('drop', 'v'),
            ('try', 'x'),
        ])

    # --- S3: pending send — both outcomes × recv before/after ----
    # recv BEFORE resolution (SENT world can deliver the pending
    # packet — its names MUST be in the candidate union);
    # recv AFTER resolution on the SENT world claims the committed
    # packet; the CLOSED world's Err arm reclaims verbatim.
    for pre in (True, False):
        script = [
            fork_pending_send('ch', 'x', pid=0),
            ('blocked', 'x'),
        ]
        if pre:
            script += [('recv', 'ch', 'v'), ('blocked', 'x')]
        script += [
            fork_resolve('ch', 'x', pid=0, vname='e'),
            ('blocked', 'x'),
            ('recv', 'ch', 'w'),           # recv AFTER resolution
            ('blocked', 'x'),
            ('drop', 'v'), ('drop', 'e'), ('drop', 'w'),
            ('try', 'x'),
        ]
        run(f"S3-pre={pre}", Phys(), Abs(), script)

    # --- S4: forwarded ⊥ -----------------------------------------
    for order in [('x', 'y'), ('y', 'x')]:
        run(f"S4-order{order}", make_phys(order),
            Abs(bags={'ch': [frozenset('x'), frozenset('y')]},
                pushes={'ch': 2}, unordered={'ch'}), [
                ('claim', 'ch', 'v1'),
                ('fwd', 'ch2', 'v1'),
                ('blocked', 'x'), ('blocked', 'y'),
                ('claim', 'ch', 'v2'),
                ('claim', 'ch2', 'w'),
                ('drop', 'v2'), ('drop', 'w'),
                ('try', 'x'), ('try', 'y'),
            ])

    # --- S5: branch-joined disjoint producers --------------------
    # Path A's ordered queue holds {x}; path B's holds {y}. The
    # join MERGES them (op under test): union may-pushes, pops by
    # min, endpoint goes unordered. Physically one path ran — the
    # other packet is a ghost that can never drain (phys has one
    # message, may-pushes=2) → permanent restriction. Documents
    # the honest coarse cost of the join approximation.
    for ran, ghost in (('x', 'y'), ('y', 'x')):
        run(f"S5-ran-{ran}", make_phys((ran,)),
            Abs(bags={'ch': [frozenset(ran)]}, pushes={'ch': 1}), [
                ('joinmerge', 'ch', [frozenset(ghost)], 1, 0),
                ('claim', 'ch', 'v'),      # C = {x,y} — full union
                ('blocked', 'x'), ('blocked', 'y'),
                ('recv', 'ch', 'w'),       # None arm — queue empty
                ('drop', 'v'),
                ('blocked', 'x'), ('blocked', 'y'),  # never releases
            ])

    # --- S6: move preserves candidates ----------------------------
    run("S6-move", make_phys(('x', 'y')),
        Abs(bags={'ch': [frozenset('x'), frozenset('y')]},
            pushes={'ch': 2}, unordered={'ch'}), [
            ('claim', 'ch', 'v'),
            ('move', 'v', 'v2'),           # ⊥ re-keys, C intact
            ('blocked', 'x'), ('blocked', 'y'),
            ('claim', 'ch', 'w'),
            ('drop', 'v2'), ('drop', 'w'),
            ('try', 'x'), ('try', 'y'),
        ])


# ---------------- required-path coverage table ----------------

REQUIRED = {
    'S1-order': 2, 'S1b-order': 2,
    'S2-ghost': 1, 'S2-singleton': 1,
    'S3-pre=True/SENT': 1, 'S3-pre=True/CLOSED': 1,
    'S3-pre=False/SENT': 1, 'S3-pre=False/CLOSED': 1,
    'S4-order': 2, 'S5-ran-': 2, 'S6-move': 1,
}


def report(tag):
    missing = []
    counts = {}
    for label, traces in LEAVES.items():
        counts[label] = len(traces)
        for tr in traces:
            for outcome in ('SENT', 'CLOSED'):
                if f'/{outcome}' in tr:
                    counts[f'{label}/{outcome}'] = \
                        counts.get(f'{label}/{outcome}', 0) + 1
    for req, n in REQUIRED.items():
        have = sum(c for k, c in counts.items() if k.startswith(req))
        if have < n:
            missing.append(f"{req}: {have} < {n} completed")
    print(f"[{tag}] paths: {dict(sorted(counts.items()))}")
    for m in missing:
        print(f"[{tag}] COVERAGE GAP: {m}")
    return missing


def reset():
    global FAIL
    CHECKS[0] = 0
    FAIL = []
    LEAVES.clear()


# ---------------- run: clean + mutants ----------------

results = {}
for tag, mut in [('clean', None), ('mutant-U', 'u'),
                 ('mutant-L', 'l'), ('mutant-P', 'p')]:
    reset()
    globals()['MUTANT'] = mut
    suite()
    gaps = report(tag)
    results[tag] = (len(FAIL), list(FAIL), gaps, CHECKS[0])

print()
ok = True
clean_fail, clean_viol, clean_gaps, clean_checks = results['clean']
print(f"clean run: {clean_checks} checks, "
      f"{clean_fail} violations")
for f in clean_viol[:10]:
    print(f"  {f}")
if clean_fail or clean_gaps:
    ok = False
for tag in ('mutant-U', 'mutant-L', 'mutant-P'):
    n, viol, _, _ = results[tag]
    status = "detected" if n else "MISSED — coverage hole"
    print(f"{tag}: {n} violations — {status}")
    for f in viol[:3]:
        print(f"    {f}")
    if not n:
        ok = False
print()
print("RESULT: PASS — borrowable(t) never preceded a live "
      "physical cap; all mutants detected" if ok else
      "RESULT: FAIL")
sys.exit(0 if ok else 1)
