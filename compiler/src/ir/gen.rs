//! AST → IR. Locals are `local` slots (memory: `load`/`store`), so
//! mutation and loops need no phi nodes — LLVM's mem2reg owns that
//! transform. Expression temporaries are SSA regs.
//!
//! Aggregates make *places* first-class: `p`, `p.next`, `a[i]` are a
//! root slot plus a path of field/index hops (`Place`). Loads, stores,
//! and later the semantic layers all speak in places; only the LLVM
//! backend knows about GEP.
//!
//! Block discipline: a statement sequence runs in the current block
//! until a terminator seals it; statements after `return` land in a
//! fresh dead block terminated by `dead` (`unreachable`).

use super::ir::*;
use crate::frontend::ast::{self, Auth, BinOp, Expr, MatchArm, Stmt, Ty, UnOp};
use crate::semantics::check::ArmBind;
use crate::Span;
use std::collections::{HashMap, HashSet};

/// N46 — a bound fact a dominating condition proves: a comparison
/// `lo < hi` (strict) or `lo <= hi`, where each operand is a
/// canonical `(key, off)` — `s<slot>` + `.field`/`.*` path suffixes,
/// or `#` for a constant (`Int(k)` norms to `#`+k). The operands are
/// kept verbatim, NOT folded into a linear inequality: arithmetic
/// wraps, so `i + 1 < s.len` only proves `s[i + 1]` in range — it
/// says nothing about `s[i]` at `i = u64::MAX` (where `i + 1` wraps
/// to 0 and the cond still holds).
struct BFact {
    lo: (String, i64),
    hi: (String, i64),
    strict: bool,
}

/// A `spawn` body packaged for synthesis into its own IR function
/// (N13: real OS threads — the body can no longer be inlined).
/// `caps` lists the body's free variables resolved in the SPAWNING
/// env: (name, declared ty, ptr-class?) — ptr-class bindings (refs,
/// handles, channels, mutexes, tasks — "slot holds a pointer") ship
/// their slot's CONTENT; storage bindings ship the slot's ADDRESS and
/// bind inside the task as aliases (the same shape `write` params get).
pub struct TaskSpec<'a> {
    name: String,
    body: &'a [Stmt],
    ret: Ty,
    caps: Vec<(String, Ty, bool)>,
    /// N57 — the `spawn` expression's span: the task fn's DWARF line.
    span: Span,
}

/// Generate the IR module for one program. `imports` carries the
/// qualified signatures/layouts of every module this program imports
/// (N17) — their fns' bodies live in their own `gen` call; only their
/// signatures are needed here so `m.f` args marshal correctly. The
/// `strings` table is shared across modules so `StrAddr` indices stay
/// consistent when the products merge.
pub fn gen<'a>(
    p: &'a ast::Program,
    imports: &crate::semantics::check::ModuleInterface,
    strings: &'a std::cell::RefCell<Vec<Vec<u8>>>,
    binds: &'a std::collections::HashMap<Span, std::collections::HashMap<String, ArmBind>>,
    file: &str,
) -> Module {
    // N19 — param'd decls are templates: only instantiated clones
    // (tparams empty) reach codegen.
    let mut fns: HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)> = p
        .fns
        .iter()
        .filter(|f| f.tparams.is_empty())
        .map(|f| {
            (
                f.name.clone(),
                (
                    f.params.iter().map(|(_, a, t)| (*a, t.clone())).collect(),
                    f.ret.clone(),
                    f.is_extern,
                ),
            )
        })
        .collect();
    fns.extend(imports.fns.iter().map(|(n, s)| (n.clone(), s.clone())));
    // The module's own declarations shadow builtins inside its call
    // dispatch — `extern fn close` means libc close, not the channel
    // builtin. Same rule the checker enforces via `local_fns` (N21B).
    let local: HashSet<String> = p.fns.iter().map(|f| f.name.clone()).collect();
    let mut structs: HashMap<String, Vec<(String, Ty)>> = p
        .structs
        .iter()
        .filter(|s| s.tparams.is_empty())
        .map(|s| (s.name.clone(), s.fields.clone()))
        .collect();
    structs.extend(imports.structs.iter().map(|(n, s)| (n.clone(), s.clone())));
    let mut enums: HashMap<String, Vec<(String, Option<Ty>)>> = p
        .enums
        .iter()
        .filter(|e| e.tparams.is_empty())
        .map(|e| (e.name.clone(), e.variants.clone()))
        .collect();
    enums.extend(imports.enums.iter().map(|(n, v)| (n.clone(), v.clone())));
    // N22 — each enum registers its machine layout as a synthetic
    // struct: { i64 tag, [N x i8] payload } where N is the widest
    // payload. Field GEPs and LLVM type derivation stay
    // struct-shaped; `m.Some` resolves through `enums` for the
    // payload's real type before this pseudo-layout answers offsets.
    let mut enum_defs: Vec<StructDef> = Vec::new();
    // `enums` is a HashMap — iterate in name order so the synthetic
    // `{tag, payload}` layout decls print deterministically.
    let mut en_names: Vec<_> = enums.keys().collect();
    en_names.sort();
    for en in en_names {
        let vs = &enums[en];
        let n = vs
            .iter()
            .filter_map(|(_, t)| t.as_ref())
            .map(|t| size_of_fields(t, &structs, &enums))
            .max()
            .unwrap_or(0);
        let fields = vec![
            ("tag".to_string(), Ty::I64),
            (
                "payload".to_string(),
                Ty::Array(Box::new(Ty::U8), n.max(0) as u32),
            ),
        ];
        enum_defs.push(StructDef {
            name: en.clone(),
            fields: fields.clone(),
        });
        structs.insert(en.clone(), fields);
    }
    let mut out: Vec<Func> = Vec::new();
    let mut externs: Vec<ExternDecl> = Vec::new();
    let mut pending: Vec<TaskSpec<'a>> = Vec::new();
    for f in &p.fns {
        if !f.tparams.is_empty() {
            continue;
        }
        if f.is_extern {
            externs.push(ExternDecl {
                name: f.name.clone(),
                params: f.params.iter().map(|(_, _, t)| t.clone()).collect(),
                ret: f.ret.clone(),
            });
            continue;
        }
        let mut g = FnGen::new(f, &fns, &local, &structs, &enums, strings, binds, file);
        out.push(g.run(f));
        pending.append(&mut g.tasks);
    }
    // Synthesized task fns may themselves spawn — drain to fixpoint.
    while let Some(spec) = pending.pop() {
        let mut g = FnGen::for_task(&fns, &local, &structs, &enums, strings, binds, file);
        out.push(g.run_task(&spec));
        pending.append(&mut g.tasks);
    }
    Module {
        structs: {
            let mut v: Vec<StructDef> = p
                .structs
                .iter()
                .filter(|s| s.tparams.is_empty())
                .map(|s| StructDef {
                    name: s.name.clone(),
                    fields: s.fields.clone(),
                })
                .collect();
            v.extend(enum_defs);
            v
        },
        fns: out,
        externs,
        strings: Vec::new(),
    }
}

/// Standalone size-of for layout computation at `gen()` time â
/// mirrors `FnGen::size_of`/`align_of`; `Struct` names resolve through
/// either aggregate table (enums carry their `{tag, payload}` layout
/// via `enums` before registration).
fn size_of_fields(
    t: &Ty,
    structs: &HashMap<String, Vec<(String, Ty)>>,
    enums: &HashMap<String, Vec<(String, Option<Ty>)>>,
) -> i64 {
    match t {
        Ty::Bool | Ty::U8 | Ty::I8 => 1,
        Ty::U16 | Ty::I16 => 2,
        Ty::U32 | Ty::I32 => 4,
        Ty::Atomic(e) => size_of_fields(e, structs, enums),
        Ty::Slice { .. } | Ty::BoxSlice(_) | Ty::Str => 16,
        Ty::Array(e, n) => {
            let s = size_of_fields(e, structs, enums);
            let a = align_of_fields(e, structs, enums);
            (s + a - 1) / a * a * (*n as i64)
        }
        Ty::Struct(n) => {
            if let Some(fs) = structs.get(n) {
                let mut off = 0i64;
                let mut ma = 1i64;
                for (_, ft) in fs {
                    let a = align_of_fields(ft, structs, enums);
                    ma = ma.max(a);
                    off = (off + a - 1) / a * a + size_of_fields(ft, structs, enums);
                }
                (off + ma - 1) / ma * ma
            } else if let Some(vs) = enums.get(n) {
                // { i64 tag, [N x i8] payload } â always 8 + max.
                let max = vs
                    .iter()
                    .filter_map(|(_, t)| t.as_ref())
                    .map(|t| size_of_fields(t, structs, enums))
                    .max()
                    .unwrap_or(0);
                8 + max
            } else {
                8
            }
        }
        _ => 8, // scalars, pointers, refs, handles, boxes, fn carriers
    }
}

fn align_of_fields(
    t: &Ty,
    structs: &HashMap<String, Vec<(String, Ty)>>,
    enums: &HashMap<String, Vec<(String, Option<Ty>)>>,
) -> i64 {
    match t {
        Ty::Bool | Ty::U8 | Ty::I8 => 1,
        Ty::U16 | Ty::I16 => 2,
        Ty::U32 | Ty::I32 => 4,
        Ty::Atomic(e) => align_of_fields(e, structs, enums),
        Ty::Array(e, _) => align_of_fields(e, structs, enums),
        Ty::Struct(n) => {
            if let Some(fs) = structs.get(n) {
                fs.iter()
                    .map(|(_, ft)| align_of_fields(ft, structs, enums))
                    .max()
                    .unwrap_or(1)
            } else {
                8
            }
        }
        _ => 8,
    }
}

struct FnGen<'a, 'b> {
    fns: &'b HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
    /// This module's own fn names — they shadow builtin call names.
    local_fns: &'b HashSet<String>,
    structs: &'b HashMap<String, Vec<(String, Ty)>>,
    /// Enum name -> variants (N22): a variant name resolves through
    /// this table to its payload type before pseudo-layout lookup.
    enums: &'b HashMap<String, Vec<(String, Option<Ty>)>>,
    ret: Ty,
    blocks: Vec<Block>,
    label: Option<String>,
    instrs: Vec<Instr>,
    terminated: bool,
    next_reg: u32,
    next_lbl: u32,
    /// name -> (root reg, type, is_alias). Capability params and
    /// storage captures bind as aliases: the reg IS the place's root.
    env: Vec<HashMap<String, (Reg, Ty, bool)>>,
    /// Bindings `promote`d — their names percolate out of dying
    /// scopes (the checker decided how far; gen just keeps them
    /// reachable so approved uses find their slots).
    promoted: HashSet<String>,
    /// Task bodies this fn queued for synthesis (`__task_<n>`).
    tasks: Vec<TaskSpec<'a>>,
    /// Inside a synthesized task fn: (end label, result ptr, result
    /// ty). `return v` stores THROUGH the result ptr (the caller's
    /// cell — it outlives the thread) and jumps to the fn's end.
    task_res: Option<(String, Reg, Ty)>,
    /// Task fn name sequence (per parent fn).
    tseq: u32,
    /// Parent fn name — task fns are `__task_<parent>_<n>`.
    fname: String,
    /// Type overlay for `spawn_ret_ty`: a task body's own `let`
    /// bindings aren't in the parent env, so ret-type inference looks
    /// here first. Only `spawn_ret_ty` populates it.
    ovl: std::cell::RefCell<HashMap<String, Ty>>,
    /// Live guard slots — a guard's scope exit emits `mml_mutex_unlock`
    /// (guards are scope-bound authority; dropping one IS releasing the
    /// lock). `unlock g` and `let g2 = g` moves remove the slot.
    open_guards: HashSet<Reg>,
    /// N25D — loop stack: (break label, continue label, env depth at
    /// loop entry). A `break`/`continue` releases every guard bound
    /// deeper than that depth, then jumps.
    /// (after_lbl, body_lbl, cond, env depth) — `continue` re-emits
    /// the test so each edge into it sees its own live fact state.
    loops: Vec<(String, String, &'a Expr, usize)>,
    /// Module-level string-literal table (N14) — `Expr::Str` pushes
    /// bytes and emits `StrAddr` with the global's index.
    strings: &'b std::cell::RefCell<Vec<Vec<u8>>>,
    /// Checker's per-arm payload bind modes, keyed by match span —
    /// a `&`/`&mut` arm gets the payload's ADDRESS in its slot.
    binds: &'b std::collections::HashMap<Span, std::collections::HashMap<String, ArmBind>>,
    /// N46 — active dominating-condition bounds facts, in program
    /// order. `if`/`while`/`&&` push, their block's end truncates,
    /// and any store to a fact's place kills it.
    bfacts: Vec<BFact>,
    /// N47 — active value equalities `(k,off) ≡ (k',off')`: copies
    /// (`let m = n`, `m = n`), `==` conds, `alloc_slice` len
    /// provenance. Same scope/kill discipline as `bfacts`.
    efacts: Vec<((String, i64), (String, i64))>,
    /// N54 — deferred calls, one list per `env` scope (pushed/popped
    /// in lockstep). Each exit edge emits its own copy of the live
    /// defers, LIFO — the lists themselves only die at scope pops.
    defers: Vec<Vec<&'a Expr>>,
    /// N57 — the span of the statement/expression currently being
    /// lowered; `emit` tags every instruction with it and `seal`
    /// tags the terminator. The last-written span wins — inner
    /// exprs give inner instrs their own lines.
    cur_span: Option<Span>,
    /// Parallel to `instrs` — the span each emitted instruction got.
    isps: Vec<Option<Span>>,
    /// This module's source file — stamped onto every `Func` for
    /// DWARF's `DIFile` table.
    file: String,
}

impl<'a, 'b> FnGen<'a, 'b> {
    fn new(
        f: &ast::FnDef,
        fns: &'b HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
        local_fns: &'b HashSet<String>,
        structs: &'b HashMap<String, Vec<(String, Ty)>>,
        enums: &'b HashMap<String, Vec<(String, Option<Ty>)>>,
        strings: &'b std::cell::RefCell<Vec<Vec<u8>>>,
        binds: &'b std::collections::HashMap<Span, std::collections::HashMap<String, ArmBind>>,
        file: &str,
    ) -> Self {
        FnGen {
            fns,
            local_fns,
            structs,
            enums,
            strings,
            binds,
            cur_span: None,
            isps: Vec::new(),
            file: file.to_string(),
            ret: f.ret.clone(),
            blocks: Vec::new(),
            label: None,
            instrs: Vec::new(),
            terminated: false,
            next_reg: 0,
            next_lbl: 0,
            env: vec![HashMap::new()],
            promoted: HashSet::new(),
            tasks: Vec::new(),
            task_res: None,
            tseq: 0,
            fname: f.name.clone(),
            ovl: std::cell::RefCell::new(HashMap::new()),
            open_guards: HashSet::new(),
            loops: Vec::new(),
            bfacts: Vec::new(),
            efacts: Vec::new(),
            defers: vec![Vec::new()],
        }
    }

    /// A blank generator for a synthesized task body — same fn/struct
    /// tables, empty env (captures land in `run_task`'s prologue).
    fn for_task(
        fns: &'b HashMap<String, (Vec<(Option<Auth>, Ty)>, Ty, bool)>,
        local_fns: &'b HashSet<String>,
        structs: &'b HashMap<String, Vec<(String, Ty)>>,
        enums: &'b HashMap<String, Vec<(String, Option<Ty>)>>,
        strings: &'b std::cell::RefCell<Vec<Vec<u8>>>,
        binds: &'b std::collections::HashMap<Span, std::collections::HashMap<String, ArmBind>>,
        file: &str,
    ) -> Self {
        FnGen {
            fns,
            local_fns,
            structs,
            enums,
            strings,
            binds,
            cur_span: None,
            isps: Vec::new(),
            file: file.to_string(),
            ret: Ty::Void,
            blocks: Vec::new(),
            label: None,
            instrs: Vec::new(),
            terminated: false,
            next_reg: 0,
            next_lbl: 0,
            env: vec![HashMap::new()],
            promoted: HashSet::new(),
            tasks: Vec::new(),
            task_res: None,
            tseq: 0,
            fname: String::new(),
            ovl: std::cell::RefCell::new(HashMap::new()),
            open_guards: HashSet::new(),
            loops: Vec::new(),
            bfacts: Vec::new(),
            efacts: Vec::new(),
            defers: vec![Vec::new()],
        }
    }

    /// The ptr flavour every opaque pointer rides on.
    fn ptr_ty() -> Ty {
        Ty::Ref {
            mutable: false,
            elem: Box::new(Ty::Void),
        }
    }

    /// `fn __task_<f>_<n>(env: ptr) -> ptr` — the runtime calls this on
    /// the spawned thread. Prologue: unpack env slots into bindings;
    /// body: the spawn block; `return v` stores through env's result
    /// pointer. Returns null (pthread contract).
    fn run_task(&mut self, spec: &TaskSpec<'a>) -> Func {
        self.label = Some("entry".to_string());
        let env_arg = self.fresh();
        self.emit(Instr::Arg {
            dst: env_arg,
            ty: Self::ptr_ty(),
            idx: 0,
        });
        let k = spec.caps.len();
        let arr_ty = Ty::Array(Box::new(Self::ptr_ty()), (k + 2) as u32);
        self.env
            .last_mut()
            .unwrap()
            .insert("$env".to_string(), (env_arg, arr_ty.clone(), true));
        // env[1] is the caller's result cell's address — `return`
        // stores through it. Void tasks leave it null.
        let rp = if spec.ret != Ty::Void {
            match self.load_place(Place {
                root: PRoot::Val(env_arg),
                root_ty: arr_ty.clone(),
                path: vec![PathElem::Index {
                    val: Opnd::Imm(1),
                    ty: Ty::I32,
                }],
                ty: Self::ptr_ty(),
            }) {
                Opnd::R(r) => r,
                _ => Reg(0),
            }
        } else {
            Reg(0)
        };
        let end = self.new_label("tend");
        self.task_res = Some((end.clone(), rp, spec.ret.clone()));
        for (i, (n, ty, pclass)) in spec.caps.iter().enumerate() {
            let cv = match self.load_place(Place {
                root: PRoot::Val(env_arg),
                root_ty: arr_ty.clone(),
                path: vec![PathElem::Index {
                    val: Opnd::Imm((i + 2) as i64),
                    ty: Ty::I32,
                }],
                ty: Self::ptr_ty(),
            }) {
                Opnd::R(r) => r,
                _ => continue,
            };
            if *pclass {
                // Ptr-class capture: the slot holds the pointer value
                // itself — rebuild an ordinary local slot.
                let slot = self.fresh();
                self.emit(Instr::Local {
                    dst: slot,
                    ty: ty.clone(),
                    name: n.clone(),
                });
                self.emit(Instr::Store {
                    place: Place {
                        root: PRoot::Slot(slot),
                        root_ty: ty.clone(),
                        path: Vec::new(),
                        ty: ty.clone(),
                    },
                    val: Opnd::R(cv),
                });
                self.env
                    .last_mut()
                    .unwrap()
                    .insert(n.clone(), (slot, ty.clone(), false));
            } else {
                // Storage capture: the env slot carries the variable's
                // ADDRESS — the task aliases the caller's storage.
                self.env
                    .last_mut()
                    .unwrap()
                    .insert(n.clone(), (cv, ty.clone(), true));
            }
        }
        self.stmts(spec.body);
        if !self.terminated || self.label.is_some() {
            // N54 — the body's top-scope defers replay on the
            // fall-through edge (return edges already carried theirs).
            self.emit_all_defers();
            self.seal(Term::Jmp(end.clone()));
        }
        self.open(end);
        // Guards still open at fn end release here — a task that
        // `lock`s without `unlock` relies on scope-exit semantics.
        let open: Vec<Reg> = self.open_guards.iter().copied().collect();
        for slot in open {
            self.unlock_slot(slot);
        }
        self.open_guards.clear();
        let zp = self.const_of(&Self::ptr_ty(), 0);
        self.seal(Term::Ret(Some(zp), Self::ptr_ty()));
        Func {
            name: spec.name.clone(),
            span: spec.span,
            file: self.file.clone(),
            params: vec![("$env".to_string(), None, Self::ptr_ty())],
            arg_regs: vec![env_arg],
            ret: Self::ptr_ty(),
            blocks: std::mem::take(&mut self.blocks),
        }
    }

    fn run(&mut self, f: &'a ast::FnDef) -> Func {
        self.label = Some("entry".to_string());
        // Params arrive as SSA values, then land in slots like locals:
        // `arg` marks the incoming value; `local`+`store` give it memory.
        let params = f.params.clone();
        let mut arg_regs = Vec::new();
        for (i, (n, a, t)) in params.iter().enumerate() {
            let incoming = self.fresh();
            arg_regs.push(incoming);
            if matches!(a, Some(Auth::Read) | Some(Auth::Write)) {
                // Place alias: the incoming `ptr` reg is the storage;
                // Var resolves straight to it — no slot, no copy.
                self.emit(Instr::Arg {
                    dst: incoming,
                    ty: Ty::Ref {
                        mutable: *a == Some(Auth::Write),
                        elem: Box::new(t.clone()),
                    },
                    idx: i as u32,
                });
                self.env
                    .last_mut()
                    .unwrap()
                    .insert(n.clone(), (incoming, t.clone(), true));
                continue;
            }
            self.emit(Instr::Arg {
                dst: incoming,
                ty: t.clone(),
                idx: i as u32,
            });
            let slot = self.fresh();
            self.emit(Instr::Local {
                dst: slot,
                ty: t.clone(),
                name: n.clone(),
            });
            self.emit(Instr::Store {
                place: Place {
                    root: PRoot::Slot(slot),
                    root_ty: t.clone(),
                    path: Vec::new(),
                    ty: t.clone(),
                },
                val: Opnd::R(incoming),
            });
            self.env
                .last_mut()
                .unwrap()
                .insert(n.clone(), (slot, t.clone(), false));
        }
        self.stmts(&f.body);
        // Implicit fallthrough return (checker doesn't require exhaustive
        // returns in v1 — a typed zero is emitted). Open guards release
        // first — fn end is their scope's death.
        if !self.terminated || self.label.is_some() {
            // N54 — top-scope defers replay on the fall-through edge.
            self.emit_all_defers();
            let open: Vec<Reg> = self.open_guards.iter().copied().collect();
            for slot in open {
                self.unlock_slot(slot);
            }
            self.open_guards.clear();
        }
        if !self.terminated || self.label.is_some() {
            let ty = self.ret.clone();
            let v = if ty == Ty::Void {
                None
            } else {
                Some(self.const_of(&ty, 0))
            };
            self.seal(Term::Ret(v, ty));
        }
        Func {
            name: f.name.clone(),
            span: f.span,
            file: self.file.clone(),
            params,
            arg_regs,
            ret: f.ret.clone(),
            blocks: std::mem::take(&mut self.blocks),
        }
    }

    fn fresh(&mut self) -> Reg {
        let r = Reg(self.next_reg);
        self.next_reg += 1;
        r
    }
    fn new_label(&mut self, hint: &str) -> String {
        let l = format!("{hint}{}", self.next_lbl);
        self.next_lbl += 1;
        l
    }
    fn emit(&mut self, i: Instr) {
        if self.label.is_none() {
            // Unreachable post-terminator code — a real block so every
            // instr lives somewhere printable.
            self.label = Some(self.new_label("dead"));
        }
        self.isps.push(self.cur_span);
        self.instrs.push(i);
    }
    fn seal(&mut self, t: Term) {
        let label = self.label.take().unwrap_or_else(|| self.new_label("dead"));
        self.blocks.push(Block {
            label,
            instrs: std::mem::take(&mut self.instrs),
            isps: std::mem::take(&mut self.isps),
            tspan: self.cur_span,
            term: t,
        });
        self.terminated = true;
    }
    fn open(&mut self, label: String) {
        self.label = Some(label);
        self.terminated = false;
    }

    /// Unlock a guard slot — the scope-bound release.
    fn unlock_slot(&mut self, slot: Reg) {
        let v = self.load_place(Place {
            root: PRoot::Slot(slot),
            root_ty: Ty::Guard(Box::new(Ty::Void)),
            path: Vec::new(),
            ty: Ty::Guard(Box::new(Ty::Void)),
        });
        self.rt_call("mml_mutex_unlock", Ty::Void, vec![(Self::ptr_ty(), v)]);
    }

    /// N54 — emit every deferred call registered in env scopes deeper
    /// than `depth`, innermost scope / latest registration first.
    /// The lists stay: each exit edge carries its own copy, and the
    /// scope's `pop` owns the removal. Deferred calls run before the
    /// dying scope's guards release — the defer is the younger claim.
    fn emit_defers_above(&mut self, depth: usize) {
        for level in (depth..self.defers.len()).rev() {
            let recs = self.defers[level].clone();
            for e in recs.iter().rev() {
                let _ = self.expr(*e, None);
            }
        }
    }

    /// N54 — the whole defer stack — `return`/`?` unwind everything.
    fn emit_all_defers(&mut self) {
        self.emit_defers_above(0);
    }

    /// Scope exit: deferred calls replay first (operand names must
    /// still resolve), then promoted bindings move one scope outward
    /// (the checker owns how far — it re-promotes or rejects further
    /// use). Live guards bound here release their lock — scope exit
    /// IS the unlock (`let g = lock(m)` may omit `unlock` precisely
    /// because the guard cannot outlive its scope).
    fn pop_scope(&mut self) {
        // A dead tail emitted nothing on its exit edge yet still pops:
        // replay only the live fall-through.
        let dl = self.defers.pop().unwrap_or_default();
        if !self.terminated {
            for e in dl.iter().rev() {
                let _ = self.expr(*e, None);
            }
        }
        let popped = self.env.pop().unwrap_or_default();
        for (_, (slot, ty, _)) in &popped {
            if matches!(ty, Ty::Guard(_)) && self.open_guards.remove(slot) {
                self.unlock_slot(*slot);
            }
        }
        if let Some(outer) = self.env.last_mut() {
            for (n, b) in popped {
                if self.promoted.contains(&n) {
                    outer.insert(n, b);
                }
            }
        }
    }

    fn lookup(&self, name: &str) -> Option<(Reg, Ty, bool)> {
        self.env.iter().rev().find_map(|s| s.get(name).cloned())
    }

    /// A variable's own place — `place(Var)` minus the Expr. Captures
    /// are always named bindings; guards can't cross a spawn boundary,
    /// so no Guard resolution lives here.
    fn var_place(&self, n: &str) -> Option<Place> {
        let (root, ty, alias) = self.lookup(n)?;
        Some(Place {
            root: if alias {
                PRoot::Val(root)
            } else {
                PRoot::Slot(root)
            },
            root_ty: ty.clone(),
            path: Vec::new(),
            ty,
        })
    }

    /// The field index of `name` within struct `sname`, if any.
    fn field_idx(&self, sname: &str, name: &str) -> Option<u32> {
        self.structs
            .get(sname)
            .and_then(|fs| fs.iter().position(|(fn_, _)| fn_ == name))
            .map(|i| i as u32)
    }

    /// N46 — canonical key for a place expr: the slot reg plus
    /// `.field`/`.*` path hops. `None` for non-places (calls, index
    /// exprs, literals) — those can't anchor a bound fact.
    fn bkey(&self, e: &Expr) -> Option<String> {
        match e {
            Expr::Var(n, _) => self.lookup(n).map(|(r, _, _)| format!("s{}", r.0)),
            Expr::Field(b, name, _) => self.bkey(b).map(|k| format!("{k}.{name}")),
            Expr::Deref(b, _) => self.bkey(b).map(|k| format!("{k}.*")),
            _ => None,
        }
    }

    /// N46 — normalize an int expr to `(key, off)`, meaning
    /// `value = key + off`: literals are `# + k`, a place is `key + 0`,
    /// `x + k`/`x - k` folds the constant. `None` = not affine in a
    /// single place (multiplies, two-place sums, calls, index reads).
    fn bnorm(&self, e: &Expr) -> Option<(String, i64)> {
        match e {
            Expr::Int(k, _) => Some(("#".to_string(), *k)),
            Expr::Bin(BinOp::Add, a, b, _) => {
                match (self.bnorm(a)?, self.bnorm(b)?) {
                    ((k, o), (c, off)) | ((c, off), (k, o)) if c == "#" => {
                        Some((k, o + off))
                    }
                    _ => None,
                }
            }
            Expr::Bin(BinOp::Sub, a, b, _) => match (self.bnorm(a)?, self.bnorm(b)?) {
                ((k, o), (c, off)) if c == "#" => Some((k, o - off)),
                _ => None,
            },
            // N47 — int casts: a value-preserving cast (same type, or
            // a widening that keeps every value — same signedness or
            // unsigned→strictly-wider signed) keeps the inner key, so
            // `u64(k)` on a `k: u32` still proves `k`-facts. Any other
            // cast is bit-exact but domain-changing: `u64(n)` on a
            // signed `n` gets an OPAQUE key — `b.len ≡ u64(n)` holds
            // verbatim (the alloc count IS that value) and connects to
            // a `u64(n)` comparison, but never collapses to `n`.
            Expr::Call(n, _, args, _)
                if crate::semantics::check::int_cast_ty(n).is_some() =>
            {
                let a = args.first()?;
                let inner = self.bnorm(a)?;
                let from = self.infer(a, None);
                let to = crate::semantics::check::int_cast_ty(n).unwrap();
                if !from.is_int() {
                    return None;
                }
                let exact = from == to
                    || (from.signed() == to.signed() && from.bits() <= to.bits())
                    || (!from.signed() && to.signed() && from.bits() < to.bits());
                if exact {
                    Some(inner)
                } else {
                    Some((format!("cast:{n}:{}:{}", inner.0, inner.1), 0))
                }
            }
            _ => self.bkey(e).map(|k| (k, 0)),
        }
    }

    /// N48 — provenance norm for `alloc_slice` counts: the count is
    /// stored through the slice's i64 len field, so `len` equals
    /// `u64(count_as_i64)`. A u64 count is exact; `i64(x)` where x is
    /// unsigned is the bit-preserving roundtrip — collapse to x's key
    /// (`u64(i64(x)) == x` always). Any other signed count gets the
    /// opaque `cast:u64` key — exact, just comparable only to other
    /// `u64(...)` exprs. Negative literal counts don't reach codegen
    /// meaningfully, so `None` there is safe.
    fn bnorm_lenkey(&self, e: &Expr) -> Option<(String, i64)> {
        let inner = self.bnorm(e)?;
        if !self.infer(e, None).signed() {
            return Some(inner);
        }
        if inner.0 == "#" {
            return if inner.1 >= 0 { Some(inner) } else { None };
        }
        if let Expr::Call(n, _, args, _) = e {
            if n == "i64" {
                if let Some(a) = args.first() {
                    if !self.infer(a, None).signed() {
                        return self.bnorm(a);
                    }
                }
            }
        }
        Some((format!("cast:u64:{}:{}", inner.0, inner.1), 0))
    }

    /// N47 — every `(key, off)` provably equal to `x` under the
    /// active equality facts (reflexive transitive closure). Offsets
    /// are part of the tuple — `(m,0) ≡ (n,1)` is only usable at
    /// exactly those offsets.
    fn reps(&self, x: &(String, i64)) -> Vec<(String, i64)> {
        let mut out = vec![x.clone()];
        let mut i = 0;
        while i < out.len() {
            let cur = out[i].clone();
            i += 1;
            for f in &self.efacts {
                let nxt = if f.0 == cur {
                    Some(f.1.clone())
                } else if f.1 == cur {
                    Some(f.0.clone())
                } else {
                    None
                };
                if let Some(n) = nxt {
                    if !out.contains(&n) {
                        out.push(n);
                    }
                }
            }
        }
        out
    }

    /// N47 — `a` and `b` provably denote the same value.
    fn peq(&self, a: &(String, i64), b: &(String, i64)) -> bool {
        self.reps(a).contains(b)
    }

    /// N48 — `key > 0` proven: a positive const rep, or a `0 < key`
    /// bound fact (`x != 0` on unsigned x lowers to exactly that).
    fn pos_proven(&self, key: &str) -> bool {
        self.reps(&(key.to_string(), 0))
            .iter()
            .any(|r| r.0 == "#" && r.1 > 0)
            || self.lt_proven(&("#".to_string(), 0), &(key.to_string(), 0))
    }

    /// N48 — facts `a & m` establishes about the RESULT value:
    /// `a & m <= m` always holds when `m` can't be negative (the AND
    /// only clears bits — unsigned `m` always qualifies; signed `m`
    /// needs a nonneg rep or a `cap-1` rep with `cap > 0`). When `m ≡
    /// cap - 1` and `cap > 0`, the stronger `a & m < cap` holds —
    /// `x & (n-1) < n` for every positive `n`, no power-of-two
    /// requirement (pow2 is about hash uniformity, not bounds). The
    /// result is also nonneg when `m` is (the top bit clears), which
    /// signed index checks need.
    fn and_facts(&mut self, dst: &(String, i64), maskn: &(String, i64), mask: &Expr) {
        let mreps = self.reps(maskn);
        let pos_mask = mreps
            .iter()
            .filter(|r| r.1 == -1 && self.pos_proven(&r.0))
            .cloned()
            .collect::<Vec<_>>();
        let nonneg = !self.infer(mask, None).signed()
            || mreps.iter().any(|r| r.0 == "#" && r.1 >= 0)
            || !pos_mask.is_empty();
        if !nonneg {
            return;
        }
        self.bfacts.push(BFact {
            lo: ("#".to_string(), 0),
            hi: dst.clone(),
            strict: false,
        });
        self.bfacts.push(BFact {
            lo: dst.clone(),
            hi: maskn.clone(),
            strict: false,
        });
        for (pk, _) in pos_mask {
            self.bfacts.push(BFact {
                lo: dst.clone(),
                hi: (pk, 0),
                strict: true,
            });
        }
    }

    /// N46 — push the facts a dominating cond proves. Only `&&`
    /// conjuncts of `<`/`<=`/`>`/`>=` count: both sides of `&&` hold
    /// on the dominated path; `||` alternatives can't be assumed
    /// together, and `==`/`!=` don't bound a range. N47: `==` is an
    /// exact equality. N48: `!x` means x's NEGATED facts hold.
    fn cond_facts(&mut self, cond: &Expr) {
        if let Expr::Unary(UnOp::Not, x, _) = cond {
            self.neg_facts(x);
            return;
        }
        if let Expr::Bin(BinOp::LAnd, a, b, _) = cond {
            self.cond_facts(a);
            self.cond_facts(b);
            return;
        }
        if let Expr::Bin(op, a, b, _) = cond {
            let (Some(l), Some(r)) = (self.bnorm(a), self.bnorm(b)) else {
                return;
            };
            // N47 — `l == r` is an exact value equality. N48 —
            // `x != 0` on an unsigned x gives `0 < x` (other `!=`
            // shapes prove nothing usable).
            if *op == BinOp::Eq {
                self.efacts.push((l, r));
                return;
            }
            if *op == BinOp::Ne {
                self.neg_ne_zero(a, b);
                return;
            }
            let (lo, hi, strict) = match op {
                BinOp::Lt => (l, r, true),
                BinOp::Le => (l, r, false),
                BinOp::Gt => (r, l, true),
                BinOp::Ge => (r, l, false),
                _ => return,
            };
            self.bfacts.push(BFact { lo, hi, strict });
        }
    }

    /// N48 — push the facts that hold when `cond` is FALSE (the else
    /// arm, or the continuation past `if c { diverge }`). Comparisons
    /// flip; `!(a || b)` gives BOTH negated conjuncts (the index
    /// guard `if i < 0 || i >= s.len { return }` → `0 <= i < s.len`);
    /// `!(a == 0)` on an unsigned `a` gives `a > 0`; `!(a != b)` is
    /// `a == b`. `!(a && b)` is disjunctive — nothing.
    fn neg_facts(&mut self, cond: &Expr) {
        match cond {
            Expr::Unary(UnOp::Not, x, _) => self.cond_facts(x),
            Expr::Bin(BinOp::LOr, a, b, _) => {
                self.neg_facts(a);
                self.neg_facts(b);
            }
            Expr::Bin(op, a, b, _) => {
                let (Some(l), Some(r)) = (self.bnorm(a), self.bnorm(b)) else {
                    return;
                };
                match op {
                    BinOp::Ne => self.efacts.push((l, r)),
                    // `!(a == 0)` on unsigned a ⇒ `a > 0`.
                    BinOp::Eq => self.neg_ne_zero(a, b),
                    _ => {
                        let (lo, hi, strict) = match op {
                            BinOp::Lt => (r, l, false),
                            BinOp::Le => (r, l, true),
                            BinOp::Gt => (l, r, false),
                            BinOp::Ge => (l, r, true),
                            _ => return,
                        };
                        self.bfacts.push(BFact { lo, hi, strict });
                    }
                }
            }
            _ => {}
        }
    }

    /// N48 — `a != b` (i.e. `!(a == b)`): `x != 0` on an unsigned x
    /// gives `0 < x`. Signed x could be negative — nothing.
    fn neg_ne_zero(&mut self, a: &Expr, b: &Expr) {
        for (x, c) in [(a, b), (b, a)] {
            if let (Some(xn), Some(cn)) = (self.bnorm(x), self.bnorm(c)) {
                if cn == ("#".to_string(), 0)
                    && xn.0 != "#"
                    && !self.infer(x, None).signed()
                {
                    self.bfacts.push(BFact {
                        lo: ("#".to_string(), 0),
                        hi: xn,
                        strict: true,
                    });
                    return;
                }
            }
        }
    }

    /// N46 — `want <= wrap(key + off)` proven by a const-lo fact:
    /// `c < key+off` means `c + 1 <= wrap(...)`, `c <= key+off` means
    /// `c <= wrap(...)` — the fact's lo const is exact (no wrap), so
    /// `want <= c + strict` suffices. `i >= 0` proving `0 <= i` and
    /// `6 <= s.len` proving `5 <= len` both flow through here. N47:
    /// either side may reach the fact through an equality (`n ≡ len`,
    /// `m ≡ 6`).
    fn flo_proves(&self, want: i64, key: &str, off: i64) -> bool {
        let hr = self.reps(&(key.to_string(), off));
        self.bfacts.iter().any(|f| {
            hr.contains(&f.hi)
                && self
                    .reps(&f.lo)
                    .iter()
                    .any(|fl| fl.0 == "#" && fl.1.saturating_add(f.strict as i64) >= want)
        })
    }

    /// N46 — `wrap(a) <= wrap(b)` proven: equal values are trivially
    /// `<=`; a fact comparing exactly these exprs works for either
    /// strictness (`i < j` implies `i <= j` on the same wrapped
    /// values); a strict fact whose lo operand is `a`'s expr shifted
    /// by exactly one (`i < len`) proves `a + 1 <= b` — the off-by-one
    /// on the LEFT side is safe because `wrap(i) < len` guarantees `i`
    /// can't be at the wrap boundary. A const `a` chains through a
    /// const-lo fact (`flo_proves`). N47: `reps` substitutes
    /// provably-equal operands on either side.
    fn le_proven(&self, a: &(String, i64), b: &(String, i64), b_unsigned: bool) -> bool {
        if self.peq(a, b) {
            return true;
        }
        let ar = self.reps(a);
        if let Some(ca) = ar.iter().find(|r| r.0 == "#") {
            if let Some(cb) = self.reps(b).iter().find(|r| r.0 == "#") {
                if ca.1 <= cb.1 {
                    return true;
                }
            }
            if b.0 != "#" {
                if ca.1 <= 0 && b_unsigned {
                    return true;
                }
                if self.flo_proves(ca.1, &b.0, b.1) {
                    return true;
                }
            }
        }
        self.bfacts.iter().any(|f| {
            self.reps(&f.hi).contains(b)
                && (ar.contains(&f.lo)
                    || (f.strict
                        && ar
                            .iter()
                            .any(|r| r.0 == f.lo.0 && f.lo.1.checked_add(1) == Some(r.1))))
        })
    }

    /// N46 — `wrap(a) < wrap(b)` proven: only an identical strict
    /// comparison (`idx < len` for `s[idx]`), or a const `a` covered
    /// by a const-lo bound (`s.len > 5` proving `s[5]`). N47: both
    /// sides substitute through equalities — `i < n` with `n ≡ len`
    /// bounds `s[i]`.
    fn lt_proven(&self, a: &(String, i64), b: &(String, i64)) -> bool {
        let ar = self.reps(a);
        let br = self.reps(b);
        self.bfacts.iter().any(|f| {
            br.contains(&f.hi)
                && ((f.strict && ar.contains(&f.lo))
                    || match (
                        ar.iter().find(|r| r.0 == "#"),
                        self.reps(&f.lo).iter().find(|r| r.0 == "#"),
                    ) {
                        (Some(ca), Some(fl)) => {
                            fl.1.saturating_add(f.strict as i64)
                                >= ca.1.saturating_add(1)
                        }
                        _ => false,
                    })
        })
    }

    /// N46 — a store through `e` kills facts anchored on its place:
    /// `x = ..` kills `x` and `x.*`, `x.f = ..` kills `x.f.*`, and a
    /// write through a `*p` deref could hit anything — clear all.
    /// `x[i] = v` normalizes to no key: element stores can't move a
    /// `len` field, so no fact dies. N47: same discipline on
    /// equalities — a write to either side breaks the pair.
    fn bkill(&mut self, e: &Expr) {
        if expr_has_deref(e) {
            for f in self.bfacts.iter_mut() {
                f.lo = ("#dead".to_string(), 0);
                f.hi = ("#dead".to_string(), 0);
            }
            for f in self.efacts.iter_mut() {
                *f = (
                    ("#dead".to_string(), 0),
                    ("#dead".to_string(), 0),
                );
            }
            return;
        }
        // `&e`/`&mut e` stores into e's referent, not e itself.
        let e = match e {
            Expr::AddrOf { place, .. } => &**place,
            _ => e,
        };
        if let Some(k) = self.bkey(e) {
            let p = format!("{k}.");
            let dead = |x: &(String, i64)| x.0 == k || x.0.starts_with(&p);
            // PLEX-0003 — tombstone in place, never remove: branch
            // marks are `vec.len()` snapshots, and a physical remove
            // lets a later push slide below the mark and leak into a
            // sibling path. "#dead" matches no real key, so dead
            // entries are inert under every proof scan.
            let dk = ("#dead".to_string(), 0);
            for f in self.bfacts.iter_mut() {
                if dead(&f.lo) || dead(&f.hi) {
                    f.lo = dk.clone();
                    f.hi = dk.clone();
                }
            }
            for f in self.efacts.iter_mut() {
                if dead(&f.0) || dead(&f.1) {
                    *f = (dk.clone(), dk.clone());
                }
            }
        }
    }

    /// N46 — `idx` provably inside `base`'s len under the active
    /// facts: `wrap(idx) < len` via `lt_proven`, plus `0 <= idx` for
    /// signed index types (unsigned can't underflow by construction).
    /// N48 — an inline `a & m` index is proven by the mask rule when
    /// `m < len` holds (`m ≡ len-1` with `len > 0`, or a direct fact).
    fn idx_proven(&self, idx: &Expr, base: &Expr, it: &Ty) -> bool {
        let Some(bk) = self.bkey(base) else {
            return false;
        };
        let lk = (format!("{bk}.len"), 0);
        let Some((ik, io)) = self.bnorm(idx) else {
            return match idx {
                Expr::Bin(BinOp::BitAnd, _, m, _) => self.and_idx_proven(m, &lk),
                _ => false,
            };
        };
        if ik == "#" && io < 0 {
            return false;
        }
        if !self.lt_proven(&(ik.clone(), io), &lk) {
            return false;
        }
        let signed = matches!(it, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64);
        ik == "#" || !signed || self.flo_proves(0, &ik, io)
    }

    /// N48 — `base[a & m]` proven: `a & m <= m < len` needs `m`
    /// nonneg (its top bit must be clear for the AND to stay bounded)
    /// and `m < len` proven — a direct bound fact, or `m ≡ p - 1`
    /// with `p ≡ len` and `p > 0` (the mask idiom).
    fn and_idx_proven(&self, m: &Expr, lk: &(String, i64)) -> bool {
        let Some(mn) = self.bnorm(m) else {
            return false;
        };
        let mreps = self.reps(&mn);
        let lreps = self.reps(lk);
        let pos_mask = mreps
            .iter()
            .filter(|r| r.1 == -1 && self.pos_proven(&r.0))
            .cloned()
            .collect::<Vec<_>>();
        let nonneg = !self.infer(m, None).signed()
            || mreps.iter().any(|r| r.0 == "#" && r.1 >= 0)
            || !pos_mask.is_empty();
        if !nonneg {
            return false;
        }
        self.lt_proven(&mn, lk)
            || pos_mask.iter().any(|(pk, _)| lreps.contains(&(pk.clone(), 0)))
    }

    /// N46 — `&b[lo..hi]` provably in range: `0 <= lo <= hi <= len`.
    /// An absent bound defaults to `0`/`len` — free at both ends.
    fn range_proven(&mut self, lo: Option<&'a Expr>, hi: Option<&'a Expr>, base: &'a Expr) -> bool {
        let Some(bk) = self.bkey(base) else {
            return false;
        };
        let lk = (format!("{bk}.len"), 0);
        let signed = |e: &Expr| {
            matches!(self.infer(e, None), Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64)
        };
        // hi <= len — an absent hi IS len.
        let hn = match hi {
            Some(h) => match self.bnorm(h) {
                Some(x) => x,
                None => return false,
            },
            None => lk.clone(),
        };
        if hi.is_some() && !self.le_proven(&hn, &lk, true) {
            return false;
        }
        let (lok, loff) = match lo {
            Some(e) => match self.bnorm(e) {
                Some(x) => x,
                None => return false,
            },
            None => ("#".to_string(), 0),
        };
        // 0 <= lo — consts and unsigned types are free.
        let lo_signed = lo.is_some_and(|e| signed(e));
        let lo_ok = (lok == "#" && loff >= 0)
            || (lok != "#" && (!lo_signed || self.flo_proves(0, &lok, loff)));
        if !lo_ok {
            return false;
        }
        // lo <= hi.
        let hi_unsigned = hi.map_or(true, |h| !signed(h));
        self.le_proven(&(lok, loff), &hn, hi_unsigned)
    }

    /// Resolve a place expression (`x`, `p.f`, `a[i]`, `p.a[i].f`) to a
    /// root slot + path. The checker has already rejected non-places.
    fn place(&mut self, e: &'a Expr) -> Option<Place> {
        match e {
            Expr::Var(n, _) => {
                let (root, ty, alias) = self.lookup(n)?;
                if let Ty::Guard(inner) = &ty {
                    // A guard's slot holds the runtime mutex OBJECT —
                    // `g.f`/`g[i]` reach the payload through
                    // `mml_mutex_data`.
                    let v = self.load_place(Place {
                        root: if alias {
                            PRoot::Val(root)
                        } else {
                            PRoot::Slot(root)
                        },
                        root_ty: ty.clone(),
                        path: Vec::new(),
                        ty: ty.clone(),
                    });
                    let base = match v {
                        Opnd::R(r) => r,
                        _ => return None,
                    };
                    let data = self.fresh();
                    self.emit(Instr::Call {
                        dst: Some(data),
                        ret: Self::ptr_ty(),
                        f: "mml_mutex_data".to_string(),
                        args: vec![(Self::ptr_ty(), Opnd::R(base))],
                    });
                    return Some(Place {
                        root: PRoot::Val(data),
                        root_ty: (**inner).clone(),
                        path: Vec::new(),
                        ty: (**inner).clone(),
                    });
                }
                Some(Place {
                    root: if alias {
                        PRoot::Val(root)
                    } else {
                        PRoot::Slot(root)
                    },
                    root_ty: ty.clone(),
                    path: Vec::new(),
                    ty,
                })
            }
            Expr::Field(base, name, _) => {
                let mut pl = self.place(base)?;
                // `s.ptr`/`s.len` — pseudo-fields of the {ptr, i64}
                // aggregate; both lower to ordinary field loads.
                if let Ty::Slice { elem, mutable } = &pl.ty {
                    let (idx, fty) = match name.as_str() {
                        "ptr" => (
                            0u32,
                            Ty::Ptr {
                                mutable: *mutable,
                                elem: elem.clone(),
                            },
                        ),
                        "len" => (1, Ty::U64),
                        _ => return None,
                    };
                    pl.ty = fty;
                    pl.path.push(PathElem::Field {
                        name: name.clone(),
                        idx,
                        ty: pl.ty.clone(),
                    });
                    return Some(pl);
                }
                // `str` shares the {ptr, i64} layout: `.len` is field
                // 1; `.bytes` is the whole aggregate re-read as a
                // `slice u8` — zero instructions, just a type change.
                if pl.ty == Ty::Str {
                    return match name.as_str() {
                        "len" => {
                            pl.ty = Ty::U64;
                            pl.path.push(PathElem::Field {
                                name: name.clone(),
                                idx: 1,
                                ty: Ty::U64,
                            });
                            Some(pl)
                        }
                        "bytes" => {
                            pl.ty = Ty::Slice {
                                mutable: false,
                                elem: Box::new(Ty::U8),
                            };
                            Some(pl)
                        }
                        _ => None, // `.ptr` / unknown — checker rejected
                    };
                }
                // `box slice` — same {ptr, i64} shape: `.ptr` = field
                // 0 (`*T`), `.len` = field 1.
                if let Ty::BoxSlice(elem) = &pl.ty {
                    return match name.as_str() {
                        "ptr" => {
                            pl.ty = Ty::Ptr {
                                mutable: true,
                                elem: elem.clone(),
                            };
                            pl.path.push(PathElem::Field {
                                name: name.clone(),
                                idx: 0,
                                ty: pl.ty.clone(),
                            });
                            Some(pl)
                        }
                        "len" => {
                            pl.ty = Ty::U64;
                            pl.path.push(PathElem::Field {
                                name: name.clone(),
                                idx: 1,
                                ty: Ty::U64,
                            });
                            Some(pl)
                        }
                        _ => None,
                    };
                }
                // N22 â an enum's variant is its payload area: field
                // 1 of {tag, payload}, leaf-typed by the variant.
                if let Ty::Struct(n) = &pl.ty {
                    if let Some(vars) = self.enums.get(n) {
                        if let Some((_, Some(pty))) =
                            vars.iter().find(|(vn, _)| vn == name)
                        {
                            pl.ty = pty.clone();
                            pl.path.push(PathElem::Field {
                                name: name.clone(),
                                idx: 1,
                                ty: pty.clone(),
                            });
                            return Some(pl);
                        }
                    }
                }
                let sname = match &pl.ty {
                    Ty::Struct(n) => n.clone(),
                    Ty::Guard(inner) | Ty::Mutex(inner) => match &**inner {
                        Ty::Struct(n) => n.clone(),
                        _ => return None,
                    },
                    _ => return None, // checker rejected
                };
                let idx = self.field_idx(&sname, name)?;
                pl.ty = self.structs.get(&sname)?[idx as usize].1.clone();
                pl.path.push(PathElem::Field {
                    name: name.clone(),
                    idx,
                    ty: pl.ty.clone(),
                });
                Some(pl)
            }
            Expr::Index(base, idx, _) => {
                let mut pl = self.place(base)?;
                let selem = match &pl.ty {
                    Ty::Slice { elem, mutable } => Some((elem.clone(), *mutable)),
                    // A `box slice` is an OWNED span — `.ptr` is `*T`.
                    Ty::BoxSlice(elem) => Some((elem.clone(), true)),
                    _ => None,
                };
                if let Some((elem, mutable)) = selem {
                    // `s[i]`/`p[i]`: load the aggregate's data ptr +
                    // len, runtime bounds-check, then index the pointer.
                    let fld = |idx2: u32, name: &str, ty: Ty| Place {
                        root: pl.root,
                        root_ty: pl.root_ty.clone(),
                        path: {
                            let mut p = pl.path.clone();
                            p.push(PathElem::Field {
                                name: name.to_string(),
                                idx: idx2,
                                ty: ty.clone(),
                            });
                            p
                        },
                        ty,
                    };
                    let dp = match self.load_place(fld(
                        0,
                        "ptr",
                        Ty::Ptr {
                            mutable,
                            elem: elem.clone(),
                        },
                    )) {
                        Opnd::R(r) => r,
                        _ => return None,
                    };
                    let sl = self.load_place(fld(1, "len", Ty::U64));
                    let it = self.infer(idx, Some(&Ty::I64));
                    let iv = match &**idx {
                        Expr::Int(k, _) => Opnd::Imm(*k),
                        _ => {
                            let iv0 = self.expr(idx, Some(&it));
                            self.i64v(iv0, &it)
                        }
                    };
                    // N46 — a dominating cond that already bounded the
                    // index makes the runtime check redundant.
                    if !self.idx_proven(idx, base, &it) {
                        // N50 — inline the fast path: `idx <u len` is a
                        // fused compare+branch (and a signed-negative
                        // index reads as unsigned-huge, so `ult` covers
                        // it too). The trap is a cold noreturn call —
                        // an extern call here would block vectorization
                        // and hoisting of everything around it.
                        let ok_lbl = self.new_label("bcok");
                        let bad_lbl = self.new_label("bcbad");
                        let c = self.fresh();
                        self.emit(Instr::Bin {
                            dst: c,
                            op: BinOp::Lt,
                            ty: Ty::U64,
                            a: iv.clone(),
                            b: sl.clone(),
                        });
                        self.seal(Term::Br(
                            Opnd::R(c),
                            ok_lbl.clone(),
                            bad_lbl.clone(),
                        ));
                        self.open(bad_lbl);
                        self.rt_call(
                            "mml_bounds_fail",
                            Ty::Void,
                            vec![(Ty::I64, iv.clone()), (Ty::I64, sl)],
                        );
                        self.seal(Term::Dead);
                        self.open(ok_lbl);
                    }
                    return Some(Place {
                        root: PRoot::Val(dp),
                        root_ty: Ty::Ptr {
                            mutable,
                            elem: elem.clone(),
                        },
                        path: vec![PathElem::Index {
                            val: iv,
                            ty: Ty::I64,
                        }],
                        ty: (*elem).clone(),
                    });
                }
                let elem = match &pl.ty {
                    Ty::Array(e, _) => (**e).clone(),
                    Ty::Guard(inner) | Ty::Mutex(inner) => match &**inner {
                        Ty::Array(e, _) => (**e).clone(),
                        _ => return None,
                    },
                    _ => return None,
                };
                let it = self.infer(idx, Some(&Ty::I64));
                let iv = self.expr(idx, Some(&it));
                pl.ty = elem;
                pl.path.push(PathElem::Index { val: iv, ty: it });
                Some(pl)
            }
            Expr::Deref(inner, _) => self.place_ptr(inner),
            _ => None,
        }
    }

    /// The type `e` will produce, with `want` as the literal-coercion hint.
    fn infer(&self, e: &'a Expr, want: Option<&Ty>) -> Ty {
        match e {
            Expr::Int(..) => match want {
                Some(t) if t.is_int() => t.clone(),
                _ => Ty::INT_DEFAULT,
            },
            Expr::Bool(..) => Ty::Bool,
            Expr::Sizeof(..) => Ty::U64,
            Expr::EnumLit { ty, .. } => (**ty).clone(),
            Expr::Try { inner, .. } => {
                // `e?` yields the `Ok` payload of e's Result (N23).
                match self.infer(inner, None) {
                    Ty::Struct(n) => self
                        .enums
                        .get(&n)
                        .and_then(|vs| {
                            vs.iter()
                                .find(|(vn, _)| vn == "Ok")
                                .and_then(|(_, t)| t.clone())
                        })
                        .unwrap_or(Ty::Void),
                    _ => Ty::Void,
                }
            }
            Expr::MethodCall { .. } | Expr::AssocFn { .. } => Ty::Void,
            Expr::Var(n, _) => self.lookup_ty(n).unwrap_or(Ty::Void),
            Expr::Unary(UnOp::Not, ..) => Ty::Bool,
            Expr::Unary(UnOp::Neg | UnOp::BitNot, i, _) => self.infer(i, want),
            Expr::Bin(op, a, b, _) => {
                if op.is_cmp() || matches!(op, BinOp::LAnd | BinOp::LOr) {
                    Ty::Bool
                } else {
                    let ta = self.infer(a, None);
                    if matches!(&**a, Expr::Int(..)) && ta == Ty::INT_DEFAULT {
                        // Literal side adopts the other side's type.
                        self.infer(b, want)
                    } else {
                        self.infer(a, want)
                    }
                }
            }
            Expr::Call(n, _tg, args, _) if crate::semantics::check::is_atomic_op(n) && !self.local_fns.contains(n) => {
                match n.as_str() {
                    "compare_exchange" => Ty::Bool,
                    "atomic_load" => {
                        let t = self.place_shallow(&args[0]).unwrap_or(Ty::Void);
                        // Ref/handle args name the referent's storage.
                        let inner = match &t {
                            Ty::Ref { elem, .. } | Ty::Handle(elem) => (**elem).clone(),
                            _ => t,
                        };
                        match inner {
                            Ty::Atomic(e) => *e,
                            other => other,
                        }
                    }
                    _ => Ty::Void,
                }
            }
            Expr::Call(n, _tg, args, _) if n == "lock" && !self.local_fns.contains(n) => {
                let t = self
                    .place_shallow(&args[0])
                    .unwrap_or_else(|| self.infer(&args[0], None));
                let inner = match t {
                    Ty::Mutex(i) => *i,
                    Ty::Ref { elem, .. } | Ty::Handle(elem) => match *elem {
                        Ty::Mutex(i) => *i,
                        _ => return Ty::Void,
                    },
                    _ => return Ty::Void,
                };
                Ty::Guard(Box::new(inner))
            }
            Expr::Call(n, _tg, args, _) if n == "channel" && !self.local_fns.contains(n) => Ty::Channel(Box::new(Ty::Void)),
            Expr::Call(n, ..) if n == "system_allocator" && !self.local_fns.contains(n) => Ty::Alloc,
            Expr::Call(n, _tg, args, _) if n == "recv" && !self.local_fns.contains(n) => {
                // The recv's type is the endpoint's declared payload:
                // `channel &mut P` recv's bind as `&mut P` locals.
                let mut et = self.infer(&args[0], None);
                loop {
                    match et {
                        Ty::Channel(i) => return *i,
                        Ty::Handle(e) | Ty::Ref { elem: e, .. } => {
                            et = *e;
                        }
                        _ => return Ty::Void,
                    }
                }
            }
            Expr::Call(n, ..) if crate::semantics::check::is_chan_op(n) && !self.local_fns.contains(n) => Ty::Void,
            Expr::Call(n, _tg, args, _) if n == "slice" && !self.local_fns.contains(n) => {
                // `slice(p, n)` — mutability/elem ride on `p`'s type.
                match args.first().map(|a| self.infer(a, None)) {
                    Some(Ty::Ptr { mutable, elem }) => Ty::Slice { mutable, elem },
                    _ => Ty::Slice {
                        mutable: false,
                        elem: Box::new(Ty::Void),
                    },
                }
            }
            Expr::Call(n, ..) if n == "cstr" && !self.local_fns.contains(n) => Ty::Ptr {
                mutable: false,
                elem: Box::new(Ty::U8),
            },
            Expr::Call(n, ..) if n == "str_from_utf8" && !self.local_fns.contains(n) => Ty::Str,
            // `replace(place, v)` — the result is the place's type
            // (the OLD value moves out whole).
            Expr::Call(n, _tg, args, _)
                if n == "replace" && !self.local_fns.contains(n) =>
            {
                args.first()
                    .map(|a| self.infer(a, None))
                    .unwrap_or(Ty::Void)
            }
            Expr::Call(n, ..) if n == "swap" && !self.local_fns.contains(n) => Ty::Void,
            Expr::Call(n, ..)
                if crate::semantics::check::int_cast_ty(n).is_some()
                    && !self.local_fns.contains(n) =>
            {
                crate::semantics::check::int_cast_ty(n).unwrap()
            }
            Expr::Call(n, ..) => self
                .fns
                .get(n)
                .map(|(_, r, _)| r.clone())
                .unwrap_or(Ty::Void),
            // A literal's declared type — `*const u8` by default,
            // `str` where a `str` is wanted.
            Expr::Str(..) => match want {
                Some(Ty::Str) => Ty::Str,
                _ => Ty::Ptr {
                    mutable: false,
                    elem: Box::new(Ty::U8),
                },
            },
            Expr::Mutex(i, _) => {
                // `mutex h` (handle): the mutex guards the referent —
                // its type is the referent's, not the handle's.
                match self.infer(i, None) {
                    Ty::Handle(elem) => Ty::Mutex(elem),
                    t => Ty::Mutex(Box::new(t)),
                }
            }
            Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {
                self.place_shallow(e).unwrap_or(Ty::Void)
            }
            Expr::StructLit(n, ..) => Ty::Struct(n.clone()),
            Expr::AddrOf { mutable, place, .. } => match &**place {
                // `&a[lo..hi]` / `&s[lo..hi]` — a slice, not a ref.
                Expr::IndexRange { base, .. } => {
                    let elem = match self.place_shallow(base) {
                        Some(Ty::Array(e, _)) => *e,
                        Some(Ty::Slice { elem, .. }) => *elem,
                        Some(Ty::BoxSlice(elem)) => *elem,
                        _ => Ty::Void,
                    };
                    Ty::Slice {
                        mutable: *mutable,
                        elem: Box::new(elem),
                    }
                }
                _ => Ty::Ref {
                    mutable: *mutable,
                    elem: Box::new(self.place_shallow(place).unwrap_or(Ty::Void)),
                },
            },
            Expr::IndexRange { base, .. } => {
                // Bare `a[lo..hi]` outside `&` — the checker rejects
                // it; type it anyway so emission can continue.
                let elem = match self.place_shallow(base) {
                    Some(Ty::Array(e, _)) => *e,
                    Some(Ty::Slice { elem, .. }) => *elem,
                    Some(Ty::BoxSlice(elem)) => *elem,
                    _ => Ty::Void,
                };
                Ty::Slice {
                    mutable: false,
                    elem: Box::new(elem),
                }
            }
            Expr::Handle(place, _) => {
                Ty::Handle(Box::new(self.place_shallow(place).unwrap_or(Ty::Void)))
            }
            Expr::Alloc { ty, count, .. } => match count {
                None => Ty::Box(ty.clone()),
                Some(_) => Ty::BoxSlice(ty.clone()),
            },
            Expr::Spawn(body, _) => Ty::Task(Box::new(self.spawn_ret_ty(body))),
            Expr::Join(i, _) => match self.infer(i, None) {
                Ty::Task(t) => *t,
                _ => Ty::Void,
            },
            Expr::Match { scrut, arms, .. } => self.match_yield_ty(scrut, arms, want),
        }
    }

    /// The endpoint cell a channel op touches: `ch` is its own slot;
    /// `send(hc, ..)` through a handle loads the pointer first — the
    /// endpoint's cell IS the referent. The place's leaf type is `ptr`
    /// (the cell's content), not `channel T`.
    fn chan_cell(&mut self, e: &'a Expr) -> Option<Place> {
        let pty = Ty::Ref {
            mutable: false,
            elem: Box::new(Ty::Void),
        };
        match self.infer(e, None) {
            Ty::Handle(_) | Ty::Ref { .. } => {
                let pl = self.place(e)?;
                match self.load_place(pl) {
                    Opnd::R(r) => Some(Place {
                        root: PRoot::Val(r),
                        root_ty: pty.clone(),
                        path: Vec::new(),
                        ty: pty,
                    }),
                    _ => None,
                }
            }
            _ => {
                let mut pl = self.place(e)?;
                pl.ty = pty;
                Some(pl)
            }
        }
    }

    /// A `match` expression's result type (N28): `want` wins, then
    /// the first concrete arm yield — default-int yields adopt it,
    /// the checker's peer rule mirrored. Arm tails reference their
    /// payload binder, which env doesn't hold yet — it rides `ovl`
    /// for each infer, like `spawn_ret_ty` does for task locals.
    fn match_yield_ty(&self, scrut: &'a Expr, arms: &'a [MatchArm], want: Option<&Ty>) -> Ty {
        let ename = match self.infer(scrut, None) {
            Ty::Struct(n) => Some(n),
            _ => None,
        };
        let mut mty: Option<Ty> = want.cloned();
        for a in arms {
            if !a.expr_arm {
                continue;
            }
            let Some(Stmt::Expr { expr, .. }) = a.body.last() else {
                continue;
            };
            // The tail's scope: payload binder + every top-level `let`
            // in the arm body — env doesn't hold any of them yet, so
            // they ride `ovl` for this infer (the `srt_walk` fold).
            let mut locals: HashMap<String, Ty> = HashMap::new();
            if let (Some(b), Some(n)) = (&a.bind, &ename) {
                if let Some(pty) = self
                    .enums
                    .get(n)
                    .and_then(|vs| {
                        vs.iter()
                            .find(|(vn, _)| vn == &a.variant)
                            .and_then(|(_, t)| t.clone())
                    })
                {
                    locals.insert(b.clone(), pty);
                }
            }
            let saved_ovl = self.ovl.borrow().clone();
            for s in &a.body[..a.body.len() - 1] {
                if let Stmt::Let { name, ty, init, .. } = s {
                    let lt = match (ty, init) {
                        (Some(w), _) => w.clone(),
                        (None, Some(i)) => {
                            *self.ovl.borrow_mut() = locals.clone();
                            let t = self.infer(i, None);
                            t
                        }
                        _ => Ty::Void,
                    };
                    locals.insert(name.clone(), lt);
                }
            }
            let mut merged = saved_ovl.clone();
            merged.extend(locals);
            *self.ovl.borrow_mut() = merged;
            let t = self.infer(expr, want);
            *self.ovl.borrow_mut() = saved_ovl;
            mty = Some(match mty {
                None => t,
                Some(Ty::INT_DEFAULT) if t.is_int() => t,
                Some(m) => m,
            });
        }
        mty.unwrap_or(Ty::Void)
    }

    /// The overlay-aware name → type lookup: task-body `let`s while
    /// `spawn_ret_ty` walks, real env otherwise.
    fn lookup_ty(&self, n: &str) -> Option<Ty> {
        self.ovl
            .borrow()
            .get(n)
            .cloned()
            .or_else(|| self.lookup(n).map(|(_, t, _)| t))
    }

    /// A spawn body's result type — first `return e` wins (the checker
    /// already enforced uniformity). The body's own `let`s feed the
    /// overlay so `return <local>` resolves against the child's scope.
    fn spawn_ret_ty(&self, body: &'a [Stmt]) -> Ty {
        self.srt_walk(body, &mut HashMap::new())
    }

    fn srt_walk(&self, stmts: &'a [Stmt], locals: &mut HashMap<String, Ty>) -> Ty {
        for s in stmts {
            match s {
                Stmt::Return {
                    value: Some(e), ..
                } => {
                    *self.ovl.borrow_mut() = locals.clone();
                    let t = self.infer(e, None);
                    self.ovl.borrow_mut().clear();
                    return t;
                }
                Stmt::Let { name, ty, init, .. } => {
                    let t = match (ty, init) {
                        (Some(w), _) => w.clone(),
                        (None, Some(i)) => {
                            *self.ovl.borrow_mut() = locals.clone();
                            let t = self.infer(i, None);
                            self.ovl.borrow_mut().clear();
                            t
                        }
                        _ => Ty::Void,
                    };
                    locals.insert(name.clone(), t);
                }
                Stmt::If {
                    then_body,
                    else_body,
                    ..
                } => {
                    let t = self.srt_walk(then_body, &mut locals.clone());
                    if t != Ty::Void {
                        return t;
                    }
                    if let Some(eb) = else_body {
                        let t = self.srt_walk(eb, &mut locals.clone());
                        if t != Ty::Void {
                            return t;
                        }
                    }
                }
                Stmt::While { body, .. } | Stmt::Region { body, .. } => {
                    let t = self.srt_walk(body, &mut locals.clone());
                    if t != Ty::Void {
                        return t;
                    }
                }
                _ => {}
            }
        }
        Ty::Void
    }

    /// Type of a path expression without emitting index operands —
    /// used by `infer` (which must not mutate instr stream).
    fn place_shallow(&self, e: &'a Expr) -> Option<Ty> {
        match e {
            Expr::Var(n, _) => self.lookup_ty(n),
            Expr::Field(base, name, _) => {
                // `place_shallow` can't see through value-position
                // bases (`f().len`) — fall back to `infer` for the
                // base's TYPE; this arm only needs it to pick the
                // member type.
                let bt = self
                    .place_shallow(base)
                    .or_else(|| Some(self.infer(base, None)))?;
                if let Ty::Slice { elem, mutable } = &bt {
                    return match name.as_str() {
                        "ptr" => Some(Ty::Ptr {
                            mutable: *mutable,
                            elem: elem.clone(),
                        }),
                        "len" => Some(Ty::U64),
                        _ => None,
                    };
                }
                if bt == Ty::Str {
                    return match name.as_str() {
                        "len" => Some(Ty::U64),
                        "bytes" => Some(Ty::Slice {
                            mutable: false,
                            elem: Box::new(Ty::U8),
                        }),
                        _ => None,
                    };
                }
                if let Ty::BoxSlice(elem) = &bt {
                    // `box slice` members mirror slice — the owning
                    // `.ptr` is writable.
                    return match name.as_str() {
                        "len" => Some(Ty::U64),
                        "ptr" => Some(Ty::Ptr {
                            mutable: true,
                            elem: elem.clone(),
                        }),
                        _ => None,
                    };
                }
                let sname = match &bt {
                    Ty::Struct(sn) => {
                        // N22 â a variant name resolves to its payload
                        // type before the pseudo-layout fields.
                        if let Some(vs) = self.enums.get(sn) {
                            if let Some((_, pt)) =
                                vs.iter().find(|(vn, _)| vn == name)
                            {
                                return pt.clone();
                            }
                        }
                        sn.clone()
                    }
                    Ty::Guard(inner) | Ty::Mutex(inner) => match &**inner {
                        Ty::Struct(sn) => sn.clone(),
                        _ => return None,
                    },
                    _ => return None,
                };
                self.structs
                    .get(&sname)
                    .and_then(|fs| fs.iter().find(|(fn_, _)| fn_ == name))
                    .map(|(_, t)| t.clone())
            }
            Expr::Index(base, _, _) => match self
                .place_shallow(base)
                .or_else(|| Some(self.infer(base, None)))?
            {
                Ty::Array(elem, _) => Some(*elem),
                Ty::Slice { elem, .. } => Some(*elem),
                Ty::BoxSlice(elem) => Some(*elem),
                Ty::Guard(inner) | Ty::Mutex(inner) => match *inner {
                    Ty::Array(elem, _) => Some(*elem),
                    _ => None,
                },
                _ => None,
            },
            Expr::IndexRange { base, .. } => match self
                .place_shallow(base)
                .or_else(|| Some(self.infer(base, None)))?
            {
                Ty::Array(elem, _) => Some(*elem),
                Ty::Slice { elem, .. } => Some(*elem),
                Ty::BoxSlice(elem) => Some(*elem),
                _ => None,
            },
            Expr::Deref(inner, _) => match self
                .place_shallow(inner)
                .or_else(|| Some(self.infer(inner, None)))?
            {
                Ty::Ref { elem, .. } | Ty::Handle(elem) | Ty::Guard(elem) | Ty::Box(elem) => {
                    Some(*elem)
                }
                _ => None,
            },
            _ => None,
        }
    }

    fn const_of(&mut self, ty: &Ty, n: i64) -> Opnd {
        let dst = self.fresh();
        self.emit(Instr::Const {
            dst,
            ty: ty.clone(),
            val: n,
        });
        Opnd::R(dst)
    }

    /// A call into `runtime/mmlrt_*` — the concurrency boundary.
    fn rt_call(&mut self, f: &str, ret: Ty, args: Vec<(Ty, Opnd)>) -> Opnd {
        let dst = if ret == Ty::Void {
            None
        } else {
            Some(self.fresh())
        };
        self.emit(Instr::Call {
            dst,
            ret,
            f: f.to_string(),
            args,
        });
        dst.map(Opnd::R).unwrap_or(Opnd::Imm(0))
    }

    /// Force an operand to i64 (runtime-boundary width for bounds,
    /// ranges, lengths). Immediates pass through; narrower regs cast.
    fn i64v(&mut self, v: Opnd, t: &Ty) -> Opnd {
        if *t == Ty::I64 || matches!(v, Opnd::Imm(_)) {
            return v;
        }
        let dst = self.fresh();
        self.emit(Instr::Cast {
            dst,
            ty: Ty::I64,
            from: t.clone(),
            a: v,
        });
        Opnd::R(dst)
    }

    /// Materialize a `{ptr, len}` slice value (N15): fill an
    /// anonymous slot, then load the aggregate back out.
    fn make_slice(&mut self, ptr: Opnd, len: Opnd, elem: Ty, mutable: bool) -> Opnd {
        let sty = Ty::Slice {
            mutable,
            elem: Box::new(elem),
        };
        self.make_agg2(sty, ptr, len)
    }

    /// `str` — same {ptr, len} materialization (N15B).
    fn make_str(&mut self, ptr: Opnd, len: Opnd) -> Opnd {
        self.make_agg2(Ty::Str, ptr, len)
    }

    /// The (data pointer, length) halves of a {ptr, i64} value.
    fn str_parts(&mut self, v: Opnd) -> (Opnd, Opnd) {
        let p = self.fresh();
        self.emit(Instr::Extract {
            dst: p,
            ty: Self::ptr_ty(),
            agg: v.clone(),
            idx: 0,
        });
        let l = self.fresh();
        self.emit(Instr::Extract {
            dst: l,
            ty: Ty::U64,
            agg: v,
            idx: 1,
        });
        (Opnd::R(p), Opnd::R(l))
    }

    /// Fill an anonymous `{ptr, len}` slot and load it back out.
    fn make_agg2(&mut self, sty: Ty, ptr: Opnd, len: Opnd) -> Opnd {
        let slot = self.fresh();
        self.emit(Instr::Local {
            dst: slot,
            ty: sty.clone(),
            name: String::new(),
        });
        let fld = |name: &str, idx: u32, ty: Ty| Place {
            root: PRoot::Slot(slot),
            root_ty: sty.clone(),
            path: vec![PathElem::Field {
                name: name.to_string(),
                idx,
                ty: ty.clone(),
            }],
            ty,
        };
        self.emit(Instr::Store {
            place: fld("ptr", 0, Self::ptr_ty()),
            val: ptr,
        });
        self.emit(Instr::Store {
            place: fld("len", 1, Ty::U64),
            val: len,
        });
        self.load_place(Place {
            root: PRoot::Slot(slot),
            root_ty: sty.clone(),
            path: Vec::new(),
            ty: sty.clone(),
        })
    }

    /// `&b[lo..hi]` — emit the runtime range check (all-constant
    /// ranges skip it; the checker already proved those), compute the
    /// span's start pointer, and build the {ptr, len} aggregate.
    fn slice_range(
        &mut self,
        base: &'a Expr,
        lo: Option<&'a Expr>,
        hi: Option<&'a Expr>,
        mutable: bool,
    ) -> Opnd {
        let Some(bpl) = self.place(base) else {
            return Opnd::Imm(0);
        };
        let (elem, limit) = match &bpl.ty {
            Ty::Array(e, n) => ((**e).clone(), Opnd::Imm(*n as i64)),
            Ty::Slice { elem, .. } | Ty::BoxSlice(elem) => {
                let lp = Place {
                    root: bpl.root,
                    root_ty: bpl.root_ty.clone(),
                    path: {
                        let mut p = bpl.path.clone();
                        p.push(PathElem::Field {
                            name: "len".to_string(),
                            idx: 1,
                            ty: Ty::U64,
                        });
                        p
                    },
                    ty: Ty::U64,
                };
                ((**elem).clone(), self.load_place(lp))
            }
            _ => return Opnd::Imm(0),
        };
        // Literal bounds fold to immediates — an all-constant range
        // skips the runtime call entirely (the checker proved it).
        let mut bound = |e: Option<&'a Expr>, dflt: Opnd| -> Opnd {
            match e {
                Some(Expr::Int(k, _)) => Opnd::Imm(*k),
                Some(be) => {
                    let it = self.infer(be, Some(&Ty::I64));
                    let v = self.expr(be, Some(&it));
                    self.i64v(v, &it)
                }
                None => dflt,
            }
        };
        let lov = bound(lo, Opnd::Imm(0));
        let hiv = bound(hi, limit.clone());
        // N46 — a dominating cond proving `0 <= lo <= hi <= len`
        // makes the runtime check redundant.
        if !matches!(
            (&lov, &hiv, &limit),
            (Opnd::Imm(_), Opnd::Imm(_), Opnd::Imm(_))
        ) && !self.range_proven(lo, hi, base)
        {
            // N50-style — inline the fast path: `hi <u lo ||
            // hi >u len` is a pair of fused compare+branches (a
            // signed-negative lo reads as unsigned-huge, so the
            // first compare also covers `lo < 0` and `lo > hi`).
            // The trap arm calls the runtime cold — a hot-path
            // extern call would block vectorization and hoisting
            // (BCE `slice_mk` measured ~24% from the call alone).
            let ok_lbl = self.new_label("rcok");
            let bad_lbl = self.new_label("rcbad");
            let mid_lbl = self.new_label("rcmid");
            let c1 = self.fresh();
            self.emit(Instr::Bin {
                dst: c1,
                op: BinOp::Lt,
                ty: Ty::U64,
                a: hiv.clone(),
                b: lov.clone(),
            });
            self.seal(Term::Br(
                Opnd::R(c1),
                bad_lbl.clone(),
                mid_lbl.clone(),
            ));
            self.open(mid_lbl);
            let c2 = self.fresh();
            self.emit(Instr::Bin {
                dst: c2,
                op: BinOp::Gt,
                ty: Ty::U64,
                a: hiv.clone(),
                b: limit.clone(),
            });
            self.seal(Term::Br(
                Opnd::R(c2),
                bad_lbl.clone(),
                ok_lbl.clone(),
            ));
            self.open(bad_lbl);
            self.rt_call(
                "mml_range_check",
                Ty::Void,
                vec![
                    (Ty::I64, lov.clone()),
                    (Ty::I64, hiv.clone()),
                    (Ty::I64, limit),
                ],
            );
            self.seal(Term::Dead);
            self.open(ok_lbl);
        }
        // Start pointer — `&base[lo]`. A re-slice steps off the
        // loaded data pointer; an array base GEPs into its storage.
        let start_pl = match &bpl.ty {
            Ty::Array(..) => Place {
                root: bpl.root,
                root_ty: bpl.root_ty.clone(),
                path: {
                    let mut p = bpl.path.clone();
                    p.push(PathElem::Index {
                        val: lov.clone(),
                        ty: Ty::I64,
                    });
                    p
                },
                ty: elem.clone(),
            },
            Ty::Slice { .. } | Ty::BoxSlice(_) => {
                // `box slice` `.ptr` is always `*T` — the owner holds
                // Write authority over its own span.
                let bm = match &bpl.ty {
                    Ty::Slice { mutable, .. } => *mutable,
                    _ => true,
                };
                let pp = Place {
                    root: bpl.root,
                    root_ty: bpl.root_ty.clone(),
                    path: {
                        let mut p = bpl.path.clone();
                        p.push(PathElem::Field {
                            name: "ptr".to_string(),
                            idx: 0,
                            ty: Ty::Ptr {
                                mutable: bm,
                                elem: Box::new(elem.clone()),
                            },
                        });
                        p
                    },
                    ty: Ty::Ptr {
                        mutable: bm,
                        elem: Box::new(elem.clone()),
                    },
                };
                match self.load_place(pp) {
                    Opnd::R(r) => Place {
                        root: PRoot::Val(r),
                        root_ty: Ty::Ptr {
                            mutable: bm,
                            elem: Box::new(elem.clone()),
                        },
                        path: vec![PathElem::Index {
                            val: lov.clone(),
                            ty: Ty::I64,
                        }],
                        ty: elem.clone(),
                    },
                    _ => return Opnd::Imm(0),
                }
            }
            _ => return Opnd::Imm(0),
        };
        let dptr = self.fresh();
        self.emit(Instr::Addr {
            dst: dptr,
            place: start_pl,
        });
        let len = match (&hiv, &lov) {
            (Opnd::Imm(h), Opnd::Imm(l)) => Opnd::Imm(h - l),
            _ => {
                let d = self.fresh();
                self.emit(Instr::Bin {
                    dst: d,
                    op: BinOp::Sub,
                    ty: Ty::I64,
                    a: hiv,
                    b: lov,
                });
                Opnd::R(d)
            }
        };
        self.make_slice(Opnd::R(dptr), len, elem, mutable)
    }

    /// C-style layout sizes matching the LLVM repr (all members
    /// align ≤ 8; structs pad naturally). Used for runtime objects
    /// that copy payloads: channel packets, mutex interiors.
    fn align_of(&self, t: &Ty) -> i64 {
        match t {
            Ty::Bool | Ty::U8 | Ty::I8 => 1,
            Ty::U16 | Ty::I16 => 2,
            Ty::U32 | Ty::I32 => 4,
            Ty::Atomic(e) => self.align_of(e),
            Ty::Array(e, _) => self.align_of(e),
            Ty::Struct(n) => self
                .structs
                .get(n.as_str())
                .map(|fs| fs.iter().map(|(_, t)| self.align_of(t)).max().unwrap_or(1))
                .unwrap_or(8),
            _ => 8,
        }
    }

    fn size_of(&self, t: &Ty) -> i64 {
        match t {
            Ty::Bool | Ty::U8 | Ty::I8 => 1,
            Ty::U16 | Ty::I16 => 2,
            Ty::U32 | Ty::I32 => 4,
            Ty::Atomic(e) => self.size_of(e),
            // `slice T`/`box slice`/`str` — {ptr, len} at the C
            // layout: 16 bytes.
            Ty::Slice { .. } => 16,
            Ty::BoxSlice(_) => 16,
            Ty::Str => 16,
            Ty::Array(e, n) => {
                let s = self.size_of(e);
                let a = self.align_of(e);
                (s + a - 1) / a * a * (*n as i64)
            }
            Ty::Struct(n) => {
                let mut off = 0i64;
                let mut ma = 1i64;
                if let Some(fs) = self.structs.get(n.as_str()) {
                    for (_, ft) in fs {
                        let a = self.align_of(ft);
                        ma = ma.max(a);
                        off = (off + a - 1) / a * a + self.size_of(ft);
                    }
                }
                (off + ma - 1) / ma * ma
            }
            _ => 8, // u64/i64 and every ptr-like
        }
    }

    /// The names a task body captures: free variables of `body` that
    /// resolve in the current env. `(name, ty, ptr-class)`.
    fn captures(&mut self, body: &'a [Stmt]) -> Vec<(String, Ty, bool)> {
        let mut used = Vec::new();
        fv_stmts(body, &mut HashSet::new(), &mut used);
        used.iter()
            .filter_map(|n| {
                self.lookup(n)
                    .map(|(_, ty, _)| (n.clone(), ty.clone(), ty.is_ptr_like()))
            })
            .collect()
    }

    /// `*e` — the place starts at the loaded pointer value (references
    /// and handles share the ptr form today). Also used by atomic ops
    /// whose place arg is itself a ref/handle (`atomic_add(h, 1)`).
    fn place_ptr(&mut self, inner: &'a Expr) -> Option<Place> {
        let t = self.infer(inner, None);
        let elem = match t.clone() {
            Ty::Ref { elem, .. } | Ty::Handle(elem) | Ty::Guard(elem) | Ty::Box(elem) => {
                *elem
            }
            _ => return None,
        };
        let mut v = self.expr(inner, None);
        if let (Ty::Guard(_), Opnd::R(base)) = (&t, &v) {
            // The guard value is the mutex OBJECT — `*g` lands on the
            // payload.
            let data = self.fresh();
            self.emit(Instr::Call {
                dst: Some(data),
                ret: Self::ptr_ty(),
                f: "mml_mutex_data".to_string(),
                args: vec![(Self::ptr_ty(), Opnd::R(*base))],
            });
            v = Opnd::R(data);
        }
        match v {
            Opnd::R(reg) => Some(Place {
                root: PRoot::Val(reg),
                root_ty: elem.clone(),
                path: Vec::new(),
                ty: elem,
            }),
            _ => None,
        }
    }

    /// Store a struct literal's fields into `place`, one field-store each.
    /// Write an enum value into a slot (N22): tag index into field 0,
    /// payload into the `[N x i8]` field-1 area, leaf-typed by the
    /// variant. Opaque pointers mean the payload store needs no cast.
    fn fill_enum(
        &mut self,
        place: &Place,
        ename: &str,
        variant: &str,
        payload: Option<&'a Expr>,
    ) {
        let idx = self
            .enums
            .get(ename)
            .and_then(|vs| vs.iter().position(|(vn, _)| vn == variant))
            .unwrap_or(0) as i64;
        let tag_place = Place {
            root: place.root,
            root_ty: place.root_ty.clone(),
            path: {
                let mut p = place.path.clone();
                p.push(PathElem::Field {
                    name: "tag".to_string(),
                    idx: 0,
                    ty: Ty::I64,
                });
                p
            },
            ty: Ty::I64,
        };
        self.emit(Instr::Store {
            place: tag_place,
            val: Opnd::Imm(idx),
        });
        if let Some(pe) = payload {
            let pty = self
                .enums
                .get(ename)
                .and_then(|vs| {
                    vs.iter()
                        .find(|(vn, _)| vn == variant)
                        .and_then(|(_, t)| t.clone())
                })
                .unwrap_or(Ty::Void);
            let v = self.expr(pe, Some(&pty));
            self.emit(Instr::Store {
                place: Place {
                    root: place.root,
                    root_ty: place.root_ty.clone(),
                    path: {
                        let mut p = place.path.clone();
                        p.push(PathElem::Field {
                            name: "payload".to_string(),
                            idx: 1,
                            ty: pty.clone(),
                        });
                        p
                    },
                    ty: pty,
                },
                val: v,
            });
        }
    }

    fn fill_place(&mut self, place: &Place, fields: &'a [(String, Expr)]) {
        let sname = match &place.ty {
            Ty::Struct(n) => n.clone(),
            _ => return,
        };
        for (fname, fe) in fields {
            let idx = match self.field_idx(&sname, fname) {
                Some(i) => i,
                None => continue,
            };
            let fty = self.structs[&sname][idx as usize].1.clone();
            let v = self.expr(fe, Some(&fty));
            self.emit(Instr::Store {
                place: Place {
                    root: place.root,
                    root_ty: place.root_ty.clone(),
                    path: {
                        let mut p = place.path.clone();
                        p.push(PathElem::Field {
                            name: fname.clone(),
                            idx,
                            ty: fty.clone(),
                        });
                        p
                    },
                    ty: fty,
                },
                val: v,
            });
        }
    }

    fn stmts(&mut self, stmts: &'a [Stmt]) {
        for s in stmts {
            self.cur_span = Some(s.span());
            match s {
                Stmt::Let { name, ty, init, .. } => {
                    let t = match (ty, init) {
                        (Some(w), _) => w.clone(),
                        (None, Some(i)) => self.infer(i, None),
                        (None, None) => Ty::Void, // checker rejected
                    };
                    // `mutex T`/`channel T`/`task T` all lower to
                    // `ptr` — the slot needs no special sizing.
                    let slot_ty = t.clone();
                    let slot = self.fresh();
                    self.emit(Instr::Local {
                        dst: slot,
                        ty: slot_ty.clone(),
                        name: name.clone(),
                    });
                    match init {
                        Some(Expr::EnumLit { ty, variant, payload, .. }) => {
                            let ename = match &**ty {
                                Ty::Struct(n) => n.clone(),
                                _ => String::new(),
                            };
                            self.fill_enum(
                                &Place {
                                    root: PRoot::Slot(slot),
                                    root_ty: t.clone(),
                                    path: Vec::new(),
                                    ty: t.clone(),
                                },
                                &ename,
                                variant,
                                payload.as_deref(),
                            );
                        }
                        Some(Expr::StructLit(_, _tg, fields, _)) => {
                            // A literal is a place-fill, not a value —
                            // write fields straight into the new slot.
                            self.fill_place(
                                &Place {
                                    root: PRoot::Slot(slot),
                                    root_ty: t.clone(),
                                    path: Vec::new(),
                                    ty: t.clone(),
                                },
                                fields,
                            );
                        }
                        Some(init) => {
                            // A guard MOVE (`let g2 = g`) hands the
                            // lock to the new binding — the consumed
                            // source's slot stops being responsible.
                            if let Expr::Var(src, _) = init {
                                if let Some((ss, Ty::Guard(_), _)) = self.lookup(src) {
                                    self.open_guards.remove(&ss);
                                }
                            }
                            let v = self.expr(init, Some(&t));
                            self.emit(Instr::Store {
                                place: Place {
                                    root: PRoot::Slot(slot),
                                    root_ty: slot_ty.clone(),
                                    path: Vec::new(),
                                    ty: slot_ty.clone(),
                                },
                                val: v,
                            });
                        }
                        None => {}
                    }
                    if matches!(t, Ty::Guard(_)) {
                        self.open_guards.insert(slot);
                    }
                    let is_span =
                        matches!(t, Ty::Slice { .. } | Ty::BoxSlice(_) | Ty::Str);
                    self.env
                        .last_mut()
                        .unwrap()
                        .insert(name.clone(), (slot, t, false));
                    // N47 — a `let` copy is an exact equality:
                    // `let m = e` gives `m ≡ e` while both stay
                    // unwritten; `alloc_slice(h, n)` gives `b.len ≡ n`;
                    // a slice/str copy shares its len too.
                    let mk = format!("s{}", slot.0);
                    match init {
                        Some(Expr::Alloc { count: Some(c), .. }) => {
                            if let Some(p) = self.bnorm_lenkey(c) {
                                self.efacts.push(((format!("{mk}.len"), 0), p));
                            }
                        }
                        Some(e) => {
                            if let Some(p) = self.bnorm(e) {
                                self.efacts.push(((mk.clone(), 0), p.clone()));
                                if is_span {
                                    self.efacts.push((
                                        (format!("{mk}.len"), 0),
                                        (format!("{}.len", p.0), p.1),
                                    ));
                                }
                            }
                            // N48 — `let i = a & mask` binds the AND's
                            // bound onto `i` (`i <= mask`, `i < cap`
                            // when `mask ≡ cap-1` with `cap > 0`).
                            if let Expr::Bin(BinOp::BitAnd, _, m, _) = e {
                                if let Some(mn) = self.bnorm(m) {
                                    let d = (mk.clone(), 0);
                                    self.and_facts(&d, &mn, m);
                                }
                            }
                        }
                        None => {}
                    }
                }
                Stmt::Assign { target, value, .. } => {
                    if let Some(place) = self.place(target) {
                        let tgt_span = matches!(
                            place.ty,
                            Ty::Slice { .. } | Ty::BoxSlice(_) | Ty::Str
                        );
                        if let Expr::EnumLit { ty, variant, payload, .. } = value {
                            let ename = match &**ty {
                                Ty::Struct(n) => n.clone(),
                                _ => String::new(),
                            };
                            self.fill_enum(
                                &place,
                                &ename,
                                variant,
                                payload.as_deref(),
                            );
                        } else if let Expr::StructLit(_, _tg, fields, _) = value {
                            self.fill_place(&place, fields);
                        } else {
                            let v = self.expr(value, Some(&place.ty));
                            self.emit(Instr::Store { place, val: v });
                        }
                        // N46 — the store may have moved a bound fact's
                        // place (`i = ..` kills `i < s.len`).
                        self.bkill(target);
                        // N47 — a copy is an exact equality: `m = e`
                        // gives `m ≡ e` until either is written again.
                        // Self-referential norms (`m = m + 1`) are NOT
                        // equalities — the lhs name changed value.
                        if let Some(k) = self.bkey(target) {
                            if let Some(p) = self.bnorm(value) {
                                if p.0 != k && !p.0.starts_with(&format!("{k}.")) {
                                    self.efacts.push(((k.clone(), 0), p.clone()));
                                    if tgt_span {
                                        self.efacts.push((
                                            (format!("{k}.len"), 0),
                                            (format!("{}.len", p.0), p.1),
                                        ));
                                    }
                                }
                            }
                            // N48 — `i = (i + 1) & mask` re-binds the
                            // AND bound each probe step.
                            if let Expr::Bin(BinOp::BitAnd, _, m, _) = value {
                                if let Some(mn) = self.bnorm(m) {
                                    let d = (k.clone(), 0);
                                    self.and_facts(&d, &mn, m);
                                }
                            }
                        }
                    }
                }
                Stmt::Return { value, .. } => {
                    // Every live guard releases before we leave —
                    // `return` collapses all scopes at once.
                    let open: Vec<Reg> = self.open_guards.iter().copied().collect();
                    for slot in open {
                        self.unlock_slot(slot);
                    }
                    self.open_guards.clear();
                    if let Some((end, rp, ty)) = self.task_res.clone() {
                        // Inside a task fn: `return v` stores through
                        // the caller's result-cell pointer and jumps
                        // to the fn's end.
                        let v = value.as_ref().map(|e| self.expr(e, Some(&ty)));
                        if let Some(v) = v {
                            self.emit(Instr::Store {
                                place: Place {
                                    root: PRoot::Val(rp),
                                    root_ty: ty.clone(),
                                    path: Vec::new(),
                                    ty,
                                },
                                val: v,
                            });
                        }
                        // N54 — the body's defers run on this exit edge.
                        self.emit_all_defers();
                        self.seal(Term::Jmp(end));
                    } else {
                        let ret = self.ret.clone();
                        let v = value.as_ref().map(|e| self.expr(e, Some(&ret)));
                        // N54 — deferred calls run between the return
                        // value's evaluation and the jump out.
                        self.emit_all_defers();
                        self.seal(Term::Ret(v, ret));
                    }
                }
                Stmt::If {
                    cond,
                    then_body,
                    else_body,
                    ..
                } => {
                    let c = self.expr(cond, Some(&Ty::Bool));
                    let t_lbl = self.new_label("then");
                    let e_lbl = self.new_label("else");
                    let m_lbl = self.new_label("merge");
                    let else_target = if else_body.is_some() { &e_lbl } else { &m_lbl };
                    self.seal(Term::Br(c, t_lbl.clone(), else_target.clone()));
                    self.open(t_lbl);
                    // N46 — the cond's comparisons dominate this block.
                    let bmark = (self.bfacts.len(), self.efacts.len());
                    self.cond_facts(cond);
                    self.env.push(HashMap::new());
                    self.defers.push(Vec::new());
                    self.stmts(then_body);
                    self.bfacts.truncate(bmark.0);
                    self.efacts.truncate(bmark.1);
                    self.pop_scope();
                    let then_dead = self.terminated && self.label.is_none();
                    if !self.terminated || self.label.is_some() {
                        self.seal(Term::Jmp(m_lbl.clone()));
                    }
                    if let Some(eb) = else_body {
                        self.open(e_lbl);
                        // N48 — the else arm runs under `!cond`.
                        let emark = (self.bfacts.len(), self.efacts.len());
                        self.neg_facts(cond);
                        self.env.push(HashMap::new());
                        self.defers.push(Vec::new());
                        self.stmts(eb);
                        self.bfacts.truncate(emark.0);
                        self.efacts.truncate(emark.1);
                        self.pop_scope();
                        let else_dead = self.terminated && self.label.is_none();
                        if !self.terminated || self.label.is_some() {
                            self.seal(Term::Jmp(m_lbl.clone()));
                        }
                        self.open(m_lbl);
                        // N48 — a dead else arm leaves the then-path:
                        // `cond` held and dominates the continuation.
                        if else_dead {
                            self.cond_facts(cond);
                        }
                    } else {
                        self.open(m_lbl);
                        // N48 — `if !c { diverge }` / `if c { return }`:
                        // a dead then-arm leaves the else-path, where
                        // `!cond` dominates everything after — the
                        // guard/assert idiom.
                        if then_dead {
                            self.neg_facts(cond);
                        }
                    }
                }
                Stmt::While { cond, body, .. } => {
                    let c_lbl = self.new_label("cond");
                    let b_lbl = self.new_label("body");
                    let a_lbl = self.new_label("after");
                    self.seal(Term::Jmp(c_lbl.clone()));
                    self.open(c_lbl.clone());
                    let c = self.expr(cond, Some(&Ty::Bool));
                    self.seal(Term::Br(c, b_lbl.clone(), a_lbl.clone()));
                    self.loops
                        .push((a_lbl.clone(), b_lbl.clone(), cond, self.env.len()));
                    self.open(b_lbl.clone());
                    // N46 — the loop test dominates the body: `while
                    // i < s.len` proves `s[i]` in range here.
                    let bmark = (self.bfacts.len(), self.efacts.len());
                    self.cond_facts(cond);
                    self.env.push(HashMap::new());
                    self.defers.push(Vec::new());
                    self.stmts(body);
                    self.pop_scope();
                    self.loops.pop();
                    if !self.terminated || self.label.is_some() {
                        // N48 — the back-edge test is re-emitted under
                        // the END-OF-BODY fact state: ambient facts
                        // proved the entry edge, and whatever the body
                        // re-established (`i = (i+1) & mask` re-pushes
                        // `i < len`) proves this one. A stale ambient
                        // fact can't reach a later iteration's check.
                        let c2 = self.expr(cond, Some(&Ty::Bool));
                        self.seal(Term::Br(c2, b_lbl, a_lbl.clone()));
                    }
                    self.bfacts.truncate(bmark.0);
                    self.efacts.truncate(bmark.1);
                    self.open(a_lbl);
                }
                Stmt::Break { .. } | Stmt::Continue { .. } => {
                    let (brk, blbl, cond, depth) =
                        self.loops.last().expect("checker verified").clone();
                    // N54 — defers of the scopes this exit unwinds
                    // replay on the edge, while their names resolve.
                    self.emit_defers_above(depth);
                    // Guards bound inside the loop die at the exit —
                    // release them before the jump, same death `}` gives.
                    // `contains`, not `remove`: sibling paths still reach
                    // the scope's own pop, which emits their unlock —
                    // removing here would leak the lock on every
                    // non-break iteration.
                    let regs: Vec<Reg> = self.env[depth..]
                        .iter()
                        .flat_map(|scope| {
                            scope
                                .values()
                                .filter(|(_, ty, _)| matches!(ty, Ty::Guard(_)))
                                .map(|(r, _, _)| *r)
                        })
                        .collect();
                    for slot in regs {
                        if self.open_guards.contains(&slot) {
                            self.unlock_slot(slot);
                        }
                    }
                    if matches!(s, Stmt::Break { .. }) {
                        self.seal(Term::Jmp(brk));
                    } else {
                        // N48 — `continue` re-tests under the facts
                        // live HERE, not the entry edge's: inner
                        // scopes are swapped out so the cond's names
                        // resolve to the slots the loop entry saw.
                        let tail = self.env.split_off(depth);
                        let c = self.expr(cond, Some(&Ty::Bool));
                        self.env.extend(tail);
                        self.seal(Term::Br(c, blbl, brk));
                    }
                }
                Stmt::Match { scrut, arms, span } => {
                    self.gen_match(scrut, arms, span, None);
                }

                Stmt::Region { body, .. } => {
                    // Semantic frames only: scoping, no code.
                    self.env.push(HashMap::new());
                    self.defers.push(Vec::new());
                    self.stmts(body);
                    self.pop_scope();
                }
                Stmt::Consume { .. } => {
                    // No code — the checker marked the place consumed.
                }
                Stmt::Defer { expr, .. } => {
                    // N54 — registration only: the call's instructions
                    // are emitted at each of this scope's exit edges.
                    self.defers.last_mut().unwrap().push(expr);
                }
                Stmt::Promote { target, .. } => {
                    // No code — the checker moved the binding's owner.
                    // Mark the name so it survives scope exits.
                    if let Expr::Var(n, _) = target {
                        self.promoted.insert(n.clone());
                    }
                }
                Stmt::Expr { expr, .. } => {
                    self.expr(expr, None);
                }
            }
        }
    }

    /// `match` codegen — shared by `Stmt::Match` (want `None`, the
    /// result operand is discarded) and `Expr::Match` (N28). The tag
    /// compare-chain is the physical switch; `=> expr` arms store
    /// their yield into a `$match` local the merge loads.
    fn gen_match(&mut self, scrut: &'a Expr, e_arms: &'a [MatchArm], span: &Span, want: Option<&Ty>) -> Opnd {
        let arms = e_arms;

                    // Load the tag, then a compare-chain per arm â the
                    // physical switch is an ordinary branch tree.
                    let mty = self.match_yield_ty(scrut, arms, want);
                    let slot = if mty == Ty::Void {
                        None
                    } else {
                        let s = self.fresh();
                        self.emit(Instr::Local {
                            dst: s,
                            ty: mty.clone(),
                            name: "$match".to_string(),
                        });
                        Some(s)
                    };
                    let end = self.new_label("match.end");
                    let mut arm_labels: Vec<(String, String)> = Vec::new(); // (this, next)
                    for _ in arms {
                        arm_labels.push((self.new_label("arm"), self.new_label("next")));
                    }
                    let tagv = self.place(scrut).map(|mut sp| {
                        sp.ty = Ty::I64;
                        sp.path.push(PathElem::Field {
                            name: "tag".to_string(),
                            idx: 0,
                            ty: Ty::I64,
                        });
                        self.load_place(sp)
                    });
                    let ename = match self.infer(scrut, None) {
                        Ty::Struct(n) => Some(n),
                        _ => None,
                    };
                    self.env.push(HashMap::new());
                    self.defers.push(Vec::new());
                    for (i, a) in arms.iter().enumerate() {
                        let (this, next) = arm_labels[i].clone();
                        let vidx = ename
                            .as_ref()
                            .and_then(|n| self.enums.get(n))
                            .and_then(|vs| {
                                vs.iter().position(|(vn, _)| vn == &a.variant)
                            });
                        match (a.variant.as_str(), vidx) {
                            ("_", _) | (_, None) => {
                                // Wildcard / bad arm â unconditional.
                                self.seal(Term::Jmp(this.clone()));
                            }
                            (_, Some(vi)) => {
                                let c = self.fresh();
                                self.emit(Instr::Bin {
                                    dst: c,
                                    op: BinOp::Eq,
                                    ty: Ty::I64,
                                    a: tagv.clone().unwrap_or(Opnd::Imm(0)),
                                    b: Opnd::Imm(vi as i64),
                                });
                                self.seal(Term::Br(Opnd::R(c), this.clone(), next.clone()));
                            }
                        }
                        self.open(this);
                        // `V(b)` — bind the payload the way the
                        // checker bound it: `Value` loads a copy into
                        // the fresh slot; `Ref`/`RefMut` store the
                        // payload's ADDRESS — the binder is `&T`/`&mut T`
                        // and `*b` reaches the live payload.
                        if let (Some(b), Some(n)) = (&a.bind, &ename) {
                            if let Some(pty) = self
                                .enums
                                .get(n)
                                .and_then(|vs| {
                                    vs.iter()
                                        .find(|(vn, _)| vn == &a.variant)
                                        .and_then(|(_, t)| t.clone())
                                })
                            {
                                // Rebuild the scrutinee place — the
                                // payload is field 1, leaf = variant ty.
                                let pp = self.place(scrut).map(|mut p| {
                                    p.ty = pty.clone();
                                    p.path.push(PathElem::Field {
                                        name: a.variant.clone(),
                                        idx: 1,
                                        ty: pty.clone(),
                                    });
                                    p
                                });
                                let mode = self
                                    .binds
                                    .get(span)
                                    .and_then(|m| m.get(&a.variant))
                                    .copied()
                                    .unwrap_or(ArmBind::Value);
                                if mode == ArmBind::Value {
                                    let pv = pp.map(|p| self.load_place(p));
                                    let slot = self.fresh();
                                    self.emit(Instr::Local {
                                        dst: slot,
                                        ty: pty.clone(),
                                        name: b.clone(),
                                    });
                                    self.emit(Instr::Store {
                                        place: Place {
                                            root: PRoot::Slot(slot),
                                            root_ty: pty.clone(),
                                            path: Vec::new(),
                                            ty: pty.clone(),
                                        },
                                        val: pv.unwrap_or(Opnd::Imm(0)),
                                    });
                                    self.env
                                        .last_mut()
                                        .unwrap()
                                        .insert(b.clone(), (slot, pty, false));
                                } else {
                                    let rty = Ty::Ref {
                                        mutable: false,
                                        elem: Box::new(pty.clone()),
                                    };
                                    let pa = pp.map(|p| {
                                        let a = self.fresh();
                                        self.emit(Instr::Addr { dst: a, place: p });
                                        Opnd::R(a)
                                    });
                                    let slot = self.fresh();
                                    self.emit(Instr::Local {
                                        dst: slot,
                                        ty: rty.clone(),
                                        name: b.clone(),
                                    });
                                    self.emit(Instr::Store {
                                        place: Place {
                                            root: PRoot::Slot(slot),
                                            root_ty: rty.clone(),
                                            path: Vec::new(),
                                            ty: rty.clone(),
                                        },
                                        val: pa.unwrap_or(Opnd::Imm(0)),
                                    });
                                    self.env
                                        .last_mut()
                                        .unwrap()
                                        .insert(b.clone(), (slot, rty, false));
                                }
                            }
                        }
                        // N28 — an `=> expr` arm's tail is its yield:
                        // evaluate it under the unified type and store
                        // it in the result slot the caller allocated.
                        let dmark = self.defers.last().map(|l| l.len()).unwrap_or(0);
                        let (prefix, tail) = if a.expr_arm && !a.body.is_empty() {
                            (&a.body[..a.body.len() - 1], a.body.last())
                        } else {
                            (&a.body[..], None)
                        };
                        self.stmts(prefix);
                        if let (Some(Stmt::Expr { expr, .. }), Some(slot)) = (tail, slot) {
                            if !self.terminated {
                                let v = self.expr(expr, Some(&mty));
                                self.emit(Instr::Store {
                                    place: Place {
                                        root: PRoot::Slot(slot),
                                        root_ty: mty.clone(),
                                        path: Vec::new(),
                                        ty: mty.clone(),
                                    },
                                    val: v,
                                });
                            }
                        }
                        // N54 — arm bodies share the match's defer
                        // list: drain the entries THIS arm registered
                        // (the arm's own scope dies here).
                        let dl: Vec<&Expr> = self
                            .defers
                            .last_mut()
                            .map(|l| l.split_off(dmark))
                            .unwrap_or_default();
                        if !self.terminated {
                            for e in dl.iter().rev() {
                                let _ = self.expr(e, None);
                            }
                            self.seal(Term::Jmp(end.clone()));
                        }
                        self.open(next);
                    }
                    // Fell off the end: either a wildcard arm already
                    // handled everything, or the checker reported a
                    // non-exhaustive match. Dead terminator either way.
                    self.seal(Term::Dead);
                    self.open(end);
                    self.env.pop();
                    self.defers.pop();
                    match slot {
                        Some(s) => self.load_place(Place {
                            root: PRoot::Slot(s),
                            root_ty: mty.clone(),
                            path: Vec::new(),
                            ty: mty,
                        }),
                        None => Opnd::Imm(0),
                    }
                
    }

    /// Load the leaf value of a place expression into a fresh reg.
    fn load_place(&mut self, place: Place) -> Opnd {
        let dst = self.fresh();
        self.emit(Instr::Load { dst, place });
        Opnd::R(dst)
    }

    fn expr(&mut self, e: &'a Expr, want: Option<&Ty>) -> Opnd {
        self.cur_span = Some(e.span());
        match e {
            Expr::Match { scrut, arms, span, .. } => self.gen_match(scrut, arms, span, want),
            Expr::Int(n, _) => {
                let t = match want {
                    Some(t) if t.is_int() => t.clone(),
                    // `0` under a pointer want — C's null constant
                    // (`inttoptr`).
                    Some(t) if t.is_ptr_like() => t.clone(),
                    _ => Ty::INT_DEFAULT,
                };
                self.const_of(&t, *n)
            }
            Expr::Bool(b, _) => self.const_of(&Ty::Bool, *b as i64),
            // `sizeof<T>` — a layout constant: computed against the
            // struct table exactly like `alloc` does internally.
            Expr::MethodCall { .. } | Expr::AssocFn { .. } => {
                unreachable!("method/assoc sugar survives past monomorphization")
            }
            Expr::Sizeof(t, _) => {
                let sz = self.size_of(t);
                self.const_of(&Ty::U64, sz)
            }
            Expr::Var(n, _)
                if matches!(self.lookup(n), Some((_, Ty::Guard(_), _))) =>
            {
                // The guard's VALUE is the stored ptr to protected
                // storage — `*g`, moves and unlock args all want it.
                // (`g.x`/`g[i]` never reach here: they go through
                // place(), which resolves the pointee instead.)
                let (root, ty, alias) = self.lookup(n).unwrap();
                self.load_place(Place {
                    root: if alias {
                        PRoot::Val(root)
                    } else {
                        PRoot::Slot(root)
                    },
                    root_ty: ty.clone(),
                    path: Vec::new(),
                    ty,
                })
            }
            Expr::Var(..) | Expr::Field(..) | Expr::Index(..) | Expr::Deref(..) => {
                match self.place(e) {
                    Some(pl) => self.load_place(pl),
                    None => {
                        // A slice's pseudo-fields on a VALUE-position
                        // slice — `f().len`, `g().ptr` — extract the
                        // member straight from the aggregate.
                        if let Expr::Field(b, name, _) = e {
                            match self.infer(b, None) {
                                Ty::Slice { elem, mutable } => {
                                    let (idx, ty) = match name.as_str() {
                                        "ptr" => (
                                            0u32,
                                            Ty::Ptr { mutable, elem },
                                        ),
                                        "len" => (1, Ty::U64),
                                        _ => return Opnd::Imm(0),
                                    };
                                    let av = self.expr(b, None);
                                    let dst = self.fresh();
                                    self.emit(Instr::Extract { dst, ty, agg: av, idx });
                                    return Opnd::R(dst);
                                }
                                Ty::Str => {
                                    // `f().bytes` IS the aggregate —
                                    // re-read as `slice u8`, zero cost.
                                    if name == "bytes" {
                                        return self.expr(b, None);
                                    }
                                    if name == "len" {
                                        let av = self.expr(b, None);
                                        let dst = self.fresh();
                                        self.emit(Instr::Extract {
                                            dst,
                                            ty: Ty::U64,
                                            agg: av,
                                            idx: 1,
                                        });
                                        return Opnd::R(dst);
                                    }
                                }
                                _ => {}
                            }
                        }
                        Opnd::Imm(0) // checker rejected; keep emitting
                    }
                }
            }
            Expr::AddrOf { mutable, place, .. } => match &**place {
                // `&a[lo..hi]` / `&s[lo..hi]` — build {ptr, len}.
                Expr::IndexRange { base, lo, hi, .. } => {
                    self.slice_range(base, lo.as_deref(), hi.as_deref(), *mutable)
                }
                _ => match self.place(place) {
                    Some(pl) => {
                        let dst = self.fresh();
                        self.emit(Instr::Addr { dst, place: pl });
                        Opnd::R(dst)
                    }
                    None => Opnd::Imm(0),
                },
            },
            Expr::Handle(place, _) => match self.place(place) {
                Some(pl) => {
                    let dst = self.fresh();
                    self.emit(Instr::MakeHandle { dst, place: pl });
                    Opnd::R(dst)
                }
                None => Opnd::Imm(0),
            },
            Expr::Str(bytes, _) => {
                let idx = {
                    let mut st = self.strings.borrow_mut();
                    st.push(bytes.clone());
                    (st.len() - 1) as u32
                };
                let dst = self.fresh();
                self.emit(Instr::StrAddr {
                    dst,
                    idx,
                    len: (bytes.len() + 1) as u32,
                });
                if want == Some(&Ty::Str) {
                    return self.make_str(Opnd::R(dst), Opnd::Imm(bytes.len() as i64));
                }
                Opnd::R(dst)
            }
            Expr::Spawn(body, sp) => {
                // N13: a real OS thread. Package the body's free vars
                // into an env frame — [0] task*, [1] result-cell ptr,
                // [2+i] capture i — and hand a synthesized `__task_*`
                // fn to the runtime. The task token IS env's address.
                let inner = self.spawn_ret_ty(body);
                let caps = self.captures(body);
                let arr_ty =
                    Ty::Array(Box::new(Self::ptr_ty()), (caps.len() + 2) as u32);
                let env = self.fresh();
                self.emit(Instr::Local {
                    dst: env,
                    ty: arr_ty.clone(),
                    name: "$env".to_string(),
                });
                let env_slot = |i: i64| Place {
                    root: PRoot::Slot(env),
                    root_ty: arr_ty.clone(),
                    path: vec![PathElem::Index {
                        val: Opnd::Imm(i),
                        ty: Ty::I32,
                    }],
                    ty: Self::ptr_ty(),
                };
                if inner != Ty::Void {
                    // The result cell lives in THIS frame — the task
                    // stores through env[1]; `join` reads it back.
                    let cell = self.fresh();
                    self.emit(Instr::Local {
                        dst: cell,
                        ty: inner.clone(),
                        name: "$res".to_string(),
                    });
                    let rp = self.fresh();
                    self.emit(Instr::Addr {
                        dst: rp,
                        place: Place {
                            root: PRoot::Slot(cell),
                            root_ty: inner.clone(),
                            path: Vec::new(),
                            ty: inner.clone(),
                        },
                    });
                    self.emit(Instr::Store {
                        place: env_slot(1),
                        val: Opnd::R(rp),
                    });
                }
                for (i, (n, ty, pc)) in caps.iter().enumerate() {
                    let pl = self.var_place(n).unwrap_or(Place {
                        root: PRoot::Val(Reg(u32::MAX)),
                        root_ty: Ty::Void,
                        path: Vec::new(),
                        ty: Ty::Void,
                    });
                    let v = if *pc {
                        // Ptr-class: the slot's CONTENT is the pointer.
                        self.load_place(pl)
                    } else {
                        // Storage: the slot's ADDRESS — the task
                        // aliases the caller's frame directly.
                        let dst = self.fresh();
                        self.emit(Instr::Addr { dst, place: pl });
                        Opnd::R(dst)
                    };
                    let _ = ty;
                    self.emit(Instr::Store {
                        place: env_slot((i + 2) as i64),
                        val: v,
                    });
                }
                let fname = format!("__task_{}_{}", self.fname, self.tseq);
                self.tseq += 1;
                self.tasks.push(TaskSpec {
                    name: fname.clone(),
                    body,
                    ret: inner,
                    caps,
                    span: *sp,
                });
                let envp = self.fresh();
                self.emit(Instr::Addr {
                    dst: envp,
                    place: Place {
                        root: PRoot::Slot(env),
                        root_ty: arr_ty.clone(),
                        path: Vec::new(),
                        ty: arr_ty.clone(),
                    },
                });
                let task = self.rt_call(
                    "mml_task_spawn",
                    Self::ptr_ty(),
                    vec![
                        (Self::ptr_ty(), Opnd::Fun(fname)),
                        (Self::ptr_ty(), Opnd::R(envp)),
                    ],
                );
                self.emit(Instr::Store {
                    place: env_slot(0),
                    val: task,
                });
                Opnd::R(envp)
            }
            Expr::Join(inner, _) => {
                // N46 — the task's suspended writes land here; they
                // can hit any captured place: drop all bound facts.
                for f in self.bfacts.iter_mut() {
                    f.lo = ("#dead".to_string(), 0);
                    f.hi = ("#dead".to_string(), 0);
                }
                for f in self.efacts.iter_mut() {
                    *f = (
                        ("#dead".to_string(), 0),
                        ("#dead".to_string(), 0),
                    );
                }
                // Token = env ptr: [0] is the runtime task, [1] the
                // result-cell pointer.
                let ev = match self.expr(inner, None) {
                    Opnd::R(r) => r,
                    _ => return Opnd::Imm(0),
                };
                let task = self.fresh();
                self.emit(Instr::Load {
                    dst: task,
                    place: Place {
                        root: PRoot::Val(ev),
                        root_ty: Self::ptr_ty(),
                        path: Vec::new(),
                        ty: Self::ptr_ty(),
                    },
                });
                self.rt_call(
                    "mml_task_join",
                    Ty::Void,
                    vec![(Self::ptr_ty(), Opnd::R(task))],
                );
                let rty = match self.infer(inner, None) {
                    Ty::Task(t) => *t,
                    _ => Ty::Void,
                };
                if rty == Ty::Void {
                    return Opnd::Imm(0);
                }
                let rp = self.fresh();
                self.emit(Instr::Load {
                    dst: rp,
                    place: Place {
                        root: PRoot::Val(ev),
                        root_ty: Ty::Array(Box::new(Self::ptr_ty()), 2),
                        path: vec![PathElem::Index {
                            val: Opnd::Imm(1),
                            ty: Ty::I32,
                        }],
                        ty: Self::ptr_ty(),
                    },
                });
                self.load_place(Place {
                    root: PRoot::Val(rp),
                    root_ty: rty.clone(),
                    path: Vec::new(),
                    ty: rty,
                })
            }
            Expr::EnumLit { ty, variant, payload, .. } => {
                let t = (**ty).clone();
                let ename = match &t {
                    Ty::Struct(n) => n.clone(),
                    _ => return Opnd::Imm(0),
                };
                let slot = self.fresh();
                self.emit(Instr::Local {
                    dst: slot,
                    ty: t.clone(),
                    name: String::new(),
                });
                self.fill_enum(
                    &Place {
                        root: PRoot::Slot(slot),
                        root_ty: t.clone(),
                        path: Vec::new(),
                        ty: t.clone(),
                    },
                    &ename,
                    variant,
                    payload.as_deref(),
                );
                self.load_place(Place {
                    root: PRoot::Slot(slot),
                    root_ty: t.clone(),
                    path: Vec::new(),
                    ty: t,
                })
            }
            Expr::Try { inner, .. } => {
                // `expr?` â N23: a mid-expression tag branch. The Err
                // path rebuilds THIS fn's Result and returns; the Ok
                // path continues, yielding the payload â the same
                // `match` desugar the checker models.
                let ty = self.infer(inner, None);
                let ename = match &ty {
                    Ty::Struct(n) => n.clone(),
                    _ => return Opnd::Imm(0),
                };
                let vars = self.enums.get(&ename).cloned().unwrap_or_default();
                let ok_idx = vars
                    .iter()
                    .position(|(vn, _)| vn == "Ok")
                    .unwrap_or(0);
                let err_idx = vars
                    .iter()
                    .position(|(vn, _)| vn == "Err")
                    .unwrap_or(1);
                let ok_ty = vars[ok_idx].1.clone().unwrap_or(Ty::Void);
                let err_ty = vars[err_idx].1.clone().unwrap_or(Ty::Void);
                // Materialize the scrutinee enum into a place â
                // variant payloads live under field 1.
                let pl = match self.place(inner) {
                    Some(p) => p,
                    None => {
                        let v = self.expr(inner, Some(&ty));
                        let slot = self.fresh();
                        self.emit(Instr::Local {
                            dst: slot,
                            ty: ty.clone(),
                            name: String::new(),
                        });
                        self.emit(Instr::Store {
                            place: Place {
                                root: PRoot::Slot(slot),
                                root_ty: ty.clone(),
                                path: Vec::new(),
                                ty: ty.clone(),
                            },
                            val: v,
                        });
                        Place {
                            root: PRoot::Slot(slot),
                            root_ty: ty.clone(),
                            path: Vec::new(),
                            ty: ty.clone(),
                        }
                    }
                };
                let tagp = Place {
                    root: pl.root,
                    root_ty: pl.root_ty.clone(),
                    path: {
                        let mut p = pl.path.clone();
                        p.push(PathElem::Field {
                            name: "tag".to_string(),
                            idx: 0,
                            ty: Ty::I64,
                        });
                        p
                    },
                    ty: Ty::I64,
                };
                let tagv = self.load_place(tagp);
                let c = self.fresh();
                self.emit(Instr::Bin {
                    dst: c,
                    op: BinOp::Eq,
                    ty: Ty::I64,
                    a: tagv,
                    b: Opnd::Imm(err_idx as i64),
                });
                let err_l = self.new_label("try.err");
                let ok_l = self.new_label("try.ok");
                self.seal(Term::Br(Opnd::R(c), err_l.clone(), ok_l.clone()));
                // Err block â `return Result<_,E>::Err(payload)`.
                self.open(err_l);
                let rty = self.ret.clone();
                let rslot = self.fresh();
                self.emit(Instr::Local {
                    dst: rslot,
                    ty: rty.clone(),
                    name: String::new(),
                });
                let rname = match &rty {
                    Ty::Struct(n) => n.clone(),
                    _ => ename.clone(),
                };
                let rerr_idx = self
                    .enums
                    .get(&rname)
                    .and_then(|vs| vs.iter().position(|(vn, _)| vn == "Err"))
                    .unwrap_or(1) as i64;
                self.emit(Instr::Store {
                    place: Place {
                        root: PRoot::Slot(rslot),
                        root_ty: rty.clone(),
                        path: vec![PathElem::Field {
                            name: "tag".to_string(),
                            idx: 0,
                            ty: Ty::I64,
                        }],
                        ty: Ty::I64,
                    },
                    val: Opnd::Imm(rerr_idx),
                });
                let ev = self.load_place(Place {
                    root: pl.root,
                    root_ty: pl.root_ty.clone(),
                    path: {
                        let mut p = pl.path.clone();
                        p.push(PathElem::Field {
                            name: "Err".to_string(),
                            idx: 1,
                            ty: err_ty.clone(),
                        });
                        p
                    },
                    ty: err_ty.clone(),
                });
                self.emit(Instr::Store {
                    place: Place {
                        root: PRoot::Slot(rslot),
                        root_ty: rty.clone(),
                        path: vec![PathElem::Field {
                            name: "Err".to_string(),
                            idx: 1,
                            ty: err_ty.clone(),
                        }],
                        ty: err_ty,
                    },
                    val: ev,
                });
                let rv = self.load_place(Place {
                    root: PRoot::Slot(rslot),
                    root_ty: rty.clone(),
                    path: Vec::new(),
                    ty: rty.clone(),
                });
                // N54 — `?` unwinds the whole stack: every live defer
                // replays before the Err return leaves.
                self.emit_all_defers();
                self.seal(Term::Ret(Some(rv), rty));
                // Ok block â the expression's value is the payload.
                self.open(ok_l);
                self.load_place(Place {
                    root: pl.root,
                    root_ty: pl.root_ty.clone(),
                    path: {
                        let mut p = pl.path;
                        p.push(PathElem::Field {
                            name: "Ok".to_string(),
                            idx: 1,
                            ty: ok_ty.clone(),
                        });
                        p
                    },
                    ty: ok_ty,
                })
            }
            Expr::StructLit(n, _tg, fields, _) => {
                // In value position (arg, operand): materialize into an
                // anonymous slot, then load the aggregate back out.
                let slot = self.fresh();
                let t = Ty::Struct(n.clone());
                self.emit(Instr::Local {
                    dst: slot,
                    ty: t.clone(),
                    name: String::new(),
                });
                self.fill_place(
                    &Place {
                        root: PRoot::Slot(slot),
                        root_ty: t.clone(),
                        path: Vec::new(),
                        ty: t.clone(),
                    },
                    fields,
                );
                self.load_place(Place {
                    root: PRoot::Slot(slot),
                    root_ty: t.clone(),
                    path: Vec::new(),
                    ty: t,
                })
            }
            Expr::Unary(UnOp::Neg, inner, _) => {
                let t = self.infer(inner, want);
                let a = self.expr(inner, Some(&t));
                let dst = self.fresh();
                self.emit(Instr::Unary {
                    dst,
                    op: UnOp::Neg,
                    ty: t,
                    a,
                });
                Opnd::R(dst)
            }
            Expr::Unary(UnOp::Not, inner, _) => {
                let a = self.expr(inner, Some(&Ty::Bool));
                let dst = self.fresh();
                self.emit(Instr::Unary {
                    dst,
                    op: UnOp::Not,
                    ty: Ty::Bool,
                    a,
                });
                Opnd::R(dst)
            }
            Expr::Unary(UnOp::BitNot, inner, _) => {
                // `~x` = `x ^ -1` — one xor, all widths.
                let t = self.infer(inner, want);
                let a = self.expr(inner, Some(&t));
                let dst = self.fresh();
                self.emit(Instr::Bin {
                    dst,
                    op: BinOp::BitXor,
                    ty: t,
                    a,
                    b: Opnd::Imm(-1),
                });
                Opnd::R(dst)
            }
            Expr::Bin(op @ (BinOp::LAnd | BinOp::LOr), a, b, _) => {
                // N25C — `a && b` ≡ if a { b } else { false },
                // `a || b` ≡ if a { true } else { b }. A named bool
                // slot + a store per arm, load at the merge — no phi.
                let slot = self.fresh();
                self.emit(Instr::Local {
                    dst: slot,
                    ty: Ty::Bool,
                    name: "$sc".to_string(),
                });
                let mk_place = || Place {
                    root: PRoot::Slot(slot),
                    root_ty: Ty::Bool,
                    path: vec![],
                    ty: Ty::Bool,
                };
                let av = self.expr(a, Some(&Ty::Bool));
                let run = self.new_label("sc.run");
                let skip = self.new_label("sc.skip");
                let done = self.new_label("sc.done");
                // `&&` runs b when a holds; `||` runs it when a fails.
                if *op == BinOp::LAnd {
                    self.seal(Term::Br(av, run.clone(), skip.clone()));
                } else {
                    self.seal(Term::Br(av, skip.clone(), run.clone()));
                }
                self.open(run);
                // N46 — `b` in `a && b` runs only when `a` held: its
                // comparisons dominate this block. (`a || b`'s rhs
                // runs when `a` FAILED — nothing to assume.)
                let bmark = (self.bfacts.len(), self.efacts.len());
                if *op == BinOp::LAnd {
                    self.cond_facts(a);
                }
                let bv = self.expr(b, Some(&Ty::Bool));
                self.bfacts.truncate(bmark.0);
                self.efacts.truncate(bmark.1);
                self.emit(Instr::Store {
                    place: mk_place(),
                    val: bv,
                });
                self.seal(Term::Jmp(done.clone()));
                self.open(skip);
                self.emit(Instr::Store {
                    place: mk_place(),
                    val: Opnd::Imm(if *op == BinOp::LAnd { 0 } else { 1 }),
                });
                self.seal(Term::Jmp(done.clone()));
                self.open(done);
                let dst = self.fresh();
                self.emit(Instr::Load {
                    dst,
                    place: mk_place(),
                });
                Opnd::R(dst)
            }
            Expr::Bin(op, a, b, _) => {
                // `str == str` / `!=` — bytes compare through the
                // runtime (len check + memcmp), not pointer identity.
                if matches!(op, BinOp::Eq | BinOp::Ne)
                    && (self.infer(a, None) == Ty::Str || self.infer(b, None) == Ty::Str)
                {
                    let av = self.expr(a, Some(&Ty::Str));
                    let bv = self.expr(b, Some(&Ty::Str));
                    let (pa, la) = self.str_parts(av);
                    let (pb, lb) = self.str_parts(bv);
                    let eq = self.rt_call(
                        "mml_str_eq",
                        Ty::I32,
                        vec![
                            (Self::ptr_ty(), pa),
                            (Ty::I64, la),
                            (Self::ptr_ty(), pb),
                            (Ty::I64, lb),
                        ],
                    );
                    let dst = self.fresh();
                    self.emit(Instr::Bin {
                        dst,
                        op: if *op == BinOp::Eq {
                            BinOp::Ne
                        } else {
                            BinOp::Eq
                        },
                        ty: Ty::I32,
                        a: eq,
                        b: Opnd::Imm(0),
                    });
                    return Opnd::R(dst);
                }
                let t = self.infer(e, want);
                let oty = if op.is_cmp() {
                    // Operand type: the non-literal side, else i32.
                    let ta = self.infer(a, None);
                    let tb = self.infer(b, None);
                    match (matches!(&**a, Expr::Int(..)), matches!(&**b, Expr::Int(..))) {
                        (true, false) => tb,
                        (false, true) => ta,
                        _ => ta,
                    }
                } else {
                    t
                };
                let va = self.expr(a, Some(&oty));
                let vb = self.expr(b, Some(&oty));
                if matches!(op, BinOp::Shl | BinOp::Shr) {
                    // N25A — shift amount ≥ width traps (no C-style
                    // masking); constants were already checked.
                    let ok = matches!(&**b, Expr::Int(k, _) if *k >= 0 && (*k as u32) < oty.bits());
                    if !ok {
                        let amt = self.i64v(vb.clone(), &oty);
                        self.rt_call(
                            "mml_shift_check",
                            Ty::Void,
                            vec![(Ty::I64, amt), (Ty::I64, Opnd::Imm(oty.bits() as i64))],
                        );
                    }
                }
                let dst = self.fresh();
                self.emit(Instr::Bin {
                    dst,
                    op: *op,
                    ty: oty,
                    a: va,
                    b: vb,
                });
                Opnd::R(dst)
            }
            Expr::Call(n, _tg, args, _) if crate::semantics::check::is_mutex_op(n) && !self.local_fns.contains(n) => {
                // Real machine lock (N13): `mutex T` is a ptr to the
                // runtime object; the guard value IS that pointer.
                if n == "unlock" {
                    // Explicit release — the slot drops out of the
                    // scope-exit set so it isn't unlocked twice.
                    if let Expr::Var(gn, _) = &args[0] {
                        if let Some((gs, Ty::Guard(_), _)) = self.lookup(gn) {
                            self.open_guards.remove(&gs);
                        }
                    }
                    let v = self.expr(&args[0], None);
                    self.rt_call(
                        "mml_mutex_unlock",
                        Ty::Void,
                        vec![(Self::ptr_ty(), v)],
                    );
                    return Opnd::Imm(0);
                }
                let t0 = self
                    .place_shallow(&args[0])
                    .unwrap_or_else(|| self.infer(&args[0], None));
                let pl = match &t0 {
                    // `lock(h)` / `lock(&mut m)` — the referent's cell
                    // holds the object pointer.
                    Ty::Ref { .. } | Ty::Handle(_) => self.place_ptr(&args[0]),
                    // `lock(m)` — the binding's cell holds it.
                    Ty::Mutex(_) => self.place(&args[0]),
                    _ => None,
                };
                let Some(pl) = pl else {
                    return Opnd::Imm(0);
                };
                let base = self.load_place(pl);
                self.rt_call(
                    "mml_mutex_lock",
                    Self::ptr_ty(),
                    vec![(Self::ptr_ty(), base)],
                )
            }
            Expr::Mutex(i, _) => {
                if matches!(self.infer(i, None), Ty::Handle(_)) {
                    // `mutex h` — wrap the referent's storage; the
                    // domain object points at it, no copy.
                    let hp = self.expr(i, None);
                    self.rt_call(
                        "mml_mutex_wrap",
                        Self::ptr_ty(),
                        vec![(Self::ptr_ty(), hp)],
                    )
                } else {
                    // `mutex v` — runtime object with inline payload.
                    let it = self.infer(i, None);
                    let v = self.expr(i, Some(&it));
                    let sz = self.const_of(&Ty::I64, self.size_of(&it));
                    let base = self.rt_call(
                        "mml_mutex_new",
                        Self::ptr_ty(),
                        vec![(Ty::I64, sz)],
                    );
                    let d = self.rt_call(
                        "mml_mutex_data",
                        Self::ptr_ty(),
                        vec![(Self::ptr_ty(), base.clone())],
                    );
                    if let Opnd::R(dr) = d {
                        self.emit(Instr::Store {
                            place: Place {
                                root: PRoot::Val(dr),
                                root_ty: it.clone(),
                                path: Vec::new(),
                                ty: it,
                            },
                            val: v,
                        });
                    }
                    base
                }
            }
            Expr::Call(n, _tg, args, _) if n == "channel" && !self.local_fns.contains(n) => {
                // A runtime queue sized for the declared packet shape
                // (`channel T` — T's size travels in `want`).
                let sz = match want {
                    Some(Ty::Channel(t)) => self.size_of(t),
                    _ => 8,
                };
                let szv = self.const_of(&Ty::I64, sz.max(8));
                self.rt_call(
                    "mml_chan_new",
                    Self::ptr_ty(),
                    vec![(Ty::I64, szv)],
                )
            }
            Expr::Call(n, _tg, args, _) if crate::semantics::check::is_chan_op(n) && !self.local_fns.contains(n) => {
                match n.as_str() {
                    "send" => {
                        // The runtime copies the packet out of a
                        // materialized cell — transport, not alias.
                        let Some(ep) = self.chan_cell(&args[0]) else {
                            return Opnd::Imm(0);
                        };
                        let ch = self.load_place(ep);
                        // The packet cell is the channel's DECLARED
                        // payload type — the runtime copies its esz
                        // bytes, so the cell's width must match the
                        // queue's, not the argument expression's.
                        let mut vt = self.infer(&args[1], None);
                        let mut et = self.infer(&args[0], None);
                        loop {
                            match et {
                                Ty::Channel(i) => {
                                    if *i != Ty::Void {
                                        vt = *i;
                                    }
                                    break;
                                }
                                Ty::Handle(e) | Ty::Ref { elem: e, .. } => et = *e,
                                _ => break,
                            }
                        }
                        let v = self.expr(&args[1], Some(&vt));
                        let cell = self.fresh();
                        self.emit(Instr::Local {
                            dst: cell,
                            ty: vt.clone(),
                            name: "$pkt".to_string(),
                        });
                        self.emit(Instr::Store {
                            place: Place {
                                root: PRoot::Slot(cell),
                                root_ty: vt.clone(),
                                path: Vec::new(),
                                ty: vt.clone(),
                            },
                            val: v,
                        });
                        let cp = self.fresh();
                        self.emit(Instr::Addr {
                            dst: cp,
                            place: Place {
                                root: PRoot::Slot(cell),
                                root_ty: vt.clone(),
                                path: Vec::new(),
                                ty: vt,
                            },
                        });
                        self.rt_call(
                            "mml_chan_send",
                            Ty::Void,
                            vec![(Self::ptr_ty(), ch), (Self::ptr_ty(), Opnd::R(cp))],
                        );
                        Opnd::Imm(0)
                    }
                    "recv" => {
                        // Blocking recv into an out-cell.
                        let Some(ep) = self.chan_cell(&args[0]) else {
                            return Opnd::Imm(0);
                        };
                        let ch = self.load_place(ep);
                        let mut pt = Ty::I64;
                        let mut et = self.infer(&args[0], None);
                        loop {
                            match et {
                                Ty::Channel(i) => {
                                    pt = *i;
                                    break;
                                }
                                Ty::Handle(e) | Ty::Ref { elem: e, .. } => et = *e,
                                _ => break,
                            }
                        }
                        if pt == Ty::Void {
                            pt = Ty::I64;
                        }
                        let cell = self.fresh();
                        self.emit(Instr::Local {
                            dst: cell,
                            ty: pt.clone(),
                            name: "$pkt".to_string(),
                        });
                        let cp = self.fresh();
                        self.emit(Instr::Addr {
                            dst: cp,
                            place: Place {
                                root: PRoot::Slot(cell),
                                root_ty: pt.clone(),
                                path: Vec::new(),
                                ty: pt.clone(),
                            },
                        });
                        self.rt_call(
                            "mml_chan_recv",
                            Ty::Void,
                            vec![(Self::ptr_ty(), ch), (Self::ptr_ty(), Opnd::R(cp))],
                        );
                        self.load_place(Place {
                            root: PRoot::Slot(cell),
                            root_ty: pt.clone(),
                            path: Vec::new(),
                            ty: pt,
                        })
                    }
                    _ => {
                        // `close` — flag + wake blocked receivers.
                        let Some(ep) = self.chan_cell(&args[0]) else {
                            return Opnd::Imm(0);
                        };
                        let ch = self.load_place(ep);
                        self.rt_call(
                            "mml_chan_close",
                            Ty::Void,
                            vec![(Self::ptr_ty(), ch)],
                        );
                        Opnd::Imm(0)
                    }
                }
            }
            Expr::Call(n, _tg, args, _) if crate::semantics::check::is_atomic_op(n) && !self.local_fns.contains(n) => {
                // The first arg names atomic storage; ref/handle args
                // mean the REFERENT (the checker resolved it already).
                let t0 = self.infer(&args[0], None);
                let pl = match &t0 {
                    Ty::Ref { .. } | Ty::Handle(_) => self.place_ptr(&args[0]),
                    _ => self.place(&args[0]),
                };
                let Some(pl) = pl else { return Opnd::Imm(0) };
                match n.as_str() {
                    "atomic_load" => {
                        let dst = self.fresh();
                        self.emit(Instr::AtomicLoad { dst, place: pl });
                        Opnd::R(dst)
                    }
                    "atomic_store" => {
                        let v = self.expr(&args[1], Some(&pl.ty));
                        self.emit(Instr::AtomicStore { place: pl, val: v });
                        self.bkill(&args[0]);
                        Opnd::Imm(0)
                    }
                    "atomic_add" => {
                        let v = self.expr(&args[1], Some(&pl.ty));
                        let dst = self.fresh();
                        self.emit(Instr::AtomicAdd { dst, place: pl, val: v });
                        self.bkill(&args[0]);
                        Opnd::R(dst)
                    }
                    _ => {
                        let e = self.expr(&args[1], Some(&pl.ty));
                        let nw = self.expr(&args[2], Some(&pl.ty));
                        let dst = self.fresh();
                        self.emit(Instr::AtomicCas {
                            dst,
                            place: pl,
                            expected: e,
                            new: nw,
                        });
                        self.bkill(&args[0]);
                        Opnd::R(dst)
                    }
                }
            }
            Expr::Call(n, _tg, args, _) if n == "slice" && !self.local_fns.contains(n) => {
                // `slice(p, n)` — the trusted foreign-storage
                // assertion: wrap the raw pointer + length.
                let (Some(p), Some(l)) = (args.first(), args.get(1)) else {
                    return Opnd::Imm(0);
                };
                let (elem, mutable) = match self.infer(p, None) {
                    Ty::Ptr { mutable, elem } => (*elem, mutable),
                    _ => (Ty::Void, false),
                };
                let pv = self.expr(p, None);
                let it = self.infer(l, None);
                let lv0 = self.expr(l, Some(&it));
                let lv = self.i64v(lv0, &it);
                self.make_slice(pv, lv, elem, mutable)
            }
            Expr::Call(n, _tg, args, _) if n == "cstr" && !self.local_fns.contains(n) => {
                // `cstr(s)` — the literal's own address, or field 0 of
                // the str aggregate. The checker already proved the
                // storage carries a NUL.
                let Some(a) = args.first() else {
                    return Opnd::Imm(0);
                };
                if matches!(a, Expr::Str(..)) {
                    self.expr(a, None)
                } else {
                    let v = self.expr(a, Some(&Ty::Str));
                    self.str_parts(v).0
                }
            }
            Expr::Call(n, _tg, args, _) if n == "str_from_utf8" && !self.local_fns.contains(n) => {
                // `str_from_utf8(b)` — validate, then re-tag the same
                // {ptr, len} as `str`.
                let Some(b) = args.first() else {
                    return Opnd::Imm(0);
                };
                let v = self.expr(b, None);
                let (p, l) = self.str_parts(v);
                self.rt_call(
                    "mml_str_validate",
                    Ty::Void,
                    vec![(Self::ptr_ty(), p.clone()), (Ty::I64, l.clone())],
                );
                self.make_str(p, l)
            }
            Expr::Call(n, ..) if n == "unreachable" && !self.local_fns.contains(n) => {
                // `unreachable()` — N32 termination primitive: seal
                // the block `unreachable`; whatever follows lands in
                // the dead-block machinery like post-`ret` code.
                self.seal(Term::Dead);
                Opnd::Imm(0)
            }
            // A bare `a[lo..hi]` — the checker rejects; keep emitting.
            Expr::IndexRange { .. } => Opnd::Imm(0),
            Expr::Call(n, ..) if n == "system_allocator" && !self.local_fns.contains(n) => {
                // The allocator token is a runtime pointer — NULL is
                // the system heap.
                self.const_of(&Ty::Alloc, 0)
            }
            Expr::Call(n, _tg, args, _)
                if crate::semantics::check::int_cast_ty(n).is_some()
                    && !self.local_fns.contains(n) =>
            {
                // `u64(x)`/`i32(x)` — the checker proved the arg is
                // an int; emit sext/zext/trunc by width+signedness
                // (Instr::Cast lowers equal widths to a copy).
                let ty = crate::semantics::check::int_cast_ty(n).unwrap();
                let Some(a) = args.first() else {
                    return Opnd::Imm(0);
                };
                let from = self.infer(a, None);
                let v = self.expr(a, Some(&from));
                if from == ty {
                    return v;
                }
                let dst = self.fresh();
                self.emit(Instr::Cast {
                    dst,
                    ty,
                    from,
                    a: v,
                });
                Opnd::R(dst)
            }
            Expr::Call(n, _tg, args, _)
                if n == "free" && !self.local_fns.contains(n) =>
            {
                // `free(p)` — the checker consumed the root's Own
                // authority; here we just hand the data pointer to
                // the runtime. A user-declared `free` (e.g. extern
                // `free(*void)`) is an ordinary call below.
                let Some(a) = args.first() else {
                    return Opnd::Imm(0);
                };
                let t = self.infer(a, None);
                let v = self.expr(a, Some(&t));
                let ptr = match &t {
                    Ty::BoxSlice(_) => self.str_parts(v).0,
                    _ => v,
                };
                self.rt_call(
                    "mml_free",
                    Ty::Void,
                    vec![(Self::ptr_ty(), ptr)],
                );
                Opnd::Imm(0)
            }
            // `replace(place, v)` — load the old value into a temp,
            // store the new one: the exchange is two ordinary
            // memory ops, no hidden allocation. The checker already
            // proved the place is writable and both types match.
            Expr::Call(n, _tg, args, _)
                if n == "replace" && !self.local_fns.contains(n) =>
            {
                let Some(pl) = args.first().and_then(|a| self.place(a)) else {
                    return Opnd::Imm(0);
                };
                let ty = pl.ty.clone();
                let v = args
                    .get(1)
                    .map(|a| self.expr(a, Some(&ty)))
                    .unwrap_or(Opnd::Imm(0));
                let old = self.load_place(pl.clone());
                self.emit(Instr::Store { place: pl, val: v });
                // N46 — the store may move a bound fact's place.
                self.bkill(&args[0]);
                old
            }
            // `swap(a, b)` — both loads before either store: the temp
            // is a register, never a second owner.
            Expr::Call(n, _tg, args, _)
                if n == "swap" && !self.local_fns.contains(n) =>
            {
                let (Some(pa), Some(pb)) = (
                    args.first().and_then(|a| self.place(a)),
                    args.get(1).and_then(|a| self.place(a)),
                ) else {
                    return Opnd::Imm(0);
                };
                let va = self.load_place(pa.clone());
                let vb = self.load_place(pb.clone());
                self.emit(Instr::Store {
                    place: pa,
                    val: vb,
                });
                self.emit(Instr::Store {
                    place: pb,
                    val: va,
                });
                // N46 — both stores may move bound facts' places.
                self.bkill(&args[0]);
                self.bkill(&args[1]);
                Opnd::Imm(0)
            }
            Expr::Alloc { ty, alloc, count, .. } => {
                // Evaluate the allocator token (its value is unused —
                // all allocations go through `mml_alloc`).
                let _ = self.expr(alloc, Some(&Ty::Alloc));
                let sz = self.size_of(ty);
                let szv = Opnd::Imm(sz.max(1));
                match count {
                    None => self.rt_call(
                        "mml_alloc",
                        Self::ptr_ty(),
                        vec![(Ty::I64, szv)],
                    ),
                    Some(c) => {
                        let it = self.infer(c, Some(&Ty::I64));
                        let iv = self.expr(c, Some(&it));
                        let nv = self.i64v(iv, &it);
                        let p = self.rt_call(
                            "mml_alloc_slice",
                            Self::ptr_ty(),
                            vec![(Ty::I64, szv), (Ty::I64, nv.clone())],
                        );
                        self.make_agg2(Ty::BoxSlice(ty.clone()), p, nv)
                    }
                }
            }
            Expr::Call(n, _tg, args, _) => {
                let (params, ret, _) = self
                    .fns
                    .get(n)
                    .cloned()
                    .unwrap_or_else(|| (Vec::new(), Ty::Void, false));
                let mut argv = Vec::new();
                for (i, a) in args.iter().enumerate() {
                    let (auth, at) = params
                        .get(i)
                        .map(|(a, t)| (*a, t.clone()))
                        .unwrap_or((None, self.infer(a, None)));
                    let v = match auth {
                        // Place alias: hand over the address, not a copy.
                        Some(Auth::Read) | Some(Auth::Write) => {
                            let pl = self.place(a).unwrap_or(Place {
                                root: PRoot::Val(Reg(u32::MAX)),
                                root_ty: Ty::Void,
                                path: Vec::new(),
                                ty: Ty::Void,
                            });
                            let dst = self.fresh();
                            self.emit(Instr::Addr { dst, place: pl });
                            argv.push((
                                Ty::Ref {
                                    mutable: auth == Some(Auth::Write),
                                    elem: Box::new(at),
                                },
                                Opnd::R(dst),
                            ));
                            continue;
                        }
                        _ => self.expr(a, Some(&at)),
                    };
                    argv.push((at, v));
                }
                let dst = if ret == Ty::Void {
                    None
                } else {
                    Some(self.fresh())
                };
                self.emit(Instr::Call {
                    dst,
                    ret,
                    f: n.clone(),
                    args: argv,
                });
                // N46 — the call may write through `write` params or a
                // `&mut` operand: bound facts on those places die here.
                for (i, a) in args.iter().enumerate() {
                    let mutates = params
                        .get(i)
                        .and_then(|(au, _)| *au)
                        .is_some_and(|au| au == Auth::Write)
                        || expr_has_mut_addr(a);
                    if mutates {
                        self.bkill(a);
                    }
                }
                dst.map(Opnd::R).unwrap_or(Opnd::Imm(0))
            }
        }
    }
}

/// Free-variable walk for `spawn` bodies (N13): collect `Var` names
/// used but not bound inside the body — those become env captures.
/// Let-bindings shadow from their point of declaration; nested blocks
/// scope their own; nested spawns resolve against THIS body's env
/// (their captures are captured here transitively).
fn fv_stmts(stmts: &[Stmt], bound: &mut HashSet<String>, out: &mut Vec<String>) {
    for s in stmts {
        match s {
            Stmt::Let { name, init, .. } => {
                if let Some(i) = init {
                    fv_expr(i, bound, out);
                }
                bound.insert(name.clone());
            }
            Stmt::Assign { target, value, .. } => {
                fv_expr(target, bound, out);
                fv_expr(value, bound, out);
            }
            Stmt::Return { value, .. } => {
                if let Some(v) = value {
                    fv_expr(v, bound, out);
                }
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                ..
            } => {
                fv_expr(cond, bound, out);
                fv_stmts(then_body, &mut bound.clone(), out);
                if let Some(eb) = else_body {
                    fv_stmts(eb, &mut bound.clone(), out);
                }
            }
            Stmt::While { cond, body, .. } => {
                fv_expr(cond, bound, out);
                fv_stmts(body, &mut bound.clone(), out);
            }
            Stmt::Match { scrut, arms, .. } => {
                fv_expr(scrut, bound, out);
                for a in arms {
                    let mut b2 = bound.clone();
                    if let Some(bn) = &a.bind {
                        b2.insert(bn.clone());
                    }
                    fv_stmts(&a.body, &mut b2, out);
                }
            }
            Stmt::Region { body, .. } => fv_stmts(body, &mut bound.clone(), out),
            Stmt::Promote { target, .. } | Stmt::Consume { target, .. } => {
                fv_expr(target, bound, out)
            }
            Stmt::Defer { expr, .. } => fv_expr(expr, bound, out),
            Stmt::Break { .. } | Stmt::Continue { .. } => {}
            Stmt::Expr { expr, .. } => fv_expr(expr, bound, out),
        }
    }
}

fn fv_expr(e: &Expr, bound: &HashSet<String>, out: &mut Vec<String>) {
    match e {
        Expr::Var(n, _) => {
            if !bound.contains(n) && !out.contains(n) {
                out.push(n.clone());
            }
        }
        Expr::Int(..) | Expr::Bool(..) | Expr::Str(..) | Expr::Sizeof(..) => {}
        Expr::MethodCall { recv, args, .. } => {
            fv_expr(recv, bound, out);
            for a in args {
                fv_expr(a, bound, out);
            }
        }
        Expr::AssocFn { args, .. } => {
            for a in args {
                fv_expr(a, bound, out);
            }
        }
        Expr::Unary(_, i, _)
        | Expr::Field(i, _, _)
        | Expr::Deref(i, _)
        | Expr::Handle(i, _)
        | Expr::Mutex(i, _)
        | Expr::Join(i, _) => fv_expr(i, bound, out),
        Expr::Bin(_, a, b, _) | Expr::Index(a, b, _) => {
            fv_expr(a, bound, out);
            fv_expr(b, bound, out);
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            fv_expr(base, bound, out);
            if let Some(e) = lo {
                fv_expr(e, bound, out);
            }
            if let Some(e) = hi {
                fv_expr(e, bound, out);
            }
        }
        Expr::Call(_, _tg, args, _) => {
            for a in args {
                fv_expr(a, bound, out);
            }
        }
        Expr::StructLit(_, _tg, fields, _) => {
            for (_, fe) in fields {
                fv_expr(fe, bound, out);
            }
        }
        Expr::EnumLit { payload, .. } => {
            if let Some(p) = payload {
                fv_expr(p, bound, out);
            }
        }
        Expr::Try { inner, .. } => fv_expr(inner, bound, out),
        Expr::AddrOf { place, .. } => fv_expr(place, bound, out),
        Expr::Alloc { alloc, count, .. } => {
            fv_expr(alloc, bound, out);
            if let Some(c) = count {
                fv_expr(c, bound, out);
            }
        }
        Expr::Spawn(body, _) => fv_stmts(body, &mut bound.clone(), out),
        Expr::Match { scrut, arms, .. } => {
            fv_expr(scrut, bound, out);
            for a in arms {
                let mut b2 = bound.clone();
                if let Some(bn) = &a.bind {
                    b2.insert(bn.clone());
                }
                fv_stmts(&a.body, &mut b2, out);
            }
        }
    }
}

/// N46 — does `e` contain a `*p` deref anywhere? A store through an
/// unknown pointer can hit any place, so it clears all bound facts.
fn expr_has_deref(e: &Expr) -> bool {
    match e {
        Expr::Deref(..) => true,
        Expr::Var(..) | Expr::Int(..) | Expr::Bool(..) | Expr::Str(..)
        | Expr::Sizeof(..) => false,
        Expr::MethodCall { recv, args, .. } => {
            expr_has_deref(recv) || args.iter().any(expr_has_deref)
        }
        Expr::AssocFn { args, .. } | Expr::Call(_, _, args, _) => {
            args.iter().any(expr_has_deref)
        }
        Expr::Unary(_, i, _)
        | Expr::Field(i, _, _)
        | Expr::Handle(i, _)
        | Expr::Mutex(i, _)
        | Expr::Join(i, _)
        | Expr::Try { inner: i, .. } => expr_has_deref(i),
        Expr::Bin(_, a, b, _) | Expr::Index(a, b, _) => {
            expr_has_deref(a) || expr_has_deref(b)
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            expr_has_deref(base)
                || lo.as_deref().is_some_and(expr_has_deref)
                || hi.as_deref().is_some_and(expr_has_deref)
        }
        Expr::StructLit(_, _, fields, _) => {
            fields.iter().any(|(_, fe)| expr_has_deref(fe))
        }
        Expr::EnumLit { payload, .. } => {
            payload.as_deref().is_some_and(expr_has_deref)
        }
        Expr::AddrOf { place, .. } => expr_has_deref(place),
        Expr::Alloc { alloc, count, .. } => {
            expr_has_deref(alloc) || count.as_deref().is_some_and(expr_has_deref)
        }
        // A `spawn`/`match` operand can't be a store target.
        Expr::Spawn(..) | Expr::Match { .. } => false,
    }
}

/// N46 — does `e` hand out a `&mut` of some place? The callee can
/// write through it, so the referent's bound facts die at the call.
fn expr_has_mut_addr(e: &Expr) -> bool {
    match e {
        Expr::AddrOf { mutable, place, .. } => {
            *mutable || expr_has_mut_addr(place)
        }
        Expr::Var(..) | Expr::Int(..) | Expr::Bool(..) | Expr::Str(..)
        | Expr::Sizeof(..) | Expr::Spawn(..) => false,
        Expr::MethodCall { recv, args, .. } => {
            expr_has_mut_addr(recv) || args.iter().any(expr_has_mut_addr)
        }
        Expr::AssocFn { args, .. } | Expr::Call(_, _, args, _) => {
            args.iter().any(expr_has_mut_addr)
        }
        Expr::Unary(_, i, _)
        | Expr::Field(i, _, _)
        | Expr::Deref(i, _)
        | Expr::Handle(i, _)
        | Expr::Mutex(i, _)
        | Expr::Join(i, _)
        | Expr::Try { inner: i, .. } => expr_has_mut_addr(i),
        Expr::Bin(_, a, b, _) | Expr::Index(a, b, _) => {
            expr_has_mut_addr(a) || expr_has_mut_addr(b)
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            expr_has_mut_addr(base)
                || lo.as_deref().is_some_and(expr_has_mut_addr)
                || hi.as_deref().is_some_and(expr_has_mut_addr)
        }
        Expr::StructLit(_, _, fields, _) => {
            fields.iter().any(|(_, fe)| expr_has_mut_addr(fe))
        }
        Expr::EnumLit { payload, .. } => {
            payload.as_deref().is_some_and(expr_has_mut_addr)
        }
        Expr::Alloc { alloc, count, .. } => {
            expr_has_mut_addr(alloc) || count.as_deref().is_some_and(expr_has_mut_addr)
        }
        Expr::Match { scrut, arms, .. } => {
            expr_has_mut_addr(scrut)
                || arms.iter().any(|a| {
                    a.body.iter().any(|s| stmt_has_mut_addr(s))
                })
        }
    }
}

/// N46 — stmt-level counterpart of `expr_has_mut_addr`.
fn stmt_has_mut_addr(s: &Stmt) -> bool {
    match s {
        Stmt::Let { init, .. } | Stmt::Return { value: init, .. } => {
            init.as_ref().is_some_and(expr_has_mut_addr)
        }
        Stmt::Assign { target, value, .. } => {
            expr_has_mut_addr(target) || expr_has_mut_addr(value)
        }
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            expr_has_mut_addr(cond)
                || then_body.iter().any(stmt_has_mut_addr)
                || else_body
                    .as_ref()
                    .is_some_and(|b| b.iter().any(stmt_has_mut_addr))
        }
        Stmt::While { cond, body, .. } => {
            expr_has_mut_addr(cond) || body.iter().any(stmt_has_mut_addr)
        }
        Stmt::Region { body, .. } => body.iter().any(stmt_has_mut_addr),
        Stmt::Match { scrut, arms, .. } => {
            expr_has_mut_addr(scrut)
                || arms.iter().any(|a| a.body.iter().any(stmt_has_mut_addr))
        }
        Stmt::Expr { expr, .. } | Stmt::Consume { target: expr, .. }
        | Stmt::Promote { target: expr, .. } | Stmt::Defer { expr, .. } => {
            expr_has_mut_addr(expr)
        }
        Stmt::Break { .. } | Stmt::Continue { .. } => false,
    }
}
