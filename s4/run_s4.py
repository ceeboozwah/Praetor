#!/usr/bin/env python3
"""CHAN-004 S4 — validation harness.

Runs every scenario in a generated corpus through:
  1. stage-0 checker (`ourc`)           — verdict + diagnostics
  2. stage-1 self-hosted checker (`our`)— verdict + diagnostics + run
  3. oracle build                        — instrumented runtime tracks
     every user allocation: double-free, unowned-free, free-null,
     and leaks are reported as `ORACLE:` stderr lines
  4. ASan build (POSIX, best-effort)     — deterministic UAF/leak
     detection via -fsanitize=address on the emitted .ll
  5. timeout                             — blocking programs must
     declare `expect: timeout`; every run is bounded so deadlocks
     terminate the harness instead of hanging CI

Results are recorded per program as JSONL; a failing program is one
whose actual behavior disagrees with the manifest's expected verdict
or whose oracle/sanitizer pass reports a violation.
"""
import argparse
import json
import os
import re
import shutil
import subprocess
import sys

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def run(cmd, timeout, env=None, cwd=None):
    try:
        p = subprocess.run(cmd, capture_output=True, text=True,
                           timeout=timeout, env=env, cwd=cwd)
        return ("ok", p.returncode, p.stdout, p.stderr)
    except subprocess.TimeoutExpired:
        return ("timeout", None, "", "")


def build_oracle_rt(work):
    """Patched runtime copy: mml_alloc/mml_free renamed to *_raw,
    the tracking wrappers appended. Regenerated from pristine
    sources every run so upstream runtime changes propagate."""
    src_dir = os.path.join(REPO, "compiler", "runtime")
    dst = os.path.join(work, "oracle-rt")
    os.makedirs(dst, exist_ok=True)
    for name in ("mmlrt.h", "mmlrt_posix.c", "mmlrt_win32.c"):
        shutil.copy2(os.path.join(src_dir, name), dst)
    common = open(os.path.join(src_dir, "mmlrt_common.c")).read()
    a = common.replace("void *mml_alloc(long long size) {",
                       "void *mml_alloc_raw(long long size) {")
    b = a.replace("void mml_free(void *p) {",
                  "void mml_free_raw(void *p) {")
    assert "mml_alloc_raw" in b and "mml_free_raw" in b, \
        "oracle patch anchor lost — mml_alloc/mml_free signatures changed"
    b += open(os.path.join(REPO, "s4", "oracle_tail.c")).read()
    open(os.path.join(dst, "mmlrt_common.c"), "w").write(b)
    return dst


def cc_for(work):
    for c in ("clang", "cc", "gcc"):
        if shutil.which(c):
            return c
    return None


def build_asan_rt(work, cc):
    """Runtime objects for the ASan link (malloc interception is
    link-time, so plain objects suffice)."""
    d = os.path.join(work, "asan-rt")
    os.makedirs(d, exist_ok=True)
    plat = "mmlrt_win32.c" if sys.platform == "win32" else "mmlrt_posix.c"
    objs = []
    for name in ("mmlrt_common.c", plat):
        o = os.path.join(d, name.replace(".c", ".o"))
        r = run([cc, "-O1", "-c",
                 os.path.join(REPO, "compiler", "runtime", name),
                 "-o", o], 60)
        if r[0] != "ok" or r[1] != 0:
            return None
        objs.append(o)
    return objs


def scenario_one(entry, args, oracle_rt, asan_objs, cc):
    src = os.path.join(args.corpus, entry["file"])
    stem = entry["file"][:-5]
    w = os.path.join(args.work, stem)
    os.makedirs(w, exist_ok=True)
    expect = entry["expect"]
    res = {"file": entry["file"], "expect": expect,
           "seed": entry.get("seed"), "family": entry.get("family")}
    fails = []

    # --- stage-0 verdict ---
    st, rc, so, se = run([args.s0, src, "-o", os.path.join(w, "s0")],
                         args.timeout)
    if st != "ok":
        res.update(s0_verdict="compile-timeout")
        fails.append("s0 compile timed out")
        return res, fails
    s0_diag = (se + so).strip()
    res["s0_verdict"] = "accept" if rc == 0 else "reject"
    res["s0_diag"] = s0_diag.splitlines()[0] if s0_diag else ""

    # --- stage-1 verdict (accept/reject must match stage-0) ---
    st, rc1, so1, se1 = run([args.s1, src, "-o", os.path.join(w, "s1bin")],
                            args.timeout)
    if st != "ok":
        res["s1_verdict"] = "compile-timeout"
        fails.append("s1 compile timed out")
    else:
        res["s1_verdict"] = "accept" if rc1 == 0 else "reject"
        s1_diag = (se1 + so1).strip()
        res["s1_diag"] = s1_diag.splitlines()[0] if s1_diag else ""
        if res["s1_verdict"] != res["s0_verdict"]:
            fails.append(
                f"verdict divergence: s0={res['s0_verdict']} "
                f"s1={res['s1_verdict']}")

    # --- expectation check on the verdict ---
    if expect.startswith("reject"):
        want = expect[len("reject:"):]
        if res["s0_verdict"] != "reject":
            fails.append(f"expected reject, s0 accepted")
        elif want and want.lower() not in s0_diag.lower():
            fails.append(f"reject diag missing '{want}': {res['s0_diag']}")
        return res, fails
    if res["s0_verdict"] != "accept":
        fails.append(f"expected accept, s0 rejected: {res['s0_diag']}")
        return res, fails

    # --- run the stage-0 binary ---
    s0_exe = os.path.join(w, stem)
    st, rc_run, so_r, se_r = run([s0_exe], args.timeout)
    res["s0_run"] = "timeout" if st == "timeout" else f"exit:{rc_run}"

    # --- oracle build + run ---
    env = dict(os.environ, OUR_RUNTIME=oracle_rt)
    st, rc_o, so_o, se_o = run(
        [args.s0, src, "-o", os.path.join(w, "ora")], args.timeout, env=env)
    oracle_flags = []
    if st == "ok" and rc_o == 0:
        ora_exe = os.path.join(w, stem)
        st, rc_or, so_or, se_or = run([ora_exe], args.timeout)
        res["oracle_run"] = ("timeout" if st == "timeout"
                             else f"exit:{rc_or}")
        oracle_flags = [l for l in se_or.splitlines()
                        if l.startswith("ORACLE")]
        res["oracle"] = oracle_flags
        if oracle_flags:
            fails.append(f"oracle: {oracle_flags}")
        if res["oracle_run"] != res["s0_run"]:
            fails.append(
                f"oracle run differs: {res['oracle_run']} "
                f"vs {res['s0_run']}")
    else:
        res["oracle_run"] = "build-failed"
        fails.append("oracle build failed")

    # --- ASan build + run (POSIX best-effort) ---
    ll = os.path.join(w, stem + ".ll")
    if asan_objs and cc and os.path.exists(ll):
        o = os.path.join(w, "asan.o")
        exe = os.path.join(w, "asan-bin")
        r1 = run([cc, "-fsanitize=address", "-O1", "-c", ll, "-o", o],
                 120)
        ok = r1[0] == "ok" and r1[1] == 0
        if ok:
            r2 = run([cc, "-fsanitize=address", o, *asan_objs,
                      "-o", exe] + (["-lpthread"]
                                    if sys.platform.startswith("linux")
                                    else []), 120)
            ok = r2[0] == "ok" and r2[1] == 0
        if ok:
            st, rc_a, so_a, se_a = run([exe], args.timeout)
            res["asan_run"] = ("timeout" if st == "timeout"
                               else f"exit:{rc_a}")
            asan_bad = [l for l in se_a.splitlines()
                        if "AddressSanitizer" in l
                        or "LeakSanitizer" in l]
            res["asan"] = asan_bad[:4]
            if asan_bad:
                fails.append(f"asan: {asan_bad[:2]}")
            if res["asan_run"] != res["s0_run"] and \
               expect != "timeout":
                fails.append(
                    f"asan run differs: {res['asan_run']} "
                    f"vs {res['s0_run']}")
        else:
            res["asan_run"] = "build-skipped"

    # --- stage-1 runtime ---
    s1_exe = os.path.join(w, "s1bin")
    if res.get("s1_verdict") == "accept" and os.path.exists(s1_exe):
        st, rc_s1, so_s1, se_s1 = run([s1_exe], args.timeout)
        res["s1_run"] = ("timeout" if st == "timeout"
                         else f"exit:{rc_s1}")
        if res["s1_run"] != res["s0_run"]:
            fails.append(
                f"s1 run differs: {res['s1_run']} vs {res['s0_run']}")

    # --- expected runtime verdict ---
    if expect == "timeout":
        if res["s0_run"] != "timeout":
            fails.append(f"expected timeout, got {res['s0_run']}")
    elif expect.startswith("exit:"):
        if res["s0_run"] != expect:
            fails.append(f"expected {expect}, got {res['s0_run']}")
    return res, fails


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--s0", default=os.path.join(REPO, "target",
                                               "release", "ourc"))
    ap.add_argument("--s1", default=os.path.join(REPO, "target",
                                               "our"))
    ap.add_argument("--corpus", required=True)
    ap.add_argument("--work", default="/tmp/s4work")
    ap.add_argument("--timeout", type=int, default=10)
    ap.add_argument("--no-asan", action="store_true")
    ap.add_argument("--filter", default=None)
    ap.add_argument("--results", default=None)
    args = ap.parse_args()

    entries = [json.loads(l) for l in
               open(os.path.join(args.corpus, "manifest.jsonl"))]
    if args.filter:
        entries = [e for e in entries if args.filter in e["file"]]
    os.makedirs(args.work, exist_ok=True)
    oracle_rt = build_oracle_rt(args.work)
    cc = cc_for(args.work) if not args.no_asan else None
    asan_objs = None
    if cc and sys.platform != "win32":
        asan_objs = build_asan_rt(args.work, cc)

    results = []
    nfail = 0
    for e in entries:
        res, fails = scenario_one(e, args, oracle_rt, asan_objs, cc)
        res["ok"] = not fails
        if fails:
            res["fails"] = fails
            nfail += 1
            print(f"FAIL {e['file']}: {'; '.join(fails)}")
        results.append(res)
    out = args.results or os.path.join(args.work, "results.jsonl")
    with open(out, "w") as f:
        for r in results:
            f.write(json.dumps(r) + "\n")
    print(f"{len(results) - nfail}/{len(results)} scenarios clean; "
          f"results -> {out}")
    sys.exit(1 if nfail else 0)


if __name__ == "__main__":
    main()
