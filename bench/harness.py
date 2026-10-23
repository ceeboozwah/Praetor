#!/usr/bin/env python3
"""bench/harness.py — PraetorBench 1.2 comparative benchmark harness.

Methodology: bench/METHODOLOGY.md (frozen with this harness).
Matrix: N workloads x {ours, c, cxx, rs} x {debug, release} x
{sm, md, lg}.

Validity rules (methodology §5/§6 — enforced, not advisory):
  * a cell needs exactly REPS measured reps, all exit_status == 0,
    all with timing data — failures are recorded, never dropped
  * every rep in a cell shares one stdout digest AND one output-file
    digest — per-cell disagreement => digest-mismatch
  * all languages in a (workload, mode) share the digests —
    disagreement => work-mismatch, the whole (work, mode) is invalid
  * build failures and missing sources are recorded as cells, not
    silently absent
  * invalid cells never enter report.md's headline table or the
    legacy results.csv — they are listed separately with reasons

Execution order: WARMUP unrecorded rounds then REPS measured rounds,
cells interleaved with a seeded per-round shuffle — never
all-one-language-then-next. Headline statistic: median wall + 95%
bootstrap CI of the median (10k seeded resamples). Min is diagnostic
only. Paired bootstrap CIs of per-iteration differences are emitted
for ours-vs-{c,rs} in report.md.

Artifacts per run (bench/out/<timestamp>/):
  raw.csv  summary.csv  environment.json  compiler-versions.txt
  report.md  run-manifest.json  raw.jsonl
  (+ bench/results.csv — PUBLISH=1 full-matrix runs only)

Checkpointing/resume (post-1.2 hardening — measurement unchanged):
  run-manifest.json and raw.jsonl exist BEFORE any rep runs. Every
  measured rep is appended to raw.jsonl and fsynced the moment it
  completes, so partial runs lose nothing already measured and
  `wc -l raw.jsonl` is live progress (expected total = the 'total
  reps' line printed at startup). Per-rep progress goes to stdout.

  RESUME=<out-dir> first verifies the stored run-manifest.json
  field-for-field (suite, seeds, reps, matrix, input digests, source
  digests, compiler commit + worktree state) — incompatible
  experiments are refused, never silently pooled — then reloads
  raw.jsonl, tolerating a truncated final record but refusing on a
  corrupt interior record, malformed row, or duplicate rep key, and
  skips completed (workload, lang, mode, iteration) reps. The seeded
  shuffle keeps the remaining schedule deterministic; loaded rows
  keep their own suite/commit stamps.

  A rep that raises an unexpected harness-level exception is
  recorded with exit_status=-9 (classifies the cell invalid) rather
  than aborting the suite — Ctrl-C still aborts, and the partial
  run is resumable.

Canonical history (bench/results.csv) is written ONLY by an
explicit PUBLISH=1 run over the full unfiltered matrix with
standard rep counts. Filtered (ONLY=) or non-standard-reps runs are
refused before they can overwrite official results; on publish,
attempted-but-invalid cells keep their historical rows and are
reported, not silently replaced.

Ours builds: `ourc --show-llvm` (frontend -> LLVM IR) then clang
compiles the .ll and links the runtime objects — the same pipeline
`build_with` runs, with -O2 added only in release (the runtime is
always -O2, as build_with does it).

Server workloads are driven by the shared C drivers (httpload,
kvload): driver rc==0 plus a driver-stdout digest is the output
verification; the server's own exit status is recorded separately.

Kernel timing: a workload may print `__kernel_s=<seconds>` to
separate its measured core from process wall; captured per run.
"""
import csv, hashlib, json, os, platform, random, re, statistics
import subprocess, sys, time, socket

ROOT = os.path.dirname(os.path.abspath(__file__))
SRC = os.path.join(ROOT, "src")
DATA = os.path.join(ROOT, "data")
OUT = os.path.join(ROOT, "build")
REPO = os.path.dirname(ROOT)
OURC = os.path.join(REPO, "target", "release", "ourc")
SUITE = "PraetorBench-1.2"  # post-CHAN-004-S4 baseline + bounded-channel sweep — methodology §10
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
# measured reps per scale — multi-second workloads get fewer;
# REPS env overrides all scales uniformly.
REPS_SCALE = {"sm": 30, "md": 20, "lg": 10}
REPS = int(os.environ.get("REPS", "0")) or None
WARMUP = int(os.environ.get("WARMUP", "2"))
SEED = int(os.environ.get("SEED", "0x5EED"), 0)
GEN_SEED = 0xC0FFEE  # bench/gen.py — recorded in environment.json
TIMEOUT = int(os.environ.get("TIMEOUT", "300"))
CC = "clang"
RT_OBJS = []  # populated by main() before any build_ours call

SCALES = ("sm", "md", "lg")

os.makedirs(OUT, exist_ok=True)

# workload -> {scale: argv template}. Cells are named "work.scale";
# identical inputs/outputs across all four languages at each scale.
# "@out:NAME" marks a produced output file (resolved under
# bench/build/ per cell, content-digested per run). Plain names
# resolve under bench/data/; all-digit args pass through.
WORKLOADS = {
    "fcopy":     {"sm": ["copy_sm.bin", "@out:fcopy.dst"],
                  "md": ["copy_md.bin", "@out:fcopy.dst"],
                  "lg": ["copy_lg.bin", "@out:fcopy.dst"]},
    "wordfreq":  {"sm": ["corpus_sm.txt"], "md": ["corpus_md.txt"],
                  "lg": ["corpus_lg.txt"]},
    "jsonparse": {"sm": ["doc_sm.json"], "md": ["doc_md.json"],
                  "lg": ["doc_lg.json"]},
    "mapbench":  {"sm": ["1000000"], "md": ["4000000"],
                  "lg": ["12000000"]},
    "psum":      {"sm": ["10000000"], "md": ["40000000"],
                  "lg": ["120000000"]},
    "prodcons":  {"sm": ["500000"], "md": ["2000000"],
                  "lg": ["8000000"]},
    # CHAN-004 post-S4 — bounded-channel sweep: argv = [count, cap].
    # Same packet sequence as prodcons; cap is the runtime queue
    # bound (bchannel / sync_channel / ring+cv).
    "prodcons_bchan_c1":    {"sm": ["500000", "1"],
                             "md": ["2000000", "1"],
                             "lg": ["8000000", "1"]},
    "prodcons_bchan_c16":   {"sm": ["500000", "16"],
                             "md": ["2000000", "16"],
                             "lg": ["8000000", "16"]},
    "prodcons_bchan_c256":  {"sm": ["500000", "256"],
                             "md": ["2000000", "256"],
                             "lg": ["8000000", "256"]},
    "prodcons_bchan_c4096": {"sm": ["500000", "4096"],
                             "md": ["2000000", "4096"],
                             "lg": ["8000000", "4096"]},
}

# capacity-sweep workload names share one source per language —
# cap arrives via argv, not via compiled-in constants.
WORK_SRC = {k: "prodcons_bchan" for k in WORKLOADS
            if k.startswith("prodcons_bchan_")}
SERVERS = {  # scale = client request count (same shared drivers)
    "httpsrv": {"sm": ("httpload", 400), "md": ("httpload", 1500),
                "lg": ("httpload", 4000)},
    "kvsrv":   {"sm": ("kvload", 400), "md": ("kvload", 1500),
                "lg": ("kvload", 4000)},
}

# /usr/bin/time field regexes — macOS `-l` vs GNU `-v` formats.
BSD = sys.platform == "darwin"
T_RE = {
    "wall_s": (r"([\d.]+)\s+real",        r"wall clock\) time.*: (\S+)"),
    "user_s": (r"([\d.]+)\s+user",        r"User time \(seconds\): ([\d.]+)"),
    "sys_s":  (r"([\d.]+)\s+sys",         r"System time \(seconds\): ([\d.]+)"),
    "rss_bytes": (r"(\d+)\s+maximum resident set size",
                  r"Maximum resident set size \(kbytes\): (\d+)"),
}

FLAGS = {
    "ours": "ourc --show-llvm | clang -O2(release)/-O0(debug) + mmlrt -O2",
    "c":    "clang -O2(release)/-O0(debug)",
    "cxx":  "clang++ -O2(release)/-O0(debug)",
    "rs":   "rustc -C opt-level=2(release)/0(debug)",
}

# every workload in src/ is a controlled Track-A port: same
# algorithm, data structure, hash, growth policy, output format.
# Track identity is a property of the IMPLEMENTATION (TRACKS map) —
# deliberately not overridable by env, so a label can't be slapped
# on a divergent source.
# Track A = strictly controlled (same algorithm AND same storage /
# sync strategy, per independent review): mapbench, psum, fcopy.
# The rest use each language's native facility — channels (prodcons),
# DOM/vector layout (jsonparse), map abstraction (kvsrv), entry
# representation (wordfreq), networking API (httpsrv) — so they are
# Track B idiomatic/native-runtime comparisons, NOT controlled cells.
TRACKS = {"mapbench": "A", "psum": "A", "fcopy": "A",
          "wordfreq": "B", "httpsrv": "B", "jsonparse": "B",
          "kvsrv": "B", "prodcons": "B",
          # bounded channel — each language's native bounded facility
          # (bchannel / pthread ring+cv / std::mutex ring /
          # sync_channel): idiomatic, not controlled-identical.
          "prodcons_bchan_c1": "B", "prodcons_bchan_c16": "B",
          "prodcons_bchan_c256": "B", "prodcons_bchan_c4096": "B"}

def track_of(work):
    return TRACKS.get(work.split(".")[0], "B")


def run(cmd, timeout=None, **kw):
    try:
        return subprocess.run(cmd, shell=isinstance(cmd, str),
                              capture_output=True, timeout=timeout, **kw)
    except subprocess.TimeoutExpired:
        class T:  # synthesize a failed CompletedProcess
            returncode = -3
            stdout = b""
            stderr = b"harness: timeout"
        return T()

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
    # -o keeps ourc's emitted binary out of src/ (it lands in build/).
    dt, r = timed([OURC, src, "--show-llvm", "-o", f"{exe}.ourc"],
                  text=True)
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

def build_cc(src, stem, release, cxx=False):
    exe = os.path.join(OUT, stem)
    opt = "-O2" if release else "-O0"
    cmd = [("clang++" if cxx else CC), opt, "-o", exe, src]
    if "prodcons" in stem or "psum" in stem:
        cmd += ["-lpthread"]
    t, r = timed(cmd, text=True)
    if r.returncode != 0:
        print(f"  !! {r.stderr[:400]}")
        return None, t
    return exe, t

def build_c(src, stem, release):
    return build_cc(src, stem, release, cxx=False)

def build_cxx(src, stem, release):
    return build_cc(src, stem, release, cxx=True)

def build_rs(src, stem, release):
    exe = os.path.join(OUT, stem)
    opt = "2" if release else "0"
    t, r = timed(["rustc", "-C", f"opt-level={opt}", "-o", exe, src],
                 text=True)
    if r.returncode != 0:
        print(f"  !! {r.stderr[:400]}")
        return None, t
    return exe, t

BUILDERS = {"ours": build_ours, "c": build_c, "cxx": build_cxx,
            "rs": build_rs}
EXT = {"ours": "lang", "c": "c", "cxx": "cpp", "rs": "rs"}
LANGS = ("ours", "c", "cxx", "rs")
KERNEL_RE = re.compile(rb"__kernel_s=([0-9.eE+-]+)")

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

def resolve_args(name, scale, stem):
    """argv template -> (argv, [(val,is_file)] inputs, output files)."""
    args, ins, outs = [], [], []
    for a in WORKLOADS[name][scale]:
        if a.startswith("@out:"):
            p = os.path.join(OUT, f"{stem}.{a[5:]}")
            args.append(p)
            outs.append(p)
        elif a.isdigit():
            args.append(a)
            ins.append((a, False))
        else:
            p = os.path.join(DATA, a)
            args.append(p)
            ins.append((p, True))
    return args, ins, outs

_manifest = None

def _load_manifest():
    """gen.py's manifest: {name: {sha256, size, seed}} — the record
    that lets the harness VERIFY data files instead of trusting them."""
    global _manifest
    if _manifest is None:
        try:
            with open(os.path.join(DATA, "manifest.json")) as f:
                _manifest = json.load(f)
        except (OSError, json.JSONDecodeError):
            _manifest = {}
    return _manifest

def _input_digest(ins):
    """SHA-256 over the workload's inputs: file contents for data
    files (each verified against the gen.py manifest), literal text
    for op-count args. Returns None if any required input is missing
    or fails manifest verification — never silently falls back."""
    h = hashlib.sha256()
    manifest = _load_manifest()
    for val, is_file in ins:
        if not is_file:
            h.update(str(val).encode())
        else:
            if not os.path.exists(val):
                return None
            fh = hashlib.sha256()
            with open(val, "rb") as f:
                for chunk in iter(lambda: f.read(1 << 22), b""):
                    fh.update(chunk)
            if manifest:
                ent = manifest.get(os.path.basename(val))
                if not ent or ent["sha256"] != fh.hexdigest():
                    return None  # unverified input — invalid
            h.update(fh.digest())
        h.update(b"\x00")
    return h.hexdigest()

def _file_digest(paths):
    """One digest over all declared output files' contents; a missing
    file is hashed as the marker 'MISSING' so it can't equal a real
    digest. Paths are not hashed — digests compare content only."""
    h = hashlib.sha256()
    for p in paths:
        if os.path.exists(p):
            with open(p, "rb") as f:
                h.update(f.read())
        else:
            h.update(b"MISSING")
        h.update(b"\x00")
    return h.hexdigest()

def measure_one(exe, args, outs):
    """One timed run under /usr/bin/time. Always returns a per-iteration
    record — a failed or incomplete run is evidence, not silence."""
    m = dict(wall_s=None, user_s=None, sys_s=None, kernel_s=None,
             rss_bytes=None, file_sha256=None)
    # a declared output file must be produced by THIS rep — delete
    # any prior artifact so a stale file can't satisfy verification
    for p in outs:
        if os.path.exists(p):
            os.remove(p)
    r = run(["/usr/bin/time", "-l" if BSD else "-v", exe, *args],
            timeout=TIMEOUT)
    m["exit_status"] = r.returncode
    m["stdout_sha256"] = hashlib.sha256(r.stdout).hexdigest()
    k = KERNEL_RE.search(r.stdout)
    if k:
        m["kernel_s"] = float(k.group(1))
    m.update(_parse_time(r.stderr))
    if outs:
        m["file_sha256"] = _file_digest(outs)
    if r.returncode != 0:
        err = r.stderr.decode("utf-8", "replace")[:300]
        print(f"  !! run {exe}: rc={r.returncode} {err}")
    return m

# --- server measurement -----------------------------------------

def measure_server_once(cell, port):
    """One server rep. Verification = driver rc==0 AND deterministic
    driver stdout digest; the server's own rc is recorded too."""
    drv_name, nconn = cell["driver"], cell["nconn"]
    driver = os.path.join(OUT, drv_name)
    m = dict(wall_s=None, user_s=None, sys_s=None, kernel_s=None,
             rss_bytes=None, file_sha256=None, stdout_sha256=None)
    p = subprocess.Popen(["/usr/bin/time", "-l" if BSD else "-v",
                          cell["exe"], str(port), str(nconn)],
                         stdout=subprocess.DEVNULL,
                         stderr=subprocess.PIPE)
    t = time.monotonic()
    d = run([driver, str(port), str(nconn)])
    wall = time.monotonic() - t
    try:
        _, serr = p.communicate(timeout=60)
    except subprocess.TimeoutExpired:
        p.kill()
        _, serr = p.communicate()
        print(f"  !! {cell['work']}_{cell['lang']} rep timed out")
        m["exit_status"] = -3
        return m
    if d.returncode != 0:
        print(f"  !! {cell['work']}_{cell['lang']} driver rc={d.returncode}")
        m["exit_status"] = -2
        return m
    m["exit_status"] = p.returncode  # the SERVER's own rc, distinct
    m["wall_s"] = wall             # driver wall = load absorbed
    m["stdout_sha256"] = hashlib.sha256(d.stdout).hexdigest()
    m.update(_parse_time(serr))
    m["wall_s"] = wall             # driver wall overrides server wall
    return m

# --- statistics ---------------------------------------------------

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

def paired_diff_ci(a_rows, b_rows, rng, resamples=10000):
    """95% bootstrap CI of the per-iteration wall difference
    median(a_i - b_i), pairing by shared iteration index."""
    a = {r["iteration"]: r["wall_s"] for r in a_rows
         if r["wall_s"] is not None}
    b = {r["iteration"]: r["wall_s"] for r in b_rows
         if r["wall_s"] is not None}
    diffs = [a[i] - b[i] for i in a.keys() & b.keys()]
    if len(diffs) < 2:
        return None
    lo, hi = boot_ci(diffs, rng, resamples)
    return statistics.median(diffs), lo, hi

def complete_groups(verdicts):
    """A (work, mode) group is complete only when EVERY expected
    implementation is valid — partial groups prohibit claims."""
    groups = {}
    for (work, _lang, mode), v in verdicts.items():
        groups.setdefault((work, mode), True)
        if v != "yes":
            groups[(work, mode)] = False
    return {k for k, ok in groups.items() if ok}

def compute_pairs(bycell, valid_keys, done_groups):
    """Paired diff CIs — only between valid cells inside complete
    groups, with all repetitions present (REPS enforced upstream)."""
    pairs = []
    for (work, lang, mode), rows in bycell.items():
        if lang == "ours" or (work, mode) not in done_groups:
            continue
        if (work, lang, mode) not in valid_keys:
            continue
        base_key = (work, "ours", mode)
        if base_key not in valid_keys:
            continue
        r = paired_diff_ci(bycell[base_key], rows, random.Random(SEED))
        if r:
            pairs.append((work, mode, lang, *r))
    return pairs

def merge_legacy(old_rows, published_keys, valid_rows):
    """Drop legacy rows for cells being republished, then append the
    valid rows. Attempted-but-invalid cells are NOT in
    published_keys — their historical rows are preserved. Rows
    predating the `suite` column are labelled 'pre-1.1', never
    back-claimed as the current suite."""
    kept = []
    for r in old_rows:
        if (r["work"], r["lang"], r["mode"]) in published_keys:
            continue
        r["suite"] = r.get("suite") or "pre-1.1"
        kept.append(r)
    return kept + valid_rows

# --- run identity: manifest, checkpoint, publication -------------

def _sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 22), b""):
            h.update(chunk)
    return h.hexdigest()

def _worktree_state():
    """Digest of uncommitted changes across the tree — a resume must
    not pool reps measured under a different unrecorded compiler or
    bench state that `praetor_commit` alone cannot see."""
    r = run(["git", "-C", REPO, "diff", "HEAD"], text=True)
    diff = r.stdout if r.returncode == 0 else ""
    r = run(["git", "-C", REPO, "status", "--porcelain=v1"],
            text=True)
    status = r.stdout if r.returncode == 0 else ""
    h = hashlib.sha256()
    h.update(diff.encode())
    h.update(b"\x00")
    h.update(status.encode())
    return h.hexdigest()

def cell_key(work, lang, mode):
    return f"{work}|{lang}|{mode}"

def build_manifest(ok_cells, ok_srv, src_digests, only, commit):
    """Run identity written before the first rep. On RESUME the
    stored manifest must match field-for-field before any new rep
    is pooled with the checkpoint — incompatible experiments never
    share a rundir."""
    both = ok_cells + ok_srv
    return {
        "suite": SUITE, "seed": SEED, "gen_seed": GEN_SEED,
        "warmup": WARMUP, "timeout_s": TIMEOUT,
        "reps_per_scale": REPS_SCALE, "reps_env": REPS,
        "only": only, "langs": list(LANGS), "scales": list(SCALES),
        "praetor_commit": commit,
        "worktree_sha256": _worktree_state(),
        "src_sha256": src_digests,
        "input_sha256": {cell_key(c["work"], c["lang"], c["mode"]):
                         c["input_sha256"] for c in both},
        "cells": {cell_key(c["work"], c["lang"], c["mode"]):
                  c["reps"] for c in both},
    }

def verify_manifest(stored, current):
    """Stored checkpoint manifest vs this run's identity. Returns a
    list of human-readable mismatches; empty means resumable."""
    errs = []
    for k in ("suite", "seed", "gen_seed", "warmup", "timeout_s",
              "reps_per_scale", "reps_env", "only", "langs",
              "scales", "praetor_commit", "worktree_sha256",
              "src_sha256"):
        if stored.get(k) != current.get(k):
            errs.append(f"{k}: {stored.get(k)!r} != "
                        f"{current.get(k)!r}")
    s, c = stored.get("cells", {}), current.get("cells", {})
    if s != c:
        only_s = sorted(set(s) - set(c))
        only_c = sorted(set(c) - set(s))
        rep_diff = sorted(k for k in set(s) & set(c)
                          if s[k] != c[k])
        errs.append(f"cells: only-in-checkpoint={only_s} "
                    f"only-in-run={only_c} rep-diff={rep_diff}")
    s, c = (stored.get("input_sha256", {}),
            current.get("input_sha256", {}))
    if s != c:
        diff = sorted(k for k in set(s) | set(c)
                      if s.get(k) != c.get(k))
        errs.append(f"input_sha256 mismatch: {diff}")
    return errs

REQUIRED_ROW_FIELDS = ("workload", "lang", "mode", "iteration",
                       "exit_status", "suite", "seed")

def load_raw_jsonl(path, expected):
    """Load a raw.jsonl checkpoint and REPAIR it for append.

    Record fates:
      * incomplete FINAL record (no trailing newline — the signature
        of a torn append) — the physical file is truncated back to
        the last complete record before returning, so the next
        append never concatenates onto the fragment
      * corrupt or malformed COMPLETED record (newline-terminated,
        any position) — hard refusal: a fully-written bad record is
        corruption, not a torn write
      * duplicate rep key — hard refusal
      * rows outside `expected` — kept in the file, counted as
        dropped (not this run's matrix)
    `expected` maps (work, lang, mode) -> reps."""
    raw, done, seen, dropped = [], set(), set(), 0
    with open(path, "rb") as f:
        lines = f.readlines()  # newline preserved; tail may lack it
    trunc_at = None
    last = len(lines) - 1
    for i, line in enumerate(lines):
        if not line.strip():
            continue
        complete = line.endswith(b"\n")
        is_last = (i == last)
        try:
            rec = json.loads(line)
        except json.JSONDecodeError:
            rec = None
        if rec is None or any(k not in rec
                              for k in REQUIRED_ROW_FIELDS):
            if is_last and not complete:
                # torn final append — drop the fragment on disk
                trunc_at = sum(len(l) for l in lines[:i])
                print(f"  !! raw.jsonl: incomplete final record "
                      f"(line {i + 1}) — checkpoint truncated")
                continue
            if rec is None:
                sys.exit(f"bench: {path}:{i + 1} corrupt record "
                         f"— refusing resume")
            missing = [k for k in REQUIRED_ROW_FIELDS
                       if k not in rec]
            sys.exit(f"bench: {path}:{i + 1} missing field(s) "
                     f"{missing} — refusing resume")
        key = (rec["workload"], rec["lang"], rec["mode"])
        rep_key = (*key, rec["iteration"])
        if rep_key in seen:
            sys.exit(f"bench: {path}:{i + 1} duplicate rep "
                     f"{rep_key} — refusing resume")
        seen.add(rep_key)
        if expected.get(key, -1) <= rec["iteration"]:
            dropped += 1
            continue
        raw.append(rec)
        done.add(rep_key)
    if trunc_at is not None:
        with open(path, "r+b") as f:
            f.truncate(trunc_at)
    elif lines and lines[-1] and not lines[-1].endswith(b"\n"):
        # complete-but-unterminated final record (never produced by
        # our writer, but cheap to defend) — terminate it so the
        # next append lands on a fresh line
        with open(path, "ab") as f:
            f.write(b"\n")
    return raw, done, dropped

RESULTS_FIELDS = ["work", "lang", "mode", "suite", "compile_s",
                  "run_s", "rss", "size", "loc"]

def publish_gate(only, reps_env, manifest_only=None):
    """bench/results.csv is canonical history — only an explicit,
    unfiltered, standard-reps run may write it. (ok, reason)"""
    if os.environ.get("PUBLISH") != "1":
        return False, "PUBLISH=1 not set"
    if only:
        return False, f"filtered run (ONLY={only!r})"
    if reps_env:
        return False, "non-standard rep counts (REPS env set)"
    if manifest_only:
        return False, (f"resumed run was filtered "
                       f"(manifest ONLY={manifest_only!r})")
    return True, None

# --- environment --------------------------------------------------

def _git_sha():
    r = run(["git", "-C", REPO, "rev-parse", "HEAD"], text=True)
    return r.stdout.strip() if r.returncode == 0 else "unknown"

def _ram_bytes():
    if BSD:
        r = run(["sysctl", "-n", "hw.memsize"], text=True)
        return int(r.stdout.strip()) if r.returncode == 0 else 0
    try:
        for l in open("/proc/meminfo"):
            if l.startswith("MemTotal:"):
                return int(l.split()[1]) * 1024
    except OSError:
        pass
    return 0

def collect_environment():
    return {
        "suite": SUITE,
        "grade": "lab-grade",
        "praetor_commit": _git_sha(),
        "os": platform.system(),
        "os_release": platform.release(),
        "arch": platform.machine(),
        "cpu": platform.processor() or "unknown",
        "cpu_count": os.cpu_count(),
        "ram_bytes": _ram_bytes(),
        "python": platform.python_version(),
        "hostname": platform.node(),
        "reps_per_scale": REPS_SCALE, "reps_env": REPS,
        "warmup": WARMUP, "seed": SEED, "gen_seed": GEN_SEED,
        "scales": SCALES, "tracks": TRACKS,
        "timeout_s": TIMEOUT,
        "runtime_srcs": MMLRT_SRCS, "rt_ldlibs": RT_LDLIBS,
        "flags": FLAGS,
    }

def collect_compiler_versions():
    lines = []
    for name, cmd in [("ourc", [OURC, "--version"]),
                      ("clang", [CC, "--version"]),
                      ("rustc", ["rustc", "--version"]),
                      ("clang++", ["clang++", "--version"])]:
        r = run(cmd, text=True)
        v = r.stdout.strip().splitlines()[0] if r.returncode == 0 \
            and r.stdout.strip() else "unavailable"
        lines.append(f"{name}: {v}")
    lines.append(f"praetor_commit: {_git_sha()}")
    return "\n".join(lines) + "\n"

# --- main ---------------------------------------------------------

RAW_FIELDS = ["workload", "track", "lang", "mode", "iteration",
              "wall_s", "user_s", "sys_s", "kernel_s", "rss_bytes",
              "exit_status", "stdout_sha256", "file_sha256",
              "input_sha256", "praetor_commit", "suite", "seed"]

def classify(cell, rows, group_digests, group_inputs):
    """validity: 'yes' or a failure reason. Order matters — a build
    failure explains the empty row set."""
    if cell.get("missing"):
        return "missing-src"
    if cell.get("build_error"):
        return "build-failed"
    if len(rows) != cell.get("reps", REPS or 0):
        return "incomplete-reps"
    if any(r["exit_status"] != 0 for r in rows):
        return "run-failed"
    if cell.get("input_missing"):
        return "missing-input"
    if any(r["wall_s"] is None for r in rows):
        return "missing-timing"
    digs = {(r["stdout_sha256"], r["file_sha256"]) for r in rows}
    if len(digs) > 1:
        return "digest-mismatch"
    key = (cell["work"], cell["mode"])
    if len(group_inputs.get(key, set())) > 1:
        return "input-mismatch"
    if len(group_digests.get(key, set())) > 1:
        return "work-mismatch"
    return "yes"

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
    commit = _git_sha()

    # fail fast: an explicit publish request on a filtered or
    # non-standard-reps run is refused before a suite-length wait
    if os.environ.get("PUBLISH") == "1":
        ok_pub, why = publish_gate(only, REPS)
        if not ok_pub:
            sys.exit(f"bench: refusing to publish — {why}")

    # Phase 1 — build every expected cell; failures are RECORDED as
    # cells so a missing implementation can't silently succeed.
    cells, srv_cells = [], []
    for name in sorted(WORKLOADS):
        for scale in SCALES:
            wkey = f"{name}.{scale}"
            if only and only not in wkey:
                continue
            reps = REPS or REPS_SCALE[scale]
            for mode in ("debug", "release"):
                rel = mode == "release"
                for lang in LANGS:
                    src = os.path.join(
                        SRC, f"{WORK_SRC.get(name, name)}.{EXT[lang]}")
                    stem = f"{wkey}_{mode}_{lang}"
                    c = dict(work=wkey, lang=lang, mode=mode,
                             server=False, scale=scale, reps=reps,
                             compile_s=None, size=None,
                             loc=None, missing=False, build_error=False)
                    if not os.path.exists(src):
                        c["missing"] = True
                        cells.append(c)
                        continue
                    c["loc"] = sum(1 for _ in open(src))
                    c["src_path"] = src
                    exe, ct = BUILDERS[lang](src, stem, rel)
                    c["compile_s"] = ct
                    if not exe:
                        c["build_error"] = True
                        cells.append(c)
                        continue
                    c["exe"] = exe
                    c["size"] = os.path.getsize(exe)
                    c["args"], ins, c["outs"] = resolve_args(
                        name, scale, stem)
                    dig = _input_digest(ins)
                    c["input_sha256"] = dig or "MISSING-INPUT"
                    c["input_missing"] = dig is None
                    cells.append(c)
    for name in sorted(SERVERS):
        for scale in SCALES:
            wkey = f"{name}.{scale}"
            if only and only not in wkey:
                continue
            reps = REPS or REPS_SCALE[scale]
            drv, nconn = SERVERS[name][scale]
            for mode in ("debug", "release"):
                for lang in LANGS:
                    src = os.path.join(SRC, f"{name}.{EXT[lang]}")
                    stem = f"{wkey}_{mode}_{lang}"
                    c = dict(work=wkey, lang=lang, mode=mode,
                             server=True, scale=scale, reps=reps,
                             driver=drv, nconn=nconn,
                             compile_s=None, size=None,
                             loc=None, missing=False, build_error=False)
                    if not os.path.exists(src):
                        c["missing"] = True
                        srv_cells.append(c)
                        continue
                    c["loc"] = sum(1 for _ in open(src))
                    c["src_path"] = src
                    exe, ct = BUILDERS[lang](src, stem, mode == "release")
                    c["compile_s"] = ct
                    if not exe:
                        c["build_error"] = True
                        srv_cells.append(c)
                        continue
                    c["exe"] = exe
                    c["size"] = os.path.getsize(exe)
                    # server "input" = driver + request count
                    dig = _input_digest([(drv, False), (str(nconn), False)])
                    c["input_sha256"] = dig or "MISSING-INPUT"
                    c["input_missing"] = dig is None
                    srv_cells.append(c)

    # Phase 2 — warmup rounds then measured rounds, cells interleaved
    # with a seeded per-round shuffle. Workloads and servers follow
    # the same schedule; server reps carry an unused-port counter.
    #
    # The run directory + raw.jsonl checkpoint exist BEFORE any rep
    # runs: every rep is appended and fsynced on completion, so a
    # crash loses only the in-flight rep. RESUME=<out-dir> reloads a
    # partial run's raw.jsonl and skips recorded reps.
    rng = random.Random(SEED)
    ok_cells = [c for c in cells if not (c["missing"] or c["build_error"])]
    ok_srv = [c for c in srv_cells if not (c["missing"] or c["build_error"])]

    resume_from = os.environ.get("RESUME")
    if resume_from:
        rundir = os.path.abspath(resume_from)
    else:
        rundir = os.path.join(ROOT, "out",
                              time.strftime("%Y%m%d-%H%M%S"))
    os.makedirs(rundir, exist_ok=True)
    raw_path = os.path.join(rundir, "raw.jsonl")
    manifest_path = os.path.join(rundir, "run-manifest.json")

    src_digests = {}
    for c in cells + srv_cells:
        p = c.get("src_path")
        if p and os.path.exists(p):
            src_digests.setdefault(os.path.basename(p),
                                   _sha256_file(p))
    manifest = build_manifest(ok_cells, ok_srv, src_digests,
                              only, commit)

    # run identity files up front — a partial rundir must be
    # self-describing even if the suite never reaches Phase 4. On
    # resume the originals are kept: they describe when the loaded
    # reps ran (per-row praetor_commit preserves provenance), and the
    # stored manifest is NEVER rewritten — only verified.
    manifest_only = None
    if resume_from:
        if not os.path.exists(manifest_path):
            sys.exit(f"bench: {manifest_path} missing — cannot "
                     f"verify resume compatibility")
        stored = json.load(open(manifest_path))
        manifest_only = stored.get("only")
        errs = verify_manifest(stored, manifest)
        if errs:
            detail = "\n    ".join(errs)
            sys.exit(f"bench: RESUME refused — checkpoint manifest "
                     f"does not match this run:\n    {detail}")
    else:
        if os.path.exists(raw_path):
            # timestamp collision into a populated rundir —
            # appending would silently pool two experiments
            sys.exit(f"bench: {raw_path} already exists — pass "
                     f"RESUME={rundir} to continue that run")
        with open(manifest_path, "w") as f:
            json.dump(manifest, f, indent=2)
        with open(os.path.join(rundir, "environment.json"), "w") as f:
            json.dump(collect_environment(), f, indent=2)
        with open(os.path.join(rundir, "compiler-versions.txt"),
                  "w") as f:
            f.write(collect_compiler_versions())

    expected = {(c["work"], c["lang"], c["mode"]): c["reps"]
                for c in ok_cells + ok_srv}
    raw, done = [], set()
    if resume_from and os.path.exists(raw_path):
        raw, done, dropped = load_raw_jsonl(raw_path, expected)
        print(f"RESUME {rundir}: {len(raw)} prior reps loaded"
              + (f", {dropped} rows dropped (not in this matrix)"
                 if dropped else ""))
    total_reps = sum(expected.values())
    done_ctr = [len(done)]
    jsonl = open(raw_path, "a")

    def record(row):
        raw.append(row)
        jsonl.write(json.dumps(row) + "\n")
        jsonl.flush()
        os.fsync(jsonl.fileno())
        done_ctr[0] += 1
        print(f"  [{done_ctr[0]:>4}/{total_reps}] {row['workload']} "
              f"{row['lang']} {row['mode']} it={row['iteration']} "
              f"wall={row['wall_s']}s rc={row['exit_status']}")
        sys.stdout.flush()

    def guarded_measure(fn, *a):
        """A measurement exception is a recorded failed rep
        (exit_status -9 → cell invalid), never a suite abort."""
        try:
            return fn(*a)
        except Exception as e:
            print(f"  !! measure raised: {e!r}")
            return dict(wall_s=None, user_s=None, sys_s=None,
                        kernel_s=None, rss_bytes=None,
                        file_sha256=None,
                        stdout_sha256=hashlib.sha256(b"").hexdigest(),
                        exit_status=-9)

    def pending(c):
        return any((c["work"], c["lang"], c["mode"], it) not in done
                   for it in range(c["reps"]))

    for _ in range(WARMUP):
        for c in ok_cells:
            if pending(c):  # resume warms only cells still owed reps
                try:
                    run([c["exe"], *c["args"]], timeout=TIMEOUT)
                except Exception as e:
                    print(f"  !! warmup {c['work']}_{c['lang']}: {e!r}")
    # measured rounds: rounds = max reps across live cells; a cell
    # sits out once its own rep count is met — the interleave holds.
    port_ctr = [0]
    max_reps = max([c["reps"] for c in ok_cells + ok_srv] or [0])
    print(f"total reps: {total_reps} ({len(done)} already recorded)")
    for it in range(max_reps):
        order = [c for c in ok_cells if it < c["reps"]]
        rng.shuffle(order)
        for c in order:
            key = (c["work"], c["lang"], c["mode"], it)
            if key in done:
                continue
            m = guarded_measure(measure_one, c["exe"], c["args"],
                                c["outs"])
            record(dict(workload=c["work"], track=track_of(c["work"]),
                        lang=c["lang"], mode=c["mode"],
                        iteration=it, praetor_commit=commit,
                        input_sha256=c["input_sha256"],
                        suite=SUITE, seed=SEED, **m))
        srv_order = [c for c in ok_srv if it < c["reps"]]
        rng.shuffle(srv_order)
        for c in srv_order:
            key = (c["work"], c["lang"], c["mode"], it)
            if key in done:
                continue
            port_ctr[0] += 1
            m = guarded_measure(measure_server_once, c,
                                18000 + port_ctr[0])
            record(dict(workload=c["work"], track=track_of(c["work"]),
                        lang=c["lang"], mode=c["mode"],
                        iteration=it, praetor_commit=commit,
                        input_sha256=c["input_sha256"],
                        suite=SUITE, seed=SEED, **m))
    jsonl.close()

    # Phase 3 — validity classification. Cross-language digest checks
    # group by (work, mode): every language's rep digests — stdout,
    # output files AND inputs — must agree.
    group_digests, group_inputs = {}, {}
    for r in raw:
        if r["exit_status"] == 0:
            k = (r["workload"], r["mode"])
            group_digests.setdefault(k, set()).add(
                (r["stdout_sha256"], r["file_sha256"]))
            group_inputs.setdefault(k, set()).add(r["input_sha256"])

    bycell = {}
    for r in raw:
        bycell.setdefault((r["workload"], r["lang"], r["mode"]),
                          []).append(r)
    all_cells = {(c["work"], c["lang"], c["mode"]): c
                 for c in cells + srv_cells}

    summary, bad, verdicts = [], [], {}
    for key in sorted(all_cells):
        work, lang, mode = key
        c = all_cells[key]
        rows = bycell.get(key, [])
        walls = [r["wall_s"] for r in rows if r["wall_s"] is not None]
        rss = max([r["rss_bytes"] or 0 for r in rows] or [0])
        verdict = classify(c, rows, group_digests, group_inputs)
        verdicts[key] = verdict
        s = summarize(walls) if len(walls) >= 2 else dict(
            n=len(walls), median_s=walls[0] if walls else None,
            ci_lo_s=None, ci_hi_s=None, mean_s=None, sd_s=None,
            min_s=None, max_s=None, p5_s=None, p95_s=None, cv=None)
        row = dict(workload=work, track=track_of(c["work"]), lang=lang, mode=mode,
                   valid=verdict, compile_s=c["compile_s"],
                   peak_rss_bytes=rss, size_bytes=c["size"],
                   loc=c["loc"], **s)
        (summary if verdict == "yes" else bad).append(row)
        med = f'{s["median_s"]:7.3f}' if s["median_s"] is not None else "    n/a"
        lo = s["ci_lo_s"] if s["ci_lo_s"] is not None else 0
        hi = s["ci_hi_s"] if s["ci_hi_s"] is not None else 0
        print(f"{work:10} {lang:4} {mode:7} n={s['n']:>2} "
              f"med={med}s [{lo:.3f}–{hi:.3f}] "
              f"rss={rss:>10} size={c['size'] or 0:>9} loc={c['loc'] or 0:>4}"
              + ("" if verdict == "yes" else f"  !! {verdict}"))

    # paired ours-vs-{c,rs} difference CIs per (work, mode) — only
    # between valid cells in COMPLETE groups; a close result is only
    # a claim when the paired CI excludes zero.
    valid_keys = {k for k, v in verdicts.items() if v == "yes"}
    done = complete_groups(verdicts)
    pairs = compute_pairs(bycell, valid_keys, done)

    # Phase 4 — artifacts. rundir/environment/compiler-versions were
    # already written before Phase 2 (self-describing partial runs);
    # refreshed here so they describe the run that finished.
    ts = os.path.basename(rundir)

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
        w.writerows(summary + bad)

    with open(os.path.join(rundir, "environment.json"), "w") as f:
        json.dump(collect_environment(), f, indent=2)
    with open(os.path.join(rundir, "compiler-versions.txt"), "w") as f:
        f.write(collect_compiler_versions())

    with open(os.path.join(rundir, "report.md"), "w") as f:
        f.write(f"# {SUITE} — {ts} (lab-grade)\n\n")
        f.write("| workload | lang | mode | n | median s | 95% CI | "
                "rss B | size B | loc |\n|---|---|---|---|---|---|---|---|---|\n")
        for r in summary:
            f.write(f"| {r['workload']} | {r['lang']} | {r['mode']} | "
                    f"{r['n']} | {r['median_s']:.4f} | "
                    f"{r['ci_lo_s']:.4f}–{r['ci_hi_s']:.4f} | "
                    f"{r['peak_rss_bytes']} | {r['size_bytes']} | "
                    f"{r['loc']} |\n")
        if pairs:
            f.write("\n## paired ours − {lang} wall diff (median, 95% CI)\n\n")
            f.write("| workload | mode | vs | median diff s | 95% CI |\n"
                    "|---|---|---|---|---|\n")
            for work, mode, lang, med, lo, hi in pairs:
                f.write(f"| {work} | {mode} | {lang} | {med:+.4f} | "
                        f"{lo:+.4f}–{hi:+.4f} |\n")
        if bad:
            f.write("\n## invalid cells (excluded above)\n\n")
            f.write("| workload | lang | mode | reason | n |\n"
                    "|---|---|---|---|---|\n")
            for r in bad:
                f.write(f"| {r['workload']} | {r['lang']} | {r['mode']} | "
                        f"{r['valid']} | {r['n']} |\n")

    # canonical results.csv — explicit PUBLISH=1 + full unfiltered
    # standard-reps matrix only. Only VALID cells publish; an
    # attempted-but-invalid cell's historical rows are preserved and
    # reported, never silently replaced by nothing.
    csv_path = os.path.join(ROOT, "results.csv")
    ok_pub, why = publish_gate(only, REPS, manifest_only)
    if ok_pub:
        published = {(r["workload"], r["lang"], r["mode"])
                     for r in summary}
        new_rows = [dict(work=r["workload"], lang=r["lang"],
                         mode=r["mode"], suite=SUITE,
                         compile_s=r["compile_s"],
                         run_s=r["median_s"],
                         rss=r["peak_rss_bytes"], size=r["size_bytes"],
                         loc=r["loc"]) for r in summary]
        legacy = new_rows
        if os.path.exists(csv_path):
            with open(csv_path) as f:
                legacy = merge_legacy(list(csv.DictReader(f)),
                                      published, new_rows)
        with open(csv_path, "w", newline="") as f:
            w = csv.DictWriter(f, fieldnames=RESULTS_FIELDS)
            w.writeheader()
            w.writerows(legacy)
        print(f"published {len(new_rows)} cells to {csv_path}")
        if bad:
            print(f"  !! {len(bad)} invalid cells — historical "
                  f"rows preserved:")
            for r in bad:
                print(f"     {r['workload']} {r['lang']} "
                      f"{r['mode']}: {r['valid']}")
    else:
        print(f"results.csv NOT updated: {why}")

    print(f"\nwrote {rundir} ({len(raw)} raw rows, {len(summary)} valid / "
          f"{len(bad)} invalid cells)")

if __name__ == "__main__":
    main()
