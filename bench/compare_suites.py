#!/usr/bin/env python3
"""PraetorBench-1.2 (pre-O1) vs 1.3 (post-O1) comparison on channel
workloads. Separate benchmark runs -> INDEPENDENT-SAMPLE bootstrap
(resample each run's reps independently, CI on median difference).
Usage: compare_suites.py <1.2-raw.csv> <1.3-raw.csv>
"""
import csv, statistics, sys, random

r12, r13 = sys.argv[1], sys.argv[2]

def load(path):
    rows = {}
    for r in csv.DictReader(open(path)):
        if r["lang"] != "ours":
            continue
        key = (r["workload"], r["mode"])
        rows.setdefault(key, []).append(float(r["wall_s"]))
    return rows

a, b = load(r12), load(r13)
rng = random.Random(0xC1)

def boot_diff_med(x, y, n=20000):
    diffs = []
    for _ in range(n):
        sx = [x[rng.randrange(len(x))] for _ in x]
        sy = [y[rng.randrange(len(y))] for _ in y]
        diffs.append(statistics.median(sy) - statistics.median(sx))
    diffs.sort()
    return diffs[int(0.025*n)], diffs[int(0.975*n)]

print(f"{'cell':38} {'1.2_med':>8} {'1.3_med':>8} {'d_med':>7} {'indep 95% CI':>18} {'n1/n2':>7}")
for key in sorted(set(a) & set(b)):
    if "prodcons" not in key[0]:
        continue
    x, y = a[key], b[key]
    lo, hi = boot_diff_med(x, y)
    print(f"{'.'.join(key):38} {statistics.median(x):8.3f} {statistics.median(y):8.3f} {statistics.median(y)-statistics.median(x):7.3f} [{lo:+.3f},{hi:+.3f}] {len(x)}/{len(y)}")
