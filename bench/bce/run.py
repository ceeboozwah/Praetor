#!/usr/bin/env python3
"""bench/bce/run.py — Bounds-Check Elimination / safety-cost suite.

For each experiment: compile the Praetor source (the emitted .ll is
parsed for `mml_bounds_fail` call sites — the retained-check count),
compile the unchecked C twin, run both REPS times, verify identical
output. Report: check sites per fn + wall-time ratio ours/C.

A ratio ~1.0 on an elided experiment = the proof reached zero-cost
codegen. The delta between scan_opaque (kept check) and scan_guarded
(elided) IS the residual safety cost per element.

env: REPS (default 9), WARMUP (default 1), SCALE=sm|lg (default lg)
artifacts: bench/out-bce/<ts>/{raw.csv,ir.json,report.md}
"""
import csv, hashlib, json, os, platform, random, re, subprocess, \
    sys, time

ROOT = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(ROOT))
SRC = os.path.join(ROOT, "src")
OUT = os.path.join(ROOT, "build")
REPORTS = os.path.join(ROOT, "out")
os.makedirs(OUT, exist_ok=True)
os.makedirs(REPORTS, exist_ok=True)

OURC = os.path.join(REPO, "target", "release", "ourc")
RUNTIME = os.path.join(REPO, "compiler", "runtime")
if sys.platform == "win32":
    MMLRT_SRCS = ["mmlrt_common.c", "mmlrt_win32.c"]
    RT_LDLIBS = ["-fuse-ld=lld", "-lws2_32", "-Wl,/stack:8388608"]
else:
    MMLRT_SRCS = ["mmlrt_common.c", "mmlrt_posix.c"]
    RT_LDLIBS = [] if sys.platform == "darwin" else ["-lpthread"]
CC = "clang"
RT_OBJS = []
REPS = int(os.environ.get("REPS") or "0") or 9
WARMUP = int(os.environ.get("WARMUP") or "0") or 1
SCALE = os.environ.get("SCALE", "lg")

# experiment -> (c twin, argv by scale)
EXPS = {
    "scan_guarded": ("scan.c",     {"sm": "4000000",  "lg": "32000000"}),
    "scan_eq":      ("scan.c",     {"sm": "4000000",  "lg": "32000000"}),
    "scan_opaque":  ("scan.c",     {"sm": "4000000",  "lg": "32000000"}),
    "scan_rev":     ("scan.c",     {"sm": "4000000",  "lg": "32000000"}),
    "idx_guarded":  ("idx.c",      {"sm": "2000000",  "lg": "16000000"}),
    "idx_bare":     ("idx.c",      {"sm": "2000000",  "lg": "16000000"}),
    "slice_mk":     ("slice_mk.c", {"sm": "1000000",  "lg": "8000000"}),
    "auth_pass":    ("auth_pass.c",{"sm": "8000000",  "lg": "64000000"}),
    "own_move":     ("own_move.c", {"sm": "500000",   "lg": "4000000"}),
    "scan_hot_g":   ("scan_hot.c", {"sm": "2000",     "lg": "16000"}),
    "scan_hot_k":   ("scan_hot.c", {"sm": "2000",     "lg": "16000"}),
}
# per-fn count of `call void @mml_bounds_fail` we EXPECT in the
# emitted IR for the hot function — evidence, not a gate. None =
# "report only". 0 = full elision expected (N46/N47 territory).
EXPECT = {
    "scan_guarded": {"scan": 0}, "scan_eq": {"scan": 0},
    "scan_opaque": None,  # kept check expected, count reported
    "scan_rev": None,
    "idx_guarded": {"probe": 0}, "idx_bare": None,
    "slice_mk": None, "auth_pass": None, "own_move": None,
    "scan_hot_g": {"scan": 0}, "scan_hot_k": None,
}


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, timeout=600, **kw)


def run_timed(exe, arg):
    t0 = time.perf_counter()
    p = subprocess.run([exe, arg], capture_output=True, timeout=600)
    wall = time.perf_counter() - t0
    return dict(wall_s=wall, rc=p.returncode,
                out_sha=hashlib.sha256(p.stdout).hexdigest(),
                out=p.stdout.decode(errors="replace").strip())


def bounds_sites(ll_path):
    """fn name -> count of `call void @mml_bounds_fail` in its body."""
    counts, cur = {}, None
    if not os.path.exists(ll_path):
        return counts
    for line in open(ll_path, errors="replace"):
        m = re.match(r"define\s+.*@([\w.]+)\(", line)
        if m:
            cur = m.group(1)
            counts.setdefault(cur, 0)
        elif cur and "call void @mml_bounds_fail" in line:
            counts[cur] += 1
    return counts


def build_ours(src, exe):
    """ourc --show-llvm -> .ll text; clang -O2 .ll -> .o -> exe.
    Returns (ll_path, None) or (None, err)."""
    ll = exe + ".ll"
    obj = exe + ".o"
    r = sh([OURC, src, "--show-llvm", "-o", exe + ".ourc"], text=True)
    if r.returncode != 0:
        return None, r.stderr[:600]
    with open(ll, "w") as f:
        f.write(r.stdout)
    for cmd in ([CC, "-O2", "-c", ll, "-o", obj],
                [CC, "-O2", obj, *RT_OBJS, *RT_LDLIBS, "-o", exe]):
        r = sh(cmd, text=True)
        if r.returncode != 0:
            return None, f"clang: {r.stderr[:400]}"
    return ll, None


def main():
    only = os.environ.get("ONLY")
    rows, ir_evidence, checks = [], {}, []

    # runtime objects — recompiled every run (evidence, not cache)
    for s in MMLRT_SRCS:
        o = os.path.join(OUT, s.replace(".c", ".o"))
        r = sh([CC, "-O2", "-c", os.path.join(RUNTIME, s), "-o", o])
        if r.returncode != 0:
            sys.exit(f"bench/bce: runtime {s}: {r.stderr[:400]}")
        RT_OBJS.append(o)

    for exp in sorted(EXPS):
        if only and only not in exp:
            continue
        ctwin, args = EXPS[exp]
        arg = args[SCALE]
        lang_src = os.path.join(SRC, f"{exp}.lang")
        exe = os.path.join(OUT, exp)

        # compile ours; the emitted .ll is parsed for retained checks
        ll, err = build_ours(lang_src, exe)
        if not ll:
            print(f"{exp}: ourc FAILED\n{err}")
            ir_evidence[exp] = {"build": "failed"}
            continue
        sites = bounds_sites(ll)
        ir_evidence[exp] = {"sites": sites,
                            "total": sum(sites.values())}

        cexe = os.path.join(OUT, exp + "_c")
        r = sh([CC, "-O2", "-o", cexe, os.path.join(SRC, ctwin)])
        if r.returncode != 0:
            print(f"{exp}: clang FAILED\n{r.stderr.decode()[:200]}")
            continue

        # verify identical output before timing
        a, b = run_timed(exe, arg), run_timed(cexe, arg)
        if a["rc"] != 0 or b["rc"] != 0 or a["out_sha"] != b["out_sha"]:
            print(f"{exp}: OUTPUT MISMATCH ours={a['out']!r} "
                  f"c={b['out']!r} rc={a['rc']},{b['rc']}")
            ir_evidence[exp]["verified"] = False
            continue
        ir_evidence[exp]["verified"] = True

        for _ in range(WARMUP):
            run_timed(exe, arg)
            run_timed(cexe, arg)
        rng = random.Random(0xBCE)
        order = [("ours", exe), ("c", cexe)] * REPS
        rng.shuffle(order)
        for impl, x in order:
            m = run_timed(x, arg)
            rows.append(dict(exp=exp, impl=impl, arg=arg, **{
                k: v for k, v in m.items() if k != "out"}))

        ours = sorted(r["wall_s"] for r in rows
                      if r["exp"] == exp and r["impl"] == "ours")
        cref = sorted(r["wall_s"] for r in rows
                      if r["exp"] == exp and r["impl"] == "c")
        mo, mc = ours[len(ours) // 2], cref[len(cref) // 2]
        exp_sites = EXPECT.get(exp)
        flag = ""
        if exp_sites:
            for fn, want in exp_sites.items():
                got = sites.get(fn, 0)
                if got != want:
                    flag = f"  !! {fn}: {got} sites, expected {want}"
                    checks.append(f"{exp}/{fn}: {got}!={want}")
        print(f"{exp:14} sites={ir_evidence[exp]['total']:>2} "
              f"ours={mo:.3f}s c={mc:.3f}s "
              f"ratio={mo / mc:.3f}{flag}")

    ts = time.strftime("%Y%m%d-%H%M%S")
    rd = os.path.join(REPORTS, ts)
    os.makedirs(rd, exist_ok=True)
    with open(os.path.join(rd, "raw.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["exp", "impl", "arg",
                                        "wall_s", "rc", "out_sha"])
        w.writeheader()
        w.writerows(rows)
    with open(os.path.join(rd, "ir.json"), "w") as f:
        json.dump(dict(suite="PraetorBench-BCE-0.1", scale=SCALE,
                       reps=REPS, os=platform.system(),
                       arch=platform.machine(),
                       evidence=ir_evidence), f, indent=1)
    print(f"\nwrote {rd}" + (f"  ({len(checks)} expectation mismatches)"
                            if checks else "  (expectations met)"))
    return 1 if checks else 0


if __name__ == "__main__":
    sys.exit(main() or 0)
