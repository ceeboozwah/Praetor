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
    p = subprocess.run([exe, *args], capture_output=True, env=e)
    return time.monotonic() - t, p

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
            for seg in variants:
                tag = f"{exp}.{impl}" + (f".seg{seg}" if seg is not None else "")
                walls, digest, bad = [], None, 0
                for i in range(WARMUP):
                    w, p = timed(exe, [N], {"MML_CHAN_SEG": str(seg)} if seg is not None else None)
                for i in range(REPS):
                    w, p = timed(exe, [N], {"MML_CHAN_SEG": str(seg)} if seg is not None else None)
                    d = hashlib.sha256(p.stdout).hexdigest()
                    if p.returncode != 0 or (digest and d != digest):
                        bad += 1
                    else:
                        digest = d
                        walls.append(w)
                if not walls:
                    print(f"{tag:28} INVALID (all reps failed)")
                    continue
                med = statistics.median(walls)
                lo, hi = boot_ci(walls, rng)
                qalloc = "n/a"
                if seg is not None and seg:
                    qalloc = f"~1/{seg}/msg + payload"
                elif seg == 0:
                    qalloc = "1/msg + payload"
                flag = f"  !! {bad} bad reps" if bad else ""
                print(f"{tag:28} med={med:.3f}s [{lo:.3f}–{hi:.3f}]  "
                      f"queue_allocs={qalloc}{flag}")

if __name__ == "__main__":
    main()
