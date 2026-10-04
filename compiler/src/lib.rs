//! `ourc` — the native compiler. A separate crate from the memory-model
//! lab: this pipeline lowers *source* to *machine code*. The lab's
//! semantic checkers get imported here one experiment at a time — never
//! all at once.
//!
//! ```text
//! hello.lang → parse → typecheck → our IR → LLVM IR → .o → link → hello
//! ```

pub mod backend_llvm;
pub mod driver;
pub mod frontend;
pub mod ir;
pub mod semantics;

/// A source location.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// A diagnostic — one struct for the whole pipeline: parse errors, type
/// errors, and later the semantic checker's findings.
#[derive(Debug, Clone)]
pub struct Diag {
    pub span: Span,
    pub msg: String,
    /// N17 — the file the diagnostic belongs to when it isn't the one
    /// being compiled directly (errors inside imported modules).
    /// `None` = the root source file.
    pub file: Option<String>,
}

impl Diag {
    pub fn new(span: Span, msg: impl Into<String>) -> Self {
        Diag {
            span,
            msg: msg.into(),
            file: None,
        }
    }
}
