#!/usr/bin/env python3
"""bench/selftest.py — harness correctness battery.

The harness enforces PraetorBench 1.0 validity rules; these tests
prove the enforcement works: failed builds, failed runs, missing
timing fields, digest mismatches, missing sources, and the paired
difference CI. Run: python3 bench/selftest.py
"""
import os, stat, sys, tempfile

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import harness as H

FAIL = []

def check(name, cond):
    print(("PASS " if cond else "FAIL ") + name)
    if not cond:
        FAIL.append(name)

def stub(body):
    """Executable shell stub; /usr/bin/time accepts it."""
    fd, p = tempfile.mkstemp(suffix=".sh")
    with os.fdopen(fd, "w") as f:
        f.write("#!/bin/sh\n" + body)
    os.chmod(p, os.stat(p).st_mode | stat.S_IXUSR)
    return p

def fake_cell(**kw):
    base = dict(work="w", lang="ours", mode="release", missing=False,
                build_error=False)
    base.update(kw)
    return base

def fake_row(**kw):
    base = dict(wall_s=0.1, exit_status=0, stdout_sha256="a",
                file_sha256=None, iteration=0)
    base.update(kw)
    return base

def main():
    H.REPS = 3  # smaller expectation for synthetic rows

    # 1. a crashing program is recorded, not dropped
    exe = stub("exit 7\n")
    m = H.measure_one(exe, [], [])
    check("failed run records exit_status", m["exit_status"] == 7)
    check("failed run has timing fields",
          "wall_s" in m and "rss_bytes" in m)

    # 2. a normal run digests stdout and parses timing
    exe = stub("echo hello\n")
    m = H.measure_one(exe, [], [])
    check("good run rc=0 + digest + wall",
          m["exit_status"] == 0 and len(m["stdout_sha256"]) == 64
          and m["wall_s"] is not None)

    # 3. __kernel_s self-report is captured
    exe = stub("echo __kernel_s=0.042\n")
    m = H.measure_one(exe, [], [])
    check("kernel_s captured", m["kernel_s"] == 0.042)

    # 4. declared output files are digested; missing => MISSING marker
    d = tempfile.mkdtemp()
    outp = os.path.join(d, "x.out")
    exe = stub(f"printf hi > {outp}\n")
    m = H.measure_one(exe, [], [outp])
    check("output file digested", m["file_sha256"] is not None)
    exe = stub("true\n")
    m2 = H.measure_one(exe, [], [outp + ".absent"])
    check("missing output => distinct digest",
          m2["file_sha256"] != m["file_sha256"])

    # 5. classify() verdicts
    c = fake_cell()
    gd = {("w", "release"): {("a", None)}}
    rows = [fake_row(iteration=i) for i in range(H.REPS)]
    check("all-good => yes", classify := H.classify(c, rows, gd) == "yes")
    check("build-failed",
          H.classify(fake_cell(build_error=True), [], gd) == "build-failed")
    check("missing-src",
          H.classify(fake_cell(missing=True), [], gd) == "missing-src")
    check("incomplete-reps",
          H.classify(c, rows[:-1], gd) == "incomplete-reps")
    bad = list(rows); bad[0] = fake_row(iteration=0, exit_status=3)
    check("run-failed", H.classify(c, bad, gd) == "run-failed")
    bad = list(rows); bad[0] = fake_row(iteration=0, wall_s=None)
    check("missing-timing", H.classify(c, bad, gd) == "missing-timing")
    bad = list(rows); bad[1] = fake_row(iteration=1, stdout_sha256="b")
    check("digest-mismatch",
          H.classify(c, bad, gd) == "digest-mismatch")
    gd2 = {("w", "release"): {("a", None), ("b", None)}}
    check("work-mismatch", H.classify(c, rows, gd2) == "work-mismatch")

    # 6. summarize + paired_diff_ci sanity
    s = H.summarize([1.0, 2.0, 3.0, 4.0])
    check("median", s["median_s"] == 2.5 and s["n"] == 4)
    import random
    a = [fake_row(iteration=i, wall_s=1.0) for i in range(5)]
    b = [fake_row(iteration=i, wall_s=2.0) for i in range(5)]
    r = H.paired_diff_ci(a, b, random.Random(1))
    check("paired diff = -1.0", r and abs(r[0] + 1.0) < 1e-9)
    check("paired CI excludes noise on constant data",
          r and r[1] == r[2] == -1.0)

    print(f"\n{('ALL PASS' if not FAIL else 'FAILURES: ' + str(FAIL))}")
    sys.exit(1 if FAIL else 0)

if __name__ == "__main__":
    main()
