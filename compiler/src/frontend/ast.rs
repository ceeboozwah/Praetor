//! The `.lang` AST — the small native-language surface, not the lab's
//! `.mml` grammar. Memory types get added one at a time.

use crate::Span;
use std::fmt;

/// Machine-width integers plus bool, plus value-aggregates: named
/// structs and fixed-size arrays. Sign is a property of the TYPE —
/// the same bit pattern, different operations (sdiv vs udiv).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    U8,
    U16,
    U32,
    U64,
    I8,
    I16,
    I32,
    I64,
    Bool,
    Void,
    /// A named struct — layout lives in the program's struct table.
    /// Generic INSTANCES canonicalize into this too: `Vec<i32>` is
    /// `Struct("vec.Vec<i32>")` after monomorphization (N19).
    Struct(String),
    /// `Name<args>` — an uninstantiated generic application. Exists
    /// only between parse and monomorphization: the instantiate pass
    /// rewrites every App to its canonical `Struct("Name<a,b>")` key
    /// (or substitutes it away inside a generic body). Never reaches
    /// the checker.
    App(String, Vec<Ty>),
    /// `[n]T` — fixed size, embedded by value.
    Array(Box<Ty>, u32),
    /// `&T` / `&mut T` — a reference carrying region provenance.
    Ref { mutable: bool, elem: Box<Ty> },
    /// `handle T` — identity only: no authority, no liveness keeping.
    /// Runtime form is a pointer today; generation checks later.
    Handle(Box<Ty>),
    /// `task T` — a spawned task token: holds the child's authority
    /// claims + suspended effects; `join` applies them and yields T.
    Task(Box<Ty>),
    /// `atomic T` — a storage class, not a representation: same bits
    /// as `T`, but in a concurrency window only the atomic builtins
    /// may touch it. Sequential plain access stays legal.
    Atomic(Box<Ty>),
    /// `mutex T` — T stored behind a synchronization domain. The
    /// interior is reachable only through a live `lock` guard; the
    /// mutex object itself is the domain handle (N11).
    Mutex(Box<Ty>),
    /// `guard T` — a lock's token: affine, scope-bound authority over
    /// the mutex's protected interior. Inferred only; never declared.
    Guard(Box<Ty>),
    /// `channel T` — an endpoint that transports authority packets:
    /// `send` cuts authority out of the sender's graph, `recv` installs
    /// it in the receiver's, `close` consumes the endpoint. `channel()`
    /// with no annotation yields `channel void` — an opaque endpoint
    /// that accepts any payload (N12).
    Channel(Box<Ty>),
    /// `*T` / `*const T` — a raw C pointer (N14). Opaque in the
    /// language: no deref, no arithmetic — it exists to cross the FFI
    /// boundary. `*void` is C's `void*`. The mutability marks what a
    /// callee may do through it: `*T` claims Write on the referent,
    /// `*const T` claims Read.
    Ptr { mutable: bool, elem: Box<Ty> },
    /// `slice T` / `slice mut T` — a (ptr, len) view over a span of
    /// storage (N15). Born `&a[lo..hi]` / `&mut a[lo..hi]` — the
    /// binding's edge onto the base's place IS its authority, so
    /// `s[i]` resolves through the same borrow rules as `(*r).f`.
    /// `mutable` mirrors `&mut`: `slice` reads; `slice mut` also
    /// writes. `slice(p, n)` asserts foreign storage — trusted like
    /// `handle`, carrying no graph edges.
    Slice { mutable: bool, elem: Box<Ty> },
    /// `str` — an immutable string view (N15B): same {ptr, len}
    /// representation as `slice u8`, plus the string invariant that
    /// the bytes are text. Never writable, so it carries no authority
    /// of its own — but a `str_from_utf8(b)` conversion DOES hold the
    /// slice's Read claim, so the viewed bytes can't be written while
    /// the string lives. `s.len` is the byte count; `s.bytes` re-reads
    /// the view as `slice u8` (byte indexing stays explicit — `s[i]`
    /// would hide the byte/character ambiguity). `cstr(s)` hands
    /// literal-provenance storage to C as `*const u8`.
    Str,
    /// `box T` — an owning pointer to one heap cell (N16). The
    /// binding's own key IS the storage root: `*p` names `p#e`, so a
    /// `&*p` borrow targets the root and `free(p)` — a consume — sees
    /// every live borrow for free. No edges of its own; authority is
    /// intrinsic, like a foreign slice's. Affine: `let q = p`, a call
    /// arg, or `send` moves the root.
    Box(Box<Ty>),
    /// `box slice T` — an owning pointer to a heap SPAN ({ptr, len},
    /// like `slice` but owning). `p[i]`/`p.len`/`p.ptr` mirror slice
    /// members; `&p[lo..hi]` derives an ordinary `slice mut T` view
    /// onto `p#e`. `free(p)` releases the whole span.
    BoxSlice(Box<Ty>),
    /// `alloc` — an allocator token. Opaque capability value: today
    /// only `system_allocator()` produces one and `alloc`/`alloc_slice`
    /// consume it. Carries no authority of its own — it names WHO owns
    /// the cells, not WHICH memory.
    Alloc,
}

impl Ty {
    pub fn is_int(&self) -> bool {
        match self {
            Ty::U8 | Ty::U16 | Ty::U32 | Ty::U64 | Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64 => true,
            // The storage class is transparent to the value's shape.
            Ty::Atomic(e) => e.is_int(),
            _ => false,
        }
    }
    /// Pointer-shaped at the machine level — all lower to `ptr`.
    pub fn is_ptr_like(&self) -> bool {
        matches!(
            self,
            Ty::Ref { .. }
                | Ty::Handle(_)
                | Ty::Guard(_)
                | Ty::Channel(_)
                | Ty::Mutex(_)
                | Ty::Task(_)
                | Ty::Ptr { .. }
                | Ty::Box(_)
                | Ty::Alloc
        )
    }
    /// `atomic T` value-level → `T` (atomicity tags the storage, not
    /// the bits). Everything else maps to itself.
    pub fn unatomic(&self) -> Ty {
        match self {
            Ty::Atomic(e) => e.unatomic(),
            t => t.clone(),
        }
    }
    pub fn is_aggregate(&self) -> bool {
        matches!(self, Ty::Struct(_) | Ty::App(..) | Ty::Array(..))
    }
    pub fn signed(&self) -> bool {
        matches!(self, Ty::I8 | Ty::I16 | Ty::I32 | Ty::I64)
    }
    pub fn bits(&self) -> u32 {
        match self {
            Ty::U8 | Ty::I8 => 8,
            Ty::U16 | Ty::I16 => 16,
            Ty::U32 | Ty::I32 | Ty::Bool => 32,
            Ty::U64 | Ty::I64 => 64,
            Ty::Atomic(e) => e.bits(),
            _ => 0,
        }
    }
    /// The type a bare integer literal defaults to.
    pub const INT_DEFAULT: Ty = Ty::I32;
    /// `i64` bounds of this type's range (literal fitting).
    pub fn range(&self) -> Option<(i64, i64)> {
        Some(match self {
            Ty::Atomic(e) => return e.range(),
            Ty::U8 => (0, u8::MAX as i64),
            Ty::U16 => (0, u16::MAX as i64),
            Ty::U32 => (0, u32::MAX as i64),
            Ty::U64 => (0, i64::MAX),
            Ty::I8 => (i8::MIN as i64, i8::MAX as i64),
            Ty::I16 => (i16::MIN as i64, i16::MAX as i64),
            Ty::I32 => (i32::MIN as i64, i32::MAX as i64),
            Ty::I64 => (i64::MIN, i64::MAX),
            _ => return None,
        })
    }
}

impl fmt::Display for Ty {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        f.write_str(&match self {
            Ty::U8 => "u8".to_string(),
            Ty::U16 => "u16".to_string(),
            Ty::U32 => "u32".to_string(),
            Ty::U64 => "u64".to_string(),
            Ty::I8 => "i8".to_string(),
            Ty::I16 => "i16".to_string(),
            Ty::I32 => "i32".to_string(),
            Ty::I64 => "i64".to_string(),
            Ty::Bool => "bool".to_string(),
            Ty::Void => "void".to_string(),
            Ty::Struct(n) => n.clone(),
            Ty::App(n, args) => {
                let inner: Vec<String> = args.iter().map(|a| format!("{a}")).collect();
                format!("{n}<{}>", inner.join(","))
            }
            Ty::Array(e, n) => return write!(f, "[{n}]{e}"),
            Ty::Ref { mutable, elem } => {
                return write!(f, "&{}{}", if *mutable { "mut " } else { "" }, elem)
            }
            Ty::Handle(e) => return write!(f, "handle {e}"),
            Ty::Task(e) => return write!(f, "task {e}"),
            Ty::Atomic(e) => return write!(f, "atomic {e}"),
            Ty::Mutex(e) => return write!(f, "mutex {e}"),
            Ty::Guard(e) => return write!(f, "guard {e}"),
            Ty::Channel(e) => return write!(f, "channel {e}"),
            Ty::Ptr { mutable, elem } => {
                return write!(f, "*{}{}", if *mutable { "" } else { "const " }, elem)
            }
            Ty::Slice { mutable, elem } => {
                return write!(f, "slice {}{}", if *mutable { "mut " } else { "" }, elem)
            }
            Ty::Str => "str".to_string(),
            Ty::Box(e) => return write!(f, "box {e}"),
            Ty::BoxSlice(e) => return write!(f, "box slice {e}"),
            Ty::Alloc => "alloc".to_string(),
        })
    }
}

#[derive(Debug)]
pub struct Program {
    /// Optional `module name;` — must equal the file's stem when the
    /// file is loaded as a module (N17).
    pub module: Option<String>,
    /// `import a.b` — module paths resolved by the loader; the leaf
    /// segment is the qualifier (`import lib.math` → `math.f`).
    pub imports: Vec<Import>,
    pub structs: Vec<StructDecl>,
    pub enums: Vec<EnumDecl>,
    pub fns: Vec<FnDef>,
}

/// `import a.b.c` — a filesystem path in segments (`a/b/c.lang`).
#[derive(Debug)]
pub struct Import {
    pub path: Vec<String>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct StructDecl {
    pub name: String,
    /// `struct Vec<T>` — type parameter names (N19). Empty for
    /// ordinary decls; the monomorph pass clones this per
    /// instantiation, so param'd decls are templates the checker
    /// and codegen skip entirely.
    pub tparams: Vec<String>,
    pub fields: Vec<(String, Ty)>,
    /// `pub struct` — visible to importing modules (N17). All fields
    /// are public for now; field-level hiding is a later milestone.
    pub is_pub: bool,
    pub span: Span,
}

/// `enum E { A, B(T) }` — a tagged union (N22). Physical layout is
/// `{ i64 tag, [max-payload x i8] }`; semantically each variant's
/// payload is just another place subtree (`x#e.B`), so authority,
/// init, and affinity compose with no new rules.
#[derive(Debug, Clone)]
pub struct EnumDecl {
    pub name: String,
    /// `enum Maybe<T>` — type parameters (N19 machinery applies).
    pub tparams: Vec<String>,
    /// Variant name → optional payload type. One payload slot per
    /// variant for now; nullary variants are tag-only.
    pub variants: Vec<(String, Option<Ty>)>,
    pub is_pub: bool,
    pub span: Span,
}

/// Capability authority on a parameter: `read`/`write`/`own` —
/// the param binds as a PLACE ALIAS into the caller's storage
/// (read/write) or a moved value (own), not a fresh local.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Auth {
    Read,
    Write,
    Own,
    /// Synchronized access — task holds only; never a param contract.
    /// An `Atomic` hold tolerates other `Atomic` access and conflicts
    /// with every unsynchronized touch (N10).
    Atomic,
    /// Synchronized access under a mutex domain (N11) — the edge's
    /// TARGET names the domain (`t -> m#e`). Two Sync holds are
    /// compatible iff they claim the same domain; a Sync hold races
    /// every unsynchronized access to storage the domain protects.
    Sync,
}

#[derive(Debug, Clone)]
pub struct MatchArm {
    /// Variant name — or `_` for the catch-all.
    pub variant: String,
    /// `Some(v)`'s payload binder; `None` for nullary variants, `_`,
    /// or a discarded payload.
    pub bind: Option<String>,
    /// N28 — the arm yields a value: `=> expr`, or `=> { stmts; expr }`
    /// with a trailing bare expression (no `;`). The body's last stmt
    /// is then a `Stmt::Expr` checked under the match's expected type.
    /// `false` arms yield nothing — they may only diverge (`=> return
    /// e`, `=> { return e; }`) or fall through as Void.
    pub expr_arm: bool,
    pub body: Vec<Stmt>,
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    /// `fn f<T>` — type parameter names (N19). Param'd fns are
    /// templates: instantiated clones carry concrete types and an
    /// empty list here.
    pub tparams: Vec<String>,
    /// (name, authority, type) — `None` authority = ordinary local param.
    pub params: Vec<(String, Option<Auth>, Ty)>,
    pub ret: Ty,
    pub body: Vec<Stmt>,
    /// `extern fn` — a C-ABI declaration: signature only, no body.
    /// Params are FFI-safe types (`*T`, ints, bools); the call is an
    /// opaque graph transform — `*T` args claim Write on the referent,
    /// `*const T` claim Read (N14).
    pub is_extern: bool,
    /// `pub fn` — exported through the module interface (N17).
    pub is_pub: bool,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        ty: Option<Ty>,
        init: Option<Expr>,
        span: Span,
        /// Span of the bound identifier — `span` covers the whole
        /// statement (the `let` keyword), so index/hover on the name
        /// needs its own span.
        name_span: Span,
    },
    Assign {
        target: Expr,
        value: Expr,
        span: Span,
    },
    Return {
        value: Option<Expr>,
        span: Span,
    },
    If {
        cond: Expr,
        then_body: Vec<Stmt>,
        else_body: Option<Vec<Stmt>>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `break` / `continue` (N25D) — exit the innermost loop /
    /// re-enter its condition. The checker's loop-exit merge joins
    /// every break path, every continue path, and the fall-through
    /// path; unreachable statements after one check on the dead path
    /// (same as `return`).
    Break {
        span: Span,
    },
    Continue {
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
    /// `match e { V(b) => { .. }, .. }` — discriminates an enum's
    /// tag (N22). `V(b)` binds the payload by value into a fresh
    /// local; `_` is a catch-all arm.
    Match {
        scrut: Expr,
        arms: Vec<MatchArm>,
        span: Span,
    },
    /// `region name { .. }` — a named lifetime frame. Purely a semantic
    /// frame: no runtime code, but it owns every local bound inside.
    Region {
        name: String,
        body: Vec<Stmt>,
        span: Span,
    },
    /// `promote x` — lift a binding one region outward, edge-closure
    /// permitting. Structural lifetime movement, no runtime effect.
    Promote {
        target: Expr,
        span: Span,
    },
    /// `consume p` / `consume p.f` — the affine transfer: the place's
    /// authority is used up exactly once (N7's temporal transition).
    Consume {
        target: Expr,
        span: Span,
    },
    /// `defer call(..)` — N54 scoped cleanup: schedule an explicit
    /// void call to run, LIFO, on every normal exit of this scope
    /// (`}`-fallthrough, `return`, `?`, `break`, `continue`). Not a
    /// destructor: the operand places are frozen at registration —
    /// moves, stores, and rebinding them reject until the defer
    /// fires — and the replay performs the call's own effects (its
    /// consumes discharge R19). `process.exit`/`unreachable` unwind
    /// nothing: defers do not run on terminated paths.
    Defer {
        expr: Expr,
        span: Span,
    },
}

impl Stmt {
    pub fn span(&self) -> Span {
        match self {
            Stmt::Let { span, .. }
            | Stmt::Assign { span, .. }
            | Stmt::Return { span, .. }
            | Stmt::If { span, .. }
            | Stmt::While { span, .. }
            | Stmt::Break { span }
            | Stmt::Continue { span }
            | Stmt::Expr { span, .. }
            | Stmt::Match { span, .. }
            | Stmt::Region { span, .. }
            | Stmt::Promote { span, .. }
            | Stmt::Consume { span, .. }
            | Stmt::Defer { span, .. } => *span,
        }
    }
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Bool(bool, Span),
    Var(String, Span),
    Unary(UnOp, Box<Expr>, Span),
    Bin(BinOp, Box<Expr>, Box<Expr>, Span),
    /// `f(args)` / `f<Ts>(args)` — explicit type args ride the call
    /// (N19). `targs` is empty for ordinary calls; the monomorph pass
    /// folds them into the canonical name `f<a,b>` and clears it.
    Call(String, Vec<Ty>, Vec<Expr>, Span),
    /// `base.name` — a place path segment (also a value when read).
    Field(Box<Expr>, String, Span),
    /// `base[i]` — the other path segment.
    Index(Box<Expr>, Box<Expr>, Span),
    /// `base[lo..hi]` — a half-open range: only meaningful under
    /// `&`/`&mut`, where it makes a `slice` view over the span.
    IndexRange {
        base: Box<Expr>,
        lo: Option<Box<Expr>>,
        hi: Option<Box<Expr>>,
        span: Span,
    },
    /// `Point { x: e, .. }` — a place-fill, not a plain value: gen
    /// materializes it into a slot with per-field stores. `targs`
    /// carries `Vec<i32>`-style applications until canonicalized (N19).
    StructLit(String, Vec<Ty>, Vec<(String, Expr)>, Span),
    /// `&e` / `&mut e` — take the address of a place. The result is a
    /// value with provenance = the place's owner region.
    AddrOf {
        mutable: bool,
        place: Box<Expr>,
        span: Span,
    },
    /// `handle <place>` — makes an identity, not a borrow.
    Handle(Box<Expr>, Span),
    /// `spawn { ... }` — a task: the body is analyzed in a forked
    /// graph; its holds apply now, its effects at `join`.
    Spawn(Vec<Stmt>, Span),
    /// `join t` — apply the task's suspended effects; yields its
    /// result. The task token is consumed.
    Join(Box<Expr>, Span),
    /// `mutex e` — wrap a value in a synchronization domain. A handle
    /// operand protects its referent's storage instead.
    Mutex(Box<Expr>, Span),
    /// `*e` — deref a reference. A place when the expr is a place.
    Deref(Box<Expr>, Span),
    /// `"..."` — a C string literal: decoded bytes, type `*const u8`,
    /// backed by a null-terminated private global (N14).
    Str(Vec<u8>, Span),
    /// `alloc<T>(a)` / `alloc_slice<T>(a, n)` — heap allocation through
    /// an allocator token (N16). `count: None` → `box T` (one cell);
    /// `Some(n)` → `box slice T` (a span). The result is an owning
    /// binding: its key IS the new storage root.
    Alloc {
        ty: Box<Ty>,
        alloc: Box<Expr>,
        count: Option<Box<Expr>>,
        span: Span,
    },
    /// `sizeof<T>` — the type's byte size as a u64 constant (N19 —
    /// generic code like `Vec<T>` needs `v.len * sizeof<T>` where the
    /// concrete code wrote `v.len * 4`).
    Sizeof(Box<Ty>, Span),
    /// `recv.name<targs>(args)` — method sugar (N20). Desugared in the
    /// mono pass to `Call("<mod>.<name>", targs, [recv, ..args])`
    /// where `<mod>` is the module that declared the receiver's struct
    /// type; the callee must take `self` as its first parameter. No
    /// dispatch machinery — the node never survives to the checker.
    MethodCall {
        recv: Box<Expr>,
        name: String,
        targs: Vec<Ty>,
        args: Vec<Expr>,
        span: Span,
    },
    /// `Type::name(args)` — associated-fn sugar (N20): desugars to
    /// `Call("<mod>.<name>", ty-args, args)` in mono.
    AssocFn {
        ty: Box<Ty>,
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// `Maybe<i32>::Some(x)` / `Maybe::None` — a variant construction
    /// (N22). Produced by the monomorph pass from `AssocFn` when the
    /// base type names an enum; the parser never emits this node.
    EnumLit {
        ty: Box<Ty>,
        variant: String,
        payload: Option<Box<Expr>>,
        span: Span,
    },
    /// `expr?` — error propagation (N23). Desugars semantically to
    /// `match expr { Ok(v) => v, Err(e) => return Err(e) }`: the Ok
    /// path yields the payload; the Err path diverges into an early
    /// return of the current fn's `Result<_, E>`. No new machinery —
    /// the payload move and the divergence are N22/N18 facts.
    Try {
        inner: Box<Expr>,
        span: Span,
    },
    /// `match e { V(b) => expr, .. }` — the statement form gains a
    /// value (N28). Each `expr_arm` yields its trailing expression;
    /// block arms diverge or fall through as Void. Every live arm's
    /// yield type must unify; authority/init/tag effects join exactly
    /// like the statement form's N-arm merge.
    Match {
        scrut: Box<Expr>,
        arms: Vec<MatchArm>,
        span: Span,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum UnOp {
    Neg,
    Not,
    /// `~x` — bitwise complement (N25A).
    BitNot,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Eq,
    Ne,
    Lt,
    Gt,
    Le,
    Ge,
    // N25A — bitwise. Same-typed integer operands only; the result
    // is the operand type. `>>` is logical on unsigned, arithmetic
    // on signed — never a mask: an out-of-range amount traps at
    // runtime and rejects at compile time when constant.
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
    // N25C — short-circuit. Not binops semantically: `a && b`
    // evaluates `b` only when `a` held (`||`: when it didn't).
    // Bool operands; the checker's RHS effects join as Maybe.
    LAnd,
    LOr,
}

impl BinOp {
    pub fn is_cmp(self) -> bool {
        matches!(
            self,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge
        )
    }
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Bool(_, s)
            | Expr::Var(_, s)
            | Expr::Unary(_, _, s)
            | Expr::Bin(_, _, _, s)
            | Expr::Call(_, _, _, s)
            | Expr::Field(_, _, s)
            | Expr::Index(_, _, s)
            | Expr::IndexRange { span: s, .. }
            | Expr::StructLit(_, _, _, s)
            | Expr::AddrOf { span: s, .. }
            | Expr::Handle(_, s)
            | Expr::Spawn(_, s)
            | Expr::Join(_, s)
            | Expr::Mutex(_, s)
            | Expr::Deref(_, s)
            | Expr::Str(_, s)
            | Expr::Alloc { span: s, .. }
            | Expr::Sizeof(_, s) => *s,
            Expr::MethodCall { span: s, .. } | Expr::AssocFn { span: s, .. } => *s,
            Expr::EnumLit { span: s, .. } => *s,
            Expr::Try { span: s, .. } => *s,
            Expr::Match { span: s, .. } => *s,
        }
    }
}

/// A compact s-expression printer for `--show-ast`.
pub fn show_ast(p: &Program) -> String {
    let mut out = String::new();
    for st in &p.structs {
        let fs: Vec<String> = st
            .fields
            .iter()
            .map(|(n, t)| format!("{n}: {t}"))
            .collect();
        out.push_str(&format!("(struct {} ({}))
", st.name, fs.join(" ")));
    }
    for f in &p.fns {
        let params: Vec<String> = f
            .params
            .iter()
            .map(|(n, a, t)| {
                let a = match a {
                    Some(Auth::Read) => "read ",
                    Some(Auth::Write) => "write ",
                    Some(Auth::Own) => "own ",
                    Some(Auth::Atomic) => "atomic ",
                    Some(Auth::Sync) => "sync ",
                    None => "",
                };
                format!("{a}{n}: {t}")
            })
            .collect();
        out.push_str(&format!(
            "({}fn {} ({}) -> {}\n",
            if f.is_extern { "extern " } else { "" },
            f.name,
            params.join(" "),
            f.ret
        ));
        for st in &f.body {
            show_stmt(&mut out, st, 1);
        }
        out.push_str(")\n");
    }
    out
}

fn indent(out: &mut String, d: usize) {
    for _ in 0..d {
        out.push_str("  ");
    }
}

fn show_stmt(out: &mut String, s: &Stmt, d: usize) {
    indent(out, d);
    match s {
        Stmt::Let { name, ty, init, .. } => {
            let t = ty.as_ref().map(|t| format!(": {t}")).unwrap_or_default();
            out.push_str(&format!("(let {name}{t}"));
            if let Some(i) = init {
                out.push(' ');
                show_expr(out, i);
            }
            out.push_str(")\n");
        }
        Stmt::Break { .. } => out.push_str("(break)\n"),
        Stmt::Continue { .. } => out.push_str("(continue)\n"),
        Stmt::Assign { target, value, .. } => {
            out.push_str("(assign ");
            show_expr(out, target);
            out.push(' ');
            show_expr(out, value);
            out.push_str(")\n");
        }
        Stmt::Return { value, .. } => {
            out.push_str("(return");
            if let Some(v) = value {
                out.push(' ');
                show_expr(out, v);
            }
            out.push_str(")\n");
        }
        Stmt::If {
            cond,
            then_body,
            else_body,
            ..
        } => {
            out.push_str("(if ");
            show_expr(out, cond);
            out.push('\n');
            for st in then_body {
                show_stmt(out, st, d + 1);
            }
            if let Some(eb) = else_body {
                indent(out, d + 1);
                out.push_str("else\n");
                for st in eb {
                    show_stmt(out, st, d + 1);
                }
            }
            indent(out, d);
            out.push_str(")\n");
        }
        Stmt::While { cond, body, .. } => {
            out.push_str("(while ");
            show_expr(out, cond);
            out.push('\n');
            for st in body {
                show_stmt(out, st, d + 1);
            }
            indent(out, d);
            out.push_str(")\n");
        }
        Stmt::Match { scrut, arms, .. } => {
            out.push_str("(match ");
            show_expr(out, scrut);
            for a in arms {
                out.push_str(&format!(" [{} => {} stmts]", a.variant, a.body.len()));
            }
            out.push_str(")\n");
        }
        Stmt::Region { name, body, .. } => {
            out.push_str(&format!("(region {name}\n"));
            for st in body {
                show_stmt(out, st, d + 1);
            }
            indent(out, d);
            out.push_str(")\n");
        }
        Stmt::Promote { target, .. } => {
            out.push_str("(promote ");
            show_expr(out, target);
            out.push_str(")\n");
        }
        Stmt::Consume { target, .. } => {
            out.push_str("(consume ");
            show_expr(out, target);
            out.push_str(")\n");
        }
        Stmt::Defer { expr, .. } => {
            out.push_str("(defer ");
            show_expr(out, expr);
            out.push_str(")\n");
        }
        Stmt::Expr { expr, .. } => {
            show_expr(out, expr);
            out.push('\n');
        }
    }
}

fn show_expr(out: &mut String, e: &Expr) {
    match e {
        Expr::Int(n, _) => out.push_str(&format!("{n}")),
        Expr::Str(b, _) => {
            out.push_str(&format!("\"{}\"", String::from_utf8_lossy(b)));
        }
        Expr::Sizeof(t, _) => out.push_str(&format!("(sizeof {t})")),
        Expr::MethodCall { recv, name, args, .. } => {
            show_expr(out, recv);
            out.push_str(&format!(".{name}(..{})", args.len()));
        }
        Expr::AssocFn { ty, name, .. } => out.push_str(&format!("{ty}::{name}(..)")),
        Expr::EnumLit { ty, variant, .. } => {
            out.push_str(&format!("{ty}::{variant}(lit)"))
        }
        Expr::Try { inner, .. } => {
            show_expr(out, inner);
            out.push('?');
        }
        Expr::Alloc {
            ty, alloc, count, ..
        } => {
            match count {
                Some(c) => {
                    out.push_str(&format!("alloc_slice<{ty}>("));
                    show_expr(out, alloc);
                    out.push_str(", ");
                    show_expr(out, c);
                }
                None => {
                    out.push_str(&format!("alloc<{ty}>("));
                    show_expr(out, alloc);
                }
            }
            out.push(')');
        }
        Expr::Bool(b, _) => out.push_str(&format!("{b}")),
        Expr::Var(n, _) => out.push_str(n),
        Expr::Unary(op, e, _) => {
            out.push_str(match op {
                UnOp::Neg => "(- ",
                UnOp::Not => "(! ",
                UnOp::BitNot => "(~ ",
            });
            show_expr(out, e);
            out.push(')');
        }
        Expr::Bin(op, a, b, _) => {
            let sym = match op {
                BinOp::Add => "+",
                BinOp::Sub => "-",
                BinOp::Mul => "*",
                BinOp::Div => "/",
                BinOp::Mod => "%",
                BinOp::Eq => "==",
                BinOp::Ne => "!=",
                BinOp::Lt => "<",
                BinOp::Gt => ">",
                BinOp::Le => "<=",
                BinOp::Ge => ">=",
                BinOp::BitAnd => "&",
                BinOp::BitOr => "|",
                BinOp::BitXor => "^",
                BinOp::Shl => "<<",
                BinOp::Shr => ">>",
                BinOp::LAnd => "&&",
                BinOp::LOr => "||",
            };
            out.push_str(&format!("({sym} "));
            show_expr(out, a);
            out.push(' ');
            show_expr(out, b);
            out.push(')');
        }
        Expr::Call(n, targs, args, _) => {
            if targs.is_empty() {
                out.push_str(&format!("({n}"));
            } else {
                let inner: Vec<String> =
                    targs.iter().map(|t| format!("{t}")).collect();
                out.push_str(&format!("({n}<{}>", inner.join(",")));
            }
            for a in args {
                out.push(' ');
                show_expr(out, a);
            }
            out.push(')');
        }
        Expr::Field(b, n, _) => {
            show_expr(out, b);
            out.push_str(&format!(".{n}"));
        }
        Expr::Index(b, i, _) => {
            show_expr(out, b);
            out.push('[');
            show_expr(out, i);
            out.push(']');
        }
        Expr::IndexRange { base, lo, hi, .. } => {
            show_expr(out, base);
            out.push('[');
            if let Some(l) = lo {
                show_expr(out, l);
            }
            out.push_str("..");
            if let Some(h) = hi {
                show_expr(out, h);
            }
            out.push(']');
        }
        Expr::AddrOf { mutable, place, .. } => {
            out.push_str(if *mutable { "(&mut " } else { "(& " });
            show_expr(out, place);
            out.push(')');
        }
        Expr::Handle(e, _) => {
            out.push_str("(handle ");
            show_expr(out, e);
            out.push(')');
        }
        Expr::Spawn(body, _) => {
            out.push_str("(spawn");
            for st in body {
                out.push(' ');
                show_stmt(out, st, 1);
            }
            out.push(')');
        }
        Expr::Join(e, _) => {
            out.push_str("(join ");
            show_expr(out, e);
            out.push(')');
        }
        Expr::Mutex(e, _) => {
            out.push_str("(mutex ");
            show_expr(out, e);
            out.push(')');
        }
        Expr::Deref(e, _) => {
            out.push_str("(* ");
            show_expr(out, e);
            out.push(')');
        }
        Expr::StructLit(n, targs, fields, _) => {
            if targs.is_empty() {
                out.push_str(&format!("({n} {{"));
            } else {
                let inner: Vec<String> =
                    targs.iter().map(|t| format!("{t}")).collect();
                out.push_str(&format!("({n}<{}> {{", inner.join(",")));
            }
            for (i, (fn_, fe)) in fields.iter().enumerate() {
                if i > 0 {
                    out.push(' ');
                }
                out.push_str(&format!("{fn_}: "));
                show_expr(out, fe);
            }
            out.push_str("})");
        }
        Expr::Match { scrut, arms, .. } => {
            out.push_str("(match ");
            show_expr(out, scrut);
            for a in arms {
                out.push(' ');
                out.push_str(&a.variant);
                if let Some(b) = &a.bind {
                    out.push_str(&format!("({b})"));
                }
                out.push_str(" =>");
                for st in &a.body {
                    out.push(' ');
                    show_stmt(out, st, 1);
                }
            }
            out.push(')');
        }
    }
}
