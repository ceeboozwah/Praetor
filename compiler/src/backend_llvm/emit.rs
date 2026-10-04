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

type Structs<'a> = HashMap<&'a str, &'a Vec<(String, Ty)>>;

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
fn emit_place(pl: &Place, structs: &Structs, out: &mut String, tmp: &mut u32) -> String {
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
            PathElem::Field { idx, .. } => {
                let fty = match &cur {
                    Ty::Struct(n) => structs[n.as_str()][*idx as usize].1.clone(),
                    // A slice aggregate is exactly `{ptr, i64}` —
                    // field 0 is the data pointer, 1 the length.
                    Ty::Slice { elem, mutable } => {
                        if *idx == 0 {
                            Ty::Ptr {
                                mutable: *mutable,
                                elem: elem.clone(),
                            }
                        } else {
                            Ty::U64
                        }
                    }
                    // `str` shares the {ptr, i64} layout — field 0 is
                    // the `*const u8` data pointer, 1 the byte length.
                    Ty::Str => {
                        if *idx == 0 {
                            Ty::Ptr {
                                mutable: false,
                                elem: Box::new(Ty::U8),
                            }
                        } else {
                            Ty::U64
                        }
                    }
                    // `box slice` — owning {ptr, len}: field 0 is the
                    // writable `*T`, 1 the element count.
                    Ty::BoxSlice(elem) => {
                        if *idx == 0 {
                            Ty::Ptr {
                                mutable: true,
                                elem: elem.clone(),
                            }
                        } else {
                            Ty::U64
                        }
                    }
                    _ => unreachable!("field path on non-aggregate"),
                };
                (format!("i32 {idx}"), fty, false)
            }
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
            out.push_str(&format!(
                "    {g} = getelementptr {}, ptr {addr}, i32 0, {sel}\n",
                llty(&cur)
            ));
        }
        addr = g;
        cur = next;
    }
    addr
}

fn emit_instr(i: &Instr, structs: &Structs, out: &mut String, tmp: &mut u32) {
    match i {
        // The arg value's LLVM name is the parameter itself — no instr.
        Instr::Arg { .. } => {}
        Instr::Local { dst, ty, .. } => {
            out.push_str(&format!("    {} = alloca {}\n", r(dst), llty(ty)));
        }
        Instr::Const { dst, ty, val } => {
            if llty(ty) == "ptr" {
                // A null pointer constant (`channel()`'s empty cell).
                out.push_str(&format!("    {} = inttoptr i64 {} to ptr\n", r(dst), val));
            } else {
                // Materialize the immediate as a value: `add ty 0, n`.
                out.push_str(&format!("    {} = add {} 0, {}\n", r(dst), llty(ty), val));
            }
        }
        Instr::Load { dst, place } => {
            let addr = emit_place(place, structs, out, tmp);
            out.push_str(&format!(
                "    {} = load {}, ptr {}\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
        }
        Instr::MakeHandle { dst, place } | Instr::Addr { dst, place } => {
            let addr = emit_place(place, structs, out, tmp);
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
            let addr = emit_place(place, structs, out, tmp);
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
            let addr = emit_place(place, structs, out, tmp);
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    {} = load atomic {}, ptr {} seq_cst, align {al}\n",
                r(dst),
                llty(&place.ty),
                addr
            ));
        }
        Instr::AtomicStore { place, val } => {
            let addr = emit_place(place, structs, out, tmp);
            let al = (place.ty.bits() / 8).max(1);
            out.push_str(&format!(
                "    store atomic {} {}, ptr {} seq_cst, align {al}\n",
                llty(&place.ty),
                o(val),
                addr
            ));
        }
        Instr::AtomicAdd { dst, place, val } => {
            let addr = emit_place(place, structs, out, tmp);
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
            let addr = emit_place(place, structs, out, tmp);
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

/// Runtime boundary (N13): signature table for `runtime/mmlrt.c`.
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
        // N15 — dynamic slice/array bounds enforcement.
        "mml_bounds_check" => ("void", &["i64", "i64"]),
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

pub fn emit(m: &Module, triple: &str) -> String {
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
            out.push_str(&format!("declare {} @{}({})\n", ret, llsym(n), args.join(", ")));
        }
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
    let structs: Structs = m
        .structs
        .iter()
        .map(|s| (s.name.as_str(), &s.fields))
        .collect();
    for st in &m.structs {
        let fs: Vec<String> = st.fields.iter().map(|(_, t)| llty(t)).collect();
        out.push_str(&format!("%{} = type {{ {} }}\n", llsym(&st.name), fs.join(", ")));
    }
    if !m.structs.is_empty() {
        out.push('\n');
    }
    for f in &m.fns {
        let mut tmp = 0u32;
        let params: Vec<String> = f
            .params
            .iter()
            .zip(f.arg_regs.iter())
            .map(|((_, a, t), r)| {
                let lt = match a {
                    Some(Auth::Read) | Some(Auth::Write) => "ptr".to_string(),
                    _ => llty(t),
                };
                format!("{lt} {}", self::r(r))
            })
            .collect();
        // N26A — the C runtime owns `main` (it builds the argv view);
        // the user's entry emits as `mml_main` and `mml_main_args`
        // below forwards to it with the `slice str` it declared.
        let sym = if f.name == "main" { "mml_main" } else { &f.name };
        out.push_str(&format!("define {} @{}({}) {{\n", llty(&f.ret), llsym(sym), params.join(", ")));
        // Frame slots are allocated ONCE at entry — an `alloca` inside
        // a loop body would grow the stack every iteration (threads'
        // stacks are small; N13's while-loop `let g = lock(m)` proved
        // it). Hoist every `local` to the top of the entry block.
        let mut locals: Vec<&Instr> = Vec::new();
        for b in &f.blocks {
            for i in &b.instrs {
                if let Instr::Local { .. } = i {
                    locals.push(i);
                }
            }
        }
        for (bi, b) in f.blocks.iter().enumerate() {
            out.push_str(&format!("{}:\n", b.label));
            if bi == 0 {
                for i in &locals {
                    emit_instr(i, &structs, &mut out, &mut tmp);
                }
            }
            for i in &b.instrs {
                if let Instr::Local { .. } = i {
                    continue;
                }
                emit_instr(i, &structs, &mut out, &mut tmp);
            }
            emit_term(&b.term, &mut out);
        }
        out.push_str("}\n\n");
    }
    // N26A — `mmlrt.c`'s `main` always calls `mml_main_args` with the
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
