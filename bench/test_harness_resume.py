#!/usr/bin/env python3
"""bench/test_harness_resume.py — focused tests for the harness
checkpoint/resume/publish hardening (PR-35 review requirements).

Covers:
  * interruption during checkpoint writing — a truncated final
    raw.jsonl record is skipped, not fatal
  * corrupt INTERIOR checkpoint records — hard refusal
  * malformed rows and duplicate rep keys — hard refusal
  * resume identity — manifest mismatches are reported, not pooled
  * canonical-results protection — publish requires PUBLISH=1 AND a
    full unfiltered standard-reps matrix
  * merge semantics — invalid attempted cells keep historical rows

Run: python3 bench/test_harness_resume.py
"""
import json, os, sys, tempfile
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import harness as H

PASS = []


def check(name, cond):
    PASS.append((name, cond))
    print(f"{'PASS' if cond else 'FAIL'} {name}")


def row(work, lang, mode, it, **kw):
    d = dict(workload=work, track="B", lang=lang, mode=mode,
             iteration=it, wall_s=1.0, user_s=1.0, sys_s=0.0,
             kernel_s=None, rss_bytes=1, exit_status=0,
             stdout_sha256="x", file_sha256=None,
             input_sha256="i", praetor_commit="c", suite="S", seed=1)
    d.update(kw)
    return d


def write_jsonl(path, lines):
    with open(path, "wb") as f:
        for l in lines:
            f.write(l if isinstance(l, bytes) else l.encode())


def exits(fn, *a):
    try:
        fn(*a)
        return None
    except SystemExit as e:
        return e


EXPECTED = {("w", "c", "release"): 3, ("w", "ours", "release"): 3}
ROWS = [row("w", "c", "release", i) for i in range(3)] + \
       [row("w", "ours", "release", i) for i in range(3)]


def test_truncated_tail():
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        good = [json.dumps(r) + "\n" for r in ROWS]
        frag = b'{"workload": "w", "lang": "c'  # torn write, no \n
        write_jsonl(p, good + [frag])
        raw, done, dropped = H.load_raw_jsonl(p, EXPECTED)
        check("truncated-final-record skipped", len(raw) == 6
              and len(done) == 6 and dropped == 0)
        # physical repair: fragment gone, file newline-terminated
        with open(p, "rb") as f:
            content = f.read()
        check("torn bytes truncated from disk",
              content.endswith(b"\n") and frag not in content
              and len(content) == sum(len(l) for l in good))


def test_newline_terminated_corrupt_tail_refused():
    """A corrupt record WITH its newline is not a torn write — it
    was fully written and must be refused even at the tail."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        good = [json.dumps(r) + "\n" for r in ROWS]
        write_jsonl(p, good + [b'{"workload": BROKEN}\n'])
        e = exits(H.load_raw_jsonl, p, EXPECTED)
        check("corrupt NEWLINE-TERMINATED tail refused",
              e is not None and e.code != 0)


def test_unterminated_malformed_tail_truncated():
    """A last line that parses-or-not but lacks required fields AND
    lacks its newline is an incomplete write — truncate, don't
    refuse."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        good = [json.dumps(r) + "\n" for r in ROWS]
        r = row("w", "c", "release", 0)
        del r["exit_status"]
        write_jsonl(p, good + [json.dumps(r)])  # valid JSON, no \n
        raw, done, dropped = H.load_raw_jsonl(p, EXPECTED)
        check("unterminated missing-field tail truncated",
              len(raw) == 6 and open(p, "rb").read().endswith(b"\n"))


def test_unterminated_valid_tail_terminated():
    """A complete record lacking only its newline (never produced by
    our writer) is kept AND the file is terminated for append."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        write_jsonl(p, [json.dumps(r) + "\n" for r in ROWS[:-1]]
                    + [json.dumps(ROWS[-1])])  # no trailing \n
        raw, done, dropped = H.load_raw_jsonl(p, EXPECTED)
        check("unterminated valid tail kept + terminated",
              len(raw) == 6
              and open(p, "rb").read().endswith(b"\n"))


def test_e2e_interrupt_append_resume():
    """End-to-end: interrupt during append, resume + append more,
    resume again — every completed row stays readable, unique, and
    classified."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        # run 1: writes 4 complete reps, then dies mid-append
        write_jsonl(p, [json.dumps(r) + "\n" for r in ROWS[:4]]
                    + [b'{"workload": "w", "lang'])
        # resume 1: loads 4, repairs tail, appends 2 more reps
        raw, done, dropped = H.load_raw_jsonl(p, EXPECTED)
        check("resume-1 loads pre-interruption reps", len(raw) == 4)
        with open(p, "a") as f:
            for r in ROWS[4:]:
                f.write(json.dumps(r) + "\n")
        # resume 2: all 6 reps readable + unique + classified
        raw, done, dropped = H.load_raw_jsonl(p, EXPECTED)
        keys = [(r["workload"], r["lang"], r["mode"],
                 r["iteration"]) for r in raw]
        check("resume-2: all rows readable+unique+classified",
              len(raw) == 6 and len(set(keys)) == 6
              and len(done) == 6 and dropped == 0)
        # the file itself is fully parseable — no poisoned line
        with open(p, "rb") as f:
            ok = all(json.loads(l) for l in f
                     if l.strip())
        check("checkpoint file fully valid after repair+append", ok)


def test_corrupt_interior():
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        good = [json.dumps(r) + "\n" for r in ROWS]
        good.insert(2, '{"workload": BROKEN\n')
        write_jsonl(p, good)
        e = exits(H.load_raw_jsonl, p, EXPECTED)
        check("corrupt-interior-record refused",
              e is not None and e.code != 0)


def test_missing_field():
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        r = row("w", "c", "release", 0)
        del r["exit_status"]
        write_jsonl(p, [json.dumps(r) + "\n"])
        e = exits(H.load_raw_jsonl, p, EXPECTED)
        check("malformed-row refused", e is not None and e.code != 0)


def test_duplicate_rep():
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        lines = [json.dumps(ROWS[0]) + "\n"] * 2
        write_jsonl(p, lines)
        e = exits(H.load_raw_jsonl, p, EXPECTED)
        check("duplicate-rep-key refused",
              e is not None and e.code != 0)


def test_out_of_matrix_dropped():
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "raw.jsonl")
        write_jsonl(p, [json.dumps(ROWS[0]) + "\n",
                        json.dumps(row("gone", "c", "release", 0))
                        + "\n"])
        raw, done, dropped = H.load_raw_jsonl(p, EXPECTED)
        check("off-matrix rows dropped",
              len(raw) == 1 and dropped == 1)


def _manifest(**over):
    m = dict(suite="S", seed=1, gen_seed=2, warmup=2, timeout_s=300,
             reps_per_scale={"sm": 30}, reps_env=None, only=None,
             langs=["ours"], scales=["sm"],
             praetor_commit="abc", worktree_sha256="w",
             src_sha256={"x.lang": "s"},
             input_sha256={"w|c|release": "i"},
             cells={"w|c|release": 30})
    m.update(over)
    return m


def test_manifest_compat():
    base = _manifest()
    check("identical manifests resumable",
          H.verify_manifest(base, _manifest()) == [])
    for k, label in [("seed", "seed"), ("suite", "suite"),
                     ("reps_env", "reps_env"),
                     ("praetor_commit", "commit"),
                     ("worktree_sha256", "worktree"),
                     ("src_sha256", "src digests")]:
        other = dict(_manifest())
        other[k] = "DIFFERENT" if not isinstance(other[k], dict) \
            else {"changed": "x"}
        errs = H.verify_manifest(base, other)
        check(f"manifest {label} mismatch detected",
              any(k in e or "src_sha256" in e for e in errs))
    errs = H.verify_manifest(
        base, _manifest(cells={"w|c|release": 30,
                               "extra|c|release": 30}))
    check("manifest cell-set mismatch detected", any("cells" in e
                                                   for e in errs))
    errs = H.verify_manifest(
        base, _manifest(input_sha256={"w|c|release": "DIFF"}))
    check("manifest input-digest mismatch detected",
          any("input_sha256" in e for e in errs))


def test_publish_gate():
    saved = dict(os.environ)
    try:
        os.environ.pop("PUBLISH", None)
        ok, _ = H.publish_gate(None, None)
        check("no PUBLISH=1 → not published", not ok)
        os.environ["PUBLISH"] = "1"
        ok, why = H.publish_gate("psum", None)
        check("PUBLISH+ONLY refused", not ok and "ONLY" in why)
        ok, why = H.publish_gate(None, 5)
        check("PUBLISH+REPS refused", not ok and "REPS" in why)
        ok, why = H.publish_gate(None, None, manifest_only="x")
        check("PUBLISH on filtered checkpoint refused", not ok)
        ok, _ = H.publish_gate(None, None)
        check("PUBLISH full standard matrix allowed", ok)
    finally:
        os.environ.clear()
        os.environ.update(saved)


def test_merge_preserves_history():
    old = [dict(work="w", lang="c", mode="release", suite="s1",
                compile_s=1, run_s=2, rss=3, size=4, loc=5),
           dict(work="w", lang="ours", mode="release", suite="s1",
                compile_s=1, run_s=2, rss=3, size=4, loc=5)]
    new = [dict(work="w", lang="c", mode="release", suite="s2",
                compile_s=1, run_s=9, rss=3, size=4, loc=5)]
    # only the valid 'c' cell publishes; the invalid 'ours' cell's
    # historical row must survive. Merge keys are
    # (suite, work, lang, mode) — s2 must not supersede s1's row.
    out = H.merge_legacy(old, {("s2", "w", "c", "release")}, new)
    keys = [(r["suite"], r["work"], r["lang"], r["mode"], r["run_s"])
            for r in out]
    check("invalid cell's historical row preserved",
          ("s1", "w", "ours", "release", 2) in keys)
    check("prior suite's matching cell preserved across revisions",
          ("s1", "w", "c", "release", 2) in keys)
    check("new suite cell appended", ("s2", "w", "c", "release", 9)
          in keys)


def _summary_row(work, lang, mode, med):
    return dict(workload=work, lang=lang, mode=mode, compile_s=1.0,
                median_s=med, peak_rss_bytes=3, size_bytes=4, loc=5)


def test_publish_preserves_prior_suites():
    """The PR-36-review case: publishing PraetorBench-1.3 must keep
    every 1.2 and pre-1.1 row — same cells, different suite."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "results.csv")
        import csv as _csv
        with open(p, "w", newline="") as f:
            w = _csv.DictWriter(f, fieldnames=H.RESULTS_FIELDS)
            w.writeheader()
            for i in range(288):
                w.writerow(dict(work=f"w{i}", lang="ours",
                                mode="release", suite="PraetorBench-1.2",
                                compile_s=1, run_s=2, rss=3, size=4,
                                loc=5))
            for i in range(48):
                w.writerow(dict(work=f"legacy{i}", lang="ours",
                                mode="release", suite="",
                                compile_s=1, run_s=2, rss=3, size=4,
                                loc=5))
        new = [_summary_row("w0", "ours", "release", 9.0)]
        n, total = H.publish_results(p, new, "PraetorBench-1.3")
        rows = list(_csv.DictReader(open(p)))
        check("publish returns counts", n == 1 and total == 337)
        suites = {}
        for r in rows:
            suites[r["suite"]] = suites.get(r["suite"], 0) + 1
        check("all 288 1.2 rows survive", suites.get("PraetorBench-1.2") == 288)
        check("48 pre-1.1 rows relabelled, not dropped",
              suites.get("pre-1.1") == 48)
        check("1.3 row appended", suites.get("PraetorBench-1.3") == 1)
        keys = [(r["suite"], r["work"], r["lang"], r["mode"]) for r in rows]
        check("canonical keys unique", len(keys) == len(set(keys)))


def test_publish_replaces_same_suite_only():
    """Re-publishing a suite supersedes only its own cells."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "results.csv")
        n, _ = H.publish_results(p, [_summary_row("w0", "ours",
                                                  "release", 1.0)],
                                 "sA")
        n, _ = H.publish_results(p, [_summary_row("w0", "ours",
                                                  "release", 2.0),
                                     _summary_row("w0", "c",
                                                  "release", 3.0)],
                                 "sB")
        n, total = H.publish_results(p, [_summary_row("w0", "ours",
                                                      "release", 4.0)],
                                     "sA")
        import csv as _csv
        rows = list(_csv.DictReader(open(p)))
        check("republish count", total == 3)
        sa = [r for r in rows if r["suite"] == "sA"]
        sb = [r for r in rows if r["suite"] == "sB"]
        check("sA replaced by its own republish",
              len(sa) == 1 and float(sa[0]["run_s"]) == 4.0)
        check("sB untouched by sA republish",
              len(sb) == 2)


def test_publish_atomic_and_backup():
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "results.csv")
        H.publish_results(p, [_summary_row("w0", "ours", "release", 1.0)],
                          "sA")
        first = open(p).read()
        H.publish_results(p, [_summary_row("w0", "c", "release", 2.0)],
                          "sB")
        check("backup holds pre-publish state",
              open(p + ".bak").read() == first)
        check("no temp file left", not os.path.exists(p + ".tmp"))


def test_publish_duplicate_refused():
    """A canonical file with duplicate keys must fail the pre-write
    check rather than being silently 'repaired' by overwrite."""
    with tempfile.TemporaryDirectory() as d:
        p = os.path.join(d, "results.csv")
        import csv as _csv
        with open(p, "w", newline="") as f:
            w = _csv.DictWriter(f, fieldnames=H.RESULTS_FIELDS)
            w.writeheader()
            for _ in range(2):   # same (suite,work,lang,mode) twice
                w.writerow(dict(work="w0", lang="ours",
                                mode="release", suite="sA",
                                compile_s=1, run_s=2, rss=3, size=4,
                                loc=5))
        try:
            H.publish_results(p, [_summary_row("w1", "ours",
                                               "release", 1.0)], "sB")
            check("duplicate canonical rows refused", False)
        except ValueError:
            check("duplicate canonical rows refused", True)


def main():
    test_truncated_tail()
    test_newline_terminated_corrupt_tail_refused()
    test_unterminated_malformed_tail_truncated()
    test_unterminated_valid_tail_terminated()
    test_e2e_interrupt_append_resume()
    test_corrupt_interior()
    test_missing_field()
    test_duplicate_rep()
    test_out_of_matrix_dropped()
    test_manifest_compat()
    test_publish_gate()
    test_merge_preserves_history()
    test_publish_preserves_prior_suites()
    test_publish_replaces_same_suite_only()
    test_publish_atomic_and_backup()
    test_publish_duplicate_refused()
    fails = [n for n, ok in PASS if not ok]
    print(f"\n{len(PASS) - len(fails)}/{len(PASS)} passed")
    sys.exit(1 if fails else 0)


if __name__ == "__main__":
    main()
