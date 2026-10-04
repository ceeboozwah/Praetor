//! Recursive-descent parser for MML.

use crate::ast::*;
use crate::lexer::{Tok, Token};

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    /// When true, an `Ident` followed by `{` is NOT treated as a struct literal.
    /// Used while parsing `if`/`while` conditions so the `{` opens the block.
    no_struct: bool,
}

type PResult<T> = Result<T, String>;

impl Parser {
    pub fn new(toks: Vec<Token>) -> Self {
        Parser {
            toks,
            pos: 0,
            no_struct: false,
        }
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }

    fn span(&self) -> Span {
        self.toks[self.pos].span
    }

    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos].tok.clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }

    fn eat(&mut self, want: &Tok) -> PResult<()> {
        if self.peek() == want {
            self.bump();
            Ok(())
        } else {
            Err(format!(
                "at {}: expected {:?}, found {:?}",
                self.span(),
                want,
                self.peek()
            ))
        }
    }

    fn expect_ident(&mut self) -> PResult<String> {
        match self.bump() {
            Tok::Ident(s) => Ok(s),
            other => Err(format!("at {}: expected identifier, found {other:?}", self.span())),
        }
    }

    pub fn parse_program(&mut self) -> PResult<Program> {
        let mut items = Vec::new();
        while *self.peek() != Tok::Eof {
            items.push(self.parse_item()?);
        }
        Ok(Program { items })
    }

    fn parse_item(&mut self) -> PResult<Item> {
        match self.peek() {
            Tok::Struct => Ok(Item::Struct(self.parse_struct()?)),
            Tok::Fn => Ok(Item::Fn(self.parse_fn()?)),
            other => Err(format!(
                "at {}: expected `struct` or `fn`, found {other:?}",
                self.span()
            )),
        }
    }

    fn parse_struct(&mut self) -> PResult<StructDef> {
        let span = self.span();
        self.eat(&Tok::Struct)?;
        let name = self.expect_ident()?;
        self.eat(&Tok::LBrace)?;
        let mut fields = Vec::new();
        while *self.peek() != Tok::RBrace {
            let fname = self.expect_ident()?;
            self.eat(&Tok::Colon)?;
            let ty = self.parse_type()?;
            fields.push(Field { name: fname, ty });
            if *self.peek() == Tok::Comma {
                self.bump();
            }
        }
        self.eat(&Tok::RBrace)?;
        Ok(StructDef { name, fields, span })
    }

    fn parse_fn(&mut self) -> PResult<FnDef> {
        let span = self.span();
        self.eat(&Tok::Fn)?;
        let name = self.expect_ident()?;
        self.eat(&Tok::LParen)?;
        let mut params = Vec::new();
        while *self.peek() != Tok::RParen {
            params.push(self.parse_param()?);
            if *self.peek() == Tok::Comma {
                self.bump();
            }
        }
        self.eat(&Tok::RParen)?;
        let ret = if *self.peek() == Tok::Arrow {
            self.bump();
            Some(self.parse_type()?)
        } else {
            None
        };
        let body = self.parse_block()?;
        Ok(FnDef {
            name,
            params,
            ret,
            body,
            span,
        })
    }

    fn parse_param(&mut self) -> PResult<Param> {
        let span = self.span();
        let mode = match self.peek() {
            Tok::Own => {
                self.bump();
                Mode::Own
            }
            Tok::Ref => {
                self.bump();
                Mode::Ref
            }
            Tok::Mut => {
                self.bump();
                Mode::Mut
            }
            // default mode is `own` (by-value) when unannotated
            _ => Mode::Own,
        };
        let name = self.expect_ident()?;
        self.eat(&Tok::Colon)?;
        let ty = self.parse_type()?;
        Ok(Param {
            mode,
            name,
            ty,
            span,
        })
    }

    fn parse_type(&mut self) -> PResult<Type> {
        match self.peek() {
            Tok::Amp => {
                self.bump();
                if *self.peek() == Tok::Mut {
                    self.bump();
                    Ok(Type::MutRef(Box::new(self.parse_type()?)))
                } else {
                    Ok(Type::Ref(Box::new(self.parse_type()?)))
                }
            }
            Tok::Mutex => {
                self.bump();
                self.eat(&Tok::Lt)?;
                let inner = self.parse_type()?;
                self.eat(&Tok::Gt)?;
                Ok(Type::Mutex(Box::new(inner)))
            }
            Tok::Ident(_) => {
                let name = self.expect_ident()?;
                // `atomic<T>` / `channel<T>` — parameterized types whose
                // names are contextual idents, not keywords.
                if *self.peek() == Tok::Lt {
                    let ctor = match name.as_str() {
                        "atomic" => Some(Type::Atomic as fn(Box<Type>) -> Type),
                        "channel" => Some(Type::Channel as fn(Box<Type>) -> Type),
                        _ => None,
                    };
                    if let Some(ctor) = ctor {
                        self.bump();
                        let inner = self.parse_type()?;
                        self.eat(&Tok::Gt)?;
                        return Ok(ctor(Box::new(inner)));
                    }
                }
                Ok(Type::Named(name))
            }
            other => Err(format!("at {}: expected type, found {other:?}", self.span())),
        }
    }

    fn parse_block(&mut self) -> PResult<Block> {
        let span = self.span();
        self.eat(&Tok::LBrace)?;
        let mut stmts = Vec::new();
        while *self.peek() != Tok::RBrace {
            stmts.push(self.parse_stmt()?);
        }
        self.eat(&Tok::RBrace)?;
        Ok(Block { stmts, span })
    }

    fn parse_stmt(&mut self) -> PResult<Stmt> {
        let span = self.span();
        match self.peek() {
            Tok::Let => {
                self.bump();
                let name = self.expect_ident()?;
                let ty = if *self.peek() == Tok::Colon {
                    self.bump();
                    Some(self.parse_type()?)
                } else {
                    None
                };
                self.eat(&Tok::Assign)?;
                let init = self.parse_expr()?;
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Let {
                    name,
                    ty,
                    init,
                    span,
                })
            }
            Tok::Return => {
                self.bump();
                let value = if *self.peek() == Tok::Semi {
                    None
                } else {
                    Some(self.parse_expr()?)
                };
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Return { value, span })
            }
            Tok::If => {
                self.bump();
                let cond = self.parse_cond()?;
                let then_block = self.parse_block()?;
                let else_block = if *self.peek() == Tok::Else {
                    self.bump();
                    Some(self.parse_block()?)
                } else {
                    None
                };
                Ok(Stmt::If {
                    cond,
                    then_block,
                    else_block,
                    span,
                })
            }
            Tok::While => {
                self.bump();
                let cond = self.parse_cond()?;
                let body = self.parse_block()?;
                Ok(Stmt::While { cond, body, span })
            }
            Tok::Region => {
                self.bump();
                let name = self.expect_ident()?;
                let body = self.parse_block()?;
                Ok(Stmt::Region { name, body, span })
            }
            Tok::Free => {
                self.bump();
                let value = self.parse_expr()?;
                self.eat(&Tok::Semi)?;
                Ok(Stmt::Free { value, span })
            }
            _ => {
                // expression statement or assignment
                let expr = self.parse_expr()?;
                if *self.peek() == Tok::Assign {
                    self.bump();
                    let value = self.parse_expr()?;
                    self.eat(&Tok::Semi)?;
                    Ok(Stmt::Assign {
                        target: expr,
                        value,
                        span,
                    })
                } else {
                    self.eat(&Tok::Semi)?;
                    Ok(Stmt::Expr { expr, span })
                }
            }
        }
    }

    // ----- expressions with precedence climbing -----

    fn parse_expr(&mut self) -> PResult<Expr> {
        self.parse_bin(0)
    }

    /// Parse an `if`/`while` condition, disabling struct-literal syntax so the
    /// trailing `{` is unambiguously the start of the block.
    fn parse_cond(&mut self) -> PResult<Expr> {
        let prev = self.no_struct;
        self.no_struct = true;
        let e = self.parse_expr();
        self.no_struct = prev;
        e
    }

    fn parse_bin(&mut self, min_prec: u8) -> PResult<Expr> {
        let mut lhs = self.parse_unary()?;
        loop {
            let (op, prec) = match self.peek() {
                Tok::OrOr => (BinOp::Or, 1),
                Tok::AndAnd => (BinOp::And, 2),
                Tok::EqEq => (BinOp::Eq, 3),
                Tok::Ne => (BinOp::Ne, 3),
                Tok::Lt => (BinOp::Lt, 4),
                Tok::Gt => (BinOp::Gt, 4),
                Tok::Pipe => (BinOp::BitOr, 5),
                Tok::Caret => (BinOp::BitXor, 6),
                Tok::Amp => (BinOp::BitAnd, 7),
                Tok::Shl => (BinOp::Shl, 8),
                Tok::Shr => (BinOp::Shr, 8),
                Tok::Plus => (BinOp::Add, 9),
                Tok::Minus => (BinOp::Sub, 9),
                Tok::Star => (BinOp::Mul, 10),
                Tok::Slash => (BinOp::Div, 10),
                _ => break,
            };
            if prec < min_prec {
                break;
            }
            let span = self.span();
            self.bump();
            let rhs = self.parse_bin(prec + 1)?;
            lhs = Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                span,
            };
        }
        Ok(lhs)
    }

    fn parse_unary(&mut self) -> PResult<Expr> {
        let span = self.span();
        match self.peek() {
            Tok::Amp => {
                self.bump();
                if *self.peek() == Tok::Mut {
                    self.bump();
                    Ok(Expr::MutRef(Box::new(self.parse_unary()?), span))
                } else {
                    Ok(Expr::Ref(Box::new(self.parse_unary()?), span))
                }
            }
            Tok::Star => {
                self.bump();
                Ok(Expr::Deref(Box::new(self.parse_unary()?), span))
            }
            Tok::Promote => {
                self.bump();
                Ok(Expr::Persist {
                    kind: PersistKind::Promote,
                    inner: Box::new(self.parse_unary()?),
                    span,
                })
            }
            Tok::Mutex => {
                self.bump();
                Ok(Expr::Persist {
                    kind: PersistKind::Mutex,
                    inner: Box::new(self.parse_unary()?),
                    span,
                })
            }
            Tok::Handle => {
                self.bump();
                Ok(Expr::Persist {
                    kind: PersistKind::Handle,
                    inner: Box::new(self.parse_unary()?),
                    span,
                })
            }
            Tok::Spawn => {
                self.bump();
                Ok(Expr::Spawn {
                    body: self.parse_block()?,
                    span,
                })
            }
            Tok::Join => {
                self.bump();
                Ok(Expr::Join {
                    inner: Box::new(self.parse_unary()?),
                    span,
                })
            }
            _ => self.parse_postfix(),
        }
    }

    fn parse_postfix(&mut self) -> PResult<Expr> {
        let mut e = self.parse_primary()?;
        loop {
            match self.peek() {
                Tok::Dot => {
                    let span = self.span();
                    self.bump();
                    let field = self.expect_ident()?;
                    e = Expr::Field(Box::new(e), field, span);
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn parse_primary(&mut self) -> PResult<Expr> {
        let span = self.span();
        match self.peek().clone() {
            Tok::Int(n) => {
                self.bump();
                Ok(Expr::Int(n, span))
            }
            Tok::True => {
                self.bump();
                Ok(Expr::Bool(true, span))
            }
            Tok::False => {
                self.bump();
                Ok(Expr::Bool(false, span))
            }
            Tok::LParen => {
                self.bump();
                let e = self.parse_expr()?;
                self.eat(&Tok::RParen)?;
                Ok(e)
            }
            Tok::New => {
                self.bump();
                let region = if *self.peek() == Tok::LParen {
                    self.bump();
                    let r = self.expect_ident()?;
                    self.eat(&Tok::RParen)?;
                    Some(r)
                } else {
                    None
                };
                let ty = self.parse_type()?;
                Ok(Expr::Alloc { region, ty, span })
            }
            Tok::Ident(name) => {
                self.bump();
                match self.peek() {
                    // struct literal:  Name { field: expr, .. }
                    Tok::LBrace if !self.no_struct => {
                        self.bump();
                        let mut fields = Vec::new();
                        while *self.peek() != Tok::RBrace {
                            let fname = self.expect_ident()?;
                            self.eat(&Tok::Colon)?;
                            let val = self.parse_expr()?;
                            fields.push((fname, val));
                            if *self.peek() == Tok::Comma {
                                self.bump();
                            }
                        }
                        self.eat(&Tok::RBrace)?;
                        Ok(Expr::StructLit { name, fields, span })
                    }
                    // call:  f(args)
                    Tok::LParen => {
                        self.bump();
                        let mut args = Vec::new();
                        while *self.peek() != Tok::RParen {
                            args.push(self.parse_expr()?);
                            if *self.peek() == Tok::Comma {
                                self.bump();
                            }
                        }
                        self.eat(&Tok::RParen)?;
                        Ok(Expr::Call { name, args, span })
                    }
                    _ => Ok(Expr::Var(name, span)),
                }
            }
            other => Err(format!("at {}: unexpected token {other:?}", self.span())),
        }
    }
}

pub fn parse(toks: Vec<Token>) -> Result<Program, String> {
    Parser::new(toks).parse_program()
}
