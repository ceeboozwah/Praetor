# Error index

Codes are family-level: every diagnostic reporting the same
semantic failure shares a code. `ourc --explain <CODE>` prints the
family's entry.

## E1001 — borrow conflict

A place is read, written, or consumed while a borrow of it is still live. The borrow edge's birth site is labeled — either end the borrow first or keep the access read-only.

## E1002 — concurrency hold

An unjoined task, a lock guard, or an atomic hold claims the place. `join` the task, `unlock` the guard, or route the access through the synchronized builtin that owns the hold.

## E1003 — region escape

A reference or handle would outlive the storage it names — escaping a function frame, a `spawn` body, or a channel's lifetime. Restructure so the referent provably outlives the view.

## E1004 — channel authority

Authority passed through a `send` is channel-held until a `recv` claims it — the sender cannot touch it in between. `recv` the value, or restructure so authority never transits.

## E2001 — use after consume

The place's authority was moved or freed already — `free`, an `own` argument, a `let` move, or `send` all consume. Rebind the place, or use it before the consume.

## E2002 — maybe-consumed

The place is consumed on some control-flow paths but not all — the checker cannot prove it's alive here. Rebind it on every path, or restructure so the consume is unconditional.

## E2003 — uninitialized use

The place has no value on at least one path reaching this use. Initialize it before use, or on every branch.

## E2004 — live authority overwrite

A store would silently discard live own authority (a `box` or a payload containing one) — the language has no hidden `free`. Extract the old value first (`let old = s.p`) or `free` it.

## E2005 — abandoned own authority

A binding leaves scope — `}`, `return`, `?`, `break`, `continue` — while it still holds live own authority. Live owning authority may not disappear: move it out, return it, hand it to an owning call, or `free` it first. Process termination is exempt — `process.exit` performs no unwinding.

## E3001 — type mismatch

The value's type doesn't match what the context requires. The language does no implicit numeric or reference coercion — cast explicitly.

## E3002 — type inference failure

A generic parameter resolved to two different types, or nothing constrained it. Annotate the binding or instantiate explicitly (`f::<i32>(..)`).

## E3003 — unknown name or member

The variable, function, struct, field, or enum variant doesn't exist in this scope. Check the spelling and the module path.

## E3004 — call shape

The callee exists but the call doesn't fit it — wrong arity, a place required where a value was passed, or a builtin used with the wrong argument form.

## E4001 — control flow

A `break`/`continue` outside a loop, a missing or extra `return` value, or a divergent statement where a value is required.

## E4002 — match

The match is malformed — a duplicate arm, a payload bound where none exists, an inaccessible variant, or a scrutinee that isn't a place.

## E4003 — task or channel shape

A `spawn`/`join`/`send`/`recv`/`close` used in a way the channel discipline forbids — an unbound spawn, an unknown join, or a `?` inside a task.

## E5001 — module structure

The import can't be resolved or the namespace conflicts — a missing file, an import cycle, a `module` name that disagrees with its file, or a duplicate declaration.

## E5002 — visibility

A `pub` item names a private type, or an outsider touched a private symbol. Make the type `pub`, or keep the boundary honest by not exposing it.

## E6001 — FFI boundary

An `extern` signature uses a type the C ABI can't carry, or a raw-pointer capability is misused at the boundary.

## E7001 — syntax error

The token stream doesn't parse — check punctuation, braces, and statement terminators.

## E7002 — malformed literal

A literal is malformed or doesn't fit its type — bad digits, a bad escape, or an out-of-range value for the width.

