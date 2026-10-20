#!/usr/bin/env python3
"""CHAN-004 — executable model of the A.3.3/A.3.5 identity rules.

Dual simulation. A PHYSICAL world tracks ground truth: where each
resource's exclusive capability actually resides (binding, channel
queue, pending Result, dead). An ABSTRACT checker state tracks only
what the spec's model can know: may-bags per endpoint, ⊥ claims with
candidate sets, pending packets, transit marks.

The safety invariant, checked at every reachable state of every
nondeterministic interleaving (physical delivery order inside a
may-bag, branch outcomes, pending-send SENT/CLOSED worlds):

    abs.borrowable(t)  ⟹  no live physical capability on t

i.e. the checker may release a named source for re-borrow only when
MayOutstanding(t) = ∅ — never while an unprovable residence exists.

Scenarios:
  S1  two spawned producers, recv/drop in both orders (the QA-007
      regression shape: unknown &mut claim, then &mut on a candidate)
  S2  branch join with a may-packet — physical push may not exist
      (coarse-but-sound direction: a valid program rejected rather
      than an unsafe one accepted); plus the |C|=1 promotion case
  S3  pending bounded send — SENT vs CLOSED worlds fork at send;
      a recv before resolution may deliver the pending packet
  S4  forwarded ⊥ — a claimed unknown cap re-sent on a second
      channel keeps its candidates outstanding through the move
"""
import sys
from dataclasses import dataclass, field

FAIL = []
CHECKS = [0]
LEAVES = [0]
ALL_RES = ('x', 'y')


def check(cond, what, ctx):
    CHECKS[0] += 1
    if not cond:
        FAIL.append(f"VIOLATION: {what}  [{ctx}]")


# ---------------- physical world (ground truth) ----------------

@dataclass
class Phys:
    caps: dict = field(default_factory=dict)     # res -> 'dead' | ('bind',v) | ('chan',ep) | ('pend',k)
    queues: dict = field(default_factory=dict)   # ep -> [res,...]  real FIFO
    outcomes: dict = field(default_factory=dict) # pending id -> 'SENT' | 'CLOSED'

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
    unordered: set = field(default_factory=set)     # eps with may-bags
    bots: dict = field(default_factory=dict)        # binding -> frozenset C
    named: dict = field(default_factory=dict)       # res -> binding
    pending: list = field(default_factory=list)     # [(ep, frozenset)]

    def drained(self, ep):
        # Provable drain: pops ≥ may-pushes. Physical enqueues ≤
        # may-pushes (bag over-approximates); pops ≤ physical
        # enqueues (each Some consumes one real message). So
        # pops ≥ may-pushes forces all real messages departed.
        return self.pops.get(ep, 0) >= self.pushes.get(ep, 0)

    def candidates(self, ep):
        # §A.3.3: union of sent-names over every may-packet pushed
        # to ep PLUS every unresolved pending packet on ep — a
        # pending SENT message can physically be the delivered one.
        # Membership cannot shrink per-pop (a pop doesn't identify
        # which candidate departed).
        names = set()
        for p in self.bags.get(ep, []):
            names |= set(p)
        for e, p in self.pending:
            if e == ep:
                names |= set(p)
        return names

    def may_outstanding(self, t):
        """MayOutstanding(t) = U(t) ∪ L(t) ∪ P(t) — §A.3.5."""
        u = any(t in p for ep, bag in self.bags.items()
                for p in bag if not self.drained(ep))
        l = any(t in C for C in self.bots.values())
        p = any(t in pk for _, pk in self.pending)
        return u or l or p

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
# Each returns False when the op is infeasible on this physical
# world (a dead arm), pruning the subtree.

def op_send(phys, abs_, ep, res, ctx):
    """send(ep, &mut res) — unbounded or resolved-known: mint cap,
    enqueue physically + may-push."""
    phys.caps[res] = ('chan', ep)
    phys.queues.setdefault(ep, []).append(res)
    abs_.bags.setdefault(ep, []).append(frozenset({res}))
    abs_.pushes[ep] = abs_.pushes.get(ep, 0) + 1
    invariant(phys, abs_, ctx)
    return True


def op_claim(phys, abs_, ep, vname, ctx):
    """recv(ep) Some-arm: physical FIFO pop; abstract claim —
    verbatim if ordered, named if |C|=1 (singleton promotion),
    else ⊥ over the full candidate union."""
    if not phys.queues.get(ep):
        return False                     # None arm only on this world
    res = phys.queues[ep].pop(0)
    phys.caps[res] = ('bind', vname)
    abs_.pops[ep] = abs_.pops.get(ep, 0) + 1
    if ep in abs_.unordered:
        C = abs_.candidates(ep)
        if len(C) == 1:
            abs_.named[next(iter(C))] = vname     # provable identity
        else:
            abs_.bots[vname] = C
    else:
        abs_.named[res] = vname                   # ordered: verbatim
    invariant(phys, abs_, ctx)
    return True


def op_drop(phys, abs_, vname, ctx):
    """Binding dies: the cap it physically held dies; its ⊥ edge
    (or named edge) leaves the abstract state."""
    for t, c in phys.caps.items():
        if c == ('bind', vname):
            phys.caps[t] = 'dead'
    abs_.bots.pop(vname, None)
    for t, b in list(abs_.named.items()):
        if b == vname:
            del abs_.named[t]
    invariant(phys, abs_, ctx)
    return True


def op_borrow_attempt(phys, abs_, t, ctx):
    """&mut t attempt. borrowable(t) True means the abstract ALLOWS
    it — soundness then requires no live physical cap on t."""
    allowed = abs_.borrowable(t)
    check(not (allowed and phys.alive(t)),
          f"&mut {t} ALLOWED while physical cap alive at "
          f"{phys.caps.get(t)}", ctx)
    return True


def op_resolve_ok(phys, abs_, ep, pkt, pid, ctx):
    """Ok arm — feasible only on the SENT world: the pending packet
    commits to the may-bag (its message may still be queued, or may
    already have been delivered by an intervening recv)."""
    if phys.outcomes.get(pid) != 'SENT':
        return False                     # CLOSED world: Ok arm is dead
    abs_.pending.remove((ep, pkt))
    abs_.bags.setdefault(ep, []).append(pkt)
    abs_.pushes[ep] = abs_.pushes.get(ep, 0) + 1
    invariant(phys, abs_, ctx)
    return True


def op_resolve_err(phys, abs_, ep, pkt, pid, vname, ctx):
    """Err arm — feasible only on the CLOSED world: verbatim reclaim,
    identity never in doubt (§A.3.3)."""
    if phys.outcomes.get(pid) != 'CLOSED':
        return False                     # SENT world: Err arm is dead
    abs_.pending.remove((ep, pkt))
    for t in pkt:
        phys.caps[t] = ('bind', vname)
        abs_.named[t] = vname
    invariant(phys, abs_, ctx)
    return True


def op_forward(phys, abs_, ep2, vname, ctx):
    """Re-send a ⊥-carrying binding on ep2: the cap moves channels;
    the carried packet keeps the candidates outstanding (§A.3.5 U)."""
    for t, c in phys.caps.items():
        if c == ('bind', vname):
            phys.caps[t] = ('chan', ep2)
            phys.queues.setdefault(ep2, []).append(t)
    C = abs_.bots.pop(vname, frozenset())
    abs_.bags.setdefault(ep2, []).append(frozenset(C))
    abs_.pushes[ep2] = abs_.pushes.get(ep2, 0) + 1
    abs_.unordered.add(ep2)
    invariant(phys, abs_, ctx)
    return True


OPS = {
    'send': op_send, 'claim': op_claim, 'drop': op_drop,
    'try': op_borrow_attempt, 'ok': op_resolve_ok,
    'err': op_resolve_err, 'fwd': op_forward,
}


# ---------------- scenario driver ----------------

def run(label, phys, abs_, script):
    """DFS: 'FORK' items expand all (phys_fn, abs_fn, tag) alternatives;
    ops returning False prune the branch (dead arm on this world)."""
    def rec(i, ph, ab, trace):
        if i == len(script):
            LEAVES[0] += 1
            return
        op = script[i]
        if op[0] == 'FORK':
            for pfn, afn, tag in op[1]:
                ph2, ab2 = ph.clone(), ab.clone()
                pfn(ph2); afn(ab2)
                rec(i + 1, ph2, ab2, f"{trace}/{tag}")
            return
        if OPS[op[0]](ph, ab, *op[1:], trace):
            rec(i + 1, ph, ab, trace)
    rec(0, phys, abs_, label)


def make_phys(order):
    """Physical world: caps x,y queued on 'ch' in the given order."""
    p = Phys()
    for r in order:
        p.caps[r] = ('chan', 'ch')
    p.queues['ch'] = list(order)
    return p


def fork_pending_send(res, pid):
    """The bounded send's physical outcome — chosen by the runtime
    at the call, invisible to the checker — forks two worlds."""
    def sent(ph):
        ph.caps[res] = ('chan', 'ch')
        ph.queues.setdefault('ch', []).append(res)
        ph.outcomes[pid] = 'SENT'
    def closed(ph):
        ph.caps[res] = ('pend', pid)   # retained inside the Result
        ph.outcomes[pid] = 'CLOSED'
    def mk_abs(ab):
        ab.pending.append(('ch', frozenset({res})))
        ab.unordered.add('ch')         # live pending → unordered (A.3.6)
    return ('FORK', [(sent, mk_abs, 'SENT'), (closed, mk_abs, 'CLOSED')])


# --- S1: two spawned producers — the regression shape --------------
# Abstract: unordered bag {x},{y}. recv → ⊥{x,y}; &mut on any
# candidate must be rejected while any unprovable residence exists.
for order in [('x', 'y'), ('y', 'x')]:
    bag = {'ch': [frozenset('x'), frozenset('y')]}
    run(f"S1-order{order}", make_phys(order),
        Abs(bags=bag, pushes={'ch': 2}, unordered={'ch'}), [
            ('claim', 'ch', 'v1'),
            ('try', 'x'), ('try', 'y'),   # ⊥ live — blocked
            ('claim', 'ch', 'v2'),
            ('try', 'x'), ('try', 'y'),   # ⊥ + ⊥' live — blocked
            ('drop', 'v1'),
            ('try', 'x'),                 # ⊥' still names x — blocked
            ('drop', 'v2'),               # drained + no ⊥ → released
            ('try', 'x'), ('try', 'y'),
        ])
    # variant: drop the first claim before the second recv — the bag
    # is not provably drained (pops=1 < pushes=2) → still blocked
    run(f"S1b-order{order}", make_phys(order),
        Abs(bags={'ch': [frozenset('x'), frozenset('y')]},
            pushes={'ch': 2}, unordered={'ch'}), [
            ('claim', 'ch', 'v1'),
            ('drop', 'v1'),
            ('try', 'x'), ('try', 'y'),   # U still non-empty — blocked
            ('claim', 'ch', 'v2'),
            ('drop', 'v2'),               # drained + dead → released
            ('try', 'x'), ('try', 'y'),
        ])

# --- S2: branch join — a may-packet may not physically exist -------
# Abstract bag {x} (pushes=1) from the taken-path send; physically
# the other path ran — channel empty, no cap exists. recv → None arm
# (no pop); x stays transit-marked forever: the may-packet cannot be
# proven absent. A false rejection — the accepted safe direction.
run("S2-ghost-packet", Phys(queues={'ch': []}),
    Abs(bags={'ch': [frozenset('x')]}, pushes={'ch': 1},
        unordered={'ch'}), [
        ('try', 'x'),                     # blocked: may-packet unproven
    ])
# Control: the push WAS real — |C|=1 promotes to a verbatim claim;
# ordinary named-edge rules govern afterwards.
run("S2-singleton", make_phys(('x',)),
    Abs(bags={'ch': [frozenset('x')]}, pushes={'ch': 1},
        unordered={'ch'}), [
        ('claim', 'ch', 'v'),             # |C|=1 → named v→x
        ('try', 'x'),                     # blocked by named edge
        ('drop', 'v'),                    # cap dead → released
        ('try', 'x'),
    ])

# --- S3: pending bounded send --------------------------------------
# The physical outcome forks at send. A recv before resolution may
# deliver the pending packet (SENT world) — the claim's candidate
# union must include pending names or the model would miss it.
for pre_claim in (True, False):
    script = [
        fork_pending_send('x', pid=0),
        ('try', 'x'),                     # blocked — pending unresolved
    ]
    if pre_claim:
        script += [
            ('claim', 'ch', 'v'),         # SENT world: delivers x
            ('try', 'x'),                 # blocked — ⊥/named alive
        ]
    script += [
        ('ok', 'ch', frozenset('x'), 0),      # SENT world only
        ('err', 'ch', frozenset('x'), 0, 'e'),# CLOSED world only
        ('try', 'x'),                     # blocked — v/e or bag holds it
        ('drop', 'v'), ('drop', 'e'),
        ('try', 'x'),                     # released once provably dead
    ]
    run(f"S3-preclaim={pre_claim}", Phys(), Abs(), script)

# --- S4: forwarded ⊥ -----------------------------------------------
# Claim ⊥{x,y} on ch, re-send on ch2 — candidates stay outstanding
# through the move until the second channel provably drains.
for order in [('x', 'y'), ('y', 'x')]:
    run(f"S4-order{order}", make_phys(order),
        Abs(bags={'ch': [frozenset('x'), frozenset('y')]},
            pushes={'ch': 2}, unordered={'ch'}), [
            ('claim', 'ch', 'v1'),
            ('fwd', 'ch2', 'v1'),         # ⊥{x,y} → packet on ch2
            ('try', 'x'),                 # blocked — via U(ch2)
            ('claim', 'ch', 'v2'),        # drain ch
            ('claim', 'ch2', 'w'),        # claim the forwarded cap
            ('drop', 'v2'), ('drop', 'w'),
            ('try', 'x'), ('try', 'y'),   # all residences dead
        ])

# ---------------- report ----------------

print(f"paths explored: {LEAVES[0]}   checks: {CHECKS[0]}")
if FAIL:
    for f in FAIL:
        print(f)
    print("RESULT: FAIL")
    sys.exit(1)
print("RESULT: PASS — borrowable(t) never preceded a live physical cap")
