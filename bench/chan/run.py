#!/usr/bin/env python3
"""CHAN-001 — segmented channel experiment.

Builds each .lang source through ourc + freshly-compiled runtime
objects, then times it under MML_CHAN_SEG ∈ {0,16,64,256} (0 = the
legacy per-message queue). C/C++/Rust twins run alongside as the
baseline. Every rep's stdout digest must match the cell's reference
or the rep is invalid. Stats: median + seeded bootstrap 95% CI.
"""
import hashlib, os, random, re, statistics, subprocess, sys, time

ROOT = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(ROOT))
SRC = os.path.join(ROOT, "src")
OUT = os.path.join(ROOT, "build")
os.makedirs(OUT, exist_ok=True)
OURC = os.path.join(REPO, "target", "release", "ourc")
RUNTIME = os.path.join(REPO, "compiler", "runtime")
BSD = sys.platform == "darwin"
CC = "clang"
SEED = int(os.environ.get("SEED", "0x5EED"), 0)
REPS = int(os.environ.get("REPS", "9"))
WARMUP = int(os.environ.get("WARMUP", "1"))
ONLY = os.environ.get("ONLY")
SEGS = [0, 16, 64, 256]
N = os.environ.get("N", "8000000")

EXPS = {
    # name: [(impl, source, seg-env-driven)]
    "prodcons":    [("seg", "prodcons.lang", True),
                    ("c", "../src/prodcons.c", False),
                    ("rs", "../src/prodcons.rs", False)],
    "prodcons64":  [("seg", "prodcons64.lang", True)],
    "prodcons_2p": [("seg", "prodcons_2p.lang", True)],
}

def checked(cmd, what):
    r = subprocess.run(cmd, capture_output=True)
    if r.returncode != 0:
        print(f"BUILD FAIL {what}: {r.stderr.decode()[:400]}")
        sys.exit(1)

def rt_objs():
    objs = []
    for s in ("mmlrt_common.c", "mmlrt_posix.c"):
        o = os.path.join(OUT, s.replace(".c", ".o"))
        checked([CC, "-O2", "-c", os.path.join(RUNTIME, s), "-o", o], s)
        objs.append(o)
    return objs

def build_lang(src, stem, objs):
    ll = os.path.join(OUT, stem + ".ll")
    r = subprocess.run([OURC, src, "--show-llvm", "-o",
                        os.path.join(OUT, stem)],
                       capture_output=True, text=True)
    if r.returncode != 0:
        print(f"BUILD FAIL {src}: {r.stderr[:400]}"); sys.exit(1)
    open(ll, "w").write(r.stdout)
    exe = os.path.join(OUT, stem + ".bin")
    checked([CC, "-O2", "-x", "ir", "-c", ll, "-o", ll + ".o"], stem)
    checked([CC, "-O2", ll + ".o", *objs, "-o", exe], stem + " link")
    return exe

def build_c(src, stem):
    exe = os.path.join(OUT, stem + ".bin")
    checked([CC, "-O2", "-o", exe, src], stem)
    return exe

def build_rs(src, stem):
    exe = os.path.join(OUT, stem + ".bin")
    checked(["rustc", "-O", "-o", exe, src], stem)
    return exe

def timed(exe, args, env=None):
    e = dict(os.environ)
    if env: e.update(env)
    t = time.monotonic()
    if BSD:
        cmd = ["/usr/bin/time", "-l", exe, *args]
    else:
        cmd = ["/usr/bin/time", "-v", exe, *args]
    p = subprocess.run(cmd, capture_output=True, env=e)
    w = time.monotonic() - t
    # peak RSS of the workload process, normalized to KB
    # (darwin `time -l` reports bytes; linux `time -v` reports KB)
    m = re.search(rb"maximum resident set size[:\s]+(\d+)" if not BSD
                  else rb"(\d+)\s+maximum resident set size", p.stderr)
    rss = int(m.group(1)) // (1024 if BSD else 1) if m else 0
    return w, p, rss

def boot_ci(vals, rng, resamples=10000):
    n = len(vals)
    meds = sorted(statistics.median(
        [vals[rng.randrange(n)] for _ in range(n)])
        for _ in range(resamples))
    def pct(q):
        k = (n-1)*q if False else (len(meds)-1)*q
        lo, hi = int(k), min(int(k)+1, len(meds)-1)
        f = k - lo
        return meds[lo]*(1-f) + meds[hi]*f
    return pct(0.025), pct(0.975)

def main():
    objs = rt_objs()
    rng = random.Random(SEED)
    any_invalid = False
    for exp, impls in EXPS.items():
        if ONLY and ONLY not in exp: continue
        for impl, src, seg_env in impls:
            if impl == "seg":
                exe = build_lang(os.path.join(SRC, src), f"{exp}_{impl}", objs)
            elif impl == "c":
                exe = build_c(os.path.join(ROOT, src), f"{exp}_{impl}")
            elif impl == "rs":
                exe = build_rs(os.path.join(ROOT, src), f"{exp}_{impl}")
            variants = SEGS if seg_env else [None]
            cells = {seg: dict(walls=[], digest=None, bad=0)
                     for seg in variants}
            for i in range(WARMUP):
                for seg in variants:
                    timed(exe, [N], {"MML_CHAN_SEG": str(seg)}
                          if seg is not None else None)
            # interleaved rounds — variants shuffle per round so
            # thermal/background drift hits every variant alike
            for r in range(REPS):
                order = list(variants)
                rng.shuffle(order)
                for seg in order:
                    c = cells[seg]
                    w, p, rss = timed(exe, [N],
                                      {"MML_CHAN_SEG": str(seg)}
                                      if seg is not None else None)
                    d = hashlib.sha256(p.stdout).hexdigest()
                    if p.returncode != 0 or (c["digest"] and
                                             d != c["digest"]):
                        c["bad"] += 1
                    else:
                        c["digest"] = d
                        c["walls"].append(w)
                        c["rss"] = max(c.get("rss", 0), rss)
            # validity: EVERY rep must pass in EVERY variant, and
            # all variants must produce identical output digests
            digests = {c["digest"] for c in cells.values()
                       if c["digest"]}
            exp_ok = (len(digests) == 1 and
                      all(c["bad"] == 0 and len(c["walls"]) == REPS
                          for c in cells.values()))
            if not exp_ok:
                # suppress all publishable metrics for the
                # experiment — no timing summary is meaningful
                # when reps failed or variants disagree
                any_invalid = True
                print(f"{exp}.{impl:28} INVALID EXPERIMENT — "
                      f"{len(digests)} distinct digests, "
                      f"reps failed in "
                      f"{sum(1 for c in cells.values() if c['bad'])}"
                      f" variant(s)")
                continue
            for seg in variants:
                c = cells[seg]
                tag = f"{exp}.{impl}" +                     (f".seg{seg}" if seg is not None else "")
                med = statistics.median(c["walls"])
                lo, hi = boot_ci(c["walls"], rng)
                qalloc = ("n/a" if seg is None else
                          f"~1/{seg}/msg + payload" if seg else
                          "1/msg + payload")
                thr = int(N) / med / 1e6
                print(f"{tag:28} med={med:.3f}s [{lo:.3f}–{hi:.3f}]  "
                      f"{thr:.1f}Mpkt/s  peak_rss={c['rss']}KB  "
                      f"queue_allocs={qalloc}")
    if any_invalid:
        sys.exit(1)

if __name__ == "__main__":
    main()
