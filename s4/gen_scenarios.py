#!/usr/bin/env python3
"""CHAN-004 S4 — deterministic bounded-channel scenario generator.

Every program derives from a single integer seed: `--seed S` emits the
same corpus on any host. Programs are emitted with a JSON manifest
line recording the family's expected verdict:

    {"expect": "exit:42"}         accepted; runtime exit code
    {"expect": "reject:<substr>"} rejected; diagnostic contains substr
    {"expect": "timeout"}         accepted but blocks at runtime

The generator only emits shapes whose verdict it can derive from the
ownership contract — anything it cannot predict deterministically is
omitted from the space rather than guessed at.
"""
import argparse
import json
import os
import random
import sys

HDR = "// S4 scenario {name} (seed {seed})\n"

PT_STRUCT = "struct Point { x: i32, y: i32 }\n"
PAIR_STRUCT = "struct Pair { a: i32, b: i32 }\n"
RES_STRUCT = "struct Res { p: box i32, tag: i32 }\n"
NEST_STRUCT = "struct Inner { p: box i32 }\nstruct Nest { inner: Inner, tag: i32 }\n"

# Per-payload-type emit helpers. Each returns dict of code fragments.
def payload_info(kind):
    if kind == "i32":
        return dict(
            structs="", ch_ty="i32",
            mk_send=lambda v, i: f"send(ch, {10 + i})",
            err_arm="Err(v) => { n = n + v; }",
            some_use="n = n + v; ",
            err_arm_main="Err(v) => { return 100 + v; }",
            val=lambda g, i, j: 10 + g,
            needs_heap=False, refs=0)
    if kind == "box":
        return dict(
            structs="", ch_ty="box i32",
            mk_send=lambda v, i: (
                f"let p{i} = alloc<i32>(heap); *p{i} = {10 + i};\n"
                f"    let rr{i} = send(ch, p{i});\n"
                f"    match rr{i} {{ Ok => {{}} Err(e) => {{ free(e); }} }}"),
            err_arm=None,  # handled inline by mk_send
            err_arm_main=None,
            some_use="n = n + *v; free(v); ",
            val=lambda g, i, j: 10 + g,
            needs_heap=True, refs=0)
    if kind == "pair":
        return dict(
            structs=PAIR_STRUCT, ch_ty="Pair",
            mk_send=lambda v, i: f"send(ch, Pair {{ a: {10 + i}, b: {i} }})",
            err_arm="Err(v) => { n = n + v.a; }",
            err_arm_main="Err(v) => { return 100 + v.a; }",
            some_use="n = n + v.a + v.b; ",
            val=lambda g, i, j: 10 + 2 * g,
            needs_heap=False, refs=0)
    if kind == "res":
        return dict(
            structs=RES_STRUCT, ch_ty="Res",
            mk_send=lambda v, i: (
                f"let q{i}: Res; q{i}.p = alloc<i32>(heap); *q{i}.p = {10 + i};\n"
                f"    q{i}.tag = {i};\n"
                f"    let rr{i} = send(ch, q{i});\n"
                f"    match rr{i} {{ Ok => {{}} Err(e) => {{ free(e.p); }} }}"),
            err_arm=None,
            err_arm_main=None,
            some_use="n = n + v.tag + *v.p; free(v.p); ",
            val=lambda g, i, j: 10 + 2 * g,
            needs_heap=True, refs=0)
    if kind == "nest":
        return dict(
            structs=NEST_STRUCT, ch_ty="Nest",
            mk_send=lambda v, i: (
                f"let q{i}: Nest; q{i}.inner.p = alloc<i32>(heap);\n"
                f"    *q{i}.inner.p = {10 + i}; q{i}.tag = {i};\n"
                f"    let rr{i} = send(ch, q{i});\n"
                f"    match rr{i} {{ Ok => {{}} Err(e) => {{ free(e.inner.p); }} }}"),
            err_arm=None,
            err_arm_main=None,
            some_use="n = n + v.tag + *v.inner.p; free(v.inner.p); ",
            val=lambda g, i, j: 10 + 2 * g,
            needs_heap=True, refs=0)
    if kind == "mutref":
        return dict(
            structs=PT_STRUCT, ch_ty="&mut Point",
            mk_send=lambda v, i: f"send(ch, &mut {v})",
            err_arm="Err(e) => {}",
            err_arm_main="Err(e) => {}",
            # write works through a ⊥ claim; read needs a singleton
            # candidate set (see S4 report: unordered reads unresolved)
            some_use_write="(*v).x = 9; ",
            some_use_read="n = n + (*v).x; ",
            val_write=lambda i, j: 0,
            val_read=lambda i, j: j + 1,
            needs_heap=False, refs=1)
    if kind == "ref":
        return dict(
            structs=PT_STRUCT, ch_ty="&Point",
            mk_send=lambda v, i: f"send(ch, &{v})",
            err_arm="Err(e) => {}",
            err_arm_main="Err(e) => {}",
            some_use_read="n = n + (*v).x; ",
            val_read=lambda i, j: j + 1,
            needs_heap=False, refs=1)
    raise ValueError(kind)


def send_block(pi, var, i, indent, err_arm=None):
    """One send with a fully-matched Result. `mk_send` for owning
    payloads returns a self-contained block (the pending result must
    be bound+matched inline). `err_arm` overrides the task-scope arm
    for main-scope sends (diverging arms need no `n` accumulator)."""
    s = pi["mk_send"](var, i)
    pad = " " * indent
    if pi["err_arm"] is None:
        return pad + s + "\n"
    arm = err_arm if err_arm is not None else pi["err_arm"]
    return (pad + f"let r = {s};\n"
            + pad + "match r { Ok => {} " + arm + " }\n")


def consumer_task(name, quota, some_use, indent=4):
    """Task body: recv `quota` times, sum/use per some_use, return n."""
    pad = " " * indent
    out = pad + f"let t_{name} = spawn {{\n"
    out += pad + "    let n: i32 = 0;\n"
    out += pad + "    let i: i32 = 0;\n"
    out += pad + "    while i < " + str(quota) + " {\n"
    out += pad + "        let o = recv(ch);\n"
    out += pad + "        match o {\n"
    out += pad + "            Some(v) => { " + some_use + "}\n"
    out += pad + "            None => {}\n"
    out += pad + "        }\n"
    out += pad + "        i = i + 1;\n"
    out += pad + "    }\n"
    out += pad + "    return n;\n"
    out += pad + "};\n"
    return out


def producer_task(name, pi, var, count, start, indent=4):
    pad = " " * indent
    out = pad + f"let t_{name} = spawn {{\n"
    out += pad + "    let n: i32 = 0;\n"
    for i in range(count):
        out += send_block(pi, var, start + i, indent + 4)
    out += pad + "    return n;\n"
    out += pad + "};\n"
    return out


def gen_pc_tasks(seed, rng):
    """P producer tasks × C consumer tasks, exact totals, fixed quota.
    Deterministic: each consumer stops at its quota so the pigeonhole
    argument guarantees delivery regardless of schedule. Ref payloads
    get a fresh source per message (a transit &mut can't re-send);
    Some-arm reads are only emitted when the whole program produces
    ONE ref message — the singleton candidate set is what makes a
    ⊥ claim promote to a readable verbatim edge."""
    kind = rng.choice(["i32", "i32", "box", "pair", "res", "nest",
                       "mutref", "mutref", "ref"])
    pi = payload_info(kind)
    p = rng.choice([1, 1, 2, 3]) if not pi["refs"] else rng.choice([1, 2])
    c = rng.choice([1, 2])
    quota = rng.choice([1, 2])
    if (c * quota) % p:
        return None
    msgs_per = (c * quota) // p
    cap = rng.choice([1, 2, 4])
    # ref consumers run post-join in main — every send must fit the
    # queue or a producer blocks forever with nobody left to drain
    if pi["refs"]:
        cap = max(cap, p * msgs_per)
    name = f"pc_p{p}c{c}q{quota}cap{cap}_{kind}"

    # arm kind is uniform across consumers — the delivered message's
    # contribution must be schedule-independent
    ref_msgs = p * msgs_per
    if kind == "ref":
        if ref_msgs != 1:
            return None          # multi-candidate read: unresolved
        use = pi["some_use_read"]
        val = lambda g, i, j: (j * 4 + i) + 1
    elif kind == "mutref":
        if ref_msgs == 1 and rng.choice([False, True]):
            use = pi["some_use_read"]
            val = lambda g, i, j: (j * 4 + i) + 1
        else:
            use = pi["some_use_write"]
            val = lambda g, i, j: 0
    else:
        use = pi["some_use"]
        val = pi["val"]

    decls = pi["structs"]
    body = ""
    if pi["needs_heap"]:
        body += "    let heap = system_allocator();\n"
    if pi["refs"]:
        for j in range(p):
            for i in range(msgs_per):
                x = (j * 4 + i) + 1
                body += (f"    let r{j}_{i} = "
                         f"Point {{ x: {x}, y: 0 }};\n")
    body += f"    let ch: bchannel {pi['ch_ty']} = bchannel({cap});\n"

    if pi["refs"]:
        # consumers in a sibling task are gap cases — the abstract
        # queue is empty at their analysis and the Some binder is
        # inert. Main-line recvs AFTER the producers join see the
        # merged queue and can claim the delivered capability.
        body += "    let n: i32 = 0;\n"
    else:
        for j in range(c):
            body += consumer_task(f"c{j}", quota, use)

    for j in range(p):
        pad = " " * 4
        body += pad + f"let t_p{j} = spawn {{\n"
        body += pad + "    let n: i32 = 0;\n"
        for i in range(msgs_per):
            v = f"r{j}_{i}" if pi["refs"] else "x"
            body += send_block(pi, v, j * msgs_per + i, 8)
        body += pad + "    return n;\n"
        body += pad + "};\n"

    if pi["refs"]:
        for j in range(p):
            body += f"    join t_p{j};\n"
        for i in range(c * quota):
            body += f"    let o{i} = recv(ch);\n"
            body += f"    match o{i} {{\n"
            body += f"        Some(v) => {{ {use}}}\n"
            body += "        None => { n = n + 1000; }\n"
            body += "    }\n"
        total = "n"
    else:
        for j in range(c):
            body += f"    let s{j}: i32 = join t_c{j};\n"
        for j in range(p):
            body += f"    join t_p{j};\n"
        total = "+".join(f"s{j}" for j in range(c))
    expected_sum = sum(val(j * msgs_per + i, i, j)
                       for j in range(p) for i in range(msgs_per))
    body += f"    return {total} + {42 - expected_sum};\n"
    prog = HDR.format(name=name, seed=seed) + decls
    prog += "fn main() -> i32 {\n" + body + "}\n"
    return name, prog, {"expect": "exit:42",
                        "family": "pc", "payload": kind,
                        "producers": p, "consumers": c, "cap": cap}


def gen_gap(seed, rng):
    """Consumer task analyzed before the producer task merges —
    speculative recv has pkt=None while a physical Some delivers.
    Owned payloads are self-authorizing; ref payloads are inert."""
    kind = rng.choice(["i32", "box", "res", "mutref", "ref", "pair"])
    pi = payload_info(kind)
    use_binder = rng.choice([False, True]) if pi["refs"] else False
    name = f"gap_{kind}_{'use' if use_binder else 'inert'}"

    decls = pi["structs"]
    body = ""
    if pi["needs_heap"]:
        body += "    let heap = system_allocator();\n"
    rv = ""
    if pi["refs"]:
        body += "    let r0 = Point { x: 7, y: 0 };\n"
        rv = "r0"
    body += f"    let ch: bchannel {pi['ch_ty']} = bchannel(4);\n"
    # consumer FIRST — its analysis sees an empty abstract queue
    if use_binder:
        arm = "(*v).y = 9; " if kind == "mutref" else "n = n + (*v).x; "
    elif kind in ("box", "res"):
        arm = pi["some_use"]          # own authority: still works
    else:
        arm = "n = n + 1; "           # inert: never touches v
    body += consumer_task("c0", 1, arm)
    body += producer_task("p0", pi, rv, 1, 0)
    body += "    let s0: i32 = join t_c0;\n"
    body += "    join t_p0;\n"
    # gap inert arms return their n (0 or payload-dependent); make
    # the exit deterministic by returning a constant plus s0*0
    if kind in ("box", "res", "pair"):
        # owned payloads consumed properly inside the arm
        body += "    return s0 * 0 + 42;\n"
    else:
        body += "    return s0 * 0 + 42;\n"
    prog = HDR.format(name=name, seed=seed) + decls
    prog += "fn main() -> i32 {\n" + body + "}\n"
    if use_binder:
        return name, prog, {"expect": "reject:use of moved reference",
                            "family": "gap", "payload": kind}
    return name, prog, {"expect": "exit:42", "family": "gap",
                        "payload": kind}


def gen_close(seed, rng):
    """Deterministic close: queue all messages, spawn a close-only
    task, then drain while the closer is unjoined — the carve-out
    window where main may still use `ch`. The final recv sees
    closed-and-drained → None."""
    kind = rng.choice(["i32", "box", "pair"])
    pi = payload_info(kind)
    k = rng.choice([1, 2, 3])
    cap = rng.choice([3, 4])
    name = f"close_{kind}_k{k}"
    decls = pi["structs"]
    body = ""
    if pi["needs_heap"]:
        body += "    let heap = system_allocator();\n"
    body += f"    let ch: bchannel {pi['ch_ty']} = bchannel({cap});\n"
    for i in range(k):
        body += send_block(pi, "x", i, 4, err_arm=pi["err_arm_main"])
    body += "    let t = spawn {\n        close(ch);\n    };\n"
    # all k queued messages deliver before close is observable
    body += "    let n: i32 = 0;\n"
    for i in range(k):
        body += "    let o" + str(i) + " = recv(ch);\n"
    body += "    let oz = recv(ch);\n"
    body += "    join t;\n"
    for i in range(k):
        body += "    match o" + str(i) + " {\n"
        body += "        Some(v) => { " + pi["some_use"] + "}\n"
        # no early return — it would abandon the pending Options
        body += "        None => { n = n + 1000 + " + str(i) + "; }\n"
        body += "    }\n"
    body += "    match oz {\n"
    # unreachable arm still type-checks: an owned payload can't be
    # abandoned inside it
    oz_arm = ("Some(v) => { " + pi["some_use"] + "return 99; }"
              if kind in ("box",) else "Some(v) => { return 99; }")
    body += "        " + oz_arm + "\n"
    body += "        None => {}\n"
    body += "    }\n"
    exp = sum(10 + i + (i if kind == "pair" else 0) for i in range(k))
    body += f"    return n + {42 - exp};\n"
    prog = HDR.format(name=name, seed=seed) + decls
    prog += "fn main() -> i32 {\n" + body + "}\n"
    return name, prog, {"expect": "exit:42", "family": "close",
                        "payload": kind}


def gen_close_race(seed, rng):
    """Close while a send is blocked on a full queue: the send always
    resolves Err (closed-before-admission) and the queued packet
    always drains as Some — deterministic interior exercising both
    the CLOSED and drain paths (fixture 779/800 shape)."""
    name = f"closerace_{seed % 1000}"
    body = "    let ch: bchannel i32 = bchannel(1);\n"
    body += "    let r1 = send(ch, 40);\n"
    body += "    match r1 { Ok => {} Err(v) => { return 100 + v; } }\n"
    body += "    let t = spawn {\n        close(ch);\n    };\n"
    body += "    let r2 = send(ch, 2);\n"
    body += "    let o = recv(ch);\n"
    body += "    join t;\n"
    body += "    let n: i32 = 0;\n"
    body += "    match r2 { Ok => { n = n + 50; } Err(v) => { n = n + v; } }\n"
    body += "    match o {\n"
    body += "        Some(v) => { n = n + v; }\n"
    body += "        None => { n = n + 100; }\n"
    body += "    }\n"
    body += "    return n;\n"
    prog = HDR.format(name=name, seed=seed)
    prog += "fn main() -> i32 {\n" + body + "}\n"
    return name, prog, {"expect": "exit:42", "family": "close_race",
                        "payload": "i32"}


def gen_deadlock(seed, rng):
    """Accepted but blocking: recv on an open empty channel blocks
    forever. The harness must bound it with a timeout."""
    name = f"block_recv_{seed % 1000}"
    prog = HDR.format(name=name, seed=seed)
    prog += "fn main() -> i32 {\n"
    prog += "    let ch: bchannel i32 = bchannel(2);\n"
    prog += "    let o = recv(ch);\n"
    prog += "    match o { Some(v) => { return v; } None => { return 7; } }\n"
    prog += "}\n"
    return name, prog, {"expect": "timeout", "family": "deadlock",
                        "payload": "i32"}


def gen_invalid(seed, rng):
    """Programs the checker must reject, each violating one rule."""
    kind = rng.choice([
        "use_after_send", "unexamined", "wildcard", "abandon_owned",
        "source_write", "none_use", "opaque_ctor",
    ])
    name = f"bad_{kind}_{seed % 1000}"
    decls = ""
    body = ""
    if kind == "use_after_send":
        body += "    let heap = system_allocator();\n"
        body += "    let ch: bchannel box i32 = bchannel(2);\n"
        body += "    let p = alloc<i32>(heap); *p = 5;\n"
        body += "    let r = send(ch, p);\n"
        body += "    match r { Ok => {} Err(e) => { free(e); } }\n"
        body += "    return *p;\n"
        exp = "reject:use after consume"
    elif kind == "unexamined":
        body += "    let ch: bchannel i32 = bchannel(2);\n"
        body += "    let r = send(ch, 1);\n"
        body += "    return 42;\n"
        exp = "reject:never examined"
    elif kind == "wildcard":
        body += "    let ch: bchannel i32 = bchannel(2);\n"
        body += "    let o = recv(ch);\n"
        body += "    match o { _ => {} }\n"
        body += "    return 42;\n"
        exp = "reject:resolve the recv"
    elif kind == "abandon_owned":
        body += "    let heap = system_allocator();\n"
        body += "    let ch: bchannel box i32 = bchannel(2);\n"
        body += "    let p = alloc<i32>(heap); *p = 5;\n"
        body += "    let r = send(ch, p);\n"
        body += "    match r { Ok => {} Err(e) => { free(e); } }\n"
        body += "    let o = recv(ch);\n"
        body += "    match o { Some(v) => {} None => {} }\n"
        body += "    return 42;\n"
        exp = "reject:abandon"
    elif kind == "source_write":
        decls = PT_STRUCT
        body += "    let ch: bchannel &mut Point = bchannel(2);\n"
        body += "    let a = Point { x: 1, y: 0 };\n"
        body += "    let r = send(ch, &mut a);\n"
        body += "    match r { Ok => {} Err(e) => {} }\n"
        body += "    a.y = 5;\n"
        body += "    return 42;\n"
        exp = "reject:authority was sent through a channel"
    elif kind == "none_use":
        decls = PT_STRUCT
        body += "    let ch: bchannel &mut Point = bchannel(2);\n"
        body += "    let a = Point { x: 1, y: 0 };\n"
        body += "    let r = send(ch, &mut a);\n"
        body += "    match r { Ok => {} Err(e) => {} }\n"
        body += "    let o = recv(ch);\n"
        body += "    match o { Some(v) => {} None => { (*v).x = 1; } }\n"
        body += "    return 42;\n"
        exp = "reject:undefined variable `v`"
    elif kind == "opaque_ctor":
        # F4 — construction without a declared `bchannel T` used to
        # reach codegen and fix esz at the 8-byte fallback; the gate
        # must reject it. Vary the opaque shape: bare let, nested
        # call arg, or a `bchannel void` annotation.
        shape = rng.choice(["let", "arg", "void"])
        if shape == "let":
            body += "    let ch = bchannel(2);\n"
        elif shape == "arg":
            decls += "fn sink(c: bchannel i32) -> i32 { return 1; }\n"
            body += "    return sink(bchannel(2));\n"
        else:
            body += "    let ch: bchannel void = bchannel(2);\n"
            body += "    return 42;\n"
        exp = "reject:needs a declared payload type"
    prog = HDR.format(name=name, seed=seed) + decls
    prog += "fn main() -> i32 {\n" + body + "}\n"
    return name, prog, {"expect": exp, "family": "invalid",
                        "violation": kind}


FAMILIES = [
    (gen_pc_tasks, 5),
    (gen_gap, 3),
    (gen_close, 2),
    (gen_close_race, 1),
    (gen_deadlock, 1),
    (gen_invalid, 3),
]


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--seed", type=int, required=True)
    ap.add_argument("--count", type=int, default=1,
                    help="scenario batches; seed+i derives each")
    ap.add_argument("--out", required=True)
    args = ap.parse_args()
    os.makedirs(args.out, exist_ok=True)
    manifest = []
    for i in range(args.count):
        rng = random.Random(args.seed + i)
        seen = set()
        for gen, weight in FAMILIES:
            for _ in range(weight):
                res = gen(args.seed + i, rng)
                if res is None or res == "reject_read_unord":
                    continue
                name, prog, meta = res
                if name in seen:
                    continue
                seen.add(name)
                fname = f"{name}_s{args.seed + i}.lang"
                with open(os.path.join(args.out, fname), "w") as f:
                    f.write(prog)
                meta["file"] = fname
                meta["seed"] = args.seed + i
                manifest.append(meta)
    with open(os.path.join(args.out, "manifest.jsonl"), "w") as f:
        for m in manifest:
            f.write(json.dumps(m) + "\n")
    print(f"generated {len(manifest)} scenarios in {args.out}")


if __name__ == "__main__":
    main()
