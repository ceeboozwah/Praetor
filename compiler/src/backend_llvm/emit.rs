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
use std::collections::HashMap;


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
fn emit_place(pl: &Place, out: &mut String, tmp: &mut u32) -> String {
    let mut addr = match &pl.root {
        // Both roots are `ptr` SSA names; the slot-vs-value distinction
        // is semantic, not representational.
        PRoot::Slot(x) | PRoot::Val(x) => r(x),
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
fn emit_instr_dbg(i: &Instr, out: &mut String, tmp: &mut u32, dbg: Option<u32>) {
    let mut scratch = String::new();
    emit_instr(i, &mut scratch, tmp);
    for line in scratch.lines() {
        out.push_str(line);
        if let Some(l) = dbg {
            out.push_str(&format!(", !dbg !{l}"));
        }
        out.push('\n');
    }
}

fn emit_term_dbg(t: &Term, out: &mut String, dbg: Option<u32>) {
    let mut scratch = String::new();
    emit_term(t, &mut scratch);
    for line in scratch.lines() {
        out.push_str(line);
        if let Some(l) = dbg {
            out.push_str(&format!(", !dbg !{l}"));
        }
        out.push('\n');
    }
}

fn emit_instr(i: &Instr, out: &mut String, tmp: &mut u32) {
    match i {
        // The arg value's LLVM name is the parameter itself — no instr.
        Instr::Arg { .. } => {}
        Instr::Local { dst, ty, .. } => {
            out.push_str(&format!("    {} = alloca {}\n", r(dst), llty(ty)));
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
            } else {
                // Materialize the immediate as a value: `add ty 0, n`.
                out.push_str(&format!("    {} = add {} 0, {}\n", r(dst), lt, val));
            }
        }
        Instr::Load { dst, place } => {
            let addr = emit_place(place, out, tmp);
            out.push_str(&format!(
                "    {} = load {}, ptr {}\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
        }
        Instr::MakeHandle { dst, place } | Instr::Addr { dst, place } => {
            let addr = emit_place(place, out, tmp);
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
            let addr = emit_place(place, out, tmp);
            out.push_str(&format!(
                "    store {} {}, ptr {}\n",
                llty(&place.ty),
                o(val),
                addr
            ));
        }
        // Real atomic instructions — seq_cst until ordering gets its
        // own round; the ops are honest even under v1 sequential runs.
        Instr::AtomicLoad { dst, place } => {
            let addr = emit_place(place, out, tmp);
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    {} = load atomic {}, ptr {} seq_cst, align {al}\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
        }
        Instr::AtomicStore { place, val } => {
            let addr = emit_place(place, out, tmp);
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    store atomic {} {}, ptr {} seq_cst, align {al}\n",
                llty(&place.ty),
                o(val),
                addr
            ));
        }
        Instr::AtomicAdd { dst, place, val } => {
            let addr = emit_place(place, out, tmp);
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
            let addr = emit_place(place, out, tmp);
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
                o(agg),
                idx
            ));
            let _ = ty;
        }
        Instr::Call { dst, ret, f, args } => {
            let as_: Vec<String> = args
                .iter()
                .map(|(t, v)| format!("{} {}", llty(t), o(v)))
                .collect();
            let callee = llsym(f);
            match dst {
                Some(d) => out.push_str(&format!(
                    "    {} = call {} @{}({})\n",
                    r(d),
                    llty(ret),
                    callee,
                    as_.join(", ")
                )),
                None => out.push_str(&format!(
                    "    call {} @{}({})\n",
                    llty(ret),
                    callee,
                    as_.join(", ")
                )),
            }
        }
    }
}

fn emit_term(t: &Term, out: &mut String) {
    match t {
        Term::Ret(Some(v), ty) => {
            out.push_str(&format!("    ret {} {}\n", llty(ty), o(v)));
        }
        Term::Ret(None, _) => out.push_str("    ret void\n"),
        Term::Br(c, t, e) => {
            out.push_str(&format!("    br i1 {}, label %{}, label %{}\n", o(c), t, e));
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

    for (fi, f) in m.fns.iter().enumerate() {
        let mut tmp = 0u32;
        let na = noalias_flags(f, &m.structs);
        let params: Vec<String> = f
            .params
            .iter()
            .zip(f.arg_regs.iter())
            .zip(na.iter())
            .map(|(((_, a, t), r), na)| {
                let lt = match a {
                    Some(Auth::Read) | Some(Auth::Write) => "ptr".to_string(),
                    _ => llty(t),
                };
                let attr = if *na { " noalias" } else { "" };
                format!("{lt}{attr} {}", self::r(r))
            })
            .collect();
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
            llty(&f.ret),
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
                    emit_instr_dbg(
                        &f.blocks[lbi].instrs[lii],
                        &mut out,
                        &mut tmp,
                        locs.get(&(fi, lbi, lii as i64)).copied(),
                    );
                }
            }
            for (ii, i) in b.instrs.iter().enumerate() {
                if let Instr::Local { dst, .. } = i {
                    // The alloca was hoisted to entry; this position
                    // is where the *variable* is born — dbg.declare
                    // anchors lldb's lifetime to the source line.
                    if let Some(&(v, dl)) = var_decls.get(&(fi, bi, ii)) {
                        out.push_str(&format!(
                            "    call void @llvm.dbg.declare(metadata ptr {}, metadata !{v}, metadata !DIExpression()), !dbg !{dl}\n",
                            r(dst)
                        ));
                    }
                    continue;
                }
                if let Instr::Arg { dst, .. } = i {
                    if let Some(&(v, dl)) = arg_vals.get(&(fi, bi, ii)) {
                        out.push_str(&format!(
                            "    call void @llvm.dbg.value(metadata ptr {}, metadata !{v}, metadata !DIExpression(DW_OP_deref)), !dbg !{dl}\n",
                            r(dst)
                        ));
                    }
                }
                emit_instr_dbg(i, &mut out, &mut tmp, locs.get(&(fi, bi, ii as i64)).copied());
            }
            emit_term_dbg(&b.term, &mut out, locs.get(&(fi, bi, -1)).copied());
        }
        out.push_str("}\n\n");
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
