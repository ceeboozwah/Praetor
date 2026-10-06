//! Recursive-descent parser for the `.lang` surface.

use super::ast::*;
use super::lexer::{lex, Tok, Token};
use crate::{Diag, Span};

pub fn parse(src: &str) -> Result<Program, Vec<Diag>> {
    let toks = lex(src)?;
    let mut p = P { toks, i: 0 };
    let mut module = None;
    let mut imports = Vec::new();
    let mut structs = Vec::new();
    let mut enums = Vec::new();
    let mut fns = Vec::new();
    // `module name;` — optional first declaration, the file's own
    // module name. The loader checks it against the file stem.
    if matches!(p.peek(), Tok::Ident(n) if n == "module") {
        p.bump();
        let (n, _) = p.expect_ident()?;
        p.eat(&Tok::Semicolon)?;
        module = Some(n);
    }
    while *p.peek() != Tok::Eof {
        match p.peek().clone() {
            // `import a.b;` — bring module `b` into scope as `b`.
            Tok::Ident(n) if n == "import" => {
                let span = p.span();
                p.bump();
                let mut path = Vec::new();
                let (seg, _) = p.expect_ident()?;
                path.push(seg);
                while *p.peek() == Tok::Dot {
                    p.bump();
                    let (seg, _) = p.expect_ident()?;
                    path.push(seg);
                }
                p.eat(&Tok::Semicolon)?;
                imports.push(Import { path, span });
            }
            Tok::Struct => structs.push(p.parse_struct(false)?),
            // `enum E { A, B(T) }` — tagged union (N22).
            Tok::Ident(n) if n == "enum" => enums.push(p.parse_enum(false)?),
            // `extern fn name(..) -> ty;` — a C-ABI declaration.
            Tok::Ident(n) if n == "extern" && p.toks[p.i + 1].tok == Tok::Fn => {
                p.bump();
                fns.push(p.parse_fn(true, false)?);
            }
            // `pub` — export through the module interface. Applies to
            // `fn` and `struct`; `pub extern` is meaningless (externs
            // name global C symbols, not module members).
            Tok::Ident(n) if n == "pub" => {
                let s = p.span();
                p.bump();
                match p.peek().clone() {
                    Tok::Struct => structs.push(p.parse_struct(true)?),
                    Tok::Ident(n2) if n2 == "enum" => enums.push(p.parse_enum(true)?),
                    Tok::Fn => fns.push(p.parse_fn(false, true)?),
                    Tok::Ident(n2) if n2 == "extern" && p.toks[p.i + 1].tok == Tok::Fn => {
                        p.bump();
                        return Err(vec![Diag::new(
                            s,
                            "`pub` doesn't apply to `extern` — externs name global C \
                             symbols; declare them in the module that calls them",
                        )]);
                    }
                    t => {
                        return Err(vec![Diag::new(
                            s,
                            format!("expected `fn` or `struct` after `pub`, found {}", t.describe()),
                        )])
                    }
                }
            }
            _ => fns.push(p.parse_fn(false, false)?),
        }
    }
    Ok(Program {
        module,
        imports,
        structs,
        enums,
        fns,
    })
}

struct P {
    toks: Vec<Token>,
    i: usize,
}

impl P {
    fn peek(&self) -> &Tok {
        &self.toks[self.i].tok
    }
    fn span(&self) -> Span {
        self.toks[self.i].span
    }
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.i].tok.clone();
        if self.i + 1 < self.toks.len() {
            self.i += 1;
        }
        t
    }
    fn eat(&mut self, t: &Tok) -> Result<Span, Vec<Diag>> {
        if self.peek() == t {
            let s = self.span();
            self.bump();
            Ok(s)
        } else {
            Err(vec![Diag::new(
                self.span(),
                format!("expected {}, found {}", t.describe(), self.peek().describe()),
            )])
        }
    }
    fn err<T>(&self, msg: impl Into<String>) -> Result<T, Vec<Diag>> {
        Err(vec![Diag::new(self.span(), msg.into())])
    }
    fn expect_ident(&mut self) -> Result<(String, Span), Vec<Diag>> {
        match self.peek().clone() {
            Tok::Ident(n) => {
                let s = self.span();
                self.bump();
                Ok((n, s))
            }
            t => self.err(format!("expected identifier, found {}", t.describe())),
        }
    }

    /// Type position: bare machine types are contextual names — `u32`,
    /// `i64`, `bool`. Not keywords, same as the lab's `atomic<T>`.
    fn parse_ty(&mut self) -> Result<Ty, Vec<Diag>> {
        let (name, _span) = self.expect_ident()?;
        match name.as_str() {
            "u8" => Ok(Ty::U8),
            "u16" => Ok(Ty::U16),
            "u32" => Ok(Ty::U32),
            "u64" => Ok(Ty::U64),
            "i8" => Ok(Ty::I8),
            "i16" => Ok(Ty::I16),
            "i32" => Ok(Ty::I32),
            "i64" => Ok(Ty::I64),
            "bool" => Ok(Ty::Bool),
            "void" => Ok(Ty::Void),
            // `str` — the immutable string view (N15B).
            "str" => Ok(Ty::Str),
            // Any other name is a struct — the checker resolves it.
            // `m.T` names a struct in module `m` (N17). A trailing
            // `<args>` makes it a generic application (N19): kept as
            // `Ty::App` until monomorphization canonicalizes it to
            // `Struct("Name<args>")`.
            _ => {
                let mut qname = name;
                while *self.peek() == Tok::Dot {
                    self.bump();
                    let (seg, _) = self.expect_ident()?;
                    qname.push('.');
                    qname.push_str(&seg);
                }
                if *self.peek() == Tok::Lt {
                    let args = self.parse_targs()?;
                    Ok(Ty::App(qname, args))
                } else {
                    Ok(Ty::Struct(qname))
                }
            }
        }
    }

    /// `Type::name(args)` — associated-fn sugar (N20): desugars to a
    /// module-qualified call in mono.
    fn parse_assoc_args(&mut self, ty: Ty, span: crate::Span) -> Result<Expr, Vec<Diag>> {
        self.eat(&Tok::ColonColon)?;
        let (name, _) = self.expect_ident()?;
        // `T::name` with no parens is a nullary variant — `Maybe::None`
        // (N22). The monomorph pass decides: enum target → EnumLit,
        // fn target → an arity error as usual.
        let mut args = Vec::new();
        if *self.peek() == Tok::LParen {
            self.bump();
            while *self.peek() != Tok::RParen {
                args.push(self.parse_expr()?);
                if *self.peek() == Tok::Comma {
                    self.bump();
                } else {
                    break;
                }
            }
            self.eat(&Tok::RParen)?;
        }
        Ok(Expr::AssocFn {
            ty: Box::new(ty),
            name,
            args,
            span,
        })
    }

    /// `<T, ..>` after a `struct`/`fn` name — type parameters (N19).
    fn parse_tparams(&mut self) -> Result<Vec<String>, Vec<Diag>> {
        let mut ps = Vec::new();
        if *self.peek() != Tok::Lt {
            return Ok(ps);
        }
        self.bump();
        loop {
            let (n, s) = self.expect_ident()?;
            if ps.contains(&n) {
                return Err(vec![Diag::new(
                    s,
                    format!("duplicate type parameter `{n}`"),
                )]);
            }
            ps.push(n);
            if *self.peek() == Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.eat(&Tok::Gt)?;
        Ok(ps)
    }

    /// `<ty, ..>` at a use site — explicit type arguments (N19).
    /// Parsed speculatively in expression position: callers restore
    /// `self.i` on failure so `a < b` stays a comparison.
    fn parse_targs(&mut self) -> Result<Vec<Ty>, Vec<Diag>> {
        self.eat(&Tok::Lt)?;
        let mut args = vec![self.parse_ty_or_array()?];
        while *self.peek() == Tok::Comma {
            self.bump();
            args.push(self.parse_ty_or_array()?);
        }
        self.eat(&Tok::Gt)?;
        Ok(args)
    }

    fn parse_array_ty(&mut self) -> Result<Ty, Vec<Diag>> {
        // `[n]T` — the length is part of the type.
        self.bump(); // [
        let (n, span) = match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                (n, self.span())
            }
            t => return self.err(format!("expected array length, found {}", t.describe())),
        };
        if !(0..=u32::MAX as i64).contains(&n) || n == 0 {
            return Err(vec![Diag::new(
                span,
                format!("array length must be 1..=u32::MAX, found {n}"),
            )]);
        }
        self.eat(&Tok::RBracket)?;
        Ok(Ty::Array(Box::new(self.parse_ty_or_array()?), n as u32))
    }

    /// A type is `[n]T`, `&T`, `&mut T`, or a name — shared by every
    /// type position.
    fn parse_ty_or_array(&mut self) -> Result<Ty, Vec<Diag>> {
        match self.peek() {
            Tok::LBracket => self.parse_array_ty(),
            Tok::Amp => {
                self.bump();
                let mutable = *self.peek() == Tok::Mut;
                if mutable {
                    self.bump();
                }
                let elem = self.parse_ty_or_array()?;
                Ok(Ty::Ref {
                    mutable,
                    elem: Box::new(elem),
                })
            }
            Tok::Ident(n) if n == "handle" => {
                self.bump();
                Ok(Ty::Handle(Box::new(self.parse_ty_or_array()?)))
            }
            Tok::Ident(n) if n == "task" => {
                self.bump();
                Ok(Ty::Task(Box::new(self.parse_ty_or_array()?)))
            }
            Tok::Ident(n) if n == "atomic" => {
                self.bump();
                Ok(Ty::Atomic(Box::new(self.parse_ty_or_array()?)))
            }
            Tok::Ident(n) if n == "mutex" => {
                self.bump();
                Ok(Ty::Mutex(Box::new(self.parse_ty_or_array()?)))
            }
            Tok::Ident(n) if n == "guard" => {
                self.bump();
                Ok(Ty::Guard(Box::new(self.parse_ty_or_array()?)))
            }
            Tok::Ident(n) if n == "channel" => {
                self.bump();
                Ok(Ty::Channel(Box::new(self.parse_ty_or_array()?)))
            }
            // `box T` — an owning heap cell; `box slice T` — an
            // owning heap span (N16).
            Tok::Ident(n) if n == "box" => {
                self.bump();
                if let Tok::Ident(m) = self.peek() {
                    if m == "slice" {
                        self.bump();
                        return Ok(Ty::BoxSlice(Box::new(self.parse_ty_or_array()?)));
                    }
                }
                Ok(Ty::Box(Box::new(self.parse_ty_or_array()?)))
            }
            // `alloc` — the allocator token type (system_allocator's
            // product, `alloc`/`alloc_slice`'s first argument).
            Tok::Ident(n) if n == "alloc" => {
                self.bump();
                Ok(Ty::Alloc)
            }
            // `slice T` / `slice mut T` — a (ptr, len) view. `mut`
            // mirrors `&mut`: `slice` reads, `slice mut` writes (N15).
            Tok::Ident(n) if n == "slice" => {
                self.bump();
                let mutable = *self.peek() == Tok::Mut;
                if mutable {
                    self.bump();
                }
                Ok(Ty::Slice {
                    mutable,
                    elem: Box::new(self.parse_ty_or_array()?),
                })
            }
            // `*T` / `*const T` — raw C pointers (N14). `const` is a
            // contextual qualifier, same convention as `&mut`.
            Tok::Star => {
                self.bump();
                let mutable = match self.peek() {
                    Tok::Ident(n) if n == "const" => {
                        self.bump();
                        false
                    }
                    _ => true,
                };
                Ok(Ty::Ptr {
                    mutable,
                    elem: Box::new(self.parse_ty_or_array()?),
                })
            }
            _ => self.parse_ty(),
        }
    }

    fn parse_struct(&mut self, is_pub: bool) -> Result<StructDecl, Vec<Diag>> {
        let span = self.eat(&Tok::Struct)?;
        let (name, _) = self.expect_ident()?;
        let tparams = self.parse_tparams()?;
        self.eat(&Tok::LBrace)?;
        let mut fields = Vec::new();
        while *self.peek() != Tok::RBrace {
            let (fname, _) = self.expect_ident()?;
            self.eat(&Tok::Colon)?;
            let fty = self.parse_ty_or_array()?;
            fields.push((fname, fty));
            if *self.peek() == Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.eat(&Tok::RBrace)?;
        Ok(StructDecl {
            name,
            tparams,
            fields,
            is_pub,
            span,
        })
    }

    /// `enum E<T> { A, B(T), .. }` — variant list; a payload is one
    /// slot of any type (N22: `{tag, payload}` layout, multi-payload
    /// variants are a later widening, not a new concept).
    fn parse_enum(&mut self, is_pub: bool) -> Result<EnumDecl, Vec<Diag>> {
        let span = self.eat(&Tok::Ident("enum".to_string()))?;
        let (name, _) = self.expect_ident()?;
        let tparams = self.parse_tparams()?;
        self.eat(&Tok::LBrace)?;
        let mut variants = Vec::new();
        while *self.peek() != Tok::RBrace {
            let (vname, _vs) = self.expect_ident()?;
            let payload = if *self.peek() == Tok::LParen {
                self.bump();
                let t = self.parse_ty_or_array()?;
                self.eat(&Tok::RParen)?;
                Some(t)
            } else {
                None
            };
            variants.push((vname, payload));
            if *self.peek() == Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.eat(&Tok::RBrace)?;
        if variants.is_empty() {
            return Err(vec![Diag::new(
                span,
                format!("enum `{name}` declares no variants"),
            )]);
        }
        Ok(EnumDecl {
            name,
            tparams,
            variants,
            is_pub,
            span,
        })
    }

    fn parse_fn(&mut self, is_extern: bool, is_pub: bool) -> Result<FnDef, Vec<Diag>> {
        let span = self.eat(&Tok::Fn)?;
        let (name, _) = self.expect_ident()?;
        let tparams = self.parse_tparams()?;
        self.eat(&Tok::LParen)?;
        let mut params = Vec::new();
        while *self.peek() != Tok::RParen {
            // `read`/`write`/`own` before a param name is a capability
            // authority — contextual, never a keyword.
            let auth = match self.peek().clone() {
                Tok::Ident(n)
                    if matches!(n.as_str(), "read" | "write" | "own")
                        && matches!(self.toks[self.i + 1].tok, Tok::Ident(_))
                        && self.toks[self.i + 2].tok == Tok::Colon =>
                {
                    self.bump();
                    match n.as_str() {
                        "read" => Some(Auth::Read),
                        "write" => Some(Auth::Write),
                        _ => Some(Auth::Own),
                    }
                }
                _ => None,
            };
            let (pname, _) = self.expect_ident()?;
            self.eat(&Tok::Colon)?;
            let pty = self.parse_ty_or_array()?;
            params.push((pname, auth, pty));
            if *self.peek() == Tok::Comma {
                self.bump();
            } else {
                break;
            }
        }
        self.eat(&Tok::RParen)?;
        let ret = if *self.peek() == Tok::Arrow {
            self.bump();
            self.parse_ty_or_array()?
        } else {
            Ty::Void
        };
        let body = if is_extern {
            self.eat(&Tok::Semicolon)?;
            Vec::new()
        } else {
            self.parse_block()?
        };
        Ok(FnDef {
            name,
            tparams,
            params,
            ret,
            body,
            is_extern,
            is_pub,
            span,
        })
    }

    fn parse_block(&mut self) -> Result<Vec<Stmt>, Vec<Diag>> {
        self.eat(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != Tok::RBrace {
            stmts.push(self.parse_stmt()?);
        }
        self.eat(&Tok::RBrace)?;
        Ok(stmts)
    }

    /// `{ V(b) => .., _ => .. }` — shared by `Stmt::Match` and
    /// `Expr::Match` (N28). Arm bodies: `=> { stmts }` is a block
    /// (yields nothing); `=> expr` is a yield arm (`expr_arm`,
    /// body = one `Stmt::Expr`); `=> return e` is a diverging arm.
    fn parse_match_arms(&mut self) -> Result<Vec<MatchArm>, Vec<Diag>> {
        self.eat(&Tok::LBrace)?;
        let mut arms = Vec::new();
        while *self.peek() != Tok::RBrace {
            let (variant, _) = self.expect_ident()?;
            let bind = if *self.peek() == Tok::LParen {
                self.bump();
                let (b, _) = self.expect_ident()?;
                self.eat(&Tok::RParen)?;
                Some(b)
            } else {
                None
            };
            self.eat(&Tok::FatArrow)?;
            let span = self.span();
            let (expr_arm, body) = match self.peek() {
                Tok::Return => {
                    self.bump();
                    let e = self.parse_expr()?;
                    (
                        false,
                        vec![Stmt::Return {
                            value: Some(e),
                            span,
                        }],
                    )
                }
                Tok::LBrace => {
                    // `=> { stmts; expr? }` — a block arm. A trailing
                    // bare expression (no `;`) is the arm's yield;
                    // everything else is a statement, including `;`-ed
                    // expression statements.
                    self.bump();
                    let mut body = Vec::new();
                    let mut expr_arm = false;
                    while *self.peek() != Tok::RBrace {
                        let stmt_start = matches!(
                            self.peek(),
                            Tok::Let | Tok::Return | Tok::If | Tok::While | Tok::Region
                        ) || matches!(
                            self.peek(),
                            Tok::Ident(n)
                                if matches!(
                                    n.as_str(),
                                    "match" | "consume" | "promote" | "break" | "continue"
                                        | "defer"
                                )
                        );
                        if stmt_start {
                            body.push(self.parse_stmt()?);
                            continue;
                        }
                        let espan = self.span();
                        let e = self.parse_expr()?;
                        match self.peek() {
                            Tok::Assign => {
                                self.bump();
                                let value = self.parse_expr()?;
                                self.eat(&Tok::Semicolon)?;
                                body.push(Stmt::Assign {
                                    target: e,
                                    value,
                                    span: espan,
                                });
                            }
                            Tok::Semicolon => {
                                self.bump();
                                body.push(Stmt::Expr {
                                    expr: e,
                                    span: espan,
                                });
                            }
                            Tok::RBrace => {
                                body.push(Stmt::Expr {
                                    expr: e,
                                    span: espan,
                                });
                                expr_arm = true;
                            }
                            t => {
                                return Err(vec![Diag::new(
                                    self.span(),
                                    format!("expected `;` or `}}` after expression, found {}", t.describe()),
                                )]);
                            }
                        }
                    }
                    self.eat(&Tok::RBrace)?;
                    (expr_arm, body)
                }
                _ => (
                    true,
                    vec![Stmt::Expr {
                        expr: self.parse_expr()?,
                        span,
                    }],
                ),
            };
            arms.push(MatchArm {
                variant,
                bind,
                expr_arm,
                body,
            });
            if *self.peek() == Tok::Comma {
                self.bump();
            }
        }
        self.eat(&Tok::RBrace)?;
        Ok(arms)
    }

    fn parse_stmt(&mut self) -> Result<Stmt, Vec<Diag>> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Let => {
                self.bump();
                let (name, name_span) = self.expect_ident()?;
                let ty = if *self.peek() == Tok::Colon {
                    self.bump();
                    Some(self.parse_ty_or_array()?)
                } else {
                    None
                };
                let init = if *self.peek() == Tok::Assign {
                    self.bump();
                    Some(self.parse_expr()?)
                } else {
                    None
                };
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Let {
                    name,
                    ty,
                    init,
                    span,
                    name_span,
                })
            }
            Tok::Return => {
                self.bump();
                let value = if *self.peek() == Tok::Semicolon {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Return { value, span })
            }
            Tok::If => {
                self.bump();
                let cond = self.parse_expr()?;
                let then_body = self.parse_block()?;
                let else_body = if *self.peek() == Tok::Else {
                    self.bump();
                    Some(self.parse_block()?)
                } else {
                    None
                };
                Ok(Stmt::If {
                    cond,
                    then_body,
                    else_body,
                    span,
                })
            }
            Tok::While => {
                self.bump();
                let cond = self.parse_expr()?;
                let body = self.parse_block()?;
                Ok(Stmt::While { cond, body, span })
            }
            Tok::Region => {
                self.bump();
                let (name, _) = self.expect_ident()?;
                let body = self.parse_block()?;
                Ok(Stmt::Region { name, body, span })
            }
            Tok::Ident(n) if n == "break" => {
                self.bump();
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Break { span })
            }
            Tok::Ident(n) if n == "continue" => {
                self.bump();
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Continue { span })
            }
            Tok::Ident(n) if n == "consume" => {
                self.bump();
                let target = self.parse_unary()?;
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Consume { target, span })
            }
            // `defer f(..)` — scoped cleanup (N54). Contextual like
            // `consume`: a variable named `defer` can still assign.
            Tok::Ident(n) if n == "defer" => {
                self.bump();
                let expr = self.parse_expr()?;
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Defer { expr, span })
            }
            // `match e { V(b) => { .. }, .. }` — enum discrimination
            // (N22). `V(b)` binds the payload; `_` is the catch-all.
            Tok::Ident(n) if n == "match" => {
                self.bump();
                let scrut = self.parse_expr()?;
                return Ok(Stmt::Match {
                    scrut,
                    arms: self.parse_match_arms()?,
                    span,
                });
            }
            Tok::Ident(n) if n == "promote" => {
                // `promote x;` — contextual: `promote` followed by a
                // place expression. A variable named `promote` can
                // still be assigned (`promote = e` parses as assign
                // because `=` can't start an expression).
                self.bump();
                let target = self.parse_unary()?;
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Promote { target, span })
            }
            _ => {
                // `place = e;` (assignment) vs `e;` (expr stmt) — `=` is
                // not a binary op, so parse the lhs as an expression and
                // let the checker rule on whether it's a valid place.
                let e = self.parse_expr()?;
                if *self.peek() == Tok::Assign {
                    self.bump();
                    let value = self.parse_expr()?;
                    self.eat(&Tok::Semicolon)?;
                    return Ok(Stmt::Assign {
                        target: e,
                        value,
                        span,
                    });
                }
                self.eat(&Tok::Semicolon)?;
                Ok(Stmt::Expr { expr: e, span })
            }
        }
    }

    fn parse_expr(&mut self) -> Result<Expr, Vec<Diag>> {
        self.parse_lor()
    }
    // N25C — short-circuit levels sit loosest: `||` above `&&`
    // above `|`, C-shaped.
    fn parse_lor(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_land, &[Tok::PipePipe], |_| BinOp::LOr)
    }
    fn parse_land(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_bor, &[Tok::AmpAmp], |_| BinOp::LAnd)
    }
    fn bin(
        &mut self,
        next: fn(&mut Self) -> Result<Expr, Vec<Diag>>,
        ops: &[Tok],
        f: fn(&Tok) -> BinOp,
    ) -> Result<Expr, Vec<Diag>> {
        let mut l = next(self)?;
        while ops.contains(self.peek()) {
            let span = self.span();
            let op = f(&self.bump());
            let r = next(self)?;
            l = Expr::Bin(op, Box::new(l), Box::new(r), span);
        }
        Ok(l)
    }
    // N25A — C-shaped precedence: `|` above `^` above `&` above
    // `==`, and `<<`/`>>` between comparison and additive.
    fn parse_bor(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_bxor, &[Tok::Pipe], |_| BinOp::BitOr)
    }
    fn parse_bxor(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_band, &[Tok::Caret], |_| BinOp::BitXor)
    }
    fn parse_band(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_eq, &[Tok::Amp], |_| BinOp::BitAnd)
    }
    fn parse_eq(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_cmp, &[Tok::Eq, Tok::Ne], |t| match t {
            Tok::Eq => BinOp::Eq,
            _ => BinOp::Ne,
        })
    }
    fn parse_cmp(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(
            Self::parse_shift,
            &[Tok::Lt, Tok::Gt, Tok::Le, Tok::Ge],
            |t| match t {
                Tok::Lt => BinOp::Lt,
                Tok::Gt => BinOp::Gt,
                Tok::Le => BinOp::Le,
                _ => BinOp::Ge,
            },
        )
    }
    fn parse_shift(&mut self) -> Result<Expr, Vec<Diag>> {
        // `<<`/`>>` arrive as two tokens — `Vec<Vec<i32>>` needs the
        // `>>` pair split in type position, so the lexer never
        // merges; the merge is an expression-level rule instead.
        let pair = |p: &Self, t: Tok| {
            p.toks[p.i].tok == t && p.toks.get(p.i + 1).map(|x| &x.tok) == Some(&t)
        };
        let mut l = self.parse_add()?;
        loop {
            let op = if pair(self, Tok::Lt) {
                BinOp::Shl
            } else if pair(self, Tok::Gt) {
                BinOp::Shr
            } else {
                break;
            };
            let span = self.span();
            self.bump();
            self.bump();
            let r = self.parse_add()?;
            l = Expr::Bin(op, Box::new(l), Box::new(r), span);
        }
        Ok(l)
    }
    fn parse_add(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(Self::parse_mul, &[Tok::Plus, Tok::Minus], |t| match t {
            Tok::Plus => BinOp::Add,
            _ => BinOp::Sub,
        })
    }
    fn parse_mul(&mut self) -> Result<Expr, Vec<Diag>> {
        self.bin(
            Self::parse_unary,
            &[Tok::Star, Tok::Slash, Tok::Percent],
            |t| match t {
                Tok::Star => BinOp::Mul,
                Tok::Slash => BinOp::Div,
                _ => BinOp::Mod,
            },
        )
    }
    fn parse_unary(&mut self) -> Result<Expr, Vec<Diag>> {
        let span = self.span();
        match self.peek() {
            Tok::Minus => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Unary(UnOp::Neg, Box::new(e), span))
            }
            Tok::Bang => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Unary(UnOp::Not, Box::new(e), span))
            }
            Tok::Tilde => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Unary(UnOp::BitNot, Box::new(e), span))
            }
            Tok::Amp => {
                self.bump();
                let mutable = *self.peek() == Tok::Mut;
                if mutable {
                    self.bump();
                }
                let place = self.parse_unary()?;
                Ok(Expr::AddrOf {
                    mutable,
                    place: Box::new(place),
                    span,
                })
            }
            Tok::Star => {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Deref(Box::new(e), span))
            }
            Tok::Ident(n)
                if n == "handle"
                    && matches!(
                        self.toks[self.i + 1].tok,
                        Tok::Ident(_)
                            | Tok::Int(_)
                            | Tok::LParen
                            | Tok::Amp
                            | Tok::Star
                    ) =>
            {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Handle(Box::new(e), span))
            }
            Tok::Ident(n) if n == "spawn" && self.toks[self.i + 1].tok == Tok::LBrace => {
                self.bump();
                let body = self.parse_block()?;
                Ok(Expr::Spawn(body, span))
            }
            Tok::Ident(n)
                if n == "join"
                    && matches!(
                        self.toks[self.i + 1].tok,
                        Tok::Ident(_) | Tok::Star | Tok::LParen
                    ) =>
            {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Join(Box::new(e), span))
            }
            Tok::Ident(n)
                if n == "mutex"
                    && matches!(
                        self.toks[self.i + 1].tok,
                        Tok::Ident(_)
                            | Tok::Int(_)
                            | Tok::LParen
                            | Tok::Amp
                            | Tok::Star
                    ) =>
            {
                self.bump();
                let e = self.parse_unary()?;
                Ok(Expr::Mutex(Box::new(e), span))
            }
            _ => self.parse_postfix(),
        }
    }

    /// Postfix path segments: `e.name` and `e[i]` — the surface syntax
    /// of what the IR calls a Place.
    fn parse_postfix(&mut self) -> Result<Expr, Vec<Diag>> {
        let mut e = self.parse_primary()?;
        loop {
            let span = self.span();
            match self.peek() {
                Tok::Dot => {
                    self.bump();
                    let (name, _) = self.expect_ident()?;
                    // `e.name(..)` / `e.name<T>(..)` — a method call
                    // (N20); `e.name` stays a field otherwise.
                    let mut targs = Vec::new();
                    let mut is_call = false;
                    if *self.peek() == Tok::LParen {
                        is_call = true;
                    } else if *self.peek() == Tok::Lt {
                        let save = self.i;
                        if let Ok(t) = self.parse_targs() {
                            if *self.peek() == Tok::LParen {
                                targs = t;
                                is_call = true;
                            } else {
                                self.i = save;
                            }
                        } else {
                            self.i = save;
                        }
                    }
                    if is_call {
                        self.eat(&Tok::LParen)?;
                        let mut args = Vec::new();
                        while *self.peek() != Tok::RParen {
                            args.push(self.parse_expr()?);
                            if *self.peek() == Tok::Comma {
                                self.bump();
                            } else {
                                break;
                            }
                        }
                        self.eat(&Tok::RParen)?;
                        e = Expr::MethodCall {
                            recv: Box::new(e),
                            name,
                            targs,
                            args,
                            span,
                        };
                    } else {
                        e = Expr::Field(Box::new(e), name, span);
                    }
                }
                Tok::Question => {
                    // `expr?` — Result propagation (N23).
                    self.bump();
                    e = Expr::Try {
                        inner: Box::new(e),
                        span,
                    };
                }
                Tok::LBracket => {
                    self.bump();
                    // `a[i]` vs `a[lo..hi]` — a `..` anywhere makes it
                    // a range, meaningful only under `&`/`&mut`.
                    if *self.peek() == Tok::DotDot {
                        self.bump();
                        let hi = if *self.peek() != Tok::RBracket {
                            Some(Box::new(self.parse_expr()?))
                        } else {
                            None
                        };
                        self.eat(&Tok::RBracket)?;
                        e = Expr::IndexRange {
                            base: Box::new(e),
                            lo: None,
                            hi,
                            span,
                        };
                        continue;
                    }
                    let i = self.parse_expr()?;
                    if *self.peek() == Tok::DotDot {
                        self.bump();
                        let hi = if *self.peek() != Tok::RBracket {
                            Some(Box::new(self.parse_expr()?))
                        } else {
                            None
                        };
                        self.eat(&Tok::RBracket)?;
                        e = Expr::IndexRange {
                            base: Box::new(e),
                            lo: Some(Box::new(i)),
                            hi,
                            span,
                        };
                        continue;
                    }
                    self.eat(&Tok::RBracket)?;
                    e = Expr::Index(Box::new(e), Box::new(i), span);
                }
                _ => return Ok(e),
            }
        }
    }
    fn parse_primary(&mut self) -> Result<Expr, Vec<Diag>> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(Expr::Int(n, span))
            }
            Tok::Str(bytes) => {
                self.bump();
                Ok(Expr::Str(bytes, span))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::Bool(true, span))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::Bool(false, span))
            }
            Tok::Ident(n) => {
                self.bump();
                // `match e { V => expr }` — expression-position match
                // (N28). Same contextual-keyword tradeoff as the
                // statement form: `match` can't be an expr-level var.
                if n == "match" {
                    let scrut = self.parse_expr()?;
                    let arms = self.parse_match_arms()?;
                    return Ok(Expr::Match {
                        scrut: Box::new(scrut),
                        arms,
                        span,
                    });
                }
                // `m.f(...)` / `m.f<T>(...)` — a qualified call into
                // module `m` (N17/N19). `m.T{..}`/`m.T<T>{..}` likewise —
                // but literals need the same `ident:` guard as
                // unqualified ones, else `x.f {` in an `if`/`while`
                // condition would eat the block as fields. `<` after
                // the member MIGHT be targs or `m.f < x` — parse
                // speculatively and restore on failure.
                if *self.peek() == Tok::Dot
                    && matches!(self.toks[self.i + 1].tok, Tok::Ident(_))
                    && (self.toks[self.i + 2].tok == Tok::LParen
                        || self.toks[self.i + 2].tok == Tok::Lt
                        || self.toks[self.i + 2].tok == Tok::ColonColon
                        || (self.toks[self.i + 2].tok == Tok::LBrace
                            && matches!(self.toks[self.i + 3].tok, Tok::Ident(_))
                            && self.toks[self.i + 4].tok == Tok::Colon))
                {
                    let save = self.i;
                    self.bump(); // .
                    let (member, _) = self.expect_ident()?;
                    let qname = format!("{n}.{member}");
                    let mut targs = Vec::new();
                    let mut ok = true;
                    if *self.peek() == Tok::Lt {
                        match self.parse_targs() {
                            Ok(t) => targs = t,
                            Err(_) => ok = false,
                        }
                    }
                    if ok {
                        match self.peek() {
                            Tok::ColonColon => {
                                let ty = if targs.is_empty() {
                                    Ty::Struct(qname)
                                } else {
                                    Ty::App(qname, targs)
                                };
                                return self.parse_assoc_args(ty, span);
                            }
                            Tok::LParen => {
                                self.bump();
                                let mut args = Vec::new();
                                while *self.peek() != Tok::RParen {
                                    args.push(self.parse_expr()?);
                                    if *self.peek() == Tok::Comma {
                                        self.bump();
                                    } else {
                                        break;
                                    }
                                }
                                self.eat(&Tok::RParen)?;
                                return Ok(Expr::Call(qname, targs, args, span));
                            }
                            Tok::LBrace
                                if matches!(self.toks[self.i + 1].tok, Tok::Ident(_))
                                    && self.toks[self.i + 2].tok == Tok::Colon =>
                            {
                                self.bump();
                                let mut fields = Vec::new();
                                while *self.peek() != Tok::RBrace {
                                    let (fname, _) = self.expect_ident()?;
                                    self.eat(&Tok::Colon)?;
                                    let fe = self.parse_expr()?;
                                    fields.push((fname, fe));
                                    if *self.peek() == Tok::Comma {
                                        self.bump();
                                    } else {
                                        break;
                                    }
                                }
                                self.eat(&Tok::RBrace)?;
                                return Ok(Expr::StructLit(qname, targs, fields, span));
                            }
                            _ => ok = false,
                        }
                    }
                    if !ok {
                        // `m.f < x` — an ordinary field read compared.
                        self.i = save;
                    }
                }
                // `sizeof<T>` — byte size as u64 (N19).
                if n == "sizeof" && *self.peek() == Tok::Lt {
                    self.bump();
                    let ty = self.parse_ty_or_array()?;
                    self.eat(&Tok::Gt)?;
                    return Ok(Expr::Sizeof(Box::new(ty), span));
                }
                // `alloc<T>(a)` / `alloc_slice<T>(a, n)` — the only
                // name-carried type argument the grammar needs.
                if (n == "alloc" || n == "alloc_slice") && *self.peek() == Tok::Lt {
                    self.bump();
                    let ty = self.parse_ty_or_array()?;
                    self.eat(&Tok::Gt)?;
                    self.eat(&Tok::LParen)?;
                    let mut args = Vec::new();
                    while *self.peek() != Tok::RParen {
                        args.push(self.parse_expr()?);
                        if *self.peek() == Tok::Comma {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    let (alloc, count) = if n == "alloc_slice" {
                        if args.len() != 2 {
                            return self.err("`alloc_slice` takes (alloc, count)");
                        }
                        let c = args.pop().unwrap();
                        (args.pop().unwrap(), Some(Box::new(c)))
                    } else {
                        if args.len() != 1 {
                            return self.err("`alloc` takes (alloc)");
                        }
                        (args.pop().unwrap(), None)
                    };
                    return Ok(Expr::Alloc {
                        ty: Box::new(ty),
                        alloc: Box::new(alloc),
                        count,
                        span,
                    });
                }
                // `Vec::new(...)` — associated fn, no type args (N20).
                if *self.peek() == Tok::ColonColon {
                    return self.parse_assoc_args(Ty::Struct(n), span);
                }
                // `f<T>(args)` / `Vec<T>{..}` — explicit instantiation
                // (N19). Speculative: `x < y` fails the targ parse and
                // restores to comparison; `f<T>` without a call/lit
                // following also restores (`a < b > c` stays a chain).
                if *self.peek() == Tok::Lt {
                    let save = self.i;
                    if let Ok(targs) = self.parse_targs() {
                        match self.peek() {
                            Tok::ColonColon => {
                                return self
                                    .parse_assoc_args(Ty::App(n, targs), span);
                            }
                            Tok::LParen => {
                                self.bump();
                                let mut args = Vec::new();
                                while *self.peek() != Tok::RParen {
                                    args.push(self.parse_expr()?);
                                    if *self.peek() == Tok::Comma {
                                        self.bump();
                                    } else {
                                        break;
                                    }
                                }
                                self.eat(&Tok::RParen)?;
                                return Ok(Expr::Call(n, targs, args, span));
                            }
                            Tok::LBrace
                                if matches!(self.toks[self.i + 1].tok, Tok::Ident(_))
                                    && self.toks[self.i + 2].tok == Tok::Colon =>
                            {
                                self.bump();
                                let mut fields = Vec::new();
                                while *self.peek() != Tok::RBrace {
                                    let (fname, _) = self.expect_ident()?;
                                    self.eat(&Tok::Colon)?;
                                    let fe = self.parse_expr()?;
                                    fields.push((fname, fe));
                                    if *self.peek() == Tok::Comma {
                                        self.bump();
                                    } else {
                                        break;
                                    }
                                }
                                self.eat(&Tok::RBrace)?;
                                return Ok(Expr::StructLit(n, targs, fields, span));
                            }
                            _ => self.i = save,
                        }
                    } else {
                        self.i = save;
                    }
                }
                if *self.peek() == Tok::LBrace
                    && matches!(self.toks[self.i + 1].tok, Tok::Ident(_))
                    && self.toks[self.i + 2].tok == Tok::Colon
                {
                    self.bump();
                    let mut fields = Vec::new();
                    while *self.peek() != Tok::RBrace {
                        let (fname, _) = self.expect_ident()?;
                        self.eat(&Tok::Colon)?;
                        let fe = self.parse_expr()?;
                        fields.push((fname, fe));
                        if *self.peek() == Tok::Comma {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.eat(&Tok::RBrace)?;
                    Ok(Expr::StructLit(n, Vec::new(), fields, span))
                } else if *self.peek() == Tok::LParen {
                    self.bump();
                    let mut args = Vec::new();
                    while *self.peek() != Tok::RParen {
                        args.push(self.parse_expr()?);
                        if *self.peek() == Tok::Comma {
                            self.bump();
                        } else {
                            break;
                        }
                    }
                    self.eat(&Tok::RParen)?;
                    Ok(Expr::Call(n, Vec::new(), args, span))
                } else {
                    Ok(Expr::Var(n, span))
                }
            }
            Tok::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            t => self.err(format!("expected expression, found {}", t.describe())),
        }
    }
}
