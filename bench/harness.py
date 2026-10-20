#!/usr/bin/env python3
"""bench/harness.py — comparative systems benchmark suite.

Matrix: 8 workloads x {ours, C, Rust} x {debug, release}.
Zig is not installed on this machine — the column is absent by
design, not silently skipped.

Metrics per cell:
  compile_s   wall time of the full build pipeline
  run_s       min wall over REPS runs  (min = least noise floor)
  peak_rss    `maximum resident set size` from /usr/bin/time -l
  bin_size    stat() of the produced binary
  loc         total source lines

Ours builds: `ourc --show-llvm` (frontend -> LLVM IR) then clang
compiles the .ll and links the runtime objects — the same pipeline
`build_with` runs, with -O2 added only in release (the runtime is
always -O2, as build_with does it).

Server workloads are driven by the shared C drivers (httpload,
kvload) so client code is identical across languages.
"""
import os, re, shlex, statistics, subprocess, sys, time, socket

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
REPS = int(os.environ.get("REPS", "5"))
CC = "clang"
RT_OBJS = []  # populated by main() before any build_ours call

os.makedirs(OUT, exist_ok=True)

# workload -> (name, argv template relative to DATA)
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
                          capture_output=True, text=True, **kw)

def checked(cmd, what):
    """QA-004 — a build step that fails must stop the suite with a
    clean explanation, not let a stale object mask the breakage."""
    r = run(cmd)
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
    dt, r = timed([OURC, src, "--show-llvm"])
    t += dt
    if r.returncode != 0:
        print(f"  !! ourc {src}:\n{r.stderr[:600]}")
        return None, t
    with open(ll, "w") as f:
        f.write(r.stdout)
    for c in ([CC, *opt, "-c", ll, "-o", obj],
              [CC, *opt, obj, *RT_OBJS, *RT_LDLIBS, "-o", exe]):
        dt, r = timed(c)
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
    t, r = timed(cmd)
    if r.returncode != 0:
        print(f"  !! {r.stderr[:400]}")
        return None, t
    return exe, t

def build_rs(src, stem, release):
    exe = os.path.join(OUT, stem)
    opt = "2" if release else "0"
    t, r = timed(["rustc", "-C", f"opt-level={opt}", "-o", exe, src])
    if r.returncode != 0:
        print(f"  !! {r.stderr[:400]}")
        return None, t
    return exe, t

BUILDERS = {"ours": build_ours, "c": build_c, "rs": build_rs}
EXT = {"ours": "lang", "c": "c", "rs": "rs"}

def _wall_seconds(s):
    """`h:mm:ss` / `m:ss` (GNU time -v) or plain seconds."""
    p = s.split(":")
    if len(p) == 1:
        return float(s)
    return float(p[-1]) + 60 * float(p[-2]) + (3600 * float(p[-3]) if len(p) > 2 else 0)

def measure(exe, args, reps):
    """min wall + max RSS over `reps` runs. macOS `/usr/bin/time -l`
    and GNU `/usr/bin/time -v` emit different stderr formats."""
    bsd = sys.platform == "darwin"
    cmd = ["/usr/bin/time", "-l" if bsd else "-v", exe, *args]
    best, rss = None, 0
    for _ in range(reps):
        r = run(cmd)
        if r.returncode != 0:
            print(f"  !! run {exe}: rc={r.returncode} {r.stderr[:300]}")
            return None, None
        m = re.search(r"(\d+)\s+maximum resident set size" if bsd else
                      r"Maximum resident set size \(kbytes\): (\d+)", r.stderr)
        if m:
            rss = max(rss, int(m.group(1)) * (1 if bsd else 1024))
        w = re.search(r"([\d.]+)\s+real" if bsd else
                      r"wall clock\) time.*: (\S+)", r.stderr)
        if w:
            t = _wall_seconds(w.group(1))
            best = t if best is None else min(best, t)
    return best, rss

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
    rows = []
    port_ctr = [0]
    for name in sorted(WORKLOADS):
        if only and only not in name:
            continue
        for mode in ("debug", "release"):
            rel = mode == "release"
            for lang in ("ours", "c", "rs"):
                src = os.path.join(SRC, f"{name}.{EXT[lang]}")
                stem = f"{name}_{mode}_{lang}"
                exe, ct = BUILDERS[lang](src, stem, rel)
                if not exe:
                    continue
                args = [os.path.join(DATA, a) if not a.isdigit() else a
                        for a in WORKLOADS[name]]
                rt_s, rss = measure(exe, args, REPS)
                rows.append(dict(work=name, lang=lang, mode=mode,
                                 compile_s=ct, run_s=rt_s, rss=rss,
                                 size=os.path.getsize(exe),
                                 loc=sum(1 for _ in open(src))))
                print(f"{name:10} {lang:4} {mode:7} build={ct:6.2f}s "
                      f"run={rt_s or 0:7.3f}s rss={rss or 0:>10} "
                      f"size={os.path.getsize(exe):>9} loc={rows[-1]['loc']}")

    # server cells: run_s = driver wall (the load the server absorbed)
    for name in sorted(SERVERS):
        if only and only not in name:
            continue
        drv_name, nconn = SERVERS[name]
        driver = os.path.join(OUT, drv_name)
        for mode in ("debug", "release"):
            rel = mode == "release"
            for lang in ("ours", "c", "rs"):
                src = os.path.join(SRC, f"{name}.{EXT[lang]}")
                stem = f"{name}_{mode}_{lang}"
                exe, ct = BUILDERS[lang](src, stem, rel)
                if not exe:
                    continue
                best, rss = None, 0
                for _ in range(REPS):
                    port_ctr[0] += 1
                    port = 18000 + port_ctr[0]  # never reused — a
                    # lingering killed server can't steal conns
                    p = subprocess.Popen(["/usr/bin/time", "-l", exe,
                                          str(port), str(nconn)],
                                         stdout=subprocess.DEVNULL,
                                         stderr=subprocess.PIPE, text=True)
                    # no readiness poll: the driver's per-conn retry
                    # (~1s) covers bind+listen startup, and the server
                    # budget is exactly the driver's conn count.
                    t = time.monotonic()
                    d = run([driver, str(port), str(nconn)])
                    wall = time.monotonic() - t
                    try:
                        _, serr = p.communicate(timeout=60)
                    except subprocess.TimeoutExpired:
                        p.kill()
                        _, serr = p.communicate()
                        print(f"  !! {stem} rep timed out")
                        continue
                    if d.returncode == 0:
                        best = wall if best is None else min(best, wall)
                    m = re.search(r"(\d+)  maximum resident set size", serr)
                    if m:
                        rss = max(rss, int(m.group(1)))
                rows.append(dict(work=name, lang=lang, mode=mode,
                                 compile_s=ct, run_s=best, rss=rss,
                                 size=os.path.getsize(exe),
                                 loc=sum(1 for _ in open(src))))
                print(f"{name:10} {lang:4} {mode:7} build={ct:6.2f}s "
                      f"run={best or 0:7.3f}s rss={rss or 0:>10} "
                      f"size={os.path.getsize(exe):>9} loc={rows[-1]['loc']}")

    import csv
    csv_path = os.path.join(ROOT, "results.csv")
    # merge: an ONLY-filtered run replaces just its own cells
    new_keys = {(r["work"], r["lang"], r["mode"]) for r in rows}
    if os.path.exists(csv_path) and only:
        with open(csv_path) as f:
            rows = [r for r in csv.DictReader(f)
                    if (r["work"], r["lang"], r["mode"]) not in new_keys] + rows
    with open(csv_path, "w", newline="") as f:
        w = csv.DictWriter(f, fieldnames=["work", "lang", "mode",
                                        "compile_s", "run_s", "rss",
                                        "size", "loc"])
        w.writeheader()
        w.writerows(rows)
    print(f"\nwrote {os.path.join(ROOT, 'results.csv')} ({len(rows)} cells)")

if __name__ == "__main__":
    main()
