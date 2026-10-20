#!/usr/bin/env python3
"""bench/harness.py — PraetorBench 1.0 comparative benchmark harness.

Methodology: bench/METHODOLOGY.md (frozen with this harness).
Matrix: N workloads x {ours, c, rs} x {debug, release}. (No C++ or
Zig sources yet — the column is absent by design, not skipped.)

Per measured run we record wall/user/sys time, peak RSS, exit code,
and a SHA-256 of stdout — a cell is invalid unless every rep in it
produces the identical digest. Execution order is interleaved across
languages with a seeded per-round shuffle, never all-one-language.
Warm-up runs precede measured reps and are not recorded.

Headline statistic: median wall + 95% bootstrap CI. Min is retained
as a diagnostic only.

Artifacts per run (bench/out/<timestamp>/):
  raw.csv  summary.csv  environment.json  compiler-versions.txt
  report.md          (+ legacy bench/results.csv merge)

Ours builds: `ourc --show-llvm` (frontend -> LLVM IR) then clang
compiles the .ll and links the runtime objects — the same pipeline
`build_with` runs, with -O2 added only in release (the runtime is
always -O2, as build_with does it).

Server workloads are driven by the shared C drivers (httpload,
kvload) so client code is identical across languages.
"""
import csv, hashlib, json, os, platform, random, re, shlex, statistics
import subprocess, sys, time, socket

ROOT = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(ROOT, "src")
DATA = os.path.join(ROOT, "data")
OUT = os.path.join(ROOT, "build")
REPO = os.path.dirname(ROOT)
OURC = os.path.join(REPO, "target", "release", "ourc")
# QA-004 — the runtime was split (N58C-2): common + a platform half,
# mirroring rt_objs()/link flags in compiler/src/driver/pipeline.rs.
# Keep the two in lockstep.
RUNTIME = os.path.join(REPO, "compiler", "runtime")
if sys.platform == "win32":
    MMLRT_SRCS = ["mmlrt_common.c", "mmlrt_win32.c"]
    RT_LDLIBS = ["-fuse-ld=lld", "-lws2_32", "-Wl,/stack:8388608"]
else:
    MMLRT_SRCS = ["mmlrt_common.c", "mmlrt_posix.c"]
    RT_LDLIBS = [] if sys.platform == "darwin" else ["-lpthread"]
REPS = int(os.environ.get("REPS", "30"))
WARMUP = int(os.environ.get("WARMUP", "2"))
SEED = int(os.environ.get("SEED", "0x5EED"), 0)
TRACK = os.environ.get("TRACK", "B")  # methodology §2 — src/ is idiomatic
CC = "clang"
RT_OBJS = []  # populated by main() before any build_ours call

os.makedirs(OUT, exist_ok=True)

# workload -> argv template relative to DATA (digits pass through)
WORKLOADS = {
    "fcopy":     ["copy.bin", "out.bin"],
    "wordfreq":  ["corpus.txt"],
    "jsonparse": ["doc.json"],
    "mapbench":  ["4000000"],
    "psum":      ["40000000"],
    "prodcons":  ["2000000"],
}
SERVERS = {
    "httpsrv": ("httpload", 1500),
    "kvsrv":   ("kvload", 1500),
}


def run(cmd, **kw):
    return subprocess.run(cmd, shell=isinstance(cmd, str),
                          capture_output=True, **kw)

def checked(cmd, what):
    """QA-004 — a build step that fails must stop the suite with a
    clean explanation, not let a stale object mask the breakage."""
    r = run(cmd, text=True)
    if r.returncode != 0:
        sys.exit(f"bench: {what} failed:\n{r.stderr[:800]}")
    return r

def timed(cmd, **kw):
    t = time.monotonic()
    r = run(cmd, **kw)
    return time.monotonic() - t, r

def build_ours(src, stem, release):
    """ours build: ourc -> .ll, clang .ll -> .o, link mmlrt.o."""
    ll = os.path.join(OUT, f"{stem}.ll")
    obj = os.path.join(OUT, f"{stem}.o")
    exe = os.path.join(OUT, stem)
    opt = ["-O2"] if release else []
    t = 0.0
    dt, r = timed([OURC, src, "--show-llvm"], text=True)
    t += dt
    if r.returncode != 0:
        print(f"  !! ourc {src}:\n{r.stderr[:600]}")
        return None, t
    with open(ll, "w") as f:
        f.write(r.stdout)
    for c in ([CC, *opt, "-c", ll, "-o", obj],
              [CC, *opt, obj, *RT_OBJS, *RT_LDLIBS, "-o", exe]):
        dt, r = timed(c, text=True)
        t += dt
        if r.returncode != 0:
            print(f"  !! clang {c}:\n{r.stderr[:400]}")
            return None, t
    return exe, t

def build_c(src, stem, release):
    exe = os.path.join(OUT, stem)
    opt = "-O2" if release else "-O0"
    cmd = [CC, opt, "-o", exe, src]
    if "prodcons" in stem or "psum" in stem:
        cmd += ["-lpthread"]
    t, r = timed(cmd, text=True)
    if r.returncode != 0:
        print(f"  !! {r.stderr[:400]}")
        return None, t
    return exe, t

def build_rs(src, stem, release):
    exe = os.path.join(OUT, stem)
    opt = "2" if release else "0"
    t, r = timed(["rustc", "-C", f"opt-level={opt}", "-o", exe, src],
                 text=True)
    if r.returncode != 0:
        print(f"  !! {r.stderr[:400]}")
        return None, t
    return exe, t

BUILDERS = {"ours": build_ours, "c": build_c, "rs": build_rs}
EXT = {"ours": "lang", "c": "c", "rs": "rs"}

# /usr/bin/time field regexes — macOS `-l` vs GNU `-v` formats.
BSD = sys.platform == "darwin"
T_RE = {
    "wall_s": (r"([\d.]+)\s+real",        r"wall clock\) time.*: (\S+)"),
    "user_s": (r"([\d.]+)\s+user",        r"User time \(seconds\): ([\d.]+)"),
    "sys_s":  (r"([\d.]+)\s+sys",         r"System time \(seconds\): ([\d.]+)"),
    "rss_bytes": (r"(\d+)\s+maximum resident set size",
                  r"Maximum resident set size \(kbytes\): (\d+)"),
}

def _wall_seconds(s):
    """`h:mm:ss` / `m:ss` (GNU time -v) or plain seconds."""
    p = s.split(":")
    if len(p) == 1:
        return float(s)
    return float(p[-1]) + 60 * float(p[-2]) + (3600 * float(p[-3]) if len(p) > 2 else 0)

def _parse_time(stderr_b):
    """Extract wall/user/sys/rss from /usr/bin/time stderr."""
    s = stderr_b.decode("utf-8", "replace")
    out = {}
    for k, (bsd_re, gnu_re) in T_RE.items():
        m = re.search(bsd_re if BSD else gnu_re, s)
        if not m:
            continue
        v = m.group(1)
        if k == "rss_bytes":
            out[k] = int(v) * (1 if BSD else 1024)
        else:
            out[k] = _wall_seconds(v) if ":" in v else float(v)
    return out

def measure_one(exe, args):
    """One timed run under /usr/bin/time. Returns per-iteration dict
    or None on failure."""
    r = run(["/usr/bin/time", "-l" if BSD else "-v", exe, *args])
    m = dict(wall_s=None, user_s=None, sys_s=None, rss_bytes=None,
             exit_status=r.returncode,
             sha256=hashlib.sha256(r.stdout).hexdigest())
    if r.returncode != 0:
        err = r.stderr.decode("utf-8", "replace")[:300]
        print(f"  !! run {exe}: rc={r.returncode} {err}")
        return None
    m.update(_parse_time(r.stderr))
    return m

def warmup(exe, args):
    for _ in range(WARMUP):
        r = run([exe, *args])
        if r.returncode != 0:
            print(f"  !! warmup {exe}: rc={r.returncode}")
            return False
    return True

def wait_port(port, timeout=5.0):
    end = time.monotonic() + timeout
    while time.monotonic() < end:
        try:
            s = socket.create_connection(("127.0.0.1", port), 0.2)
            s.close()
            return True
        except OSError:
            time.sleep(0.05)
    return False

# --- statistics -------------------------------------------------

def pct(sorted_vals, q):
    """Interpolated percentile on a sorted list."""
    n = len(sorted_vals)
    if n == 1:
        return sorted_vals[0]
    k = (n - 1) * q
    lo, hi = int(k), min(int(k) + 1, n - 1)
    f = k - lo
    return sorted_vals[lo] * (1 - f) + sorted_vals[hi] * f

def boot_ci(vals, rng, resamples=10000):
    """95% percentile bootstrap CI of the median (seeded)."""
    n = len(vals)
    meds = sorted(statistics.median(
        [vals[rng.randrange(n)] for _ in range(n)])
        for _ in range(resamples))
    return pct(meds, 0.025), pct(meds, 0.975)

def summarize(walls):
    s = sorted(walls)
    rng = random.Random(SEED)
    ci_lo, ci_hi = boot_ci(s, rng)
    med = statistics.median(s)
    mean = statistics.mean(s)
    return dict(n=len(s), median_s=med, ci_lo_s=ci_lo, ci_hi_s=ci_hi,
                mean_s=mean, sd_s=statistics.stdev(s) if len(s) > 1 else 0.0,
                min_s=s[0], max_s=s[-1], p5_s=pct(s, 0.05),
                p95_s=pct(s, 0.95), cv=(statistics.stdev(s) / mean
                    if len(s) > 1 and mean else 0.0))

# --- environment ------------------------------------------------

def _git_sha():
    r = run(["git", "-C", REPO, "rev-parse", "HEAD"], text=True)
    return r.stdout.strip() if r.returncode == 0 else "unknown"

def collect_environment():
    return {
        "suite": "PraetorBench-1.0",
        "grade": "lab-grade",
        "praetor_commit": _git_sha(),
        "os": platform.system(),
        "os_release": platform.release(),
        "arch": platform.machine(),
        "cpu": platform.processor() or "unknown",
        "cpu_count": os.cpu_count(),
        "python": platform.python_version(),
        "hostname": platform.node(),
        "reps": REPS, "warmup": WARMUP, "seed": SEED, "track": TRACK,
        "runtime_srcs": MMLRT_SRCS, "rt_ldlibs": RT_LDLIBS,
    }

def collect_compiler_versions():
    lines = []
    for name, cmd in [("ourc", [OURC, "--version"]),
                      ("clang", [CC, "--version"]),
                      ("rustc", ["rustc", "--version"])]:
        r = run(cmd, text=True)
        v = r.stdout.strip().splitlines()[0] if r.returncode == 0 \
            and r.stdout.strip() else "unavailable"
        lines.append(f"{name}: {v}")
    lines.append(f"praetor_commit: {_git_sha()}")
    return "\n".join(lines) + "\n"

# --- measurement ------------------------------------------------

RAW_FIELDS = ["workload", "track", "lang", "mode", "iteration",
              "wall_s", "user_s", "sys_s", "rss_bytes", "exit_status",
              "sha256", "praetor_commit", "seed"]

def measure_workload(cells):
    """cells: list of dicts {work,lang,mode,exe,args}. Round-robin
    over cells with a seeded per-round shuffle — methodology §6."""
    rng = random.Random(SEED)
    raw = []
    for it in range(REPS):
        order = list(cells)
        rng.shuffle(order)
        for c in order:
            m = measure_one(c["exe"], c["args"])
            if m is None:
                continue
            raw.append(dict(workload=c["work"], track=TRACK,
                            lang=c["lang"], mode=c["mode"],
                            iteration=it, praetor_commit=_git_sha(),
                            seed=SEED, **m))
    return raw

def measure_server(cell, port_ctr):
    """Server cell: run_s = driver wall (the load absorbed). Server
    stdout isn't checksummed — driver rc==0 IS the verification
    (its protocol validates every response)."""
    name, lang, mode, exe = cell["work"], cell["lang"], cell["mode"], cell["exe"]
    drv_name, nconn = SERVERS[name]
    driver = os.path.join(OUT, drv_name)
    raw = []
    # servers warm up implicitly: rep 0 of REPS runs is discarded
    for it in range(REPS + 1):
        port_ctr[0] += 1
        port = 18000 + port_ctr[0]  # never reused — a lingering
        # killed server can't steal conns
        p = subprocess.Popen(["/usr/bin/time", "-l" if BSD else "-v",
                              exe, str(port), str(nconn)],
                             stdout=subprocess.DEVNULL,
                             stderr=subprocess.PIPE)
        t = time.monotonic()
        d = run([driver, str(port), str(nconn)], text=True)
        wall = time.monotonic() - t
        try:
            _, serr = p.communicate(timeout=60)
        except subprocess.TimeoutExpired:
            p.kill()
            _, serr = p.communicate()
            print(f"  !! {name}_{lang} rep timed out")
            continue
        if d.returncode != 0:
            print(f"  !! {name}_{lang} driver rc={d.returncode}")
            continue
        if it < 1:  # warm-up rep — not recorded
            continue
        m = _parse_time(serr)
        m["wall_s"] = wall
        raw.append(dict(workload=name, track=TRACK, lang=lang,
                        mode=mode, iteration=it - 1,
                        user_s=m.get("user_s"), sys_s=m.get("sys_s"),
                        rss_bytes=m.get("rss_bytes"), exit_status=0,
                        sha256="server-verified",
                        praetor_commit=_git_sha(), seed=SEED))
    return raw

# --- main -------------------------------------------------------

def main():
    if not os.path.exists(OURC):
        sys.exit(f"bench: {OURC} missing — `cargo build --release -p ourc` first")
    # shared runtime objects — ALWAYS recompiled, never cached: the
    # compile is itself evidence (a stale .o must not mask a broken
    # source tree), and every build command's status is checked.
    global RT_OBJS
    RT_OBJS = []
    for s in MMLRT_SRCS:
        o = os.path.join(OUT, s.replace(".c", ".o"))
        checked([CC, "-O2", "-c", os.path.join(RUNTIME, s), "-o", o],
                f"runtime {s}")
        RT_OBJS.append(o)
    # shared drivers
    for d in ("httpload", "kvload"):
        checked([CC, "-O2", "-o", os.path.join(OUT, d),
                 os.path.join(SRC, f"{d}.c")], f"driver {d}")

    only = os.environ.get("ONLY")

    # Phase 1 — build every cell first (compile_s is recorded), then
    # interleave the measured runs so thermal/frequency drift is
    # spread across languages rather than stacked on the last one.
    cells, srv_cells = [], []
    for name in sorted(WORKLOADS):
        if only and only not in name:
            continue
        for mode in ("debug", "release"):
            for lang in ("ours", "c", "rs"):
                src = os.path.join(SRC, f"{name}.{EXT[lang]}")
                stem = f"{name}_{mode}_{lang}"
                exe, ct = BUILDERS[lang](src, stem, rel := mode == "release")
                if not exe:
                    continue
                args = [os.path.join(DATA, a) if not a.isdigit() else a
                        for a in WORKLOADS[name]]
                cells.append(dict(work=name, lang=lang, mode=mode,
                                  exe=exe, args=args, compile_s=ct,
                                  size=os.path.getsize(exe),
                                  loc=sum(1 for _ in open(src))))
    for name in sorted(SERVERS):
        if only and only not in name:
            continue
        for mode in ("debug", "release"):
            for lang in ("ours", "c", "rs"):
                src = os.path.join(SRC, f"{name}.{EXT[lang]}")
                stem = f"{name}_{mode}_{lang}"
                exe, ct = BUILDERS[lang](src, stem, mode == "release")
                if not exe:
                    continue
                srv_cells.append(dict(work=name, lang=lang, mode=mode,
                                      exe=exe, compile_s=ct,
                                      size=os.path.getsize(exe),
                                      loc=sum(1 for _ in open(src))))

    # Phase 2 — warmups, then interleaved measured reps.
    for c in cells:
        warmup(c["exe"], c["args"])
    raw = measure_workload(cells)
    port_ctr = [0]
    for c in srv_cells:
        raw += measure_server(c, port_ctr)

    # Phase 3 — per-cell summaries. Output verification: every rep
    # in a (work, mode) pair across ALL languages must produce the
    # same stdout digest, or the cell is INVALID (a bug or dead-code
    # elimination made someone look fast).
    digests = {}
    for r in raw:
        if r["sha256"] != "server-verified":
            digests.setdefault((r["workload"], r["mode"]), set()).add(r["sha256"])
    invalid = {k for k, v in digests.items() if len(v) > 1}

    bycell = {}
    for r in raw:
        bycell.setdefault((r["workload"], r["lang"], r["mode"]), []).append(r)

    cellinfo = {(c["work"], c["lang"], c["mode"]): c
                for c in cells + srv_cells}
    summary = []
    for key, rs in sorted(bycell.items()):
        work, lang, mode = key
        walls = [r["wall_s"] for r in rs if r["wall_s"] is not None]
        rss = max(r["rss_bytes"] or 0 for r in rs)
        c = cellinfo[key]
        s = summarize(walls) if walls else dict(
            n=0, median_s=None, ci_lo_s=None, ci_hi_s=None, mean_s=None,
            sd_s=None, min_s=None, max_s=None, p5_s=None, p95_s=None, cv=None)
        row = dict(workload=work, track=TRACK, lang=lang, mode=mode,
                   valid="no" if (work, mode) in invalid else "yes",
                   compile_s=c["compile_s"], peak_rss_bytes=rss,
                   size_bytes=c["size"], loc=c["loc"], **s)
        summary.append(row)
        med = f'{s["median_s"]:7.3f}' if s["median_s"] is not None else "    n/a"
        print(f"{work:10} {lang:4} {mode:7} n={s['n']:>2} "
              f"med={med}s [{s['ci_lo_s'] or 0:.3f}–{s['ci_hi_s'] or 0:.3f}] "
              f"rss={rss:>10} size={c['size']:>9} loc={c['loc']}"
              + (" INVALID-DIGEST" if row["valid"] == "no" else ""))

    # Phase 4 — artifacts.
    ts = time.strftime("%Y%m%d-%H%M%S")
    rundir = os.path.join(ROOT, "out", ts)
    os.makedirs(rundir, exist_ok=True)

    with open(os.path.join(rundir, "raw.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=RAW_FIELDS)
        w.writeheader()
        w.writerows(raw)

    sfields = ["workload", "track", "lang", "mode", "valid", "n",
               "median_s", "ci_lo_s", "ci_hi_s", "mean_s", "sd_s",
               "min_s", "max_s", "p5_s", "p95_s", "cv", "compile_s",
               "peak_rss_bytes", "size_bytes", "loc"]
    with open(os.path.join(rundir, "summary.csv"), "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=sfields)
        w.writeheader()
        w.writerows(summary)

    with open(os.path.join(rundir, "environment.json"), "w") as f:
        json.dump(collect_environment(), f, indent=2)
    with open(os.path.join(rundir, "compiler-versions.txt"), "w") as f:
        f.write(collect_compiler_versions())

    with open(os.path.join(rundir, "report.md"), "w") as f:
        f.write(f"# PraetorBench 1.0 — {ts} ({TRACK}-track, lab-grade)\n\n")
        f.write("| workload | lang | mode | n | median s | 95% CI | "
                "rss B | size B | loc |\n|---|---|---|---|---|---|---|---|---|\n")
        for r in summary:
            if r["median_s"] is None:
                continue
            f.write(f"| {r['workload']} | {r['lang']} | {r['mode']} | "
                    f"{r['n']} | {r['median_s']:.4f} | "
                    f"{r['ci_lo_s']:.4f}–{r['ci_hi_s']:.4f} | "
                    f"{r['peak_rss_bytes']} | {r['size_bytes']} | "
                    f"{r['loc']} |\n")

    # legacy merge — ONLY-filtered runs replace just their own cells
    csv_path = os.path.join(ROOT, "results.csv")
    new_keys = {(r["workload"], r["lang"], r["mode"]) for r in summary}
    legacy = [dict(work=r["workload"], lang=r["lang"], mode=r["mode"],
                   compile_s=r["compile_s"], run_s=r["median_s"],
                   rss=r["peak_rss_bytes"], size=r["size_bytes"],
                   loc=r["loc"]) for r in summary]
    if os.path.exists(csv_path) and only:
        with open(csv_path) as f:
            legacy = [r for r in csv.DictReader(f)
                      if (r["work"], r["lang"], r["mode"]) not in new_keys] + legacy
    with open(csv_path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["work", "lang", "mode",
                                          "compile_s", "run_s", "rss",
                                          "size", "loc"])
        w.writeheader()
        w.writerows(legacy)

    print(f"\nwrote {rundir} ({len(raw)} raw rows, {len(summary)} cells)")
    if invalid:
        print("!! digest mismatches — INVALID cells:", *sorted(invalid))

if __name__ == "__main__":
    main()
