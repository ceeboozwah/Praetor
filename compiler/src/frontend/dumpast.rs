//! Canonical AST dump — the Stage-0 oracle surface for the
//! self-hosted parser (BOOTSTRAP.md §4). Structural S-expressions,
//! one node per line, two-space indent; every spanned node carries
//! `@line:col`; types print through `Ty::Display` (already the
//! canonical spelling); names print raw; byte payloads escape like
//! the token dump. No arena IDs, no addresses — semantics only.
//!
//! ```text
//! (program
//!   (import std.vec @1:8)
//!   (fn main @7:1 pub
//!     (param x none i32)
//!     (ret i32)
//!     (block
//!       (let y @8:9 ty=i32
//!         (int 42 @8:14)))))
//! ! <line> <col> <msg>          — lex or parse error (no AST)
//! ```

use super::ast::*;
use crate::Span;

/// Byte escape — same rules as the token dump's payload field.
fn esc(b: &[u8]) -> String {
    let mut o = String::with_capacity(b.len() + 2);
    for &x in b {
        match x {
            b'\\' => o.push_str("\\\\"),
            0x0a => o.push_str("\\n"),
            0x0d => o.push_str("\\r"),
            0x09 => o.push_str("\\t"),
            0x20..=0x7e => o.push(x as char),
            _ => o.push_str(&format!("\\x{x:02x}")),
        }
    }
    o
}

fn at(s: Span) -> String {
    format!(" @{}:{}", s.line, s.col)
}

fn tys(ts: &[Ty]) -> String {
    let inner: Vec<String> = ts.iter().map(|t| t.to_string()).collect();
    format!(" targs=({})", inner.join(","))
}

fn auth(a: &Option<Auth>) -> &'static str {
    match a {
        None => "none",
        Some(Auth::Read) => "read",
        Some(Auth::Write) => "write",
        Some(Auth::Own) => "own",
        // Atomic/Sync can't appear in param position (parse never
        // produces them); keep the printer total anyway.
        Some(Auth::Atomic) => "atomic",
        Some(Auth::Sync) => "sync",
    }
}

struct D {
    o: String,
    d: usize,
}

impl D {
    fn ln(&mut self, s: &str) {
        for _ in 0..self.d {
            self.o.push_str("  ");
        }
        self.o.push_str(s);
        self.o.push('\n');
    }
    fn up(&mut self, f: impl FnOnce(&mut Self)) {
        self.d += 1;
        f(self);
        self.d -= 1;
    }

    fn stmts(&mut self, ss: &[Stmt]) {
        for s in ss {
            self.stmt(s);
        }
    }

    /// A statement list in its own `(block ..)` frame — `Vec<Stmt>`
    /// has no span, so neither does the wrapper.
    fn block(&mut self, ss: &[Stmt]) {
        self.ln("(block");
        self.up(|d| d.stmts(ss));
        self.ln(")");
    }

    fn arms(&mut self, arms: &[MatchArm]) {
        for a in arms {
            self.ln(&format!(
                "(arm {}{}{}",
                a.variant,
                match &a.bind {
                    Some(b) => format!(" bind={b}"),
                    None => String::new(),
                },
                if a.expr_arm { " yield" } else { "" },
            ));
            self.up(|d| d.stmts(&a.body));
            self.ln(")");
        }
    }

    fn stmt(&mut self, s: &Stmt) {
        match s {
            Stmt::Let {
                name,
                ty,
                init,
                span,
                name_span,
            } => {
                self.ln(&format!(
                    "(let {name}{}{} name@{}:{}",
                    at(*span),
                    ty.as_ref().map(|t| format!(" ty={t}")).unwrap_or_default(),
                    name_span.line,
                    name_span.col,
                ));
                if let Some(e) = init {
                    self.up(|d| d.expr(e));
                }
                self.ln(")");
            }
            Stmt::Assign {
                target,
                value,
                span,
            } => {
                self.ln(&format!("(assign{}", at(*span)));
                self.up(|d| {
                    d.expr(target);
                    d.expr(value);
                });
                self.ln(")");
            }
            Stmt::Return { value, span } => {
                self.ln(&format!("(return{}", at(*span)));
                if let Some(e) = value {
                    self.up(|d| d.expr(e));
                }
                self.ln(")");
            }
            Stmt::If {
                cond,
                then_body,
                else_body,
                span,
            } => {
                self.ln(&format!("(if{}", at(*span)));
                self.up(|d| {
                    d.expr(cond);
                    d.block(then_body);
                    match else_body {
                        Some(b) => d.block(b),
                        None => d.ln("(none)"),
                    }
                });
                self.ln(")");
            }
            Stmt::While { cond, body, span } => {
                self.ln(&format!("(while{}", at(*span)));
                self.up(|d| {
                    d.expr(cond);
                    d.block(body);
                });
                self.ln(")");
            }
            Stmt::Break { span } => self.ln(&format!("(break{})", at(*span))),
            Stmt::Continue { span } => self.ln(&format!("(continue{})", at(*span))),
            Stmt::Expr { expr, span } => {
                self.ln(&format!("(expr{}", at(*span)));
                self.up(|d| d.expr(expr));
                self.ln(")");
            }
            Stmt::Match { scrut, arms, span } => {
                self.ln(&format!("(match{}", at(*span)));
                self.up(|d| {
                    d.expr(scrut);
                    d.arms(arms);
                });
                self.ln(")");
            }
            Stmt::Region { name, body, span } => {
                self.ln(&format!("(region {name}{}", at(*span)));
                self.up(|d| d.stmts(body));
                self.ln(")");
            }
            Stmt::Promote { target, span } => {
                self.ln(&format!("(promote{}", at(*span)));
                self.up(|d| d.expr(target));
                self.ln(")");
            }
            Stmt::Consume { target, span } => {
                self.ln(&format!("(consume{}", at(*span)));
                self.up(|d| d.expr(target));
                self.ln(")");
            }
            Stmt::Defer { expr, span } => {
                self.ln(&format!("(defer{}", at(*span)));
                self.up(|d| d.expr(expr));
                self.ln(")");
            }
        }
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Int(n, s) => self.ln(&format!("(int {n}{})", at(*s))),
            Expr::Bool(b, s) => self.ln(&format!("(bool {b}{})", at(*s))),
            Expr::Var(n, s) => self.ln(&format!("(var {n}{})", at(*s))),
            Expr::Str(b, s) => self.ln(&format!("(str \"{}\"{})", esc(b), at(*s))),
            Expr::Unary(op, e, s) => {
                let name = match op {
                    UnOp::Neg => "neg",
                    UnOp::Not => "not",
                    UnOp::BitNot => "bitnot",
                };
                self.ln(&format!("(unop {name}{}", at(*s)));
                self.up(|d| d.expr(e));
                self.ln(")");
            }
            Expr::Bin(op, l, r, s) => {
                let name = match op {
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
                    BinOp::Shl => "shl",
                    BinOp::Shr => "shr",
                    BinOp::BitAnd => "bitand",
                    BinOp::BitOr => "bitor",
                    BinOp::BitXor => "bitxor",
                    BinOp::LAnd => "land",
                    BinOp::LOr => "lor",
                };
                self.ln(&format!("(binop {name}{}", at(*s)));
                self.up(|d| {
                    d.expr(l);
                    d.expr(r);
                });
                self.ln(")");
            }
            Expr::Call(n, ta, args, s) => {
                self.ln(&format!(
                    "(call {n}{}{}",
                    at(*s),
                    if ta.is_empty() { String::new() } else { tys(ta) },
                ));
                self.up(|d| {
                    for a in args {
                        d.expr(a);
                    }
                });
                self.ln(")");
            }
            Expr::Field(b, n, s) => {
                self.ln(&format!("(field {n}{}", at(*s)));
                self.up(|d| d.expr(b));
                self.ln(")");
            }
            Expr::Index(b, i, s) => {
                self.ln(&format!("(index{}", at(*s)));
                self.up(|d| {
                    d.expr(b);
                    d.expr(i);
                });
                self.ln(")");
            }
            Expr::IndexRange { base, lo, hi, span } => {
                self.ln(&format!("(indexrange{}", at(*span)));
                self.up(|d| {
                    d.expr(base);
                    match lo {
                        Some(e) => d.expr(e),
                        None => d.ln("(none)"),
                    }
                    match hi {
                        Some(e) => d.expr(e),
                        None => d.ln("(none)"),
                    }
                });
                self.ln(")");
            }
            Expr::StructLit(n, ta, fields, s) => {
                self.ln(&format!(
                    "(structlit {n}{}{}",
                    at(*s),
                    if ta.is_empty() { String::new() } else { tys(ta) },
                ));
                self.up(|d| {
                    for (fname, fe) in fields {
                        d.ln(&format!("(field {fname}"));
                        d.up(|d| d.expr(fe));
                        d.ln(")");
                    }
                });
                self.ln(")");
            }
            Expr::AddrOf {
                mutable,
                place,
                span,
            } => {
                self.ln(&format!(
                    "(addrof {}{}",
                    if *mutable { "mut" } else { "shr" },
                    at(*span)
                ));
                self.up(|d| d.expr(place));
                self.ln(")");
            }
            Expr::Handle(e, s) => {
                self.ln(&format!("(handle{}", at(*s)));
                self.up(|d| d.expr(e));
                self.ln(")");
            }
            Expr::Join(e, s) => {
                self.ln(&format!("(join{}", at(*s)));
                self.up(|d| d.expr(e));
                self.ln(")");
            }
            Expr::Mutex(e, s) => {
                self.ln(&format!("(mutex{}", at(*s)));
                self.up(|d| d.expr(e));
                self.ln(")");
            }
            Expr::Deref(e, s) => {
                self.ln(&format!("(deref{}", at(*s)));
                self.up(|d| d.expr(e));
                self.ln(")");
            }
            Expr::Spawn(body, s) => {
                self.ln(&format!("(spawn{}", at(*s)));
                self.up(|d| d.stmts(body));
                self.ln(")");
            }
            Expr::Alloc {
                ty,
                alloc,
                count,
                span,
            } => {
                self.ln(&format!("(alloc {ty}{}", at(*span)));
                self.up(|d| {
                    d.expr(alloc);
                    match count {
                        Some(e) => d.expr(e),
                        None => d.ln("(none)"),
                    }
                });
                self.ln(")");
            }
            Expr::Sizeof(t, s) => self.ln(&format!("(sizeof {t}{})", at(*s))),
            Expr::MethodCall {
                recv,
                name,
                targs: ta,
                args,
                span,
            } => {
                self.ln(&format!(
                    "(methodcall {name}{}{}",
                    at(*span),
                    if ta.is_empty() { String::new() } else { tys(ta) },
                ));
                self.up(|d| {
                    d.expr(recv);
                    for a in args {
                        d.expr(a);
                    }
                });
                self.ln(")");
            }
            Expr::AssocFn {
                ty,
                name,
                args,
                span,
            } => {
                self.ln(&format!("(assocfn {ty} {name}{}", at(*span)));
                self.up(|d| {
                    for a in args {
                        d.expr(a);
                    }
                });
                self.ln(")");
            }
            Expr::EnumLit {
                ty,
                variant,
                payload,
                span,
            } => {
                self.ln(&format!("(enumlit {ty} {variant}{}", at(*span)));
                if let Some(p) = payload {
                    self.up(|d| d.expr(p));
                }
                self.ln(")");
            }
            Expr::Try { inner, span } => {
                self.ln(&format!("(try{}", at(*span)));
                self.up(|d| d.expr(inner));
                self.ln(")");
            }
            Expr::Match { scrut, arms, span } => {
                self.ln(&format!("(matchexpr{}", at(*span)));
                self.up(|d| {
                    d.expr(scrut);
                    d.arms(arms);
                });
                self.ln(")");
            }
        }
    }
}

/// `parse` output → canonical dump. Errors (lex or parse) print as
/// `!` lines, same convention as `--dump-tokens`.
pub fn dump_ast(prog: &Program) -> String {
    let mut d = D {
        o: String::new(),
        d: 0,
    };
    d.ln("(program");
    d.up(|d| {
        if let Some(m) = &prog.module {
            d.ln(&format!("(module {m})"));
        }
        for i in &prog.imports {
            d.ln(&format!("(import {}{})", i.path.join("."), at(i.span)));
        }
        for s in &prog.structs {
            d.ln(&format!(
                "(struct {}{}{}{}",
                s.name,
                at(s.span),
                if s.is_pub { " pub" } else { "" },
                if s.tparams.is_empty() {
                    String::new()
                } else {
                    format!(" tparams=({})", s.tparams.join(","))
                },
            ));
            d.up(|d| {
                for (n, t) in &s.fields {
                    d.ln(&format!("(field {n} {t})"));
                }
            });
            d.ln(")");
        }
        for e in &prog.enums {
            d.ln(&format!(
                "(enum {}{}{}{}",
                e.name,
                at(e.span),
                if e.is_pub { " pub" } else { "" },
                if e.tparams.is_empty() {
                    String::new()
                } else {
                    format!(" tparams=({})", e.tparams.join(","))
                },
            ));
            d.up(|d| {
                for (n, t) in &e.variants {
                    match t {
                        Some(t) => d.ln(&format!("(variant {n} {t})")),
                        None => d.ln(&format!("(variant {n})")),
                    }
                }
            });
            d.ln(")");
        }
        for f in &prog.fns {
            d.ln(&format!(
                "(fn {}{}{}{}{}",
                f.name,
                at(f.span),
                if f.is_pub { " pub" } else { "" },
                if f.is_extern { " extern" } else { "" },
                if f.tparams.is_empty() {
                    String::new()
                } else {
                    format!(" tparams=({})", f.tparams.join(","))
                },
            ));
            d.up(|d| {
                for (n, a, t) in &f.params {
                    d.ln(&format!("(param {n} {} {t})", auth(a)));
                }
                d.ln(&format!("(ret {})", f.ret));
                if !f.is_extern {
                    d.block(&f.body);
                }
            });
            d.ln(")");
        }
    });
    d.ln(")");
    d.o
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frontend::parser::parse;

    #[test]
    fn canonical_shape() {
        let src = "fn f(x: i32) -> i32 {\n    return x + 1;\n}\n";
        let p = parse(src).unwrap();
        assert_eq!(
            dump_ast(&p),
            "(program\n\
             \x20 (fn f @1:1\n\
             \x20   (param x none i32)\n\
             \x20   (ret i32)\n\
             \x20   (block\n\
             \x20     (return @2:5\n\
             \x20       (binop add @2:14\n\
             \x20         (var x @2:12)\n\
             \x20         (int 1 @2:16)\n\
             \x20       )\n\
             \x20     )\n\
             \x20   )\n\
             \x20 )\n\
             )\n"
        );
    }

    #[test]
    fn canonical_decl_forms() {
        let src = "struct S<T, U> { a: [4]u8, b: &mut T }\n\
                   fn g() { let s = S { a: 0 }; match m { A(x) => x } }\n";
        let p = parse(src).unwrap();
        assert_eq!(
            dump_ast(&p),
            "(program\n\
             \x20 (struct S @1:1 tparams=(T,U)\n\
             \x20   (field a [4]u8)\n\
             \x20   (field b &mut T)\n\
             \x20 )\n\
             \x20 (fn g @2:1\n\
             \x20   (ret void)\n\
             \x20   (block\n\
             \x20     (let s @2:10 name@2:14\n\
             \x20       (structlit S @2:18\n\
             \x20         (field a\n\
             \x20           (int 0 @2:25)\n\
             \x20         )\n\
             \x20       )\n\
             \x20     )\n\
             \x20     (match @2:30\n\
             \x20       (var m @2:36)\n\
             \x20       (arm A bind=x yield\n\
             \x20         (expr @2:48\n\
             \x20           (var x @2:48)\n\
             \x20         )\n\
             \x20       )\n\
             \x20     )\n\
             \x20   )\n\
             \x20 )\n\
             )\n"
        );
    }
}
