//! Abstract syntax tree for MML (the Memory Model Lab language).
//!
//! MML is a deliberately tiny systems language whose only purpose is to let us
//! compare competing compile-time memory-safety models on identical programs.
//!
//! The annotation vocabulary is a *superset* shared by every model. Each model
//! interprets the same annotations through its own lens:
//!
//!   param mode `own`  -> ownership: by-value move   | capability: `own<T>`  cap
//!   param mode `ref`  -> ownership: shared borrow    | capability: `read<T>` cap
//!   param mode `mut`  -> ownership: mutable borrow   | capability: `write<T>` cap
//!   `region r { .. }` -> region model: allocation scope; other models ignore it
//!   `new(r) T`        -> allocate into region `r`    | `new T` = heap allocation
//!   `free e`          -> manual release (used to provoke use-after-free)
//!
//! This lets one source file carry a *different expected verdict per model*,
//! which is exactly the signal the lab is built to surface.

use std::fmt;

/// A source location (1-based line/column) for diagnostics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// Parameter passing / reference mode. The single shared vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// Ownership transfer / full capability.
    Own,
    /// Shared, read-only access.
    Ref,
    /// Exclusive, mutable access.
    Mut,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Own => "own",
            Mode::Ref => "ref",
            Mode::Mut => "mut",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Type {
    /// A named type: a primitive (`u32`, `bool`) or a struct name.
    Named(String),
    /// A shared reference `&T`.
    Ref(Box<Type>),
    /// A mutable reference `&mut T`.
    MutRef(Box<Type>),
    /// `atomic<T>` — storage that may be accessed through the atomic
    /// builtins; ordinary access stays under normal capability rules.
    Atomic(Box<Type>),
    /// `mutex<T>` — a synchronization domain wrapping a value; its
    /// interior is reachable only through a live `lock` guard.
    Mutex(Box<Type>),
    /// `channel<T>` — an endpoint that transports authority: `send` moves
    /// authority out of the sender, `recv` installs it in the receiver.
    Channel(Box<Type>),
}

#[derive(Debug, Clone)]
pub struct Field {
    pub name: String,
    pub ty: Type,
}

#[derive(Debug, Clone)]
pub struct StructDef {
    pub name: String,
    pub fields: Vec<Field>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub mode: Mode,
    pub name: String,
    pub ty: Type,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct FnDef {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: Option<Type>,
    pub body: Block,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub struct Block {
    pub stmts: Vec<Stmt>,
    pub span: Span,
}

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        ty: Option<Type>,
        init: Expr,
        span: Span,
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
        then_block: Block,
        else_block: Option<Block>,
        span: Span,
    },
    While {
        cond: Expr,
        body: Block,
        span: Span,
    },
    /// `region NAME { .. }` — a lexical allocation scope.
    Region {
        name: String,
        body: Block,
        span: Span,
    },
    /// `free EXPR;` — manual release of an allocation.
    Free {
        value: Expr,
        span: Span,
    },
    Expr {
        expr: Expr,
        span: Span,
    },
}

#[derive(Debug, Clone)]
pub enum Expr {
    Int(i64, Span),
    Bool(bool, Span),
    Var(String, Span),
    /// `&expr`
    Ref(Box<Expr>, Span),
    /// `&mut expr`
    MutRef(Box<Expr>, Span),
    /// `*expr`
    Deref(Box<Expr>, Span),
    /// `expr.field`
    Field(Box<Expr>, String, Span),
    /// `new TYPE` (heap) or `new(region) TYPE` (region allocation).
    Alloc {
        region: Option<String>,
        ty: Type,
        span: Span,
    },
    /// A persistence primitive that produces an escape-safe, long-lived value:
    ///   `promote e` — re-home `e` so it outlives every region (a transfer).
    ///   `handle e`  — obtain a stable, generation-checked handle to `e`.
    /// Both express "this relationship is not a lifetime-tracked pointer" and so
    /// are treated as region-level-0 (never dangling) by the lifetime analysis.
    Persist {
        kind: PersistKind,
        inner: Box<Expr>,
        span: Span,
    },
    /// `Name { field: expr, .. }`
    StructLit {
        name: String,
        fields: Vec<(String, Expr)>,
        span: Span,
    },
    /// `f(args)`
    Call {
        name: String,
        args: Vec<Expr>,
        span: Span,
    },
    /// `spawn { stmts }` — fork a child task; the expression produces a task
    /// handle (a consumable resource joined later).
    Spawn { body: Block, span: Span },
    /// `join expr` — merge the completed child task's effects back into this
    /// thread; yields the child's `return`ed value (or unit).
    Join { inner: Box<Expr>, span: Span },
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        span: Span,
    },
}

impl Expr {
    pub fn span(&self) -> Span {
        match self {
            Expr::Int(_, s)
            | Expr::Bool(_, s)
            | Expr::Var(_, s)
            | Expr::Ref(_, s)
            | Expr::MutRef(_, s)
            | Expr::Deref(_, s)
            | Expr::Field(_, _, s)
            | Expr::Alloc { span: s, .. }
            | Expr::Persist { span: s, .. }
            | Expr::StructLit { span: s, .. }
            | Expr::Call { span: s, .. }
            | Expr::Spawn { span: s, .. }
            | Expr::Join { span: s, .. }
            | Expr::Binary { span: s, .. } => *s,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistKind {
    /// `promote e` — re-home an allocation so it outlives all regions.
    Promote,
    /// `handle e` — a stable, generation-checked handle (not a raw pointer).
    Handle,
    /// `mutex e` — wrap `e` in a synchronization domain; `e` moves in if
    /// it isn't Copy.
    Mutex,
}

impl PersistKind {
    pub fn as_str(self) -> &'static str {
        match self {
            PersistKind::Promote => "promote",
            PersistKind::Handle => "handle",
            PersistKind::Mutex => "mutex",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Eq,
    Ne,
    Lt,
    Gt,
    And,
    Or,
    BitAnd,
    BitOr,
    BitXor,
    Shl,
    Shr,
}

#[derive(Debug, Clone)]
pub enum Item {
    Struct(StructDef),
    Fn(FnDef),
}

#[derive(Debug, Clone, Default)]
pub struct Program {
    pub items: Vec<Item>,
}

impl Program {
    pub fn functions(&self) -> impl Iterator<Item = &FnDef> {
        self.items.iter().filter_map(|it| match it {
            Item::Fn(f) => Some(f),
            _ => None,
        })
    }

    pub fn structs(&self) -> impl Iterator<Item = &StructDef> {
        self.items.iter().filter_map(|it| match it {
            Item::Struct(s) => Some(s),
            _ => None,
        })
    }
}
