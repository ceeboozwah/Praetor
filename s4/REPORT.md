# CHAN-004 S4 — End-to-End Ownership Validation Report

Objective: challenge the complete bounded-channel implementation
(`bchannel` / `send` → `Result<void,T>` / `recv` → `option.Option<T>` /
`close`) from merged `main` (`16f6fd6`), without adding language
features. Validation is adversarial and experimental — passing finite
tests is evidence of correctness in the tested state space, **not** a
proof of universal soundness.

Branch: `chan/s4-validation`. All findings below are safety-impacting
corrections and require review before merge.

## Infrastructure

- `s4/gen_scenarios.py` — deterministic seeded generator. `--seed S`
  emits the identical corpus on any host; `manifest.jsonl` records
  per-program expected verdict (`exit:N` / `reject:<substr>` /
  `timeout`), family, payload, and seed. Families:
  - `pc` — P producer tasks × C consumer tasks, fixed per-consumer
    quotas, capacities 1–4, all send/recv ownership paths. Pigeonhole
    arguments guarantee delivery totals regardless of schedule.
  - `close` — queued sends, a close-only task, and drain inside the
    legal unjoined-closer window.
  - `close_race` — a send blocked on a full queue racing close;
    deterministically resolves `Err` while the queued packet drains
    as `Some`.
  - `gap` — a consumer task analyzed before the producer merges:
    `PendingRecv.pkt == None` at check time while a physical `Some`
    can deliver at runtime. Emitted for every payload category, in
    both `inert` (accept) and `use` (must-reject) variants.
  - `deadlock` — accepted programs that block forever; the harness
    must bound them (`expect: timeout`).
  - `bad` — intentionally invalid programs (use-after-send,
    unexamined `Result`, wildcard match, abandoned owned payload,
    source write through transit, `None`-arm binder use), each with
    an expected diagnostic substring.
- `s4/oracle_tail.c` — runtime ownership oracle. The harness patches
  a copy of `mmlrt_common.c` (`OUR_RUNTIME` override; zero compiler
  changes) renaming `mml_alloc`/`mml_free` and appending tracking
  wrappers. Reports on stderr as `ORACLE:` lines: `double-free` and
  `unowned-free` (fatal), `free-null`, `leaked` (at exit), and
  poisons freed bytes (`0xDD`) for probabilistic stale-read
  detection. Self-tested against all four violation classes.
- `s4/run_s4.py` — harness: stage-0 verdict, stage-1 verdict (must
  agree), expected-verdict check, stage-0 binary run, oracle-runtime
  build+run (must agree), `-fsanitize=address` build+run at `-O1`
  (must agree), stage-1 binary run (must agree). Python-side timeout
  on every step — works without GNU `timeout`. Writes
  `results.jsonl` with seed, expected and actual verdicts,
  diagnostics, and all run results.

## Results

- 148 scenarios across seeds 1–3, 10–14, 20–23: **148/148 clean**
  (39/39 + 56/56 + 53/53). Every intentionally-invalid program rejected with
  the expected diagnostic; every accepted program produced identical
  exit codes on stage-0, stage-1, oracle, and ASan binaries; every
  deadlock program bounded at the declared timeout on both stages.
- Checker parity `scripts/check-parity.sh 1 818`: **818/818**.
- `cargo test -p ourc`: full suite green including native corpus.
- Bootstrap: stage-0 `our.ll` ≡ stage-1 `our.ll` byte-identical;
  stage-2 → stage-3 byte-identical `.ll` and `.o` fixpoint.
- Fixture 818 (the esz regression) exits 42 on s0/s1/s2/s3.

## Defects found (all safety-impacting; need review before merge)

### F1 — `bchannel` element-size floor corrupts the stack (real runtime defect)

`bchannel T` was created with `esz = max(size_of(T), 8)`, but bounded
send/recv materialize payload-typed cells (e.g. `alloca i32`, 4
bytes). The runtime `memcpy`s `esz` bytes: send over-read the source
cell; recv wrote 4 bytes past the destination. Layout-dependent
corruption — silent at `-O0`, deterministic crash at `-O1`.

Minimized reproduction (pre-fix, `-O1`, also crashes under ASan as a
BUS in `mml_task_join`): `bchannel i32` cap 3 → `send(ch,10)` →
spawn `close(ch)` → two `recv` → `join` → inspect results. The
second `Some` delivery overwrote the live task handle. Bisection:
send-before-spawn + two `Some` deliveries was the minimal trigger.

Fix: `bchannel` emits `esz = size_of(T)` exactly
(`compiler/src/ir/gen.rs`, `selfhost/ir.lang`). The unbounded
`channel` path is self-consistent (i64 cells end-to-end) and is
unchanged. Regression: `native-tests/818_bchan_recv_small_payload.lang`.

### F2 — shared `&T` payloads lost their named target (checker defect)

`packet_names` only counted `sent` transit marks and bot candidate
sets. A `&Point` payload transits nothing, so `a` never entered
`chan_names`; the unordered claim minted a ⊥ edge with an
insufficient candidate set and `(*v).x` read as uninitialized —
`recv(bchannel &T)` was unusable. Fail-closed (rejected valid code),
never unsafe.

Fix: `packet_names`/`packet_names_add` now seed the endpoint's
candidate union with all non-`Own`, non-bot edge targets and non-bot
cap targets; `is_bot_key` added to selfhost to match stage-0's
name-pattern test (a packet can carry a forwarded bot whose row
lives in a sibling snapshot). Regression:
`native-tests/815_bchan_recv_shared_ref.lang`.

### F3 — stage-1 missed owned-payload abandonment inside generic enums (parity defect)

Stage-1 `live_own_at` descended generic enum kinds by reading the
declaration's raw variant payloads — still named `T` — so
`Option<box i32>` (and every generic-enum owned payload) slipped the
abandon check stage-0 enforces. Stage-0 rejected `return` paths
leaving `o.Some` live; stage-1 silently accepted.

Fix: `live_own_at` instantiates variants through the existing
`resolve.subst_variants` machinery before the recursive owned check
(kind 13). Regressions: `native-tests/816_err_enum_abandon_own.lang`,
`native-tests/817_err_bchan_recv_abandon.lang`.

## Coverage limitations (explicit)

- **Unordered ⊥ reads stay unresolved.** A `&`/`&mut` payload
  claimed through a multi-candidate ⊥ edge supports writes but not
  reads (candidate identity is unprovable). Pre-existing candidate
  model limitation, fail-closed, shared by `channel`.
- **Oracle scope is user allocations only.** `mml_alloc`/`mml_free`
  cover `alloc<>`/`box`; queue internals use raw `malloc`. Freed
  bytes are poisoned (probabilistic); deterministic UAF coverage
  comes from the ASan leg.
- **ASan is POSIX/macOS best-effort**; Windows uses the oracle leg.
  TSan is unusable for this runtime on macOS-arm64 (internal
  `ConsumeThreadUserId` failure on `pthread_join`) — data-race
  detection at the runtime level is not covered.
- **Schedules are generated, not exhaustive.** `pc` uses pigeonhole
  quotas for deterministic totals; `close_race` exercises the
  blocked-send-vs-close window structurally. Interleavings are not
  systematically enumerated.
- **`pc` family payloads**: i32, box, mutref, nest, pair observed;
  `res`/`ref` appear via `gap`/`close` instead. `cap` for ref
  payloads is bounded below by the total send count to keep the
  post-join main-drain shape deadlock-free.
- **`bad` expectations are diagnostic substrings**, not full-error
  equality; the two weakest are `source_write`/`none_use` (any
  rejection matches, not yet tightened to a specific diagnostic).
- The `pkt=None` gap remains experimentally validated for the tested
  cases (per S3b review): fail-closed for reference payloads,
  self-authorizing for owned payloads. Not a universal proof.
