# The Language — a programmer's guide

This is the document to read when you want to know *how do I write
this?* It teaches the syntax and the ownership rules you'll meet day to
day. It is not the formal spec — `SPEC.md` defines the laws; this file
shows you how to live inside them.

```text
LANGUAGE.md   teaches syntax + core mental model     ← you are here
SPEC.md       defines formal language laws (R1–R20, appendices)
our doc       documents public APIs (our doc std.vec)
ERRORS.md     explains diagnostics (ourc --explain E1001)
```

A program is a `.lang` file. **Two binaries matter:**

- `our` — the self-hosted compiler and project driver (primary
  toolchain; compiles single files and drives projects)
- `ourc` — the bootstrap/reference compiler and extended developer
  utility (`ourc lsp`, `ourc --explain`, `ourc test <dir>`)

Install a release tarball with its `install.sh` (INSTALL.md), or
build from the repo: `cargo build --release -p ourc`, then
`target/release/ourc selfhost/our.lang -o target/our` builds the
self-hosted driver. The status contract for this release line:
**self-hosted, native, no known semantic divergences from Stage 0 —
macOS arm64 verified; Linux/Windows verification pending.**

**The five-word mental model.** Everything the checker enforces
reduces to five ideas:

```text
Region     = lifetime / validity      — where storage is alive
Place      = storage identity         — the slot a name refers to
Capability = authority                — what a binding may do to it
Handle     = identity without authority
Own        = affine responsibility    — must be discharged, not dropped
```

`read`/`write`/`own` on a parameter pick the capability; `box`,
`alloc`, channels, and locks mint `own` authority; the checker audits
every scope exit for live `own`. That is the whole game — the rest of
this file is how to write it.

## 1. Getting started

Single file, no project:

```sh
our hello.lang -o hello && ./hello
```

```lang
fn main() -> i32 {
    return 0;
}
```

A project (multi-file, tests, dependencies):

```sh
our new hello
cd hello
our run          # build + run src/main.lang
our test         # run tests/*.lang
```

`our new` scaffolds:

```text
hello/
  project.toml
  src/main.lang
  tests/smoke.lang
```

```toml
[project]
name = "hello"
version = "0.1.0"
# toolchain = "0.2"       # pin the toolchain this project builds with —
                          # `our` re-execs a versioned sibling or errors
                          # (CHANNELS.md)

[build]
entry = "src/main.lang"

# [dependencies]
# lib = "../lib"        # path package — `import lib.util`
# net = "1.2"           # registry package (OUR_REGISTRY)
```

`version` is *the project's* version — always `0.1.0` for a fresh
scaffold, unrelated to the toolchain's. The commented `toolchain`
pin is what a reproducible build turns on.

`tests/smoke.lang` shows the test directive format — `our test`
compiles and runs every `.lang` file under `tests/` and checks
`// @exit N` (and `// @compile-error …` for negative tests):

```lang
// @exit 42

fn main() -> i32 {
    return 42;
}
```

`main` returns the process exit code. Two legal signatures —
`fn main() -> i32`, or with arguments:

```lang
fn main(args: slice str) -> i32 {
    // args[0] is the program name
    return i32(args.len);
}
```

## 2. Variables and types

`let` binds; bindings are reassignable by default:

```lang
let x = 0;              // inferred integer
let y: i64 = 3;         // annotated
let buf: [1024]u8;      // declared, uninitialized
buf[0] = 0;             // must be written before any read
```

Reading an uninitialized binding is a compile error — the checker
tracks definite initialization per path.

**Types.**

```text
scalars   i8 i16 i32 i64  u8 u16 u32 u64  bool
views     slice T          — (ptr, len) read-only span
          slice mut T      — (ptr, len) writable span
          str              — UTF-8 validated slice u8
heap      box T            — one owning heap cell
          box slice T      — owning heap span
          alloc            — an allocator token
arrays    [N]T             — inline, fixed size
refs      &T  &mut T       — borrows into existing storage
raw ptr   *const T  *mut T  *void        — FFI only
concurrency  channel T  mutex T  guard T  task T  atomic T
other     handle T         — identity, not authority
```

Integer literals are decimal by default, `0x…` hex, `0b…` binary,
with `_` separators anywhere (`1_000_000`, `0xFF_00`). Bitwise ops
`& | ^ ~ << >>` work on integers — shifts have defined semantics
(over-width shifts are a compile error, not UB). Casts look like
calls: `i64(v)`, `u8(b)`, `u32(x)`. An unannotated
integer literal defaults to `i32`, and inference does not stretch it
across a comparison or a generic argument: `while i < v.len` needs
`let i: u64 = 0` because `len` is `u64`, and `v.push(61)` on a
`Vec<u8>` needs `u8(61)`. The error (`E3001`/`E3002`) names the exact
mismatch — cast or annotate.

A note on argv: `args[i]` is a `str` view straight into the OS's argv
storage — `args[i].bytes.ptr` is therefore NUL-terminated and can be
handed to `*const u8` parameters (e.g. `fs.open`) directly.

## 3. Functions and parameter authority

```lang
fn add(a: i64, b: i64) -> i64 {
    return a + b;
}
```

A plain parameter is passed by value — fine for scalars and views.
For anything heavier, the parameter declares the **authority** the
call needs:

```lang
fn bump(write x: i64) {          // may write through the caller's place
    x = x + 1;
}

fn read_it(read x: i64) -> i64 { // read-only view of the caller's place
    return x;
}

fn take(own p: box i64) {        // ownership transfers into the call
    free(p);                     //   — the caller's `p` is consumed
}
```

```text
read   shared borrow — callee reads, caller keeps everything
write  exclusive borrow — callee may mutate; caller's other aliases pause
own    ownership moves — after the call the caller's place is consumed
```

These are the only three. There is no hidden borrow and no implicit
move — the keyword at the parameter *is* the contract, and the call
site writes nothing: `bump(x)`, `take(p)`.

## 4. Structs and enums

```lang
struct Point { x: i64, y: i64 }

enum Shape {
    Circle(i64),          // one payload per variant
    Square(i64),
    Empty,                // or none
}

fn main() -> i32 {
    let p = Point { x: 1, y: 2 };
    let s = Shape::Circle(7);
    p.x = 5;
    return i32(p.x);
}
```

`match` destructures enums; arms must cover every variant (a `_` arm
counts), and an arm can be a bare expression:

```lang
import std.io;

enum Shape { Circle(i64), Square(i64), Empty }

fn main() -> i32 {
    let s = Shape::Circle(7);
    match s {
        Circle(r) => { io.println_i64(r); }
        Square(n) => io.println_i64(n),
        _ => {}
    }
    return 0;
}
```

## 5. Control flow: if, while, match

```lang
fn main() -> i32 {
    let i = 0;
    if i > 0 { return 1; } else { i = 2; }

    while i < 10 {
        if i == 5 { break; }
        i = i + 1;
        continue;      // the above line makes this unreachable — shown for form
    }
    return i32(i);
}
```

There is no bare `{ }` block — the lexical-scope idiom is:

```lang
fn main() -> i32 {
    let x = 1;
    if true {
        let y = 2;     // dies at this `}` — a scoped temporary
        x = y;
    }
    return x;
}
```

Every scope end — `}`, `return`, `?`, `break`, `continue` — audits the
bindings that die there: a live owning resource may not simply
disappear (see §8).

A path that never returns is **divergent**, not *terminated*: a
`=> return` arm, a `?` early exit, or `process.exit(n)` drops out of
the value join instead of producing one — `match` expressions rely on
this (a divergent arm doesn't need to yield a value). `unreachable`
asserts a path is dead — reaching it is a bug, and neither
`unreachable` nor `process.exit` replays `defer`s: nothing unwinds on
a path that ends the world.

## 6. References, slices, str

```lang
fn sum(s: slice i64) -> i64 { return s[0] + s[1]; }
fn bump2(s: slice mut i64) -> void { s[0] = s[0] + 1; }

fn main() -> i32 {
    let a: [4]i64;
    a[0] = 1; a[1] = 2; a[2] = 3; a[3] = 4;

    let t = sum(&a[0..2]);        // &a[lo..hi] builds a `slice i64` view
    bump2(&mut a[0..2]);          // `&mut a[..]` — a `slice mut i64` view
    let s = &a[0..3];             // a read view can also be let-bound
    let r = &a[0];                // `&x` borrows one element: &i64
    return i32(t + s[0] + *r);
}
```

A borrow is a claim: while `&mut` is live you can't read or write the
borrowed place from elsewhere; while `&` is live you can't write it.

**A borrow lives as long as the binding holding it** — there is no
last-use expiry for named views:

- A *temporary* view (`sum(&a[0..2])`) dies when the call returns.
- A `let`-bound view keeps its source borrowed until the name dies:
  scope exit, rebinding (`s = &other[..]` releases `a`), or being
  consumed by a call that takes the view by move (`slice mut`
  parameters move; `slice`/`str` copy, so those keep it live).

So this rejects — `s` still holds `a` borrowed at the write:

```lang
fn main() -> i32 {
    let a: [2]i64; a[0] = 1; a[1] = 2;
    let b: [2]i64; b[0] = 3; b[1] = 4;
    let s = &a[0..2];
    let x = s[0];          // s's last use — but its scope isn't over
    // a[0] = 9;          // E1001: `a[0]` is borrowed
    s = &b[0..2];          // rebinding s elsewhere releases `a`
    a[0] = 9;              // now fine
    return i32(x + s[0] + a[0]);
}
```

To bound a borrow tightly, give it a smaller scope: a helper function
whose parameter *is* the view (`fn ingest(m: …, data: slice u8)` — the
borrow ends at the call), or the `if true { }` idiom. This is also why
a `let`-bound `str`/`slice` of a `Vec`'s storage blocks
`v.destroy()`: the view lives to `}`, so destroy rejects. Pass the
view into a call instead of naming it.

**Capturing conservatism.** When a view flows into a call that returns
a value, the compiler assumes the result may retain it. `m.insert(k,
v)` where `k` views `buf` makes the returned map borrow `buf` — it
can't prove `insert` didn't store the key (borrowed-key maps do
exactly that). An owned copy (`omap.dup_key`, `insert_kbuf`) avoids
the borrow entirely.

`str` is a `slice u8` whose bytes are proven UTF-8. String literals are
`str`; turn bytes into `str` explicitly. `s == t` compares `str`
content byte-for-byte:

```lang
fn main() -> i32 {
    let buf: [4]u8;
    buf[0] = 104; buf[1] = 105; buf[2] = 33; buf[3] = 0;
    let s: str = "hello";
    let t: str = str_from_utf8(&buf[0..3]);   // validates the bytes
    return i32(s.len + t.len + s.bytes.len);  // s.bytes: slice u8
}
```

Validation costs a pass over the bytes — that's the honest price of the
invariant; it happens once at conversion, never per-byte afterwards.

## 7. Heap: box, allocators, free

Heap allocation is explicit and allocator-parameterized:

```lang
let heap = system_allocator();
let p = alloc<i64>(heap);            // box i64
let buf = alloc_slice<u8>(heap, 64); // box slice u8
(*p) = 5;                            // deref a box with (*p)
buf[0] = 65;                         // box slice indexes directly
let b0 = buf[0];                     // reads work the same way
free(p);                             // explicit release
free(buf);
```

Index expressions take any integer width — `a[i]` accepts `i32`,
`i64`, `u64`, ... Range-checked at runtime; the type is unconstrained.

Viewing a `box slice` (or a `&box slice` you were handed) as a plain
slice goes through the deref: `&buf[0..buf.len]` for an owned buffer,
`&(*r)[0..(*r).len]` for a `read r: &box slice u8` parameter.

Nothing frees itself. A `box` that reaches scope end still owning is a
compile error — the compiler forces every owning value to a discharge:
`free`, an `own` call, `send`, `return`, or `defer`.

Custom allocators come from the same token type — anything returning
`alloc` plugs in:

```lang
fn f(h: alloc) {
    let p = alloc<i64>(h);
    free(p);
}
```

## 8. Moves vs copies — the ownership law

Scalars and immutable views **copy**. Anything carrying authority
**moves**:

```lang
fn main() -> i32 {
    let a = 5;
    let b = a;        // copies — both live

    let arr: [2]i64; arr[0] = 1; arr[1] = 2;
    let s = &arr[0..2];
    let t = s;        // copies — read views are shareable

    let heap = system_allocator();
    let p = alloc<i64>(heap);
    let q = p;        // MOVES — p is now consumed
    free(q);
    return a + b + i32(t[0]);
}
```

The same law applies through structure fields — authority follows the
value regardless of syntax shape (SPEC R20):

```text
slice mut field / box field  → move   (source subtree goes dead)
slice / str / POD field      → copy
v.buf = fresh                → re-slotting re-arms the place
```

**Strict discharge (R19).** A live owning value cannot evaporate at a
scope exit — `let p = alloc<i64>(heap); return;` rejects with
"cannot leave scope while `p` still owns live resources". Every owner
must reach a discharge on every path:

```lang
fn main() -> i32 {
    let heap = system_allocator();
    let p = alloc<i64>(heap);
    defer free(p);          // scheduled discharge (§14)
    // or: free(p); / take_own(p); / return p; / send(ch, p);
    return 0;
}
```

`consume x;` is the bare statement form — discards `x`'s authority with
no call, for the rare place where that's the honest semantics.

## 9. `Result` and `?`

`std.result.Result<T, E>` is the error carrier. `?` unwraps `Ok` or
returns `Err` to the caller:

```lang
import std.result;
import std.fs;
import std.vec;

fn load(heap: alloc, path: *const u8) -> result.Result<vec.Vec<u8>, i32> {
    let v = fs.slurp_r(heap, path)?;   // Err path: returns early
    return result.Result<vec.Vec<u8>, i32>::Ok(v);
}
```

`?` is a real control-flow exit: deferred calls replay on it, and live
owners that would be abandoned get flagged — the Err path is checked
just as hard as the Ok path.

`match` on a `Result` is the manual form when you want the Err
payload. Variant names resolve from the scrutinee's type — no import
is needed to *match* on `Ok`/`Err`; `import std.result` is only for
naming `result.Result` in signatures:

```lang
import std.result;
import std.fs;

fn main(args: slice str) -> i32 {
    let f = match fs.open(args[1].bytes.ptr, fs.o_read(), 0) {
        Ok(f) => { f }
        Err(e) => { return 2; }
    };
    fs.close(f);
    return 0;
}
```

Writing a file is the same shape — `fs.open` takes portable flag bits
(`fs.o_write() | fs.o_create() | fs.o_trunc()`, mapped to the host's
`O_*` values by the runtime — never write raw flag numbers) and a
decimal mode (`0644` = `420`):

```lang
import std.fs;
import std.result;

fn main() -> i32 {
    let heap = system_allocator();
    let buf = alloc_slice<u8>(heap, 3);
    buf[0] = 104; buf[1] = 105; buf[2] = 10;   // "hi\n"
    let path: str = "/tmp/out.txt";   // literals are str; .bytes.ptr is the C form
    let fd = match fs.open(path.bytes.ptr,
                           fs.o_write() | fs.o_create() | fs.o_trunc(), 420) {
        Ok(f) => { f }
        Err(e) => { free(buf); return 2; }     // buf is live — discharge it
    };
    match fs.write(fd, &buf[0..3], 3) {
        Ok(w) => {}
        Err(e) => { fs.close(fd); free(buf); return 3; }
    }
    fs.close(fd);
    free(buf);
    return 0;
}
```

Note the `Err` arms: every early exit must still discharge `buf` —
R19 audits the error path as hard as the success path. With a `?`
variant, a `defer free(buf)` up front removes the duplicated
`free`s (§14).

## 10. Modules and imports

```lang
import std.fs;
import std.vec;
import std.result;
```

A module boundary is a naming boundary only — `std.fs` functions get
fully checked against the same ownership rules as your code. In a
project, own modules are `name.mod` where `name` is the `[project]`
name — so inside a project named `hello`, `src/util.lang` is imported
as:

```text
import hello.util;        // src/util.lang — resolves inside the project
```

`pub` marks a module's public surface; without it the item is
module-private:

```lang
pub fn helper() -> i32 { return 0; }
```

`our doc std.fs` prints a package's public API with its doc comments
(`our doc <pkg>.<mod>`, e.g. `our doc std.vec`).

**A complete project with a dependency.** A *path dependency* is a
directory holding its own `project.toml`; the dep's `src/` modules
import as `<name>.<mod>`:

```text
hello/
  project.toml      [dependencies] lib = "../lib"
  src/main.lang     import lib.util;
lib/
  project.toml      [project] name = "lib", version = "1.0.0"
  src/util.lang     pub fn double(x: i32) -> i32 { ... }
```

```lang
// lib/src/util.lang — the dependency's public module
pub fn double(x: i32) -> i32 {
    return x * 2;
}
```

```sh
cd hello
our deps         # resolves lib -> ../lib, writes project.lock
our run          # builds lib.util + main, runs it
our build --offline    # resolve from the cache only — no re-fetch
our build --locked     # require project.lock to be exact; else error
```

`project.lock` records the resolved source+version+hash per dep —
commit it when reproducibility matters. Registry deps work the same
way against `OUR_REGISTRY` (PACKAGES.md), cached under
`$OUR_HOME/registry` once fetched.

## 11. Generics and methods

```lang
struct Cell<T> { v: T }

fn get<T>(read self: Cell<T>) -> T {
    return self.v;             // read self: borrows the receiver
}
```

A first parameter named `self` makes the function a method —
`v.push(x)` resolves to `Vec::push(v, x)` with `self`'s authority
applied to `v`:

```lang
import std.vec;

fn main() -> i32 {
    let heap = system_allocator();
    let v = vec.Vec<i32>::new(heap);
    v = v.push(1);            // own self consumes v; the return rebinds it
    let n = v.len;            // `len` is a public field
    v.destroy();              // own self — explicit teardown
    return i32(n);
}
```

The `self` authority is how methods express "this operation may move
the structure" — `push` takes `own self` precisely because
reallocation can relocate the buffer.

## 12. Maps: borrowed vs owned keys

Two stdlib maps, and the difference is the point:

```text
StrMap<V>   (std.map)   keys are str VIEWS — the map borrows key bytes;
                        keys must outlive the map
OMap<V>     (std.omap)  owns its key bytes — insert hands key storage
                        to the map
```

```lang
import std.map;
import std.omap;

fn main() -> i32 {
    let heap = system_allocator();
    // Borrowed keys: the literal lives in rodata — outlives the map.
    let m = map.new<i64>(heap, 128);
    match m.insert("k", i64(1)) {   // insert consumes m — rebind in every arm
        Fresh(m2) => { m = m2; }
        Replaced(o) => { m = o.m; }
        Full(o) => { m = o.m; }
    }
    m.free_storage();

    // Owned keys: insert_kbuf takes the key buffer's ownership.
    let om = omap.new<i64>(heap, 128);
    let kb = alloc_slice<u8>(heap, 4);
    kb[0] = 107;
    match om.insert_kbuf(kb, i64(1)) {
        Fresh(m2) => { om = m2; }   // map owns kb now — don't free it
        Replaced(o) => { om = o.m; }
        Full(o) => { om = o.m; }
    }
    om.free_storage();
    return 0;
}
```

Choose `StrMap` when keys live in storage that outlives the table
(parsed input, literals); choose `OMap` when the map must own them.
Both hand payload authority back on `remove`/`pop_entry` — no hidden
frees ever; drain before `free_storage` for non-copyable values.

Two consequences worth knowing:

- **`Replaced`/`Full` hand the value back** in `o.v` — with an owning
  `V` (e.g. `box slice u8`) you must discharge it yourself
  (`free(o.v)`); POD values just drop.
- **Read-only traversal is `next_entry`** — a cursor step that
  borrows the next live entry without consuming anything:

```lang
import std.map;
import std.io;

fn dump(read m: map.StrMap<i64>) {
    let i: u64 = 0;
    while true {
        match m.next_entry(i) {
            Ok(e) => { io.print_i64((*e).v); }  // borrowed — never move
            Err(x) => { break; }
        }
    }
}
```

  `i` is a caller-owned slot cursor; both maps return
  `Result<&Entry, i32>` / `Result<&OEntry, i32>`. The borrow pins the
  map — mutation is locked out while a yielded `e` lives, and any
  insert/remove between steps may reshuffle the table anyway, so a
  cursor is only valid across reads. For `OMap` keys use
  `omap.entry_key(*e)` to get the owned key bytes as a `str` view.
  `pop_entry` remains the teardown op — it drains; `next_entry` is
  for inspection and leaves the map exactly as found.

## 13. Concurrency: channels, tasks, mutexes, atomics

```lang
struct Msg { v: i64 }

fn make_msg(heap: alloc) -> box Msg {
    let m = alloc<Msg>(heap);
    (*m).v = 1;
    return m;
}

fn main() -> i32 {
    let heap = system_allocator();
    let ch: channel box Msg = channel();
    let t = spawn {
        let m = recv(ch);       // claims the packet's ownership
        free(m);
    };
    send(ch, make_msg(heap)); // send moves authority into the channel
    join t;
    close(ch);
    return 0;
}
```

- `spawn { .. }` runs a block on a thread; `join t` rejoins it. An
  unjoined task at scope end is a compile error.
- `send` transfers `own` authority into the channel — the sender's
  place is consumed; `recv` claims it on the other side. Authority in
  flight belongs to the channel: the sender may not touch it.
- `atomic i64` + `atomic_load`/`atomic_add` for shared counters.
- `mutex T` wraps a value; `lock(m)` yields a guard through which the
  fields are reachable; `unlock(g)` or scope end releases it:

```lang
import std.omap;

struct Store { m: omap.OMap<i64> }

fn main() -> i32 {
    let heap = system_allocator();
    let store: Store;
    store.m = omap.new<i64>(heap, 128);
    let mtx = mutex store;
    let g = lock(mtx);
    let n = g.m.len();
    unlock(g);
    return i32(n);
}
```

## 14. `defer` — scoped cleanup

`defer` schedules an explicit call at every normal exit of its scope —
it is not a destructor:

```lang
fn main() -> i32 {
    let heap = system_allocator();
    let p = alloc<i64>(heap);
    defer free(p);
    // ... p is discharged on }, return, ?, break, continue ...
    return 0;
}
```

Calls replay last-registered-first (LIFO). `process.exit` and
`unreachable` skip replay — those paths don't unwind.

**The one rule to internalize: `defer` freezes the *place*, not the
current value.** While the defer is live, anything that would change
what the operand names rejects:

```text
let c = p;    // move deserts the deferred call
p = q;        // rebind changes the operand's identity
let p = ..;   // shadowing re-points the captured name
promote p;    // escapes the scope that replays
return p;     // transfers the operand out from under the call
```

Reading and mutating *through* the place stays legal — the freeze is
on identity, not use. This means `defer` fits resources that *stay
put*; a `Vec` rebound by `v = v.push(..)` or a success-path handoff
keeps its manual cleanup. A pending defer is also per-path: a `break`
inside an `if` replays it on that edge, and the fallthrough path still
holds it.

## 15. FFI and raw pointers

```lang
extern fn puts(s: *const u8) -> i32;

fn main() -> i32 {
    puts("hello");           // literals lower to null-terminated globals
    return 0;
}
```

FFI types are scalars and raw pointers only: `*const T`, `*mut T`,
`*void`. Language carriers (`str`, `slice`, `box`, aggregates) don't
cross — convert at the boundary (`s.bytes.ptr` gives a `*const u8`,
`str`→C-string via `string.to_cstr`). An `extern fn` is trusted at its
signature; everything on your side of the call is still checked
normally.

## 16. Errors you'll meet

```sh
ourc --explain E2005     # full entry for any code
```

The recurring families:

| code | name | the short version |
|---|---|---|
| E1001 | borrow conflict | place used while a borrow of it is live |
| E2001 | use after consume | place's authority was moved/freed already |
| E2002 | maybe-consumed | consumed on some paths but not all |
| E2003 | uninitialized use | no value on a path reaching the read |
| E2004 | live overwrite | a store would silently drop owning authority |
| E2005 | abandoned own | live owner reaches a scope exit undisposed |
| E3003 | unknown name | variable/struct/field doesn't exist here |
| E3004 | call shape | wrong arity, or a place needed as a value |
| E5002 | visibility | an outsider touched a private symbol |
| E7001 | parse error | syntax didn't parse |

The diagnostics point at the operation *and* the claim it conflicts
with — read the note under the error, it usually names the fix
("end the borrow first", "the defer at line N reserved this place").
`ourc --explain <code>` expands any family into prose;
`ERRORS.md` is the generated index of all of them, and
`our doc std.<mod>` is the same kind of surface for stdlib APIs.

## 17. Commands

```text
our file.lang -o bin            compile one file (self-hosted driver)
our new <name>                  scaffold a project
our check                       semantics only, no codegen
our build [--release]           compile to build/<name>
our run [--release] [-- args]   build then execute
our test                        run tests/*.lang (@exit/@compile-error)
our deps                        resolved dependency graph
our fmt [--check]               format src/ + tests/; --check = CI mode
our doc <pkg>.<mod>             API docs — our doc std.vec
our toolchain                   running version + sibling pool + pin
our clean                       remove build/
our rebuild                     clean + build
our --version                   toolchain version

...with dependency commands taking --locked (exact lockfile) and
--offline (cache only) — PACKAGES.md.

ourc file.lang -o bin           bootstrap/reference compiler
ourc file.lang -g -o bin        compile with DWARF line tables
ourc file.lang --show-ir        inspect the CFG/IR
ourc file.lang --show-llvm      inspect generated LLVM
ourc --explain E1001            explain a diagnostic code
ourc lsp                        language server (stdio)
ourc test [dir]                 run the corpus-style native suite
ourc fmt <file>                 format a single file
```

`our build` in the default (debug) profile emits DWARF line tables, so
LLDB/GDB can set breakpoints by `file:line`, step by source line, and
print backtraces in `.lang` terms. `our build --release` and bare
`ourc` compile without debug info unless `-g` is passed.
