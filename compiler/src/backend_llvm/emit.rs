//! IR → textual LLVM IR. We emit `.ll` and hand it to the platform
//! clang for object code — the backend boundary is exactly here, so
//! `--show-llvm` shows the real contract with LLVM.
//!
//! Places lower to `getelementptr` chains: each path hop is
//! `gep <current aggregate>, ptr <addr>, i32 0, <selector>` — struct
//! fields select with `i32 idx`, array elements with the index's own
//! integer width. This is the ONLY place that knows about GEP; the IR
//! and everything above it speak in Places.

use crate::frontend::ast::{Auth, BinOp, Ty, UnOp};
use crate::ir::ir::*;
use std::collections::{HashMap, HashSet};


/// Canonical N19 names carry `<`/`>`/`,`/` ` — none legal in a bare
/// LLVM symbol, and none legal in a source identifier either, so the
/// mapping is injective: `vec.Vec<i32>` -> `vec.Vec-i32`, and arg
/// separators stay distinct from the `.` qualifiers already use.
fn llsym(n: &str) -> String {
    n.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '$') {
                c
            } else {
                '-'
            }
        })
        .collect()
}

fn llty(t: &Ty) -> String {
    match t {
        Ty::U8 | Ty::I8 => "i8".into(),
        Ty::U16 | Ty::I16 => "i16".into(),
        Ty::U32 | Ty::I32 => "i32".into(),
        Ty::U64 | Ty::I64 => "i64".into(),
        Ty::Bool => "i1".into(),
        Ty::Void => "void".into(),
        Ty::Struct(n) | Ty::App(n, _) => format!("%{}", llsym(n)),
        Ty::Array(e, n) => format!("[{n} x {}]", llty(e)),
        Ty::Atomic(e) => llty(e),
        // `mutex T` is a pointer to the runtime mutex object (lock
        // word + payload); `lock` yields a ptr to the payload — that
        // ptr IS the guard.
        Ty::Mutex(_) => "ptr".into(),
        Ty::Guard(_) => "ptr".into(),
        // A channel endpoint is a pointer to the runtime queue object.
        Ty::Channel(_) => "ptr".into(),
        // Opaque pointer — LLVM's `ptr`. Our IR already decided what
        // this may point at; the backend just carries it.
        Ty::Ref { .. } => "ptr".into(),
        Ty::Handle(_) => "ptr".into(),
        // A task token is a pointer to the task's environment frame;
        // env[0] holds the runtime `mml_task*` for `join`.
        Ty::Task(_) => "ptr".into(),
        // `*T` / `*const T` — the C ABI's pointer.
        Ty::Ptr { .. } => "ptr".into(),
        // `slice T` — {ptr, len}; one repr for all element types and
        // both mutabilities. Mutability is semantic, not physical.
        Ty::Slice { .. } => "{ptr, i64}".into(),
        // `str` — the same {ptr, len} pair, immutability enforced in
        // the semantics (`.bytes` re-reads it as a read-only slice).
        Ty::Str => "{ptr, i64}".into(),
        // `box T` — an owning raw pointer (N16); `alloc` tokens are
        // opaque pointers too (the system allocator needs no state).
        Ty::Box(_) | Ty::Alloc => "ptr".into(),
        // `box slice T` — owning {ptr, len}, same shape as a view.
        Ty::BoxSlice(_) => "{ptr, i64}".into(),
    }
}

fn r(r: &Reg) -> String {
    format!("%v{}", r.0)
}

// ── N51B: `noalias` eligibility ─────────────────────────────────────
// LLVM's `noalias` on a pointer param claims: during the call, memory
// accessed through pointers *based on* the argument is not accessed
// through any other provenance. The authority graph already proves
// root exclusivity for `write`/`own` place-aliases (N51A — every
// borrow path into the param's storage is rejected at the call site;
// pins 679–690). The residual risk is provenance LLVM can't see:
// loaded-pointer values that may secretly name the param's own
// storage.
//
// Self-scan (the pointee's type): `*T`, `handle`, `&`, `slice`, `str`
// fields are carriers — `handle` derefs, `*T` is FFI-minted, and
// slice/str/ref fields can hold foreign pointers populated at OTHER
// call sites via `slice(p, n)` or extern returns (invisible to this
// call's checks). Owned `box`/`box slice` recurse — the cell itself
// is distinct storage, but its CONTENTS can carry the same carriers.
// Mutex/guard/task/channel tokens address runtime heap objects, never
// the param's storage — but their payload types recurse.
//
// Sibling-scan (the other params): only `*T`/`handle` poison — the
// checked carriers (`&`, `slice`) are rejected at the call site when
// they'd alias the exclusive param, so a sibling that got through is
// provably aimed elsewhere.
fn self_carrier(
    t: &Ty,
    structs: &std::collections::HashMap<&str, &StructDef>,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    match t {
        Ty::Ptr { .. } | Ty::Handle(_) | Ty::Ref { .. } | Ty::Slice { .. } | Ty::Str => true,
        Ty::Array(e, _)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::Task(e)
        | Ty::Box(e)
        | Ty::BoxSlice(e) => self_carrier(e, structs, seen),
        Ty::Struct(n) => {
            if !seen.insert(n.clone()) {
                return false;
            }
            structs
                .get(n.as_str())
                .map(|s| s.fields.iter().any(|(_, ft)| self_carrier(ft, structs, seen)))
                .unwrap_or(false)
        }
        _ => false,
    }
}

fn sib_carrier(
    t: &Ty,
    structs: &std::collections::HashMap<&str, &StructDef>,
    seen: &mut std::collections::HashSet<String>,
) -> bool {
    match t {
        Ty::Ptr { .. } | Ty::Handle(_) => true,
        Ty::Array(e, _)
        | Ty::Atomic(e)
        | Ty::Mutex(e)
        | Ty::Guard(e)
        | Ty::Channel(e)
        | Ty::Task(e)
        | Ty::Box(e)
        | Ty::BoxSlice(e) => sib_carrier(e, structs, seen),
        Ty::Struct(n) => {
            if !seen.insert(n.clone()) {
                return false;
            }
            structs
                .get(n.as_str())
                .map(|s| s.fields.iter().any(|(_, ft)| sib_carrier(ft, structs, seen)))
                .unwrap_or(false)
        }
        _ => false,
    }
}

/// `noalias` for the proven subset only: `write` (place-alias → `ptr`)
/// and `own` on pointer-typed values (`box`). `read` never qualifies —
/// shared readers are the point. Anything uncertain gets no attribute.
fn noalias_flags(f: &Func, structs: &[StructDef]) -> Vec<bool> {
    let map: std::collections::HashMap<&str, &StructDef> =
        structs.iter().map(|s| (s.name.as_str(), s)).collect();
    let sib_bad: Vec<bool> = f
        .params
        .iter()
        .map(|(_, _, t)| sib_carrier(t, &map, &mut Default::default()))
        .collect();
    f.params
        .iter()
        .enumerate()
        .map(|(i, (_, a, t))| {
            let exclusive_ptr = match a {
                Some(Auth::Write) => true,
                Some(Auth::Own) => t.is_ptr_like(),
                _ => false,
            };
            exclusive_ptr
                && !self_carrier(t, &map, &mut Default::default())
                && !sib_bad.iter().enumerate().any(|(j, b)| j != i && *b)
        })
        .collect()
}


fn o(o: &Opnd) -> String {
    match o {
        Opnd::R(r) => self::r(r),
        Opnd::Imm(n) => format!("{n}"),
        Opnd::Fun(n) => format!("@{}", llsym(n)),
    }
}

// ── N78: large-aggregate ABI ────────────────────────────────────────
// Aggregates wider than two registers (>16B) never travel by value.
// Before this convention a `g = f(g)`-style accumulator emitted
// `load %G → call %G → store %G` per call, and the backend stacked a
// ~10KB arg copy + ~10KB sret slot on every site — `ir.g_expr` frames
// reached 3.2MB and self-hosting needed a 512MB task stack.
//
// Convention (non-extern fns only):
//   - `own`/plain param of big-agg type → `ptr` into the caller's
//     storage. `own` is a move: the caller's binding is dead after the
//     call, so the callee may use that storage in place — its local
//     slot aliases the incoming pointer; no alloca, no copy.
//   - big-agg return → hidden `ptr %sret` first param + `ret void`;
//     the callee memmoves the result into the destination.
//   - call site: the sret destination IS the slot the result would be
//     stored to (the `call → store` pair collapses to one in-place
//     write); `return f(g)` chains pass `%sret` straight through.
//     Big-agg args pass `ptr` to wherever the value already lives.
//   - values with no backing storage (extractvalue results, extern
//     returns) materialize into per-fn scratch slots, reused on a
//     free-list — bounded, hoisted to entry like all allocas.
//
// Scalars and ≤16B aggregates (slice, str, {ptr,i64}) keep the
// existing convention — they already ride registers. Externs keep
// the C convention. `noalias` eligibility is unchanged (own-ptrs are
// not marked — two own params could share caller storage via field
// splits, e.g. `f(g.a, g.b)`).

/// Aggregate whose size exceeds the two-register by-value budget.
fn big_agg(t: &Ty, structs: &[StructDef]) -> bool {
    matches!(t, Ty::Struct(_) | Ty::App(..) | Ty::Array(..))
        && ty_layout(t, structs).0 > 128
}

/// Per-function emission state for the big-agg convention.
struct Fx<'a> {
    structs: &'a [StructDef],
    /// Local-slot reg → the incoming param's LLVM name: own-big param
    /// homes alias caller storage instead of getting an alloca.
    alias: HashMap<u32, String>,
    /// Arg reg → renamed value binding (own-big param used as a value
    /// beyond its homing store — entry emits one `load` for it).
    renamed: HashMap<u32, String>,
    /// (block, instr) indices suppressed during emission.
    skip: HashSet<(usize, usize)>,
    /// Reg → the LLVM address text where its value currently lives.
    rptr: HashMap<u32, String>,
    /// Reg → root slot reg of its rptr entry (store invalidation).
    rroot: HashMap<u32, u32>,
    /// Scratch names currently referenced by rptr → their LLVM type
    /// (freed back to the right pool on invalidation).
    rscratch: HashMap<String, String>,
    /// Free scratch slots per LLVM type text.
    scratch: HashMap<String, Vec<String>>,
    /// The fn returns a big aggregate via hidden `ptr %sret`.
    sret: bool,
    /// Some block emitted a memmove — the module needs the declare.
    needs_memmove: bool,
    /// Extern fn names — they keep the C by-value convention.
    externs: &'a std::collections::HashSet<&'a str>,
    /// Reg use counts across the fn (the call lowering reads them).
    uses: HashMap<u32, u32>,
    /// The instruction after the one being emitted (store lookahead).
    next: Option<&'a Instr>,
    /// Current block's terminator (ret-of-call fast path).
    term: Option<&'a Term>,
    /// Owning fn name — diagnostics only.
    fname: &'a str,
    /// Call dst reg → place_key of the slot its sret already wrote —
    /// the following `store dst → that place` is a no-op.
    homed: HashMap<u32, String>,
}

/// Structural identity for a place — used to match a call's sret
/// target against the store that would have followed it.
fn place_key(p: &Place) -> String {
    format!("{:?}", p)
}

/// Operand rendered for aggregate-capable positions: renamed arg regs
/// print their materialized binding.
fn ov(fx: &Fx, v: &Opnd) -> String {
    if let Opnd::R(r) = v {
        if let Some(n) = fx.renamed.get(&r.0) {
            return n.clone();
        }
    }
    o(v)
}

/// Return a still-rptr'd scratch name to its type's pool.
fn rptr_free_one(fx: &mut Fx, name: &str) {
    if let Some(lt) = fx.rscratch.remove(name) {
        if let Some(p) = fx.scratch.get_mut(&lt) {
            p.push(name.to_string());
        }
    }
}

/// Drop every rptr entry — a memory write of unknown reach happened
/// (call, store through a loaded pointer). Scratch names referenced by
/// dying entries go back to the free pool.
fn rptr_clear(fx: &mut Fx) {
    let gone: Vec<String> = fx.rptr.drain().map(|(_, v)| v).collect();
    for v in gone {
        rptr_free_one(fx, &v);
    }
    fx.rroot.clear();
}

/// Drop rptr entries rooted at slot `root` — a store wrote inside that
/// slot's storage.
fn rptr_kill(fx: &mut Fx, root: u32) {
    let dead: Vec<u32> = fx
        .rroot
        .iter()
        .filter(|(_, rr)| **rr == root)
        .map(|(k, _)| *k)
        .collect();
    for k in dead {
        fx.rroot.remove(&k);
        if let Some(v) = fx.rptr.remove(&k) {
            rptr_free_one(fx, &v);
        }
    }
}

/// Record that `reg`'s value lives at `addr`. Root is the place's base
/// slot for targeted invalidation; scratch slots get their type noted
/// for pool return.
fn rptr_set(fx: &mut Fx, reg: u32, addr: String, root: u32, scratch_ty: Option<String>) {
    if let Some(lt) = scratch_ty {
        fx.rscratch.insert(addr.clone(), lt);
    }
    fx.rptr.insert(reg, addr);
    fx.rroot.insert(reg, root);
}

/// Take a scratch slot of the given LLVM type; `alloca`s are hoisted —
/// the pool is sized by the pre-pass so this never fails.
fn scratch_take(fx: &mut Fx, lt: &str) -> String {
    fx.scratch
        .get_mut(lt)
        .and_then(|p| p.pop())
        .unwrap_or_else(|| panic!("N78 scratch pool under-sized for {lt} in {}", fx.fname))
}

/// Every `Opnd::R` use in an instruction — operands, place index
/// values, call args.
fn opnd_regs(i: &Instr, f: &mut impl FnMut(u32)) {
    let mut place = |p: &Place| {
        for el in &p.path {
            if let PathElem::Index { val, .. } = el {
                if let Opnd::R(r) = val {
                    f(r.0);
                }
            }
        }
    };
    match i {
        Instr::Local { .. } | Instr::Arg { .. } | Instr::StrAddr { .. } => {}
        Instr::Const { .. } => {}
        Instr::Load { place: p, .. }
        | Instr::Addr { place: p, .. }
        | Instr::MakeHandle { place: p, .. }
        | Instr::AtomicLoad { place: p, .. } => place(p),
        Instr::Store { place: p, val } => {
            place(p);
            if let Opnd::R(r) = val {
                f(r.0);
            }
        }
        Instr::AtomicStore { place: p, val } | Instr::AtomicAdd { place: p, val, .. } => {
            place(p);
            if let Opnd::R(r) = val {
                f(r.0);
            }
        }
        Instr::AtomicCas {
            place: p,
            expected,
            new,
            ..
        } => {
            place(p);
            for v in [expected, new] {
                if let Opnd::R(r) = v {
                    f(r.0);
                }
            }
        }
        Instr::Bin { a, b, .. } => {
            for v in [a, b] {
                if let Opnd::R(r) = v {
                    f(r.0);
                }
            }
        }
        Instr::Unary { a, .. } | Instr::Cast { a, .. } | Instr::Extract { agg: a, .. } => {
            if let Opnd::R(r) = a {
                f(r.0);
            }
        }
        Instr::Call { args, .. } => {
            for (_, v) in args {
                if let Opnd::R(r) = v {
                    f(r.0);
                }
            }
        }
    }
}

fn term_regs(t: &Term, f: &mut impl FnMut(u32)) {
    match t {
        Term::Ret(Some(v), _) | Term::Br(v, _, _) => {
            if let Opnd::R(r) = v {
                f(r.0);
            }
        }
        _ => {}
    }
}

/// Analyze one fn: own-big param aliasing, scratch pool sizing, and
/// the use counts the call lowering needs. Returns the context plus
/// the hoisted scratch `alloca` lines for the entry block.
fn analyze<'a>(
    f: &'a Func,
    structs: &'a [StructDef],
    externs: &'a std::collections::HashSet<&'a str>,
) -> (Fx<'a>, Vec<String>) {
    let mut uses: HashMap<u32, u32> = HashMap::new();
    let mut bump = |r| *uses.entry(r).or_default() += 1;
    for b in &f.blocks {
        for i in &b.instrs {
            opnd_regs(i, &mut bump);
        }
        term_regs(&b.term, &mut bump);
    }

    let mut fx = Fx {
        structs,
        alias: HashMap::new(),
        renamed: HashMap::new(),
        skip: HashSet::new(),
        rptr: HashMap::new(),
        rroot: HashMap::new(),
        rscratch: HashMap::new(),
        scratch: HashMap::new(),
        sret: big_agg(&f.ret, structs),
        needs_memmove: false,
        externs,
        uses: HashMap::new(),
        next: None,
        term: None,
        homed: HashMap::new(),
        fname: &f.name,
    };

    // own-big params: if the incoming value reg's only use is the
    // homing `store arg → slot`, the slot aliases the incoming ptr —
    // no alloca, no copy. Otherwise the arg reg must materialize as a
    // value: one `load` at entry rebinds it (`renamed`).
    for ((_, a, t), ar) in f.params.iter().zip(f.arg_regs.iter()) {
        if matches!(a, Some(Auth::Read) | Some(Auth::Write)) || !big_agg(t, structs) {
            continue;
        }
        let home = f.blocks.iter().enumerate().find_map(|(bi, b)| {
            b.instrs.iter().enumerate().find_map(|(ii, ins)| {
                if let Instr::Store { place, val: Opnd::R(v) } = ins {
                    if v.0 == ar.0
                        && place.path.is_empty()
                        && place.ty == *t
                        && matches!(place.root, PRoot::Slot(_))
                    {
                        return Some((bi, ii));
                    }
                }
                None
            })
        });
        match (uses.get(&ar.0).copied().unwrap_or(0), home) {
            (1, Some((bi, ii))) => {
                if let Instr::Store { place, .. } = &f.blocks[bi].instrs[ii] {
                    if let PRoot::Slot(s) = place.root {
                        fx.alias.insert(s.0, r(ar));
                        fx.skip.insert((bi, ii));
                    }
                }
            }
            (0, _) => {}
            _ => {
                fx.renamed.insert(ar.0, format!("%a{}", ar.0));
            }
        }
    }

    // Scratch sizing: simulate the same place-backedness rules the
    // emitter uses — a reg is place-backed only after a Load / sret
    // Call dst / Store of it, and every Call or through-pointer store
    // invalidates the whole set. Over-allocation is safe; under is not.
    let mut live: HashMap<u32, u32> = HashMap::new(); // reg -> root key
    let mut held: HashMap<String, u32> = HashMap::new(); // lt -> held dst scratches
    let mut need: HashMap<String, u32> = HashMap::new();
    let bump_need = |need: &mut HashMap<String, u32>, held: &HashMap<String, u32>, lt: &str, extra: u32| {
        let cur = held.get(lt).copied().unwrap_or(0) + extra;
        if cur > *need.get(lt).unwrap_or(&0) {
            need.insert(lt.to_string(), cur);
        }
    };
    let root_key = |p: &Place| match &p.root {
        PRoot::Slot(x) | PRoot::Val(x) => x.0,
    };
    let val_root = |p: &Place| matches!(p.root, PRoot::Val(_));
    for b in &f.blocks {
        for (ii, i) in b.instrs.iter().enumerate() {
            match i {
                Instr::Load { dst, place } => {
                    live.insert(dst.0, root_key(place));
                }
                Instr::Store { place, .. }
                | Instr::AtomicStore { place, .. }
                | Instr::AtomicAdd { place, .. }
                | Instr::AtomicCas { place, .. } => {
                    if val_root(place) {
                        live.clear();
                        held.clear();
                    } else {
                        let k = root_key(place);
                        live.retain(|_, rr| *rr != k);
                    }
                }
                Instr::Call {
                    dst, ret, f: name, args,
                } => {
                    let ext = externs.contains(name.as_str());
                    let mut burst: HashMap<String, u32> = HashMap::new();
                    for (t, v) in args {
                        if ext || !big_agg(t, structs) {
                            continue;
                        }
                        let missing = matches!(v, Opnd::R(rg) if !live.contains_key(&rg.0));
                        if missing {
                            *burst.entry(llty(t)).or_default() += 1;
                        }
                    }
                    // The dst scratch is acquired alongside the arg
                    // burst while previously-held scratches still
                    // occupy the pool — all three coexist at the call.
                    let mut dst_scr: Option<String> = None;
                    if !ext {
                        if dst.is_none() && big_agg(ret, structs) {
                            dst_scr = Some(llty(ret));
                        }
                        if let (Some(d), true) = (dst, big_agg(ret, structs)) {
                            let next = b.instrs.get(ii + 1);
                            let homes = matches!(next, Some(Instr::Store { val: Opnd::R(v), .. }) if v.0 == d.0);
                            let tail = matches!(b.term, Term::Ret(Some(Opnd::R(v)), _)
                                if v.0 == d.0)
                                && fx.sret
                                && uses.get(&d.0).copied().unwrap_or(0) == 1;
                            if homes || tail {
                                live.insert(d.0, u32::MAX);
                            } else {
                                dst_scr = Some(llty(ret));
                            }
                        }
                    }
                    let mut tys: HashSet<String> = burst.keys().cloned().collect();
                    if let Some(lt) = &dst_scr {
                        tys.insert(lt.clone());
                    }
                    for lt in tys {
                        let co = held.get(&lt).copied().unwrap_or(0)
                            + burst.get(&lt).copied().unwrap_or(0)
                            + u32::from(dst_scr.as_ref() == Some(&lt));
                        if co > *need.get(&lt).unwrap_or(&0) {
                            need.insert(lt, co);
                        }
                    }
                    // The call's invalidation frees every held slot —
                    // then the new dst scratch becomes the held one
                    // (it stays reserved until the NEXT invalidating
                    // event, possibly several blocks later).
                    live.clear();
                    held.clear();
                    if let (Some(d), Some(lt)) = (dst, dst_scr) {
                        *held.entry(lt).or_default() += 1;
                        live.insert(d.0, u32::MAX);
                    }
                }
                _ => {}
            }
        }
        if let Term::Ret(Some(Opnd::R(v)), ty) = &b.term {
            if big_agg(ty, structs) && !live.contains_key(&v.0) {
                let lt = llty(ty);
                bump_need(&mut need, &held, &lt, 1);
            }
        }
    }

    // Materialize pools: `count` slots per LLVM type, named %s0..%sN
    // in a stable order (types sorted by name for determinism).
    let mut types: Vec<String> = need.keys().cloned().collect();
    types.sort();
    let mut allocas: Vec<String> = Vec::new();
    let mut scn = 0u32;
    for lt in &types {
        let n = *need.get(lt).unwrap();
        let mut pool = Vec::new();
        for _ in 0..n {
            let name = format!("%s{scn}");
            scn += 1;
            allocas.push(format!("    {name} = alloca {lt}\n"));
            pool.push(name);
        }
        fx.scratch.insert(lt.clone(), pool);
    }
    fx.uses = uses;
    (fx, allocas)
}

fn binop(op: &BinOp, ty: &Ty) -> &'static str {
    let s = ty.signed();
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => {
            if s {
                "sdiv"
            } else {
                "udiv"
            }
        }
        BinOp::Mod => {
            if s {
                "srem"
            } else {
                "urem"
            }
        }
        BinOp::Eq => "icmp eq",
        BinOp::Ne => "icmp ne",
        BinOp::Lt => {
            if s {
                "icmp slt"
            } else {
                "icmp ult"
            }
        }
        BinOp::Gt => {
            if s {
                "icmp sgt"
            } else {
                "icmp ugt"
            }
        }
        BinOp::Le => {
            if s {
                "icmp sle"
            } else {
                "icmp ule"
            }
        }
        BinOp::Ge => {
            if s {
                "icmp sge"
            } else {
                "icmp uge"
            }
        }
        // N25A — `>>` is arithmetic on signed, logical on unsigned.
        BinOp::BitAnd => "and",
        BinOp::BitOr => "or",
        BinOp::BitXor => "xor",
        BinOp::Shl => "shl",
        BinOp::Shr => {
            if s {
                "ashr"
            } else {
                "lshr"
            }
        }
        // `&&`/`||` lower to branch blocks at gen — never a Bin.
        BinOp::LAnd | BinOp::LOr => unreachable!("short-circuit lowers to blocks"),
    }
}

/// Emit the GEP chain for a place; returns the LLVM name of the final
/// address. Each hop: `gep <pointee>, ptr <addr>, i32 0, <selector>`.
fn emit_place(pl: &Place, out: &mut String, tmp: &mut u32, fx: &Fx) -> String {
    let mut addr = match &pl.root {
        // A `local` slot aliases the incoming param ptr when it homes
        // an own-big argument — the slot name never materializes.
        PRoot::Slot(x) => fx.alias.get(&x.0).cloned().unwrap_or_else(|| r(x)),
        PRoot::Val(x) => r(x),
    };
    let mut cur = pl.root_ty.clone();
    // Reaching INTO a mutex or through a guard: the address leads to a
    // cell holding the runtime object pointer — load it, then
    // `mml_mutex_data` yields the payload address. The checker only
    // allows this under a live guard; codegen stays honest anyway.
    let deref_wrap = |addr: String, cur: &mut Ty, out: &mut String, tmp: &mut u32| {
        let inner = match cur.clone() {
            Ty::Mutex(i) | Ty::Guard(i) => *i,
            _ => return addr,
        };
        let base = format!("%p{}", *tmp);
        *tmp += 1;
        out.push_str(&format!("    {base} = load ptr, ptr {addr}\n"));
        let data = format!("%p{}", *tmp);
        *tmp += 1;
        out.push_str(&format!("    {data} = call ptr @mml_mutex_data(ptr {base})\n"));
        *cur = inner;
        data
    };
    // Only a real hop reaches inside — a leaf `load m` wants the
    // object pointer itself, not the payload.
    for el in &pl.path {
        addr = deref_wrap(addr, &mut cur, out, tmp);
        let (sel, next, direct) = match el {
            // Field hops carry their resolved type from gen — the GEP
            // selector uses the CURRENT type (`cur`), so `x.Some.f`
            // lowers as `gep %Enum, 0, 1` then `gep %Payload, 0, f`
            // even though `enums` pseudo-register as {tag, [N x i8]}.
            PathElem::Field { idx, ty, .. } => (format!("i32 {idx}"), ty.clone(), false),
            PathElem::Index { val, ty } => {
                match &cur {
                    Ty::Array(e, _) => {
                        (format!("{} {}", llty(ty), o(val)), (**e).clone(), false)
                    }
                    // `s[i]` and re-slices root at the slice's LOADED
                    // data pointer — a single-index GEP, no leading 0.
                    Ty::Ptr { elem, .. } => {
                        (format!("{} {}", llty(ty), o(val)), (**elem).clone(), true)
                    }
                    _ => unreachable!("index path on non-array"),
                }
            }
        };
        let g = format!("%p{}", *tmp);
        *tmp += 1;
        if direct {
            out.push_str(&format!(
                "    {g} = getelementptr {}, ptr {addr}, {sel}\n",
                llty(&next)
            ));
        } else {
            // A `*v.field` place roots at a `*T` value — the aggregate
            // GEP'd into is `T`, the pointee, not `ptr` itself.
            let pointee = match &cur {
                Ty::Ptr { elem, .. } => (**elem).clone(),
                _ => cur.clone(),
            };
            out.push_str(&format!(
                "    {g} = getelementptr {}, ptr {addr}, i32 0, {sel}\n",
                llty(&pointee)
            ));
        }
        addr = g;
        cur = next;
    }
    addr
}

/// N57 — emit one instruction with an optional `!dbg` attachment.
/// `emit_instr` (and `emit_place` under it) can produce several LLVM
/// lines per source instr; every one carries the same source line.
fn emit_instr_dbg(i: &Instr, out: &mut String, tmp: &mut u32, dbg: Option<u32>, fx: &mut Fx) {
    let mut scratch = String::new();
    emit_instr(i, &mut scratch, tmp, fx);
    for line in scratch.lines() {
        out.push_str(line);
        if let Some(l) = dbg {
            out.push_str(&format!(", !dbg !{l}"));
        }
        out.push('\n');
    }
}

fn emit_term_dbg(t: &Term, out: &mut String, tmp: &mut u32, dbg: Option<u32>, fx: &mut Fx) {
    let mut scratch = String::new();
    emit_term(t, &mut scratch, tmp, fx);
    for line in scratch.lines() {
        out.push_str(line);
        if let Some(l) = dbg {
            out.push_str(&format!(", !dbg !{l}"));
        }
        out.push('\n');
    }
}

fn emit_instr(i: &Instr, out: &mut String, tmp: &mut u32, fx: &mut Fx) {
    match i {
        // The arg value's LLVM name is the parameter itself — no instr.
        Instr::Arg { .. } => {}
        Instr::Local { dst, ty, .. } => {
            // An own-big param's home aliases the incoming ptr — the
            // slot never materializes.
            if !fx.alias.contains_key(&dst.0) {
                out.push_str(&format!("    {} = alloca {}\n", r(dst), llty(ty)));
            }
        }
        Instr::Const { dst, ty, val } => {
            let lt = llty(ty);
            if lt == "ptr" {
                // A null pointer constant (`channel()`'s empty cell).
                out.push_str(&format!("    {} = inttoptr i64 {} to ptr\n", r(dst), val));
            } else if lt.starts_with('%') || lt.starts_with('[') {
                // Aggregates have no `add` — zero through memory.
                let s = *tmp;
                *tmp += 1;
                out.push_str(&format!(
                    "    %p{s} = alloca {lt}\n    store {lt} zeroinitializer, ptr %p{s}\n    {} = load {lt}, ptr %p{s}\n",
                    r(dst)
                ));
                // The zeroed slot backs the value — usable as an arg
                // address directly.
                rptr_set(fx, dst.0, format!("%p{s}"), u32::MAX, None);
            } else {
                // Materialize the immediate as a value: `add ty 0, n`.
                out.push_str(&format!("    {} = add {} 0, {}\n", r(dst), lt, val));
            }
        }
        Instr::Load { dst, place } => {
            let addr = emit_place(place, out, tmp, fx);
            let rk = match &place.root {
                PRoot::Slot(x) | PRoot::Val(x) => x.0,
            };
            out.push_str(&format!(
                "    {} = load {}, ptr {}\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
            rptr_set(fx, dst.0, addr, rk, None);
        }
        Instr::MakeHandle { dst, place } | Instr::Addr { dst, place } => {
            let addr = emit_place(place, out, tmp, fx);
            // `gep <leaf>, ptr <addr>, i32 0` names the address without
            // moving it — LLVM has no plain "copy pointer" instr.
            out.push_str(&format!(
                "    {} = getelementptr {}, ptr {}, i32 0\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
        }
        Instr::Store { place, val } => {
            // A call already wrote its sret result straight into this
            // place — the store is dead.
            if let Opnd::R(v) = val {
                if fx.homed.remove(&v.0).as_deref() == Some(place_key(place).as_str()) {
                    return;
                }
            }
            let addr = emit_place(place, out, tmp, fx);
            // Store of a value into the address it already occupies —
            // a self-copy left behind by the new arg convention.
            if let Opnd::R(v) = val {
                if fx.rptr.get(&v.0) == Some(&addr) {
                    return;
                }
            }
            out.push_str(&format!(
                "    store {} {}, ptr {}\n",
                llty(&place.ty),
                ov(fx, val),
                addr
            ));
            // A store writes memory: invalidate the place's whole root,
            // then record the stored reg as living at this address.
            let rk = match &place.root {
                PRoot::Slot(x) => Some(x.0),
                PRoot::Val(_) => None,
            };
            match rk {
                Some(k) => rptr_kill(fx, k),
                None => rptr_clear(fx),
            }
            if let Opnd::R(v) = val {
                rptr_set(
                    fx,
                    v.0,
                    addr,
                    rk.unwrap_or(u32::MAX),
                    None,
                );
            }
        }
        // Real atomic instructions — seq_cst until ordering gets its
        // own round; the ops are honest even under v1 sequential runs.
        Instr::AtomicLoad { dst, place } => {
            let addr = emit_place(place, out, tmp, fx);
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    {} = load atomic {}, ptr {} seq_cst, align {al}\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
            let rk = match &place.root {
                PRoot::Slot(x) | PRoot::Val(x) => x.0,
            };
            rptr_set(fx, dst.0, addr, rk, None);
        }
        Instr::AtomicStore { place, val } => {
            let addr = emit_place(place, out, tmp, fx);
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    store atomic {} {}, ptr {} seq_cst, align {al}\n",
                llty(&place.ty),
                o(val),
                addr
            ));
            match &place.root {
                PRoot::Slot(x) => rptr_kill(fx, x.0),
                PRoot::Val(_) => rptr_clear(fx),
            }
        }
        Instr::AtomicAdd { dst, place, val } => {
            let addr = emit_place(place, out, tmp, fx);
            out.push_str(&format!(
                "    {} = atomicrmw add ptr {}, {} {} seq_cst\n",
                r(dst),
                addr,
                llty(&place.ty),
                o(val),
            ));
        }
        Instr::AtomicCas {
            dst,
            place,
            expected,
            new,
        } => {
            let addr = emit_place(place, out, tmp, fx);
            let pair = format!("%p{}", *tmp);
            *tmp += 1;
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    {pair} = cmpxchg ptr {}, {} {}, {} {} seq_cst seq_cst, align {al}\n",
                addr,
                llty(&place.ty),
                o(expected),
                llty(&place.ty),
                o(new),
            ));
            out.push_str(&format!(
                "    {} = extractvalue {{{}, i1}} {pair}, 1\n",
                r(dst),
                llty(&place.ty),
            ));
            match &place.root {
                PRoot::Slot(x) => rptr_kill(fx, x.0),
                PRoot::Val(_) => rptr_clear(fx),
            }
        }
        Instr::Bin { dst, op, ty, a, b } => {
            let opname = binop(op, ty);
            out.push_str(&format!(
                "    {} = {} {} {}, {}\n",
                r(dst),
                opname,
                llty(ty),
                o(a),
                o(b)
            ));
        }
        Instr::Unary { dst, op, ty, a } => match op {
            UnOp::Neg => {
                out.push_str(&format!(
                    "    {} = sub {} 0, {}\n",
                    r(dst),
                    llty(ty),
                    o(a)
                ));
            }
            UnOp::Not => {
                out.push_str(&format!("    {} = icmp eq {} 0, {}\n", r(dst), llty(ty), o(a)));
            }
            UnOp::BitNot => {
                out.push_str(&format!("    {} = xor {} -1, {}\n", r(dst), llty(ty), o(a)));
            }
        },
        Instr::StrAddr { dst, idx, len } => {
            out.push_str(&format!(
                "    {} = getelementptr inbounds [{} x i8], ptr @.str.{}, i32 0, i32 0\n",
                r(dst),
                len,
                idx
            ));
        }
        Instr::Cast { dst, ty, from, a } => {
            let w = |t: &Ty| match t {
                Ty::Bool => 1,
                Ty::U8 | Ty::I8 => 8,
                Ty::U16 | Ty::I16 => 16,
                Ty::U32 | Ty::I32 => 32,
                _ => 64,
            };
            let (fw, tw) = (w(from), w(ty));
            if fw == tw {
                out.push_str(&format!(
                    "    {} = add {} {}, 0\n",
                    r(dst),
                    llty(ty),
                    o(a)
                ));
            } else {
                let op = if fw < tw {
                    if from.signed() {
                        "sext"
                    } else {
                        "zext"
                    }
                } else {
                    "trunc"
                };
                out.push_str(&format!(
                    "    {} = {} {} {} to {}\n",
                    r(dst),
                    op,
                    llty(from),
                    o(a),
                    llty(ty)
                ));
            }
        }
        Instr::Extract { dst, ty, agg, idx } => {
            out.push_str(&format!(
                "    {} = extractvalue {{ptr, i64}} {}, {}\n",
                r(dst),
                ov(fx, agg),
                idx
            ));
            let _ = ty;
        }
        Instr::Call { dst, ret, f, args } => {
            let ext = fx.externs.contains(f.as_str());
            let use_sret = !ext && big_agg(ret, fx.structs);
            let lt_ret = llty(ret);
            // sret destination: the slot the result would be stored to
            // (store lookahead), `%sret` for `return f(g)` chains, or a
            // scratch slot when the value has no dedicated home.
            let mut dest = String::new();
            let mut dest_scratch = false;
            let mut dest_rk = u32::MAX;
            // How the dst reg's value is reached after the call.
            enum DstHome {
                None,
                Place,   // a real slot (the store-skip lookahead)
                Sret,    // forwarded to our own %sret (tail call)
                Scratch, // pooled slot — value needs a reload to use
            }
            let mut home_kind = DstHome::None;
            if use_sret {
                if let Some(d) = dst {
                    let homed = matches!(fx.next,
                        Some(Instr::Store { place: _, val: Opnd::R(v) }) if v.0 == d.0);
                    if homed {
                        if let Some(Instr::Store { place, .. }) = fx.next {
                            let a = emit_place(place, out, tmp, fx);
                            fx.homed.insert(d.0, place_key(place));
                            dest_rk = match &place.root {
                                PRoot::Slot(x) | PRoot::Val(x) => x.0,
                            };
                            dest = a;
                            home_kind = DstHome::Place;
                        }
                    } else {
                        let tail = fx.sret
                            && matches!(fx.term,
                                Some(Term::Ret(Some(Opnd::R(v)), _)) if v.0 == d.0)
                            && fx.uses.get(&d.0).copied().unwrap_or(0) == 1;
                        if tail {
                            dest = "%sret".to_string();
                            home_kind = DstHome::Sret;
                        } else {
                            dest = scratch_take(fx, &lt_ret);
                            dest_scratch = true;
                            home_kind = DstHome::Scratch;
                        }
                    }
                } else {
                    dest = scratch_take(fx, &lt_ret);
                    dest_scratch = true;
                }
            }
            // Args: big-agg operands go by pointer to their current
            // storage; values without backing storage materialize into
            // scratch slots first.
            let mut freed: Vec<(String, String)> = Vec::new();
            let as_: Vec<String> = args
                .iter()
                .map(|(t, v)| {
                    let lt = llty(t);
                    if ext || !big_agg(t, fx.structs) {
                        return format!("{} {}", lt, ov(fx, v));
                    }
                    let addr = match v {
                        Opnd::R(rg) => match fx.rptr.get(&rg.0) {
                            Some(a) => a.clone(),
                            None => {
                                let s = scratch_take(fx, &lt);
                                out.push_str(&format!(
                                    "    store {} {}, ptr {}\n",
                                    lt,
                                    ov(fx, v),
                                    s
                                ));
                                freed.push((s.clone(), lt.clone()));
                                s
                            }
                        },
                        _ => unreachable!("big-agg arg is always a reg"),
                    };
                    format!("ptr {addr}")
                })
                .collect();
            let callee = llsym(f);
            if use_sret {
                let mut full = vec![format!("ptr {dest}")];
                full.extend(as_);
                out.push_str(&format!(
                    "    call void @{}({})\n",
                    callee,
                    full.join(", ")
                ));
            } else {
                match dst {
                    Some(d) => out.push_str(&format!(
                        "    {} = call {} @{}({})\n",
                        r(d),
                        lt_ret,
                        callee,
                        as_.join(", ")
                    )),
                    None => out.push_str(&format!(
                        "    call {} @{}({})\n",
                        lt_ret,
                        callee,
                        as_.join(", ")
                    )),
                }
            }
            // The call may write anywhere — cached addresses are dead.
            rptr_clear(fx);
            // Arg scratches are consumed by the call — recycle them.
            for (s, lt) in freed {
                if let Some(p) = fx.scratch.get_mut(&lt) {
                    p.push(s);
                }
            }
            if use_sret {
                if let Some(d) = dst {
                    // Value-uses of the dst beyond the homed store /
                    // tail ret need the reg bound — one reload.
                    let covered = match home_kind {
                        DstHome::Place | DstHome::Sret => 1,
                        _ => 0,
                    };
                    let needs_val =
                        fx.uses.get(&d.0).copied().unwrap_or(0) > covered;
                    if needs_val {
                        out.push_str(&format!(
                            "    {} = load {}, ptr {}\n",
                            r(d),
                            lt_ret,
                            dest
                        ));
                    }
                    rptr_set(
                        fx,
                        d.0,
                        dest,
                        dest_rk,
                        if dest_scratch { Some(lt_ret) } else { None },
                    );
                } else if dest_scratch {
                    // An ignored result still parks in the pool slot;
                    // it's not rptr'd, so hand it straight back.
                    if let Some(p) = fx.scratch.get_mut(&lt_ret) {
                        p.push(dest);
                    }
                }
            }
        }
    }
}

fn emit_term(t: &Term, out: &mut String, _tmp: &mut u32, fx: &mut Fx) {
    match t {
        Term::Ret(Some(v), ty) if fx.sret => {
            // The result goes to the caller's destination — a tail
            // call already wrote it there (self-copy elided).
            let src = match v {
                Opnd::R(rg) => match fx.rptr.get(&rg.0) {
                    Some(a) => a.clone(),
                    None => {
                        let lt = llty(ty);
                        let s = scratch_take(fx, &lt);
                        out.push_str(&format!(
                            "    store {} {}, ptr {}\n",
                            lt,
                            ov(fx, v),
                            s
                        ));
                        s
                    }
                },
                _ => unreachable!("big-agg ret operand is a reg"),
            };
            if src != "%sret" {
                let bytes = ty_layout(ty, fx.structs).0 / 8;
                out.push_str(&format!(
                    "    call void @llvm.memmove.p0.p0.i64(ptr %sret, ptr {src}, i64 {bytes}, i1 false)\n"
                ));
                fx.needs_memmove = true;
            }
            out.push_str("    ret void\n");
        }
        Term::Ret(Some(v), ty) => {
            out.push_str(&format!("    ret {} {}\n", llty(ty), ov(fx, v)));
        }
        Term::Ret(None, _) => out.push_str("    ret void\n"),
        Term::Br(c, t, e) => {
            out.push_str(&format!("    br i1 {}, label %{}, label %{}\n", ov(fx, c), t, e));
        }
        Term::Jmp(l) => out.push_str(&format!("    br label %{}\n", l)),
        Term::Dead => out.push_str("    unreachable\n"),
    }
}

/// Runtime boundary (N13): signature table for `runtime/mmlrt_*.c`.
/// Calls into it are ordinary `call`s — we only owe LLVM a `declare`.
fn rt_sig(n: &str) -> Option<(&'static str, &'static [&'static str])> {
    Some(match n {
        "mml_task_spawn" => ("ptr", &["ptr", "ptr"]),
        "mml_task_join" => ("void", &["ptr"]),
        "mml_mutex_new" => ("ptr", &["i64"]),
        "mml_mutex_wrap" => ("ptr", &["ptr"]),
        "mml_mutex_data" => ("ptr", &["ptr"]),
        "mml_mutex_lock" => ("ptr", &["ptr"]),
        "mml_mutex_unlock" => ("void", &["ptr"]),
        "mml_chan_new" => ("ptr", &["i64"]),
        "mml_chan_send" => ("void", &["ptr", "ptr"]),
        "mml_chan_recv" => ("void", &["ptr", "ptr"]),
        "mml_chan_close" => ("void", &["ptr"]),
        // N15 — dynamic slice/array bounds enforcement. N50 — the
        // check is an inline `icmp ult`; `_fail` is the cold trap arm.
        "mml_bounds_check" => ("void", &["i64", "i64"]),
        "mml_bounds_fail" => ("void", &["i64", "i64"]),
        "mml_range_check" => ("void", &["i64", "i64", "i64"]),
        // N25A — shift amount < operand width (no C-style masking).
        "mml_shift_check" => ("void", &["i64", "i64"]),
        // N15B — str equality (memcmp semantics) and the UTF-8 gate
        // behind `str_from_utf8`.
        "mml_str_eq" => ("i32", &["ptr", "i64", "ptr", "i64"]),
        "mml_str_validate" => ("void", &["ptr", "i64"]),
        // N16 — explicit heap: malloc with abort-on-OOM, free.
        "mml_alloc" => ("ptr", &["i64"]),
        "mml_alloc_slice" => ("ptr", &["i64", "i64"]),
        "mml_free" => ("void", &["ptr"]),
        _ => return None,
    })
}

// ── N57C: DIType construction ────────────────────────────────────
// The honest physical representation: scalars as DIBasicType, the
// pointer family as DIDerivedType(pointer), slice/str/box slice as
// {ptr, i64} composites, structs/arrays as real composites with
// member offsets computed under LLVM's default layout rules.

fn align_up(v: u64, a: u64) -> u64 {
    let a = a.max(1);
    v.div_ceil(a) * a
}

/// (size, align) in bits under the default datalayout — natural
/// alignment, i1 stored in a byte, ptr 8 bytes.
fn ty_layout(t: &Ty, structs: &[StructDef]) -> (u64, u64) {
    match t {
        Ty::Bool | Ty::U8 | Ty::I8 => (8, 8),
        Ty::U16 | Ty::I16 => (16, 16),
        Ty::U32 | Ty::I32 => (32, 32),
        Ty::U64 | Ty::I64 => (64, 64),
        Ty::Array(e, n) => {
            let (s, a) = ty_layout(e, structs);
            ((*n as u64) * align_up(s, a), a)
        }
        Ty::Struct(n) | Ty::App(n, _) => {
            let mut off = 0u64;
            let mut al = 8u64;
            if let Some(st) = structs.iter().find(|s| s.name == *n) {
                for (_, ft) in &st.fields {
                    let (s, a) = ty_layout(ft, structs);
                    off = align_up(off, a) + s;
                    al = al.max(a);
                }
            }
            (align_up(off, al), al)
        }
        Ty::Atomic(e) => ty_layout(e, structs),
        Ty::Slice { .. } | Ty::Str | Ty::BoxSlice(_) => (128, 64),
        Ty::Void => (0, 8),
        // Ref/Handle/Task/Mutex/Guard/Channel/Ptr/Box/Alloc — all `ptr`.
        _ => (64, 64),
    }
}

/// The debug-display name — the source spelling, not the LLVM symbol.
fn ty_key(t: &Ty) -> String {
    match t {
        Ty::U8 => "u8".into(),
        Ty::U16 => "u16".into(),
        Ty::U32 => "u32".into(),
        Ty::U64 => "u64".into(),
        Ty::I8 => "i8".into(),
        Ty::I16 => "i16".into(),
        Ty::I32 => "i32".into(),
        Ty::I64 => "i64".into(),
        Ty::Bool => "bool".into(),
        Ty::Void => "void".into(),
        Ty::Struct(n) | Ty::App(n, _) => n.clone(),
        Ty::Array(e, n) => format!("[{n}]{}", ty_key(e)),
        Ty::Ref { mutable, elem } => {
            format!("&{}{}", if *mutable { "mut " } else { "" }, ty_key(elem))
        }
        Ty::Ptr { mutable, elem } => {
            format!("*{}{}", if *mutable { "" } else { "const " }, ty_key(elem))
        }
        Ty::Handle(e) => format!("handle {}", ty_key(e)),
        Ty::Task(e) => format!("task {}", ty_key(e)),
        Ty::Atomic(e) => format!("atomic {}", ty_key(e)),
        Ty::Mutex(e) => format!("mutex {}", ty_key(e)),
        Ty::Guard(e) => format!("guard {}", ty_key(e)),
        Ty::Channel(e) => format!("channel {}", ty_key(e)),
        Ty::Slice { mutable, elem } => {
            format!("slice {}{}", if *mutable { "mut " } else { "" }, ty_key(elem))
        }
        Ty::Str => "str".into(),
        Ty::Box(e) => format!("box {}", ty_key(e)),
        Ty::BoxSlice(e) => format!("box slice {}", ty_key(e)),
        Ty::Alloc => "alloc".into(),
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Reserve a meta slot for a composite BEFORE filling it, so a type
/// that mentions itself behind a pointer (`box T`) resolves to this
/// node instead of recursing forever.
fn ditype(
    t: &Ty,
    structs: &[StructDef],
    meta: &mut Vec<String>,
    cache: &mut HashMap<String, u32>,
) -> u32 {
    let key = ty_key(t);
    if let Some(&id) = cache.get(&key) {
        return id;
    }
    let (size, align) = ty_layout(t, structs);
    match t {
        Ty::Atomic(e) => return ditype(e, structs, meta, cache),
        Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64
        | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64
        | Ty::Bool | Ty::Void => {
            let enc = match t {
                Ty::Bool => "DW_ATE_boolean",
                Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 => "DW_ATE_unsigned",
                // `void` has no encoding; `*void` is handled by the
                // pointer arm's `baseType: null` — this leaf is only a
                // defensive placeholder.
                _ => "DW_ATE_signed",
            };
            let id = meta.len() as u32;
            meta.push(format!(
                "!DIBasicType(name: \"{key}\", size: {size}, encoding: {enc})"
            ));
            cache.insert(key, id);
            id
        }
        Ty::Ref { elem, .. } | Ty::Ptr { elem, .. } | Ty::Box(elem) | Ty::Handle(elem) => {
            // `*void` — C's `void *` — has no pointee type.
            let base = if **elem == Ty::Void {
                "null".to_string()
            } else {
                format!("!{}", ditype(elem, structs, meta, cache))
            };
            let id = meta.len() as u32;
            meta.push(format!(
                "!DIDerivedType(tag: DW_TAG_pointer_type, name: \"{key}\", baseType: {base}, size: 64)"
            ));
            cache.insert(key, id);
            id
        }
        // The opaque handles — a pointer to storage the debugger can't
        // name (runtime mutex/queue/env). `baseType: null` = `void *`.
        Ty::Mutex(_) | Ty::Guard(_) | Ty::Channel(_) | Ty::Task(_) | Ty::Alloc => {
            let id = meta.len() as u32;
            meta.push(format!(
                "!DIDerivedType(tag: DW_TAG_pointer_type, name: \"{key}\", baseType: null, size: 64)"
            ));
            cache.insert(key, id);
            id
        }
        Ty::Slice { elem, .. } | Ty::BoxSlice(elem) => {
            composite(meta, cache, structs, key, size, align, vec![
                ("ptr".to_string(), Ty::Ptr { mutable: true, elem: elem.clone() }),
                ("len".to_string(), Ty::I64),
            ])
        }
        Ty::Str => composite(meta, cache, structs, key, size, align, vec![
            ("ptr".to_string(), Ty::Ptr { mutable: false, elem: Box::new(Ty::U8) }),
            ("len".to_string(), Ty::I64),
        ]),
        Ty::Array(e, n) => {
            let base = ditype(e, structs, meta, cache);
            let sub = meta.len() as u32;
            meta.push(format!("!DISubrange(count: {n})"));
            let tup = meta.len() as u32;
            meta.push(format!("!{{!{sub}}}"));
            let id = meta.len() as u32;
            meta.push(format!(
                "!DICompositeType(tag: DW_TAG_array_type, name: \"{key}\", baseType: !{base}, size: {size}, align: {align}, elements: !{tup})"
            ));
            cache.insert(key, id);
            id
        }
        Ty::Struct(n) | Ty::App(n, _) => {
            let members: Vec<(String, Ty)> = structs
                .iter()
                .find(|s| s.name == *n)
                .map(|s| s.fields.clone())
                .unwrap_or_default();
            composite(meta, cache, structs, key, size, align, members)
        }
    }
}

fn composite(
    meta: &mut Vec<String>,
    cache: &mut HashMap<String, u32>,
    structs: &[StructDef],
    name: String,
    size: u64,
    align: u64,
    members: Vec<(String, Ty)>,
) -> u32 {
    let id = meta.len() as u32;
    meta.push(String::new()); // reserved — filled after members exist
    cache.insert(name.clone(), id);
    let mut mids = Vec::new();
    let mut off = 0u64;
    for (mn, mt) in &members {
        let (ms, ma) = ty_layout(mt, structs);
        off = align_up(off, ma);
        let base = ditype(mt, structs, meta, cache);
        let mid = meta.len() as u32;
        meta.push(format!(
            "!DIDerivedType(tag: DW_TAG_member, name: \"{}\", baseType: !{base}, size: {ms}, offset: {off})",
            esc(mn)
        ));
        mids.push(mid);
        off += ms;
    }
    let tup = meta.len() as u32;
    meta.push(format!(
        "!{{{}}}",
        mids.iter().map(|m| format!("!{m}")).collect::<Vec<_>>().join(", ")
    ));
    meta[id as usize] = format!(
        "!DICompositeType(tag: DW_TAG_structure_type, name: \"{}\", size: {size}, align: {align}, elements: !{tup})",
        esc(&name)
    );
    id
}

/// `debug` gates the N57 DWARF line tables (`!dbg` attachments and the
/// metadata tail). Off by default for ourc output — like C's `-g` —
/// so textual `.ll` artifacts stay free of metadata noise; `our
/// build`'s debug profile turns it on.
pub fn emit(m: &Module, triple: &str, debug: bool) -> String {
    let mut out = String::new();
    out.push_str(&format!("target triple = \"{triple}\"\n\n"));
    // Declare the runtime entry points this module calls into.
    let mut used = std::collections::BTreeSet::new();
    for f in &m.fns {
        for b in &f.blocks {
            for i in &b.instrs {
                if let Instr::Call { f, .. } = i {
                    if f.starts_with("mml_") {
                        used.insert(f.as_str());
                    }
                }
            }
        }
    }
    for n in used {
        if let Some((ret, args)) = rt_sig(n) {
            // N50 — trap helpers never return; say so so LLVM treats
            // their call edges as terminal cold paths.
            let attrs = if n == "mml_bounds_fail" { " noreturn cold" } else { "" };
            out.push_str(&format!(
                "declare {} @{}({}){}\n",
                ret, llsym(n), args.join(", "), attrs
            ));
        }
    }
    if debug {
        out.push_str("declare void @llvm.dbg.declare(metadata, metadata, metadata)\n");
        out.push_str("declare void @llvm.dbg.value(metadata, metadata, metadata)\n");
    }
    // N14 — the C-ABI boundary: `extern fn`s are declares; literals
    // are private null-terminated globals.
    for x in &m.externs {
        let ps: Vec<String> = x.params.iter().map(llty).collect();
        out.push_str(&format!(
            "declare {} @{}({})\n",
            llty(&x.ret),
            llsym(&x.name),
            ps.join(", ")
        ));
    }
    for (i, bytes) in m.strings.iter().enumerate() {
        let mut enc = String::new();
        for &b in bytes {
            match b {
                0x20..=0x7e if b != b'"' && b != b'\\' => enc.push(b as char),
                _ => enc.push_str(&format!("\\{b:02X}")),
            }
        }
        out.push_str(&format!(
            "@.str.{i} = private unnamed_addr constant [{} x i8] c\"{}\\00\"\n",
            bytes.len() + 1,
            enc
        ));
    }
    if out.ends_with(")\n") {
        out.push('\n');
    }
    for st in &m.structs {
        let fs: Vec<String> = st.fields.iter().map(|(_, t)| llty(t)).collect();
        out.push_str(&format!("%{} = type {{ {} }}\n", llsym(&st.name), fs.join(", ")));
    }
    if !m.structs.is_empty() {
        out.push('\n');
    }
    // N57 — DWARF line tables. One `DICompileUnit`, one `DIFile` per
    // source file, one distinct `DISubprogram` per fn, one
    // `DILocation` per span-carrying instruction/terminator. Numbering
    // is assigned in emission order: !0 CU, !1-2 module flags, !3..
    // files, then shared subroutine type, then per-fn subprogram and
    // its locations.
    let mut meta: Vec<String> = Vec::new();
    let mut subs: Vec<u32> = Vec::new();
    let mut locs: HashMap<(usize, usize, i64), u32> = HashMap::new();
    // N57C — (fn, block, instr) → (DILocalVariable id, DILocation id),
    // for `Local` slots (dbg.declare at the slot's source position)
    // and place-alias `Arg`s (dbg.value + deref: the variable IS the
    // pointee). Debug intrinsics are invalid without a !dbg position,
    // so each var borrows its instr's location or mints one at the
    // fn's own line.
    let mut var_decls: HashMap<(usize, usize, usize), (u32, u32)> = HashMap::new();
    let mut arg_vals: HashMap<(usize, usize, usize), (u32, u32)> = HashMap::new();
    if debug {
        let mut file_ids: HashMap<&str, u32> = HashMap::new();
        let mut files: Vec<&str> = Vec::new();
        for f in &m.fns {
            if !file_ids.contains_key(f.file.as_str()) {
                file_ids.insert(&f.file, (3 + files.len()) as u32);
                files.push(&f.file);
            }
        }
        let cu_file = m
            .fns
            .iter()
            .find(|f| f.name == "main")
            .map(|f| file_ids[f.file.as_str()])
            .unwrap_or(3);
        meta.push(format!(
            "distinct !DICompileUnit(language: DW_LANG_C11, file: !{cu_file}, producer: \"ourc\", isOptimized: false, runtimeVersion: 0, emissionKind: FullDebug)"
        ));
        meta.push("!{i32 2, !\"Dwarf Version\", i32 4}".to_string());
        meta.push("!{i32 2, !\"Debug Info Version\", i32 3}".to_string());
        for f in &files {
            let (dir, base) = match f.rfind('/') {
                Some(i) => (&f[..i], &f[i + 1..]),
                None => ("", *f),
            };
            meta.push(format!(
                "!DIFile(filename: \"{}\", directory: \"{}\")",
                base.replace('\\', "\\\\").replace('"', "\\\""),
                dir.replace('\\', "\\\\").replace('"', "\\\""),
            ));
        }
        let empty_tup = meta.len() as u32;
        meta.push("!{}".to_string());
        let st_ty = meta.len() as u32;
        meta.push(format!("!DISubroutineType(types: !{empty_tup})"));
        // Per-fn subprogram id + (fn, block, instr|-1 term) -> location id.
        for (fi, f) in m.fns.iter().enumerate() {
            let fid = file_ids[f.file.as_str()];
            let sub = meta.len() as u32;
            subs.push(sub);
            let sym = if f.name == "main" { "mml_main" } else { f.name.as_str() };
            meta.push(format!(
                "distinct !DISubprogram(name: \"{}\", linkageName: \"{}\", scope: !{fid}, file: !{fid}, line: {}, type: !{st_ty}, scopeLine: {}, spFlags: DISPFlagDefinition, unit: !0)",
                f.name.replace('\\', "\\\\").replace('"', "\\\""),
                llsym(sym),
                f.span.line,
                f.span.line,
            ));
            for (bi, b) in f.blocks.iter().enumerate() {
                for (ii, s) in b.isps.iter().enumerate() {
                    if let Some(sp) = s {
                        locs.insert((fi, bi, ii as i64), meta.len() as u32);
                        meta.push(format!(
                            "!DILocation(line: {}, column: {}, scope: !{sub})",
                            sp.line, sp.col
                        ));
                    }
                }
                if let Some(sp) = b.tspan {
                    locs.insert((fi, bi, -1), meta.len() as u32);
                    meta.push(format!(
                        "!DILocation(line: {}, column: {}, scope: !{sub})",
                        sp.line, sp.col
                    ));
                }
            }
        }
        // N57C — variables. Named `Local`s get a DILocalVariable at
        // the slot's declaration point; place-alias params (`read`/
        // `write`) are pointers already, so dbg.value + DW_OP_deref
        // describes the referent itself. Anonymous temporaries and
        // `$`-prefixed synthesis stay invisible — the same surface
        // discipline diagnostics use for `$rvN`.
        let mut ty_cache: HashMap<String, u32> = HashMap::new();
        for (fi, f) in m.fns.iter().enumerate() {
            let sub = subs[fi];
            let fid = file_ids[f.file.as_str()];
            for (bi, b) in f.blocks.iter().enumerate() {
                for (ii, i) in b.instrs.iter().enumerate() {
                    match i {
                        Instr::Local { name, ty, .. }
                            if !name.is_empty() && !name.starts_with('$') =>
                        {
                            let t = ditype(ty, &m.structs, &mut meta, &mut ty_cache);
                            let line = b
                                .isps
                                .get(ii)
                                .and_then(|s| *s)
                                .map(|s| s.line)
                                .unwrap_or(f.span.line);
                            let dl = locs.entry((fi, bi, ii as i64)).or_insert_with(|| {
                                let l = meta.len() as u32;
                                meta.push(format!(
                                    "!DILocation(line: {line}, column: 1, scope: !{sub})"
                                ));
                                l
                            });
                            let v = meta.len() as u32;
                            meta.push(format!(
                                "!DILocalVariable(name: \"{}\", scope: !{sub}, file: !{fid}, line: {line}, type: !{t})",
                                esc(name)
                            ));
                            var_decls.insert((fi, bi, ii), (v, *dl));
                        }
                        Instr::Arg { idx, .. } => {
                            let (n, a, t) = &f.params[*idx as usize];
                            if matches!(a, Some(Auth::Read) | Some(Auth::Write))
                                && !n.starts_with('$')
                            {
                                let dt = ditype(t, &m.structs, &mut meta, &mut ty_cache);
                                let dl = locs.entry((fi, bi, ii as i64)).or_insert_with(|| {
                                    let l = meta.len() as u32;
                                    meta.push(format!(
                                        "!DILocation(line: {}, column: 1, scope: !{sub})",
                                        f.span.line
                                    ));
                                    l
                                });
                                let v = meta.len() as u32;
                                meta.push(format!(
                                    "!DILocalVariable(name: \"{}\", scope: !{sub}, file: !{fid}, line: {}, type: !{dt}, arg: {})",
                                    esc(n),
                                    f.span.line,
                                    idx + 1
                                ));
                                arg_vals.insert((fi, bi, ii), (v, *dl));
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    let extern_names: std::collections::HashSet<&str> =
        m.externs.iter().map(|e| e.name.as_str()).collect();
    let mut needs_memmove = false;
    for (fi, f) in m.fns.iter().enumerate() {
        let mut tmp = 0u32;
        let na = noalias_flags(f, &m.structs);
        let (mut fx, allocas) = analyze(f, &m.structs, &extern_names);
        let mut params: Vec<String> = Vec::new();
        if fx.sret {
            params.push("ptr %sret".to_string());
        }
        params.extend(
            f.params
                .iter()
                .zip(f.arg_regs.iter())
                .zip(na.iter())
                .map(|(((_, a, t), r), na)| {
                    let lt = match a {
                        Some(Auth::Read) | Some(Auth::Write) => "ptr".to_string(),
                        _ if big_agg(t, &m.structs) => "ptr".to_string(),
                        _ => llty(t),
                    };
                    let attr = if *na { " noalias" } else { "" };
                    format!("{lt}{attr} {}", self::r(r))
                }),
        );
        let ret_lt = if fx.sret {
            "void".to_string()
        } else {
            llty(&f.ret)
        };
        // N26A — the C runtime owns `main` (it builds the argv view);
        // the user's entry emits as `mml_main` and `mml_main_args`
        // below forwards to it with the `slice str` it declared.
        let sym = if f.name == "main" { "mml_main" } else { &f.name };
        let dsub = if debug {
            format!(" !dbg !{}", subs[fi])
        } else {
            String::new()
        };
        out.push_str(&format!(
            "define {} @{}({}){} {{\n",
            ret_lt,
            llsym(sym),
            params.join(", "),
            dsub
        ));
        // Frame slots are allocated ONCE at entry — an `alloca` inside
        // a loop body would grow the stack every iteration (threads'
        // stacks are small; N13's while-loop `let g = lock(m)` proved
        // it). Hoist every `local` to the top of the entry block.
        let mut locals: Vec<(usize, usize)> = Vec::new();
        for (bi, b) in f.blocks.iter().enumerate() {
            for (ii, i) in b.instrs.iter().enumerate() {
                if let Instr::Local { .. } = i {
                    locals.push((bi, ii));
                }
            }
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            out.push_str(&format!("{}:\n", b.label));
            if bi == 0 {
                for &(lbi, lii) in &locals {
                    if fx.skip.contains(&(lbi, lii)) {
                        continue;
                    }
                    emit_instr_dbg(
                        &f.blocks[lbi].instrs[lii],
                        &mut out,
                        &mut tmp,
                        locs.get(&(fi, lbi, lii as i64)).copied(),
                        &mut fx,
                    );
                }
                // N78 scratch slots + own-big param value rebinds.
                for a in &allocas {
                    out.push_str(a);
                }
                for (i, ((_, a, t), ar)) in
                    f.params.iter().zip(f.arg_regs.iter()).enumerate()
                {
                    let _ = (i, a);
                    if let Some(n) = fx.renamed.get(&ar.0) {
                        out.push_str(&format!(
                            "    {} = load {}, ptr {}\n",
                            n,
                            llty(t),
                            r(ar)
                        ));
                    }
                }
            }
            for (ii, i) in b.instrs.iter().enumerate() {
                if fx.skip.contains(&(bi, ii)) {
                    continue;
                }
                if let Instr::Local { dst, .. } = i {
                    // The alloca was hoisted to entry; this position
                    // is where the *variable* is born — dbg.declare
                    // anchors lldb's lifetime to the source line.
                    if let Some(&(v, dl)) = var_decls.get(&(fi, bi, ii)) {
                        if !fx.alias.contains_key(&dst.0) {
                            out.push_str(&format!(
                                "    call void @llvm.dbg.declare(metadata ptr {}, metadata !{v}, metadata !DIExpression()), !dbg !{dl}\n",
                                r(dst)
                            ));
                        }
                    }
                    continue;
                }
                if let Instr::Arg { dst, .. } = i {
                    if let Some(&(v, dl)) = arg_vals.get(&(fi, bi, ii)) {
                        out.push_str(&format!(
                            "    call void @llvm.dbg.value(metadata ptr {}, metadata !{v}, metadata !DIExpression(DW_OP_deref)), !dbg !{dl}\n",
                            fx.renamed.get(&dst.0).cloned().unwrap_or_else(|| r(dst))
                        ));
                    }
                }
                fx.next = b.instrs.get(ii + 1);
                fx.term = Some(&b.term);
                emit_instr_dbg(i, &mut out, &mut tmp, locs.get(&(fi, bi, ii as i64)).copied(), &mut fx);
            }
            fx.next = None;
            fx.term = Some(&b.term);
            emit_term_dbg(&b.term, &mut out, &mut tmp, locs.get(&(fi, bi, -1)).copied(), &mut fx);
        }
        out.push_str("}\n\n");
        needs_memmove |= fx.needs_memmove;
    }
    // N78 — sret'd returns move through memmove; declare on use.
    if needs_memmove {
        out.push_str("declare void @llvm.memmove.p0.p0.i64(ptr, ptr, i64, i1)\n\n");
    }
    // The metadata tail — CU/flags/files/subprograms/locations.
    if debug {
        out.push_str("!llvm.dbg.cu = !{!0}\n");
        out.push_str("!llvm.module.flags = !{!1, !2}\n");
        for (i, m_) in meta.iter().enumerate() {
            out.push_str(&format!("!{i} = {m_}\n"));
        }
    }
    // N26A — the runtime's platform entry always calls `mml_main_args` with
    // stack-built `slice str`. This shim forwards to the user entry
    // under whichever signature it declared, widening the exit code
    // to i64 for the rt (which narrows it back to `int`).
    if let Some(f) = m.fns.iter().find(|f| f.name == "main") {
        let rt = llty(&f.ret);
        out.push_str("define i64 @mml_main_args({ptr, i64} %mml_args) {\nentry:\n");
        let call = if f.params.is_empty() {
            format!("call {rt} @mml_main()")
        } else {
            format!("call {rt} @mml_main({{ptr, i64}} %mml_args)")
        };
        if f.ret.bits() == 64 {
            out.push_str(&format!("    %r = {call}\n    ret i64 %r\n"));
        } else {
            let ext = if f.ret.signed() { "sext" } else { "zext" };
            out.push_str(&format!(
                "    %r = {call}\n    %w = {ext} {rt} %r to i64\n    ret i64 %w\n"
            ));
        }
        out.push_str("}\n\n");
    }
    out
}
