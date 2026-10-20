#!/usr/bin/env python3
"""CHAN-002 — channel memory attribution experiment.

Runs mem_pc.lang (64B by-value payloads) under controlled
producer/consumer sleep rates with MML_CHAN_STATS=1, capturing the
runtime's live-queue counters plus process peak RSS via
/usr/bin/time.

A cell is publishable ONLY if: rc==0, the stats line parsed, the
RSS measurement parsed, and the stdout digest matches every other
seg variant of the same rate. Any missing evidence suppresses the
row and exits nonzero.

`_peak` counters are non-simultaneous maxima; at_bytes_peak_*
is the simultaneous snapshot taken when st_bytes maxed.
"""
import hashlib, os, re, statistics, subprocess, sys, time

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

# (name, prod_us, cons_us) — producer sleeps prod_us after every
# 64 msgs; consumer sleeps cons_us per msg.
RATES = [
    ("matched",      0, 0),
    ("cons-slow1",   0, 1),
    ("cons-slow5",   0, 5),
    ("prod-slow5",   5, 0),
]

ST_RE = re.compile(
    r"msgs_peak=(\d+) slots_peak=(\d+) bytes_peak=(\d+) "
    r"at_bytes_peak_msgs=(\d+) at_bytes_peak_slots=(\d+) "
    r"occ=([\d.]+)")
RSS_RE = (re.compile(r"(\d+)\s+maximum resident set size") if BSD
          else re.compile(r"maximum resident set size[:\s]+(\d+)"))

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
    """Returns (rc, digest, rss_kb, stats-tuple|None, wall_s)."""
    env = dict(os.environ)
    env["MML_CHAN_STATS"] = "1"
    if seg: env["MML_CHAN_SEG"] = str(seg)
    else:   env.pop("MML_CHAN_SEG", None)
    name, pus, cus = rate
    cmd = [exe, N, str(pus), str(cus)]
    if BSD: cmd = ["/usr/bin/time", "-l", *cmd]
    else:   cmd = ["/usr/bin/time", "-v", *cmd]
    t = time.monotonic()
    p = subprocess.run(cmd, capture_output=True, env=env)
    w = time.monotonic() - t
    err = p.stderr.decode()
    st = ST_RE.search(err)
    m = RSS_RE.search(err)
    rss = int(m.group(1)) // (1024 if BSD else 1) if m else None
    stats = (tuple(int(x) for x in st.groups()[:5])
             + (float(st.group(6)),)) if st else None
    dig = hashlib.sha256(p.stdout).hexdigest()[:12]
    return p.returncode, dig, rss, stats, w

def timed_plain(exe, stats_on, reps=5):
    """matched/seg64 with stats on vs off — residual-overhead check."""
    walls = []
    for _ in range(reps):
        env = dict(os.environ)
        env["MML_CHAN_SEG"] = "64"
        if stats_on: env["MML_CHAN_STATS"] = "1"
        t = time.monotonic()
        p = subprocess.run([exe, N, "0", "0"], capture_output=True,
                           env=env)
        walls.append(time.monotonic() - t)
        if p.returncode != 0: return None
    return statistics.median(walls)

def main():
    exe = build()
    digests = {}
    invalid = 0
    print(f"{'rate':12} {'seg':>5} {'msgs_pk':>9} {'slots_pk':>9} "
          f"{'bytes_pk':>9} {'bp_msgs':>9} {'bp_slots':>9} "
          f"{'occ':>6} {'rss_MB':>7}")
    for rate in RATES:
        for seg in SEGS:
            rc, dig, rss, st, w = run(exe, rate, seg)
            digests.setdefault(rate[0], set()).add(dig)
            ok = (rc == 0 and rss is not None and st is not None)
            if not ok:
                invalid += 1
                print(f"{rate[0]:12} {seg:>5} INVALID "
                      f"(rc={rc} stats={'y' if st else 'MISSING'} "
                      f"rss={'y' if rss else 'MISSING'})")
                continue
            mp, sp, bp, bpm, bps, occ = st
            print(f"{rate[0]:12} {seg:>5} {mp:>9} {sp:>9} "
                  f"{bp:>9} {bpm:>9} {bps:>9} {occ:>6} "
                  f"{rss/1024:>7.1f}")
    bad = {k: v for k, v in digests.items() if len(v) != 1}
    if bad:
        print(f"!! digest mismatch across segs: {bad}")
        invalid += 1
    # residual-overhead check: stats on vs off, matched/seg64
    off = timed_plain(exe, False)
    on = timed_plain(exe, True)
    if off and on:
        print(f"stats overhead (matched/seg64, n={N}, 5-rep "
              f"median): off={off:.3f}s on={on:.3f}s "
              f"ratio={on/off:.3f}")
    else:
        print("stats overhead check FAILED")
        invalid += 1
    if invalid:
        sys.exit(1)

if __name__ == "__main__":
    main()
