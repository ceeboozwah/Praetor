//! Our own IR — the seam between typed AST and LLVM. Deliberately
//! honest about representation: *named* locals are memory slots
//! (`local`/`load`/`store`), *anonymous* temporaries are SSA values.
//! That mirrors the language's philosophy: a variable is a location;
//! an expression result is a value. Regions, capabilities, and
//! authority effects will later ride on this same instruction stream.
//!
//! ```text
//! fn main() -> i32:
//! entry:
//!     x = local.i32
//!     v0 = const.i32 40
//!     v1 = const.i32 2
//!     v2 = add.i32 v0, v1
//!     store.i32 x, v2
//!     v3 = load.i32 x
//!     ret.i32 v3
//! ```

use crate::frontend::ast::{BinOp, Ty, UnOp};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Reg(pub u32);

#[derive(Debug)]
pub struct Module {
    /// Struct layouts — the backend emits `%Name = type { .. }` and
    /// resolves field paths against this table.
    pub structs: Vec<StructDef>,
    pub fns: Vec<Func>,
    /// C-ABI declarations (N14) — `declare`s, no bodies.
    pub externs: Vec<ExternDecl>,
    /// String literals — each becomes a null-terminated private
    /// global `@.str.N`; `StrAddr` refs index this table.
    pub strings: Vec<Vec<u8>>,
}

#[derive(Debug)]
pub struct ExternDecl {
    pub name: String,
    /// Param types only — externs never carry capability contracts.
    pub params: Vec<Ty>,
    pub ret: Ty,
}

#[derive(Debug)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<(String, Ty)>,
}

#[derive(Debug)]
pub struct Func {
    pub name: String,
    /// (name, capability authority, type) — read/write params arrive
    /// as `ptr` place-aliases; own/plain params as values.
    pub params: Vec<(String, Option<crate::frontend::ast::Auth>, Ty)>,
    /// The SSA regs carrying incoming argument values — params arrive as
    /// values and are stored into their `local` slots at entry.
    pub arg_regs: Vec<Reg>,
    pub ret: Ty,
    pub blocks: Vec<Block>,
}

#[derive(Debug)]
pub struct Block {
    pub label: String,
    pub instrs: Vec<Instr>,
    pub term: Term,
}

#[derive(Debug)]
pub enum Instr {
    /// Marks `dst` as the incoming SSA value of parameter `idx`.
    Arg { dst: Reg, ty: Ty, idx: u32 },
    /// A named local's memory slot (the physical-transparency point).
    Local { dst: Reg, ty: Ty, name: String },
    Const { dst: Reg, ty: Ty, val: i64 },
    /// Load the leaf value at `place` (root slot + field/index path).
    Load { dst: Reg, place: Place },
    /// Store `val` into `place`.
    Store { place: Place, val: Opnd },
    /// `load atomic` — one synchronized read. seq_cst until memory
    /// orderings get their own round.
    AtomicLoad { dst: Reg, place: Place },
    /// `store atomic`.
    AtomicStore { place: Place, val: Opnd },
    /// `atomicrmw add` — dst gets the OLD value.
    AtomicAdd { dst: Reg, place: Place, val: Opnd },
    /// `cmpxchg` — dst gets the success flag (both arms one access).
    AtomicCas {
        dst: Reg,
        place: Place,
        expected: Opnd,
        new: Opnd,
    },
    /// `&place` — the address of a place as a value. This is where a
    /// reference is born; provenance lives in the checker, the IR just
    /// records which place's address was taken.
    Addr { dst: Reg, place: Place },
    /// `handle <place>` — an identity token for a place. Carries no
    /// borrow authority; the checker has already proven whatever a
    /// later `*h` may do. Backend form is a plain pointer today —
    /// generation indices can replace it without touching semantics.
    MakeHandle { dst: Reg, place: Place },
    Bin { dst: Reg, op: BinOp, ty: Ty, a: Opnd, b: Opnd },
    Unary { dst: Reg, op: UnOp, ty: Ty, a: Opnd },
    Call {
        dst: Option<Reg>,
        ret: Ty,
        f: String,
        args: Vec<(Ty, Opnd)>,
    },
    /// `&@.str.N` — a string literal's address (N14). `len` is the
    /// global's size INCLUDING the null terminator; `idx` is its slot
    /// in `Module::strings`.
    StrAddr { dst: Reg, idx: u32, len: u32 },
    /// Integer width change — `sext`/`zext`/`trunc` picked from the
    /// from/to widths and the source's signedness (N15 slice bounds).
    Cast {
        dst: Reg,
        ty: Ty,
        from: Ty,
        a: Opnd,
    },
    /// `extractvalue` — a slice's ptr/len member out of a VALUE (the
    /// aggregate in a register, e.g. `f().len`).
    Extract {
        dst: Reg,
        ty: Ty,
        agg: Opnd,
        idx: u32,
    },
}

/// An addressable location: a root local slot plus a field/index path.
/// This is the IR's version of the lab's node paths — `p`, `p.next`,
/// `a[i]` are all places, and later semantic layers (alias edges,
/// capabilities, protected fields) attach to exactly these paths.
#[derive(Debug)]
pub struct Place {
    /// Where the path starts: a `local` slot's address (stack storage),
    /// or a loaded pointer value (deref roots — `*r`, `(*r).x`).
    pub root: PRoot,
    /// The type stored in the root slot (the path's starting aggregate).
    pub root_ty: Ty,
    pub path: Vec<PathElem>,
    /// Leaf type — what a load/store on this place moves.
    pub ty: Ty,
}

#[derive(Debug, Clone, Copy)]
pub enum PRoot {
    /// A `local` slot — stack storage belonging to a named variable.
    Slot(Reg),
    /// A pointer value produced by an expression (`*r` paths start at
    /// the loaded reference, not a slot).
    Val(Reg),
}

#[derive(Debug, Clone)]
pub enum PathElem {
    /// `.name` — resolved to a field index at gen time (LLVM wants the
    /// index; the name is kept for diagnostics and later edge keys).
    Field { name: String, idx: u32 },
    /// `[i]` — the index is a runtime value (or an Imm); the operand's
    /// width is whatever its type is (GEP accepts any int width).
    Index { val: Opnd, ty: Ty },
}

#[derive(Debug, Clone)]
pub enum Opnd {
    R(Reg),
    Imm(i64),
    /// A function's address — `ptr @name`. Used to hand synthesized
    /// task bodies to `mml_task_spawn`.
    Fun(String),
}

#[derive(Debug)]
pub enum Term {
    Ret(Option<Opnd>, Ty),
    Br(Opnd, String, String),
    Jmp(String),
    /// Post-`return` code lands here; unreachable from entry.
    Dead,
}

fn reg(r: &Reg) -> String {
    format!("v{}", r.0)
}

fn opnd(o: &Opnd) -> String {
    match o {
        Opnd::R(r) => reg(r),
        Opnd::Imm(n) => format!("{n}"),
        Opnd::Fun(n) => format!("@{n}"),
    }
}

fn place(p: &Place) -> String {
    let mut s = match &p.root {
        PRoot::Slot(r) => reg(r),
        PRoot::Val(r) => format!("*{}", reg(r)),
    };
    for e in &p.path {
        match e {
            PathElem::Field { name, .. } => s.push_str(&format!(".{name}")),
            PathElem::Index { val, .. } => s.push_str(&format!("[{}]", opnd(val))),
        }
    }
    s
}

fn binop_sym(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "add",
        BinOp::Sub => "sub",
        BinOp::Mul => "mul",
        BinOp::Div => "div",
        BinOp::Mod => "mod",
        BinOp::Eq => "eq",
        BinOp::Ne => "ne",
        BinOp::Lt => "lt",
        BinOp::Gt => "gt",
        BinOp::Le => "le",
        BinOp::Ge => "ge",
    }
}

impl fmt::Display for Module {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        for st in &self.structs {
            let fs: Vec<String> = st
                .fields
                .iter()
                .map(|(n, t)| format!("{n}: {t}"))
                .collect();
            writeln!(f, "struct {} {{ {} }}", st.name, fs.join(", "))?;
        }
        if !self.structs.is_empty() {
            writeln!(f)?;
        }
        for x in &self.externs {
            let ps: Vec<String> = x.params.iter().map(|t| format!("{t}")).collect();
            writeln!(f, "extern {}({}) -> {}", x.name, ps.join(", "), x.ret)?;
        }
        if !self.externs.is_empty() {
            writeln!(f)?;
        }
        for fun in &self.fns {
            write!(f, "{}", fun)?;
        }
        Ok(())
    }
}

impl fmt::Display for Func {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        let params: Vec<String> = self
            .params
            .iter()
            .map(|(n, a, t)| {
                let a = match a {
                    Some(crate::frontend::ast::Auth::Read) => "read ",
                    Some(crate::frontend::ast::Auth::Write) => "write ",
                    Some(crate::frontend::ast::Auth::Own) => "own ",
                    Some(crate::frontend::ast::Auth::Atomic) => "atomic ",
                    Some(crate::frontend::ast::Auth::Sync) => "sync ",
                    None => "",
                };
                format!("{a}{n}: {t}")
            })
            .collect();
        writeln!(f, "fn {}({}) -> {}:", self.name, params.join(", "), self.ret)?;
        for b in &self.blocks {
            writeln!(f, "{}:", b.label)?;
            for i in &b.instrs {
                writeln!(f, "    {}", i)?;
            }
            writeln!(f, "    {}", b.term)?;
        }
        Ok(())
    }
}

impl fmt::Display for Instr {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Instr::Arg { dst, ty, idx } => {
                write!(f, "{} = arg.{} #{}", reg(dst), ty, idx)
            }
            Instr::Local { dst, ty, name } => {
                write!(f, "{} = local.{} `{name}`", reg(dst), ty)
            }
            Instr::Const { dst, ty, val } => write!(f, "{} = const.{} {}", reg(dst), ty, val),
            Instr::Load { dst, place: pl } => {
                write!(f, "{} = load.{} {}", reg(dst), pl.ty, place(pl))
            }
            Instr::Store { place: pl, val } => {
                write!(f, "store.{} {}, {}", pl.ty, place(pl), opnd(val))
            }
            Instr::AtomicLoad { dst, place: pl } => {
                write!(f, "{} = aload.{} {}", reg(dst), pl.ty, place(pl))
            }
            Instr::AtomicStore { place: pl, val } => {
                write!(f, "astore.{} {}, {}", pl.ty, place(pl), opnd(val))
            }
            Instr::AtomicAdd { dst, place: pl, val } => {
                write!(f, "{} = armw.add.{} {}, {}", reg(dst), pl.ty, place(pl), opnd(val))
            }
            Instr::AtomicCas {
                dst,
                place: pl,
                expected,
                new,
            } => write!(
                f,
                "{} = cas.{} {}, {}, {}",
                reg(dst),
                pl.ty,
                place(pl),
                opnd(expected),
                opnd(new)
            ),
            Instr::Addr { dst, place: pl } => {
                write!(f, "{} = addr {} (&{})", reg(dst), place(pl), pl.ty)
            }
            Instr::MakeHandle { dst, place: pl } => {
                write!(f, "{} = handle {} (id {})", reg(dst), place(pl), pl.ty)
            }
            Instr::Bin { dst, op, ty, a, b } => write!(
                f,
                "{} = {}.{} {}, {}",
                reg(dst),
                binop_sym(op),
                ty,
                opnd(a),
                opnd(b)
            ),
            Instr::Unary { dst, op, ty, a } => write!(
                f,
                "{} = {}.{} {}",
                reg(dst),
                match op {
                    UnOp::Neg => "neg",
                    UnOp::Not => "not",
                },
                ty,
                opnd(a)
            ),
            Instr::Call { dst, ret, f: name, args } => {
                let as_: Vec<String> = args
                    .iter()
                    .map(|(t, o)| format!("{t} {}", opnd(o)))
                    .collect();
                match dst {
                    Some(d) => write!(f, "{} = call.{} {}({})", reg(d), ret, name, as_.join(", ")),
                    None => write!(f, "call.{} {}({})", ret, name, as_.join(", ")),
                }
            }
            Instr::StrAddr { dst, idx, .. } => {
                write!(f, "{} = straddr @.str.{}", reg(dst), idx)
            }
            Instr::Cast { dst, ty, from, a } => {
                write!(f, "{} = cast.{} {} -> {}", reg(dst), from, opnd(a), ty)
            }
            Instr::Extract { dst, ty, agg, idx } => {
                write!(f, "{} = extract.{} {}[{}]", reg(dst), ty, opnd(agg), idx)
            }
        }
    }
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Term::Ret(Some(v), ty) => write!(f, "ret.{} {}", ty, opnd(v)),
            Term::Ret(None, _) => write!(f, "ret.void"),
            Term::Br(c, t, e) => write!(f, "br {} -> {}, {}", opnd(c), t, e),
            Term::Jmp(l) => write!(f, "jmp {}", l),
            Term::Dead => write!(f, "dead"),
        }
    }
}
