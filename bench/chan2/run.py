#!/usr/bin/env python3
"""CHAN-002 — channel memory attribution experiment.

Runs mem_pc.lang (64B by-value payloads) under controlled
producer/consumer sleep rates with MML_CHAN_STATS=1, capturing the
runtime's live-queue counters (peak msgs, allocated slots/bytes,
occupancy) plus process peak RSS via /usr/bin/time. Goal: split
the segmented queue's RSS growth into backlog vs retained
capacity vs allocator behavior. No queue semantics changed.
"""
import os, re, subprocess, sys, hashlib

ROOT = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(ROOT))
SRC = os.path.join(ROOT, "src")
OUT = os.path.join(ROOT, "build")
os.makedirs(OUT, exist_ok=True)
OURC = os.path.join(REPO, "target", "release", "ourc")
RUNTIME = os.path.join(REPO, "compiler", "runtime")
BSD = sys.platform == "darwin"
CC = "clang"
SEGS = [0, 16, 64, 256]
N = os.environ.get("N", "200000")

# (name, prod_us, cons_us) — sleep granularity: producer sleeps
# prod_us after every 64 msgs; consumer sleeps cons_us per msg.
RATES = [
    ("matched",      0, 0),
    ("cons-slow1",   0, 1),
    ("cons-slow5",   0, 5),
    ("prod-slow5",   5, 0),
]

def checked(cmd, what):
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode != 0:
        print(f"BUILD FAIL {what}: {r.stderr.decode()[:400]}")
        sys.exit(1)

def build():
    objs = []
    for s in ("mmlrt_common.c", "mmlrt_posix.c"):
        o = os.path.join(OUT, s.replace(".c", ".o"))
        checked([CC, "-O2", "-c", os.path.join(RUNTIME, s), "-o", o], s)
        objs.append(o)
    src = os.path.join(SRC, "mem_pc.lang")
    r = subprocess.run([OURC, src, "--show-llvm", "-o",
                        os.path.join(OUT, "mem_pc")],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print(f"BUILD FAIL {src}: {r.stderr[:400]}"); sys.exit(1)
    ll = os.path.join(OUT, "mem_pc.ll")
    open(ll, "w").write(r.stdout)
    exe = os.path.join(OUT, "mem_pc.bin")
    checked([CC, "-O2", "-x", "ir", "-c", ll, "-o", ll + ".o"], "ll")
    checked([CC, "-O2", ll + ".o", *objs, "-o", exe], "link")
    return exe

def run(exe, rate, seg):
    env = dict(os.environ)
    env["MML_CHAN_STATS"] = "1"
    if seg: env["MML_CHAN_SEG"] = str(seg)
    else:   env.pop("MML_CHAN_SEG", None)
    name, pus, cus = rate
    cmd = [exe, N, str(pus), str(cus)]
    if BSD: cmd = ["/usr/bin/time", "-l", *cmd]
    else:   cmd = ["/usr/bin/time", "-v", *cmd]
    p = subprocess.run(cmd, capture_output=True, env=env)
    err = p.stderr.decode()
    st = re.search(r"msgs_peak=(\d+) slots_peak=(\d+) "
                   r"bytes_peak=(\d+) occ=([\d.]+)", err)
    m = re.search(r"maximum resident set size[:\s]+(\d+)" if not BSD
                  else r"(\d+)\s+maximum resident set size", err)
    rss = int(m.group(1)) // (1024 if BSD else 1) if m else 0
    dig = hashlib.sha256(p.stdout).hexdigest()[:12]
    return (p.returncode, dig, rss,
            tuple(int(x) for x in st.groups()[:3]) +
            (float(st.group(3+1)),) if st else None)

def main():
    exe = build()
    digests = {}
    print(f"{'rate':12} {'seg':>5} {'rc':>3} {'msgs_peak':>10} "
          f"{'slots_peak':>11} {'bytes_peak':>11} {'occ':>6} "
          f"{'rss_MB':>8}")
    for rate in RATES:
        for seg in SEGS:
            rc, dig, rss, st = run(exe, rate, seg)
            digests.setdefault(rate[0], set()).add(dig)
            if st:
                mp, sp, bp, occ = st
                print(f"{rate[0]:12} {seg:>5} {rc:>3} {mp:>10} "
                      f"{sp:>11} {bp:>11} {occ:>6} {rss/1024:>8.1f}")
            else:
                print(f"{rate[0]:12} {seg:>5} {rc:>3} "
                      f"(no stats) rss={rss/1024:.1f}MB")
    bad = {k: v for k, v in digests.items() if len(v) != 1}
    if bad:
        print(f"!! digest mismatch across segs: {bad}")
        sys.exit(1)

if __name__ == "__main__":
    main()
