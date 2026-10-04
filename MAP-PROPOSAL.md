# MAP-PROPOSAL — `StrMap<V>`: open addressing in ordinary code

Status: **spec** (N34). Falsifiable question:

```text
Can `box slice` + generics + enums + ordinary authority rules build
a real hash map — collision resolution, tombstones, rehash — with NO
compiler magic and NO `mml_` runtime shim?
```

N31/N33 evidence is the demand side: `wordfreq` and `minijson` both
linear-scan `Vec<Entry>`/`Vec<Pair>`. The point of the milestone is
not the table — it is whether *rehash invalidation*, *affine value
returns*, and *borrowed lookups* fall out of the same authority graph
as `Vec` reallocation.

## Feasibility — verified before writing this spec

Probed against the compiler as of `49b1d7a`:

| Requirement | Verdict |
|---|---|
| Zero-payload enum variants | yes — `enum E { V, W(T) }` since N22 (pin 433) |
| `box slice` of enum slots: construct, match, move-out | yes — after `49b1d7a` |
| `&V` / `&mut V` returned out of a slot through `read`/`write` params | yes — `match` proves the tag, `&m.slots[i].Full.v` is an ordinary place |
| Returned `&V` blocks `free(m.slots)` while live | yes — `cannot consume m.slots: it is borrowed` |
| Move `Entry` out of a heap slot (rehash) | yes — the `pop` idiom (`self.buf[i]` on `own`/`write self`) |
| In-language hash over `str` | yes — `s.bytes`, `^`, `*`, `u64` |
| `str` key equality | yes — builtin `==` |

`49b1d7a` fixed the two gaps the probes exposed — both general
place-lowering bugs, not map special-casing:

1. `Field` path hops re-derived their type through the `{tag,
   [N x i8]}` pseudo-layout, so `slot.Full.v` panicked emit with
   "field path on non-aggregate". `PathElem::Field` now carries its
   resolved `ty` (as `Index` already did); emit no longer
   re-derives.
2. Enum construction `place = Enum::V(..)` built tag/payload stores
   on a *fresh* path — `m.slots[1] = Slot::Full(..)` wrote tag+payload
   into `slots[0]`. Sub-places now extend `place.path`. Also fixed:
   `*v.f` roots now GEP the pointee type.

## Surface (`stdlib/map.lang`)

**Not `Map<K,V>`** — there is no `K: Hash + Eq` constraint system, so
a generic key would be fake. The honest first type is string-keyed:

```text
pub struct StrMap<V> {
    slots: box slice Slot<V>,
    len:   i64,        // live entries
    used:  i64,        // Full + Tomb slots (load factor numerator)
    heap:  alloc,
}

enum Slot<V> {
    Empty,
    Full(Entry<V>),
    Tomb,
}

struct Entry<V> { key: str, hash: u64, v: V }
```

`Entry` stores the hash — rehash and probe-short-circuit without
re-hashing, and `hash` compare before `str` compare is the ordinary
open-addressing fast path.

```text
pub fn new<V>(heap: alloc, cap: i64) -> StrMap<V>
    allocates cap slots (rounded up, pow2 >= 8), writes Empty into
    each — explicit, visible cost.

pub fn insert<V>(own self: StrMap<V>, key: str, v: V) -> Put<V>
    `own` — growth may rebind the slot buffer (same rule as
    Vec.push). Consumes key and v.

pub enum Put<V> {
    Fresh(StrMap<V>),
    Replaced(StrMap<V>, V),     // old value handed back — never
                                // silently destroyed
    Full(StrMap<V>, V),         // fixed-cap failure (N34A/B):
                                // map AND value return to caller
}

pub fn get<V>(read self: StrMap<V>, key: str) -> Result<&V, i32>
    Err(1) = missing. The &V borrows m.slots — structural mutation
    while the view lives is rejected by the plain borrow rules.

pub fn get_mut<V>(write self: StrMap<V>, key: str) -> Result<&mut V, i32>

pub fn remove<V>(write self: StrMap<V>, key: str) -> Result<V, i32>
    `write` suffices — tombstone write is in-place, no realloc.
    Returns the removed value — affine V is handed back, not dropped.

pub fn len<V>(read self: StrMap<V>) -> i64
pub fn destroy<V>(own self: StrMap<V>)
    frees the slot buffer. Affine payloads inside Full slots are the
    caller's responsibility — `remove` first, same as Vec<box T>
    (free frees the slot storage, not each box's pointee).
```

## Hash — `fnv1a`, ordinary code

```text
fn hash(key: str) -> u64 {
    let h = 0xcbf29ce484222325;      // u64 FNV offset basis
    let b = key.bytes;
    for i in 0..b.len {
        h = (h ^ u64(b[i])) * 0x100000001b3;
    }
    return h;
}
```

No shim, no locale, no trait. `u64` literals > i64::MAX spell as hex
(N33's lexer lesson).

## Probing

`i = hash % cap`, linear probe `i = (i + 1) % cap`. At each slot:

```text
Empty → not found / insertion point
Tomb  → insertion point (first tomb remembered), keep probing —
        later Full may hold the key
Full  → hash equal AND key == slot key → hit; else keep probing
```

Tombstones preserve probe chains; `used` counts Tombs so a
tombstone-dense table still triggers rehash correctly at growth.

## The authority story — the point of the milestone

```text
let v = m.get("x")?;          // &V borrows m.slots
m = m.insert(m, "y", w);      // insert takes own m — REJECTED while
                              // v is live: it borrows the buffer
                              // insert may free/rebind
```

Rehash invalidation = Vec-realloc invalidation — the same rule, zero
new machinery. `get_mut` gives `&mut V` under `write` for the
wordfreq `*v = *v + 1` update without structural mutation.

```text
StrMap<box Node>:
    insert consumes the box (Entry owns it)
    get returns &box Node — borrow, no copy
    remove returns the box — ownership handed back
    destroy frees slot storage only — remove first or leak: honest

StrMap<handle Node>:
    payload copies identity — shallow shared, the handle contract
```

Channel transport moves the whole `StrMap<box Node>` as one owned
value — whole-map authority, same as `Vec<box>`.

## Staging

**N34A — fixed-cap scalar**
`hash`, `StrMap` internals, `new`/`insert`/`get`/`len`/`destroy` on
concrete `i64` values if generic plumbing adds friction — but
probes show `Result<&V, i32>` instantiates, so generic `V` may come
free immediately. No tombstones yet: insert fails `Full` at cap.

**N34B — generic affine V + remove**
`Put<V>` enum shape, `Replaced` returning the old value, `remove` +
`Tomb` slots, `get_mut`. The `StrMap<box Node>` / `StrMap<handle>`
pins land here.

**N34C — growth**
`insert` rehashes at load factor ≥ 0.7 (used/(cap)) — alloc
`cap*2`, walk old slots, move each `Entry` into the new buffer by
stored hash (no re-hash of `str` keys), `free` old buffer. Borrow
-invalidation pins: `get` view + inserting `insert` rejects; `get`
view + non-structural `get`/`len` accepts.

**Then**: rewrite `wordfreq`'s linear scan to `get_mut`/insert;
benchmark old vs new at 10k / 100k / 1M words. `minijson` object
pairs after wordfreq proves it.

## Pins (549+)

- insert/get scalar, miss → Err(1)
- collision: two keys same bucket (fixed cap forces it) probe to
  distinct slots
- replace existing key → `Replaced(old)`
- remove → `Ok(v)`; slot becomes Tomb
- tombstone chain: A and B collide, remove A, `get(B)` still hits
- full map → `Full(m, v)` returns map + value (N34A/B)
- rehash preserves every entry (N34C)
- `StrMap<box Node>`: insert consumes box; `get` borrows `&box`;
  `remove` returns box; `destroy` after removes — no hidden free
- `StrMap<handle Node>`: copies identity, get returns `&handle`
- `let v = get(..)?; insert(..)` → **reject** (borrow blocks `own`)
- `let v = get(..)?; get(..)` / `len()` → **accept** (read∥read)
- `chan<StrMap<box Node>>` send/recv — whole-map authority moves
- `get_mut` write visible through a later `get`
- `m.slots[i] = Tomb` over a `Full(box)` slot — expected to reject
  if the checker tracks contained affine values (record the verdict
  either way — it is a semantic fact about heap-slot tracking, not
  an API choice)

## Explicitly out

- `Map<K,V>` arbitrary keys — needs `Hash + Eq` constraints; fake it
  and the milestone's answer is worthless
- trait/interface machinery for hashing
- `mml_` hash-table shim — the entire point is ordinary code
- iteration (`keys()`/`iter()`) — no `wordfreq`/`minijson` evidence
- ordered maps, Robin Hood, per-slot locks — no evidence
- deep `destroy` of affine payloads — same open question as
  `Vec<box>`; `remove`-first is the documented contract
- `str` *values* as keys ownership — keys stored as `str` views;
  callers keep key storage alive (wordfreq keys live in the input
  buffer — honest, document the lifetime)
