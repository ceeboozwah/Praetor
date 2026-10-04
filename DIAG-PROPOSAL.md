# DIAG-PROPOSAL — N35: diagnostics + source experience

Status: **spec**. N34C closed the "can the model do real work"
question (StrMap, ~42x on wordfreq). The next bottleneck is human:
can a developer read what the compiler says and work comfortably
for hours? This milestone is about the diagnostic surface, not new
semantics.

## What exists today — measured, not assumed

```text
Diag  = { span: Span{line,col}, msg: String, file: Option<String> }
render = "file:line:col: error: {msg}"        // one line, no snippet
edges = (holder, target) -> (EdgeKind, Cert)  // NO birth site
Diag::new sites in check.rs: 213
```

So today `cannot consume \`m\`: it is borrowed` is the whole story —
the *why* ("borrowed where?") and the *where* (source text) are both
absent. Everything downstream is a renderer question once `Diag`
carries labels + notes.

## Goals

```text
error[E0301]: cannot consume `m`: it is borrowed
  --> prog.lang:32:21
   |
30 |         Ok(v) => {
31 |             //   v borrows m.slots
32 |             let r = m.insert(kz, i64(99));
   |                     ^^^^^^^^^^^^^^^^ consuming `m` here
   |
note: `m` borrowed by `get`
  --> prog.lang:27:14
   |
27 |     match m.get(k5) {
   |              ^^^^^^^ borrow created here
```

## Staging

**N35A — Diag structure + snippet renderer.** The foundation; pure
plumbing, zero semantic change.

- `Span` stays `{line,col}`-point for now — caret WIDTH is inferred
  at render time by scanning the token's characters on the source
  line (ident/number/literal extent). No AST/parser change, no
  retrofitted end-spans; width inference is honest because the
  renderer can always see the text.
- `Diag` gains `labels: Vec<(Span, Option<String>)>` (secondary),
  `notes: Vec<String>` (trailing `note:`/`help:` lines). The old
  `file:line:col: error:` rendering stays under `--diag=short` so
  editor integrations and the corpus keep a stable line format.
- The renderer needs source text — tables already retain module
  sources for the module loader; keep `lines: Vec<String>` (or the
  raw src) per module and look up at print time.
- `@compile-error` pins keep passing — the directive substring-
  matches stderr and the headline message is unchanged.

**N35B — birth sites for the borrow/consume family.** The
high-value diagnostics, in order of corpus pain:

- `edges` entry gains the span where the borrow was created →
  "cannot consume/write `x`: it is borrowed" gets
  `label: borrow created here` pointing at the `&x`/`get()` site.
- `ctx.consumed` entries gain the consume span →
  "use after consume `p`" gets `note: p consumed here` at the
  `insert`/`move` site.
- "cannot overwrite `s.p`" (the N34 law) gets a note naming the
  live authority's place (the message already says it — make the
  span point at the owning subtree root).
- Init/maybe-init errors get `help:` text when the fix is
  mechanical (`x = ...` in the dead arm, add the else arm, etc).

**N35C — wording normalization + error codes.** A grep-driven audit
of the 213 sites: one phrasing family per concept
(`cannot <verb> <place>: <reason>`), consistent quoting of place
names, no stray tenses. Codes: `E####` allocated per diagnostic
*family* (borrow, consume, init, type, parse…), printed as
`error[E0301]`. Not a per-site registry — per-family, so the audit
is a lookup table, not 213 annotations. A generated `ERRORS.md`
lists code → family → example message.

**Explicitly out:** formatter (the lexer discards comments — a
reprinting formatter would silently eat them; needs trivia
preservation first, its own milestone), LSP (wants these
diagnostics serialized — N36+), per-arm payload-edge scoping
(semantic, deferred on evidence), span end-positions (until
something needs multi-line underlines).

## Pins

- golden stderr for a borrow error: snippet line + `^` caret + note
- secondary label spans point at the birth site, not the use site
- multi-error files still report all errors, in source order
- `@compile-error` corpus unchanged — headline text preserved
- `--diag=short` still emits `file:line:col: error: msg`
- `Diag` in imported-module errors renders the right file/line
  (`file` field already exists — wire it through the renderer)

## Falsifiable check

Take a newcomer-hostile error — "consume of maybe-consumed" — and
the N34B `get_mut`-in-match rejection, and verify the new output
tells the developer what moved, where, and why the other path
matters, without reading the compiler source.

## N35A — landed

`Diag` grew `labels: Vec<Label>` + `notes: Vec<String>` (`Label{span,msg,file}`); `Diag::new` stays the call-site shape — all 213 checker sites untouched. Rendering moved to `driver/render.rs`:

- `render` — `error: msg` / `--> file:l:c` / numbered gutter + source line + caret / same-file secondary labels sorted by line (foreign files get their own `-->`) / `= note:` lines. Gutter width tracks the largest line number; `\t`→4sp; `col` counts chars (UTF-8 safe). Missing source → falls back to the short line.
- `render_short` — `file:l:c: error: msg` verbatim. `--diag=short` (`ourc`) sets a thread-local mode so `pipeline::build` honors it too.
- Sources are registered at load time (root + every imported module) in a thread-local `SOURCES` map — no pipeline signature churn, no disk re-read at render time.
- `caret_width` scans ident/digit/`_`/`.` chars from the span col — `foo.bar`, `héllo`, `0xff` underline whole; `&`, `+`, `?`, `[` stay at one caret.

Pins 568–575: snippet+line-number, tabs, UTF-8 (`héllo` underlined whole), EOL token, imported-module `-->` (`diagerrmod.lang:4:15`), `$rv`-lowered `f()?` shows the user line, borrow/init snippets, root-file `-->` attribution. Unit tests pin short-mode byte-exactness, fallback, labels/notes, and the caret-width table. 575/575 corpus.

## N35B — landed

Provenance enters the checker without touching the headline contract:

- `Edges` value: `(EdgeKind, Cert)` → `EdgeFact { kind, cert, born: Option<Span> }`. Birth sites: `record_edges` (callers pass the init/assign expr span — `let r = &m` born at `&m`), call-arg `$tmp` claims (call span), `bind_task` holds (spawn span — `sr.span`), `replay_summary` creates (call span — summaries stay key-shaped), `Packet.edges` (send span — recv-installed edges label the `send`). Param-frame `$arg` edges: `born: None` (no user origin). Branch/loop/match merges keep the first recorded origin per key.
- `consumed` value: `ConsState` → `ConsFact { st, at: Vec<Span> }`. `join_consumed` unions origins (dedup, cap 4) — a Maybe fact names every dead path, never pretends there's one site.
- `born_label` / `consume_labels` helpers: "borrow created here" family labels at the conflict edge's birth; "consumed here" at each death origin. `$rv`/`$tmp`/`$arg` holders render as bare "borrowed here" — internal names never surface.
- Label sites: cap_check conflicts (read/consume/mut-borrow), mut-borrow read/write conflicts, use-after-consume (incl. free/lock/send paths), double + maybe consume, send-borrowed-target, task holds ("task `t` claimed it here" at spawn).

Pins 576–586: shared/mut borrow blocks, use-after + double + maybe consume, summary- and module-replayed borrows labeled at the call, `$rv` scrutinee borrow naming only the user expression, task-spawn claims, channel-send deaths, multi-path Maybe with BOTH origins labeled. 575 → 586 corpus, zero behavior change — every headline byte-identical.

## N35C — landed

Family-level error codes + wording normalization, no new checker
sites touched:

- `compiler/src/errors.rs` — the registry. 20 `ErrorFamily{code,
  title, doc}` entries across the planned ranges: E1001-1004
  authority (borrow/hold/region/channel), E2001-2004 affine
  (consume/maybe/uninit/overwrite), E3001-3004 types
  (mismatch/inference/unknown-name/call-shape), E4001-4003 control
  (flow/match/task-channel), E5001-5002 modules (structure/
  visibility), E6001 FFI, E7001-7002 syntax/literals.
- `classify(&Diag.msg)` maps a headline to its family at render
  time — two diagnostics reporting the same semantic failure share
  a code no matter which site emitted them. Classification is
  shape-based and total-order safe: most-specific patterns first
  (E1 holds before generic borrow; `match`/parser prefixes before
  E3's broad "found" clause). Unmatched → plain `error:` — never a
  fake code.
- `Diag::code()` — `Option<&'static str>`; renderers display
  `error[E1001]:` in long mode, `l:c: error[E1001]:` in short.
  Short mode stays one-line-per-diagnostic with labels as
  `l:c: note:` lines — the deterministic contract is preserved.
- `ourc --explain E1002` prints the family's prose; `ourc
  --emit-errors-doc` regenerates `ERRORS.md` — the registry IS the
  document, so doc drift is impossible.
- Wording normalized to the canonical shapes: `cannot <verb>
  `<place>`: <reason>` (task holds now `cannot read `x`: held by
  unjoined task `t`` — the `(spawned at l:c)` tail is gone, the
  spawn lives in the label), `use of <state> `<place>``,
  `expected `<type>`, found `<type>``, `cannot infer <thing>`,
  `unknown/private <symbol>`.
- `Tok::describe()` — parser `found` text uses source spellings
  (`expected `;`, found `}``), never Debug names.

Pins 587–597: borrow/double/maybe/uninit/inference/private/cycle/
non-exhaustive/FFI/parse all carry their family codes; 266's
`expected str` pin updated to the normalized `expected `str``
quoting. 586 → 597 corpus.

## N36A — landed

`ourc lsp` — a minimal stdio JSON-RPC language server scoped to
diagnostics. Zero new runtime deps: a ~150-line hand-rolled JSON
codec inside `driver/lsp.rs` (the crate stays dependency-free;
serde_json appears only as a dev-dependency of the subprocess
integration test).

- Methods: `initialize` (capabilities: `positionEncoding: utf-16`,
  full document sync), `shutdown`, `exit`, `didOpen`, `didChange`
  (full), `didSave` (re-check — imports live on disk), `didClose`.
  Unknown requests get `-32601`; unknown notifications are ignored.
- Every change re-runs the whole frontend via the new
  `pipeline::check_source(path, src)` — the same module loader,
  mono, and checker, but the root text comes from the editor buffer
  instead of disk. No incremental cleverness; compile is sub-second.
- `Diag` → LSP `Diagnostic`: `span`→`range` (0-based UTF-16),
  `msg`→`message` (notes appended as `note:` lines), `code()`→`code`,
  `severity` 1, `source` `"ourc"`, `labels`→`relatedInformation` —
  cross-module labels carry the module's own `file://` URI, so a
  borrow created in an imported file points there.
- Position conversion lives in ONE place: `utf16_col` + `lsp_range`
  translate the compiler's 1-based char columns to 0-based UTF-16
  code units (`é`=1, `🚀`=2). Range ends reuse the renderer's
  conservative `caret_width`.
- Root-doc diags (`file: None`) always publish under the document's
  own URI — never a canonicalized path; module diags publish under
  their canonical `file://` URI. A `dirty` set tracks published URIs
  so a fixed file gets an explicit empty `diagnostics` array.

Tests: `tests/lsp.rs` drives the real binary — initialize → didOpen
borrow error → `E1001` at 0-based `(4,9)` + relatedInformation at
`(3,12)` → didChange fix → empty publish → shutdown/exit. Unit tests
pin UTF-16 columns (incl. non-BMP), URI encode/decode, and JSON
round-trip with surrogate pairs.
