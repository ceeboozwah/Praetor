#!/usr/bin/env python3
"""bench/bce/run.py — Bounds-Check Elimination / safety-cost suite.

Per experiment: Praetor source + one or more C twins (identical work,
matched access order — verified by output digest on EVERY rep).

Check pipeline evidence — three stages per experiment:
  frontend.ll   `call void @mml_bounds_fail` sites OURC emits
  opt.ll        sites surviving `clang -O2 -emit-llvm` on that IR
  disasm        call sites to mml_bounds_fail in the final binary
That distinguishes Praetor's proof-driven elision from LLVM's own.

Validity model (PraetorBench 1.1): a cell is valid iff EVERY rep exits
0 and produces the experiment's shared output digest. Stats: median +
seeded bootstrap 95% CI; ratios are medians-of-resamples.

env: REPS (default 9), WARMUP (1), SCALE=sm|lg (lg), ONLY=<substr>
artifacts: bench/bce/out/<ts>/{raw.csv,stages.json,report.md}
"""
import csv, hashlib, json, os, platform, random, re, statistics, \
    subprocess, sys, time

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

# experiment -> {"srcs": [(impl, file)], "args": {scale: argv}}
# impl kinds: *.lang = ours (3-stage evidence), *.c = C twin.
# C twins carry matched access order (scan_rev.c is the reverse twin).
EXPS = {
    "scan_guarded": {"srcs": [("ours", "scan_guarded.lang"),
                            ("c", "scan.c")],
                     "args": {"sm": "4000000", "lg": "32000000"}},
    "scan_eq":      {"srcs": [("ours", "scan_eq.lang"),
                            ("c", "scan.c")],
                     "args": {"sm": "4000000", "lg": "32000000"}},
    "scan_opaque":  {"srcs": [("ours", "scan_opaque.lang"),
                            ("c", "scan.c")],
                     "args": {"sm": "4000000", "lg": "32000000"}},
    "scan_rev":     {"srcs": [("ours", "scan_rev.lang"),
                            ("c", "scan_rev.c")],
                     "args": {"sm": "4000000", "lg": "32000000"}},
    "scan_hot_g":   {"srcs": [("ours", "scan_hot_g.lang"),
                            ("c", "scan_hot.c")],
                     "args": {"sm": "2000", "lg": "16000"}},
    "scan_hot_k":   {"srcs": [("ours", "scan_hot_k.lang"),
                            ("c", "scan_hot_rev.c")],
                     "args": {"sm": "2000", "lg": "16000"}},
    "idx_guarded":  {"srcs": [("ours", "idx_guarded.lang"),
                            ("c", "idx.c")],
                     "args": {"sm": "2000000", "lg": "16000000"}},
    "idx_bare":     {"srcs": [("ours", "idx_bare.lang"),
                            ("c", "idx.c")],
                     "args": {"sm": "2000000", "lg": "16000000"}},
    "slice_mk":     {"srcs": [("ours", "slice_mk.lang"),
                            ("c", "slice_mk.c"),
                            ("c_chk", "slice_mk_chk.c")],
                     "args": {"sm": "1000000", "lg": "8000000"}},
    "auth_pass":    {"srcs": [("ours", "auth_pass.lang"),
                            ("c", "auth_pass.c")],
                     "args": {"sm": "8000000", "lg": "64000000"}},
    "own_move":     {"srcs": [("ours", "own_move.lang"),
                            ("c", "own_move.c")],
                     "args": {"sm": "500000", "lg": "4000000"}},
    "idx_mask":     {"srcs": [("ours", "idx_mask.lang"),
                            ("c", "idx.c")],
                     "args": {"sm": "2000000", "lg": "16000000"}},
    "ptrchase":     {"srcs": [("ours", "ptrchase.lang"),
                            ("c", "ptrchase.c")],
                     "args": {"sm": "1000000", "lg": "8000000"}},
    "auth_abi":     {"srcs": [("ours", "auth_abi.lang"),
                            ("c", "auth_abi.c"),
                            ("c_np", "auth_abi_np.c")],
                     "args": {"sm": "50000", "lg": "400000"}},
    "alloc_loop":   {"srcs": [("ours", "alloc_loop.lang"),
                            ("c", "alloc_loop.c")],
                     "args": {"sm": "200000", "lg": "2000000"}},
}
# expected mml_bounds_fail sites in the named frontend fn — evidence,
# reported (and flagged) on mismatch. None = report only.
EXPECT = {
    "scan_guarded": {"scan": 0}, "scan_eq": {"scan": 0},
    "idx_guarded": {"probe": 0}, "scan_hot_g": {"scan": 0},
    "idx_mask": {"mml_main": 1},
}


def sh(cmd, **kw):
    return subprocess.run(cmd, capture_output=True, timeout=600, **kw)


def run_timed(exe, arg):
    t0 = time.perf_counter()
    p = subprocess.run([exe, arg], capture_output=True, timeout=600)
    wall = time.perf_counter() - t0
    return dict(wall_s=wall, rc=p.returncode,
                out_sha=hashlib.sha256(p.stdout).hexdigest())


def ll_sites(path):
    """fn -> count of `call void @mml_bounds_fail` in an .ll file."""
    counts, cur = {}, None
    if not path or not os.path.exists(path):
        return counts
    for line in open(path, errors="replace"):
        m = re.match(r"define\s+.*@([\w.]+)\(", line)
        if m:
            cur = m.group(1)
            counts.setdefault(cur, 0)
        elif cur and "call void @mml_bounds_fail" in line:
            counts[cur] += 1
    return {k: v for k, v in counts.items() if v}


def disasm(exe):
    """Full disassembly text, or None. macOS objdump mislabels stubs
    (every extern call reads <_firstsym+off>) — otool is the correct
    disassembler there; objdump elsewhere."""
    tools = (["otool", "-tv", exe], ["objdump", "-d", exe]) \
        if sys.platform == "darwin" else \
        (["objdump", "-d", exe], ["otool", "-tv", exe])
    for tool in tools:
        r = sh(tool, text=True)
        if r.returncode == 0 and r.stdout:
            return r.stdout
    return None

_CALL = re.compile(r"\b(bl|callq?|jmp)\b")
_LABEL = re.compile(r"^\s*(?:[0-9a-f]+\s*)?<([^\s>]+)>:|^(_[\w.]+):")

def asm_sites_text(text, target="mml_bounds_fail"):
    """fn -> count of CALL INSTRUCTIONS targeting `target`.
    Only `bl`/`call`/`jmp` lines naming the symbol count — a symbol
    definition or literal-pool reference does not. Handles objdump
    (`<fn>:` labels) and otool (`_fn:` labels) formats."""
    sites, cur = {}, None
    for line in text.splitlines():
        lab = _LABEL.match(line)
        if lab:
            cur = (lab.group(1) or lab.group(2) or cur)
            continue
        if target in line and _CALL.search(line):
            sites[cur or "?"] = sites.get(cur or "?", 0) + 1
    return sites


def boot_median(vals, rng, n=2000):
    if not vals:
        return (None, None, None)
    med = statistics.median(vals)
    if len(vals) < 2:
        return (med, None, None)
    meds = sorted(statistics.median(
        rng.choices(vals, k=len(vals))) for _ in range(n))
    return med, meds[int(n * 0.025)], meds[int(n * 0.975)]


def boot_ratio(a, b, rng, n=2000):
    """median ratio a/b with CI over resampled pairs."""
    if len(a) < 2 or len(b) < 2:
        return None
    rs = sorted(statistics.median(rng.choices(a, k=len(a))) /
                statistics.median(rng.choices(b, k=len(b)))
                for _ in range(n))
    return rs[n // 2], rs[int(n * 0.025)], rs[int(n * 0.975)]


def build_ours(src, exe):
    """ourc --show-llvm -> frontend .ll; clang -O2 -> opt.ll, .o, exe.
    Returns (fe_ll, opt_ll, err)."""
    fe_ll, opt_ll, obj = exe + ".ll", exe + ".opt.ll", exe + ".o"
    r = sh([OURC, src, "--show-llvm", "-o", exe + ".ourc"], text=True)
    if r.returncode != 0:
        return None, None, r.stderr[:600]
    with open(fe_ll, "w") as f:
        f.write(r.stdout)
    for cmd in ([CC, "-O2", "-S", "-emit-llvm", fe_ll, "-o", opt_ll],
                [CC, "-O2", "-c", fe_ll, "-o", obj],
                [CC, "-O2", obj, *RT_OBJS, *RT_LDLIBS, "-o", exe]):
        r = sh(cmd, text=True)
        if r.returncode != 0:
            return None, None, f"clang: {r.stderr[:400]}"
    return fe_ll, opt_ll, None


def main():
    only = os.environ.get("ONLY")
    rng = random.Random(0xBCE)
    raw, evidence, flags, cells = [], {}, [], {}

    for s in MMLRT_SRCS:
        o = os.path.join(OUT, s.replace(".c", ".o"))
        r = sh([CC, "-O2", "-c", os.path.join(RUNTIME, s), "-o", o])
        if r.returncode != 0:
            sys.exit(f"bench/bce: runtime {s}: {r.stderr[:400]}")
        RT_OBJS.append(o)

    for exp in sorted(EXPS):
        if only and only not in exp:
            continue
        arg = EXPS[exp]["args"][SCALE]
        ev = {"arg": arg, "stages": {}, "impls": {}}
        built = {}
        for impl, fn in EXPS[exp]["srcs"]:
            src = os.path.join(SRC, fn)
            exe = os.path.join(OUT, f"{exp}_{impl}")
            if fn.endswith(".lang"):
                fe, opt, err = build_ours(src, exe)
                if not fe:
                    ev["impls"][impl] = {"build": "failed",
                                         "err": err}
                    continue
                dasm = disasm(exe) or ""
                ev["stages"] = {"frontend": ll_sites(fe),
                                "optimized": ll_sites(opt),
                                "disasm": asm_sites_text(dasm)}
                ev["files"] = {"fe_ll": fe, "opt_ll": opt}
                ev["impls"][impl] = {"built": True}
                with open(exe + ".asm", "w") as f:
                    f.write(dasm)
            else:
                r = sh([CC, "-O2", "-o", exe, src], text=True)
                if r.returncode != 0:
                    ev["impls"][impl] = {"build": "failed",
                                         "err": r.stderr[:200]}
                    continue
                # alloc control: the C twin's allocation cycle must
                # survive into machine code, or the ratio is void.
                dasm = disasm(exe) or ""
                ev["impls"][impl] = {
                    "built": True,
                    "asm_alloc_calls":
                        len(re.findall(r"\b(bl|callq?)\b.*malloc", dasm)),
                    "asm_free_calls":
                        len(re.findall(r"\b(bl|callq?)\b.*free", dasm))}
                if exp == "alloc_loop" and not (
                        ev["impls"][impl]["asm_alloc_calls"]
                        and ev["impls"][impl]["asm_free_calls"]):
                    flags.append(
                        f"{exp}/{impl}: C alloc cycle eliminated — "
                        f"ratio measures nothing")
                    ev["impls"][impl]["built"] = False
                    continue
            built[impl] = exe
            ev["impls"][impl].setdefault("built", True)
            ev["impls"][impl]["built"] = True
        evidence[exp] = ev
        if len(built) != len(EXPS[exp]["srcs"]):
            flags.append(f"{exp}: incomplete impls")
            print(f"{exp}: BUILD FAILED {ev['impls']}")
            continue

        # measured reps — interleaved seeded order; EVERY rep is
        # validated (rc==0 + digest); one bad rep invalidates the cell
        ref_digest = None
        for _ in range(WARMUP):
            for impl, exe in built.items():
                run_timed(exe, arg)
        order = [(impl, exe) for impl, exe in built.items()] * REPS
        rng.shuffle(order)
        for impl, exe in order:
            m = run_timed(exe, arg)
            raw.append(dict(exp=exp, impl=impl, scale=SCALE, arg=arg,
                            **m))
        for impl in built:
            reps = [r for r in raw
                    if r["exp"] == exp and r["impl"] == impl]
            ok = (len(reps) == len(order) // len(built)
                  and all(r["rc"] == 0 for r in reps))
            digs = {r["out_sha"] for r in reps}
            if len(digs) != 1:
                ok = False
            elif ref_digest is None:
                ref_digest = digs.pop()
            else:
                ok = ok and digs == {ref_digest}
            cells[(exp, impl)] = dict(
                valid=ok, n=len(reps),
                walls=[r["wall_s"] for r in reps])
            if not ok:
                flags.append(f"{exp}/{impl}: invalid cell "
                             f"(n={len(reps)} rc/digest)")

        # report
        line = f"{exp:14}"
        meds = {}
        for impl in built:
            c = cells[(exp, impl)]
            if not c["valid"]:
                line += f"  {impl}=INVALID"
                continue
            med, lo, hi = boot_median(c["walls"], random.Random(0xBCE))
            meds[impl] = (med, lo, hi)
            line += (f"  {impl}={med:.4f}s[{lo:.3f}–{hi:.3f}]")
        if "ours" in meds and "c" in meds:
            rr = boot_ratio(cells[(exp, "ours")]["walls"],
                            cells[(exp, "c")]["walls"],
                            random.Random(0xBCE))
            line += (f"  ratio={rr[0]:.3f}[{rr[1]:.3f}–{rr[2]:.3f}]"
                     if rr else "")
        if "c_chk" in meds:
            rr = boot_ratio(cells[(exp, "c_chk")]["walls"],
                            cells[(exp, "c")]["walls"],
                            random.Random(0xBCE))
            line += (f"  chk/unchk={rr[0]:.3f}[{rr[1]:.3f}–{rr[2]:.3f}]"
                     if rr else "")
        st = ev["stages"]
        if st:
            fe_tot = sum(st["frontend"].values())
            opt_tot = sum(st["optimized"].values())
            asm_tot = sum(st["disasm"].values())
            line += (f"  sites fe={fe_tot} opt={opt_tot} "
                     f"asm={asm_tot}")
        if exp in EXPECT:
            for fn, want in EXPECT[exp].items():
                got = st["frontend"].get(fn, 0)
                if got != want:
                    flags.append(f"{exp}/{fn}: fe {got}!={want}")
                    line += f"  !! {fn}={got}≠{want}"
        print(line)

    ts = time.strftime("%Y%m%d-%H%M%S")
    rd = os.path.join(REPORTS, ts)
    os.makedirs(rd, exist_ok=True)
    with open(os.path.join(rd, "raw.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["exp", "impl", "scale", "arg",
                                        "wall_s", "rc", "out_sha"])
        w.writeheader()
        w.writerows(raw)
    # evidence artifacts: the actual .ll/.opt.ll/.asm files ship with
    # the run dir so a reviewer can inspect them, not just our counts
    import shutil
    for exp, ev in evidence.items():
        for k, p in (ev.get("files") or {}).items():
            if os.path.exists(p):
                shutil.copy(p, os.path.join(rd,
                            f"{exp}.{k.replace('_ll', '.ll')}"))
        asm = os.path.join(OUT, f"{exp}_ours.asm")
        if os.path.exists(asm):
            shutil.copy(asm, os.path.join(rd, f"{exp}.asm"))
        for impl in ev["impls"]:
            e = os.path.join(OUT, f"{exp}_{impl}")
            if impl != "ours" and os.path.exists(e):
                d = disasm(e) or ""
                with open(os.path.join(rd, f"{exp}_{impl}.asm"),
                          "w") as f:
                    f.write(d)

    with open(os.path.join(rd, "stages.json"), "w") as f:
        json.dump(dict(suite="PraetorBench-BCE-0.2", scale=SCALE,
                       reps=REPS, os=platform.system(),
                       arch=platform.machine(),
                       flags=flags, evidence=evidence), f, indent=1)
    print(f"\nwrote {rd}" +
          (f"  {len(flags)} FLAG(S): {flags}" if flags
           else "  (all valid, expectations met)"))
    return 1 if flags else 0


if __name__ == "__main__":
    sys.exit(main() or 0)
