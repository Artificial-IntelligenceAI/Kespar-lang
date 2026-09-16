//! Recursive descent for design/language.md §1, §3–§7, including the
//! parenthesis rules of §6.2 and the piece/operand rules of §6.1.

use crate::ast::*;
use crate::diag::{CompileError, Result};
use crate::lexer::{Tok, Token};

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    next_id: NodeId,
}

const KEYWORDS: &[&str] = &[
    "var", "func", "return", "if", "else", "loop", "in", "break", "continue", "check", "nocheck", "true", "false",
    "mut", "immut", "MAIN", "int8", "int16", "int32", "int64", "uint8", "uint16", "uint32", "uint64", "bin32",
    "bin64", "bool", "str", "utf8", "list", "while", "for",
];

pub fn is_keyword(s: &str) -> bool {
    KEYWORDS.contains(&s)
}

pub fn parse(toks: Vec<Token>) -> Result<Program> {
    let mut p = Parser { toks, pos: 0, next_id: 0 };
    p.program()
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, n: usize) -> &Tok {
        let i = (self.pos + n).min(self.toks.len() - 1);
        &self.toks[i].tok
    }
    fn line(&self) -> u32 {
        self.toks[self.pos].line
    }
    fn bump(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn err<T>(&self, msg: impl Into<String>) -> Result<T> {
        Err(CompileError::new(self.line(), msg))
    }
    fn expect(&mut self, tok: Tok, what: &str) -> Result<()> {
        if *self.peek() == tok {
            self.bump();
            Ok(())
        } else {
            self.err(format!("expected {what}, found {}", describe(self.peek())))
        }
    }
    fn is_ident(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Ident(i) if i == s)
    }
    fn expect_ident(&mut self, s: &str) -> Result<()> {
        if self.is_ident(s) {
            self.bump();
            Ok(())
        } else {
            self.err(format!("expected `{s}`, found {}", describe(self.peek())))
        }
    }
    fn mk(&mut self, line: u32, kind: ExprKind) -> Expr {
        let id = self.next_id;
        self.next_id += 1;
        Expr { id, line, kind }
    }

    // ---- program ----

    fn program(&mut self) -> Result<Program> {
        let mut funcs = Vec::new();
        let mut main = None;
        let mut main_line = 0;
        loop {
            match self.peek() {
                Tok::Eof => break,
                Tok::Ident(s) if s == "func" => funcs.push(self.func()?),
                Tok::Ident(s) if s == "MAIN" => {
                    if main.is_some() {
                        return self.err("a second `MAIN`; a file has one");
                    }
                    main_line = self.line();
                    self.bump();
                    main = Some(self.block()?);
                }
                _ => return self.err(format!("top level holds only `func` definitions and `MAIN`, found {}", describe(self.peek()))),
            }
        }
        match main {
            Some(main) => Ok(Program { funcs, main, main_line }),
            None => Err(CompileError::new(self.line(), "no `MAIN { }` in this file")),
        }
    }

    fn func(&mut self) -> Result<Func> {
        let line = self.line();
        self.expect_ident("func")?;
        let ret = if *self.peek() == Tok::Dot {
            self.bump();
            Some(self.type_chain()?)
        } else {
            None
        };
        let name = match self.bump().tok {
            Tok::Ident(s) if !is_keyword(&s) => s,
            Tok::Name(s) => return Err(CompileError::new(line, format!("function names are bare, not quoted: `func {s}`"))),
            other => return Err(CompileError::new(line, format!("expected a function name, found {}", describe(&other)))),
        };
        self.expect(Tok::LBrack, "`[` before the parameters")?;
        let mut params = Vec::new();
        if *self.peek() != Tok::RBrack {
            loop {
                params.push(self.param()?);
                if *self.peek() == Tok::Comma {
                    self.bump();
                    if *self.peek() == Tok::RBrack {
                        break;
                    }
                } else {
                    break;
                }
            }
        }
        self.expect(Tok::RBrack, "`]` after the parameters")?;
        let body = self.block()?;
        Ok(Func { name, line, ret, params, body })
    }

    fn param(&mut self) -> Result<Param> {
        let line = self.line();
        self.expect_ident("var")?;
        let mut immut = false;
        let mut ty = None;
        if *self.peek() == Tok::Dot {
            self.bump();
            if self.is_ident("immut") {
                self.bump();
                immut = true;
                if *self.peek() == Tok::Dot {
                    self.bump();
                    ty = Some(self.type_chain()?);
                }
            } else if self.is_ident("mut") {
                self.bump();
                if *self.peek() == Tok::Dot {
                    self.bump();
                    ty = Some(self.type_chain()?);
                }
            } else {
                ty = Some(self.type_chain()?);
            }
        }
        let name = match self.bump().tok {
            Tok::Name(s) => s,
            other => return Err(CompileError::new(line, format!("expected a parameter name like `'a'`, found {}", describe(&other)))),
        };
        Ok(Param { name, ty, immut, line })
    }

    /// `int32` | `bin64` | `bool` | `str.utf8` | `list.T`
    fn type_chain(&mut self) -> Result<TypeSpec> {
        let line = self.line();
        let word = match self.bump().tok {
            Tok::Ident(s) => s,
            other => return Err(CompileError::new(line, format!("expected a type, found {}", describe(&other)))),
        };
        if let Some(w) = IntWidth::from_name(&word) {
            return Ok(TypeSpec::Int(w));
        }
        match word.as_str() {
            "bin32" => Ok(TypeSpec::Bin(BinWidth::B32)),
            "bin64" => Ok(TypeSpec::Bin(BinWidth::B64)),
            "bool" => Ok(TypeSpec::Bool),
            "str" => {
                if *self.peek() == Tok::Dot && matches!(self.peek_at(1), Tok::Ident(s) if s == "utf8") {
                    self.bump();
                    self.bump();
                    Ok(TypeSpec::Str)
                } else if *self.peek() == Tok::Dot {
                    Err(CompileError::new(line, "Kespar's only text encoding is `utf8`: write `str.utf8`"))
                } else {
                    Err(CompileError::new(line, "the typed form is fully explicit: write `str.utf8`, never `str`"))
                }
            }
            "list" => {
                if *self.peek() != Tok::Dot {
                    return Err(CompileError::new(line, "`list` needs its element type: `list.int32`"));
                }
                self.bump();
                Ok(TypeSpec::List(Box::new(self.type_chain()?)))
            }
            "int" | "uint" => Err(CompileError::new(line, format!("the typed form is fully explicit: write `{word}32` (or another width), never `{word}`"))),
            "bin" => Err(CompileError::new(line, "the typed form is fully explicit: write `bin32` or `bin64`, never `bin`")),
            "bin16" | "bin128" | "int128" | "uint128" | "deci32" | "deci64" | "deci128" | "char" => {
                Err(CompileError::new(line, format!("`{word}` is a CyborgPL type that Kespar does not have")))
            }
            _ => Err(CompileError::new(line, format!("`{word}` is not a type"))),
        }
    }

    // ---- statements ----

    fn block(&mut self) -> Result<Vec<Stmt>> {
        self.expect(Tok::LBrace, "`{`")?;
        let mut stmts = Vec::new();
        while *self.peek() != Tok::RBrace {
            if *self.peek() == Tok::Eof {
                return self.err("`}` missing before the end of the file");
            }
            stmts.push(self.stmt()?);
        }
        self.bump();
        Ok(stmts)
    }

    fn semi(&mut self) -> Result<()> {
        self.expect(Tok::Semi, "`;`")
    }

    fn stmt(&mut self) -> Result<Stmt> {
        let line = self.line();
        let kind = match self.peek().clone() {
            Tok::Ident(w) => match w.as_str() {
                "var" => self.var_stmt()?,
                "if" => self.if_stmt()?,
                "loop" => self.loop_stmt()?,
                "break" => { self.bump(); self.semi()?; StmtKind::Break }
                "continue" => { self.bump(); self.semi()?; StmtKind::Continue }
                "return" => {
                    self.bump();
                    if *self.peek() == Tok::Semi {
                        self.bump();
                        StmtKind::Return(None)
                    } else {
                        let v = self.slot_one("the return value")?;
                        self.semi()?;
                        StmtKind::Return(Some(v))
                    }
                }
                "check" => { self.bump(); StmtKind::Tier(Tier::Check, self.block()?) }
                "nocheck" => { self.bump(); StmtKind::Tier(Tier::Nocheck, self.block()?) }
                "std" => {
                    let e = self.std_call()?;
                    let k = match e.kind {
                        ExprKind::Call(ref n, ref args) if n.starts_with("std::print") => {
                            StmtKind::Print { useful: n == "std::printu.stdout", pieces: args.clone() }
                        }
                        ExprKind::Call(ref n, ref args) if n == "std::exit" => {
                            if args.len() != 1 {
                                return Err(CompileError::new(line, "`std::exit` takes one value"));
                            }
                            StmtKind::Exit(args[0].clone())
                        }
                        ExprKind::Read(_) => return Err(CompileError::new(line, "`std::read.stdin` is a value; it must be the whole value of a typed `var` declaration")),
                        _ => return Err(CompileError::new(line, "this `std::` member is a value, not a statement")),
                    };
                    self.semi()?;
                    k
                }
                "func" => return self.err("functions are declared at top level, not inside a body"),
                "MAIN" => return self.err("`MAIN` is the program's block and appears once, at top level"),
                "else" => return self.err("`else` without an `if`"),
                _ if is_keyword(&w) => return self.err(format!("`{w}` cannot start a statement")),
                _ => {
                    // a call as a statement
                    let e = self.postfix()?;
                    match e.kind {
                        ExprKind::Call(..) => {}
                        _ => return Err(CompileError::new(line, "a bare expression is not a statement (only a call is)")),
                    }
                    self.semi()?;
                    StmtKind::CallStmt(e)
                }
            },
            Tok::Name(name) => {
                self.bump();
                match self.peek() {
                    Tok::Eq => {
                        self.bump();
                        let value = self.slot_one("the value")?;
                        self.semi()?;
                        StmtKind::Assign { name, value }
                    }
                    Tok::LBrack => {
                        let mut target = self.mk(line, ExprKind::Name(name));
                        let mut index = self.index_slot()?;
                        while self.adjacent_lbrack() {
                            let l = self.line();
                            target = self.mk(l, ExprKind::Index(Box::new(target), Box::new(index)));
                            index = self.index_slot()?;
                        }
                        self.expect(Tok::Eq, "`=` after the index")?;
                        let value = self.slot_one("the value")?;
                        self.semi()?;
                        StmtKind::AssignIndex { target, index, value }
                    }
                    Tok::Walrus | Tok::DollarWalrus | Tok::DollarEq => {
                        return self.err("a declaration starts with `var`")
                    }
                    _ => return self.err(format!("expected `=` or `[` after `'{name}'`, found {}", describe(self.peek()))),
                }
            }
            other => return self.err(format!("expected a statement, found {}", describe(&other))),
        };
        Ok(Stmt { line, kind })
    }

    fn var_stmt(&mut self) -> Result<StmtKind> {
        let line = self.line();
        self.expect_ident("var")?;
        let mut mutability = Mutability::Plain;
        let mut ty = None;
        if *self.peek() == Tok::Dot {
            self.bump();
            if self.is_ident("immut") {
                self.bump();
                mutability = Mutability::Immut;
                if *self.peek() == Tok::Dot {
                    self.bump();
                    ty = Some(self.type_chain()?);
                }
            } else if self.is_ident("mut") {
                self.bump();
                mutability = Mutability::Mut;
                if *self.peek() == Tok::Dot {
                    self.bump();
                    ty = Some(self.type_chain()?);
                }
            } else {
                ty = Some(self.type_chain()?);
            }
        }
        let name = match self.bump().tok {
            Tok::Name(s) => s,
            Tok::Ident(s) => return Err(CompileError::new(line, format!("variable names are quoted: `'{s}'`"))),
            other => return Err(CompileError::new(line, format!("expected a name like `'total'`, found {}", describe(&other)))),
        };
        let (shadow, walrus) = match self.bump().tok {
            Tok::Walrus => (false, true),
            Tok::Eq => (false, false),
            Tok::DollarWalrus => (true, true),
            Tok::DollarEq => (true, false),
            other => return Err(CompileError::new(line, format!("expected `:=` or `=` after `'{name}'`, found {}", describe(&other)))),
        };
        if walrus && ty.is_some() {
            return Err(CompileError::new(line, "`:=` is for the untyped form; with a type write `=`"));
        }
        if !walrus && ty.is_none() {
            return Err(CompileError::new(line, "`=` needs a type (`var.int32 'x' = [...]`); without one write `:=`"));
        }
        let value = self.slot_one("the value")?;
        self.semi()?;
        Ok(StmtKind::Var { mutability, ty, name, shadow, value })
    }

    fn if_stmt(&mut self) -> Result<StmtKind> {
        self.expect_ident("if")?;
        let mut branches = Vec::new();
        let cond = self.slot_one("the condition")?;
        let body = self.block()?;
        branches.push((cond, body));
        let mut else_body = None;
        while self.is_ident("else") {
            self.bump();
            if *self.peek() == Tok::Dot {
                self.bump();
                self.expect_ident("if")?;
                let cond = self.slot_one("the condition")?;
                let body = self.block()?;
                branches.push((cond, body));
            } else if self.is_ident("if") {
                return self.err("`else if` is written `else.if`");
            } else {
                else_body = Some(self.block()?);
                break;
            }
        }
        Ok(StmtKind::If { branches, else_body })
    }

    fn loop_stmt(&mut self) -> Result<StmtKind> {
        let line = self.line();
        self.expect_ident("loop")?;
        if *self.peek() == Tok::LBrace {
            return Ok(StmtKind::Loop(self.block()?));
        }
        self.expect(Tok::Dot, "`{`, `.while` or `.for` after `loop`")?;
        if self.is_ident("while") {
            self.bump();
            let cond = self.slot_one("the condition")?;
            let body = self.block()?;
            Ok(StmtKind::While(cond, body))
        } else if self.is_ident("for") {
            self.bump();
            let name = match self.bump().tok {
                Tok::Name(s) => s,
                other => return Err(CompileError::new(line, format!("expected the loop variable like `'i'`, found {}", describe(&other)))),
            };
            self.expect_ident("in")?;
            let iter = self.slot_one("the list or range")?;
            let body = self.block()?;
            Ok(StmtKind::For { name, iter, body })
        } else {
            self.err("after `loop.` comes `while` or `for`")
        }
    }

    // ---- value slots ----

    /// `[ v, v, ... ]` → the values. `[v]` is one value; `[v,]` is a
    /// one-element list; `[]` is no values.
    fn slot(&mut self) -> Result<Vec<Expr>> {
        self.expect(Tok::LBrack, "`[`")?;
        let mut vals = Vec::new();
        if *self.peek() == Tok::RBrack {
            self.bump();
            return Ok(vals);
        }
        loop {
            vals.push(self.value()?);
            match self.peek() {
                Tok::Comma => {
                    self.bump();
                    if *self.peek() == Tok::RBrack {
                        // trailing comma: `[1,]` is a one-element list
                        let line = self.line();
                        self.bump();
                        if vals.len() == 1 {
                            let v = vals.pop().unwrap();
                            return Ok(vec![self.mk(line, ExprKind::List(vec![v]))]);
                        }
                        return Ok(vals);
                    }
                }
                Tok::RBrack => {
                    self.bump();
                    return Ok(vals);
                }
                _ => return self.err(format!("expected `,` or `]`, found {}", describe(self.peek()))),
            }
        }
    }

    /// A slot holding one value; several comma-separated values are a list
    /// (§6.1: "a bracket holding several comma-separated values is a list").
    fn slot_one(&mut self, what: &str) -> Result<Expr> {
        let line = self.line();
        let mut vals = self.slot()?;
        match vals.len() {
            1 => Ok(vals.pop().unwrap()),
            0 => Err(CompileError::new(line, format!("`[ ]` is empty; {what} is missing"))),
            _ => Ok(self.mk(line, ExprKind::List(vals))),
        }
    }

    fn index_slot(&mut self) -> Result<Expr> {
        let line = self.line();
        let mut vals = self.slot()?;
        match vals.len() {
            1 => Ok(vals.pop().unwrap()),
            _ => Err(CompileError::new(line, "an index is exactly one value")),
        }
    }

    /// One value: a formula, or whitespace-joined pieces.
    fn value(&mut self) -> Result<Expr> {
        let line = self.line();
        let first = self.expr()?;
        // More pieces follow with no operator between?
        if !self.piece_starts() {
            return Ok(first);
        }
        let mut pieces = Vec::new();
        self.as_piece(first, &mut pieces)?;
        while self.piece_starts() {
            let e = self.expr()?;
            self.as_piece(e, &mut pieces)?;
        }
        Ok(self.mk(line, ExprKind::Pieces(pieces)))
    }

    fn piece_starts(&self) -> bool {
        matches!(self.peek(), Tok::Text(_) | Tok::Name(_) | Tok::Newline)
            || matches!(self.peek(), Tok::Int(_) | Tok::Bin(_) | Tok::LParen | Tok::Ident(_) | Tok::LBrack)
                && !matches!(self.peek(), Tok::Ident(s) if is_operator_word(s))
    }

    fn as_piece(&self, e: Expr, out: &mut Vec<Piece>) -> Result<()> {
        match e.kind {
            ExprKind::Text(s) => out.push(Piece::Text(s)),
            ExprKind::Name(n) => out.push(Piece::Name(n, e.line)),
            ExprKind::Pieces(ps) => out.extend(ps),
            ExprKind::Index(..) | ExprKind::Bracket(_) => out.push(Piece::Value(e)),
            ExprKind::Int(_) | ExprKind::Bin(..) => {
                return Err(CompileError::new(e.line, "a bare number is an operand, never a piece; give it a name first: `[\"count: \" 'n']`"))
            }
            _ => return Err(CompileError::new(e.line, "pieces are `\"text\"`, `'name'` and `\\n`; anything else needs an operator between")),
        }
        Ok(())
    }

    // ---- expressions (§6.2) ----

    fn expr(&mut self) -> Result<Expr> {
        self.or_expr()
    }

    fn or_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.and_expr()?;
        while self.is_ident("or") {
            let line = self.line();
            self.bump();
            let rhs = self.and_expr()?;
            self.no_bare_mod(&lhs, "or")?;
            self.no_bare_mod(&rhs, "or")?;
            lhs = self.mk(line, ExprKind::Binary(BinOp::Or, Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn and_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.not_expr()?;
        while self.is_ident("and") {
            let line = self.line();
            self.bump();
            let rhs = self.not_expr()?;
            self.no_bare_mod(&lhs, "and")?;
            self.no_bare_mod(&rhs, "and")?;
            lhs = self.mk(line, ExprKind::Binary(BinOp::And, Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn not_expr(&mut self) -> Result<Expr> {
        if self.is_ident("not") {
            let line = self.line();
            self.bump();
            let operand = self.not_expr()?;
            if let ExprKind::Binary(op, ..) = &operand.kind {
                if op.is_cmp() {
                    return Err(CompileError::new(line, "`not` beside a comparison: parenthesise (`not ('a' == 'b')`)"));
                }
            }
            self.no_bare_mod(&operand, "not")?;
            return Ok(self.mk(line, ExprKind::Unary(UnOp::Not, Box::new(operand))));
        }
        self.cmp_expr()
    }

    fn cmp_expr(&mut self) -> Result<Expr> {
        let lhs = self.add_expr()?;
        let op = match self.peek() {
            Tok::EqEq => BinOp::Eq,
            Tok::NotEq => BinOp::Ne,
            Tok::Lt => BinOp::Lt,
            Tok::Gt => BinOp::Gt,
            Tok::Le => BinOp::Le,
            Tok::Ge => BinOp::Ge,
            _ => return Ok(lhs),
        };
        let line = self.line();
        self.bump();
        let rhs = self.add_expr()?;
        if matches!(self.peek(), Tok::EqEq | Tok::NotEq | Tok::Lt | Tok::Gt | Tok::Le | Tok::Ge) {
            return self.err("comparisons are never chained (`1 < 2 < 3`); parenthesise or use `and`");
        }
        self.no_bare_mod(&lhs, op.spelling())?;
        self.no_bare_mod(&rhs, op.spelling())?;
        Ok(self.mk(line, ExprKind::Binary(op, Box::new(lhs), Box::new(rhs))))
    }

    fn add_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.mul_expr()?;
        loop {
            let op = match self.peek() {
                Tok::Plus => BinOp::Add,
                Tok::Minus => BinOp::Sub,
                _ => return Ok(lhs),
            };
            let line = self.line();
            self.bump();
            let rhs = self.mul_expr()?;
            self.no_bare_mod(&lhs, op.spelling())?;
            self.no_bare_mod(&rhs, op.spelling())?;
            lhs = self.mk(line, ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)));
        }
    }

    fn mul_expr(&mut self) -> Result<Expr> {
        let mut lhs = self.unary()?;
        let mut ops_in_chain: Vec<BinOp> = Vec::new();
        loop {
            let op = match self.peek() {
                Tok::Ident(s) if s == "x" => BinOp::Mul,
                Tok::Ident(s) if s == "mod" => BinOp::Mod,
                Tok::Slash => BinOp::Div,
                _ => return Ok(lhs),
            };
            let line = self.line();
            self.bump();
            let rhs = self.unary()?;
            ops_in_chain.push(op);
            if ops_in_chain.len() > 1 && ops_in_chain.iter().any(|o| matches!(o, BinOp::Div | BinOp::Mod)) {
                return Err(CompileError::new(line, "`/` or `mod` beside `x`, `/` or `mod` has no settled reading: parenthesise"));
            }
            lhs = self.mk(line, ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)));
        }
    }

    fn unary(&mut self) -> Result<Expr> {
        if *self.peek() == Tok::Minus {
            let line = self.line();
            self.bump();
            let operand = self.unary()?;
            self.no_bare_mod(&operand, "-")?;
            return Ok(self.mk(line, ExprKind::Unary(UnOp::Neg, Box::new(operand))));
        }
        self.power()
    }

    fn power(&mut self) -> Result<Expr> {
        let base = self.postfix()?;
        if self.is_ident("xx") {
            let line = self.line();
            self.bump();
            let exp = self.unary()?; // right-associative; `2 xx -1` allowed to parse
            self.no_bare_mod(&base, "xx")?;
            self.no_bare_mod(&exp, "xx")?;
            return Ok(self.mk(line, ExprKind::Binary(BinOp::Pow, Box::new(base), Box::new(exp))));
        }
        Ok(base)
    }

    /// `mod` beside any operator requires parentheses (§6.2).
    fn no_bare_mod(&self, e: &Expr, beside: &str) -> Result<()> {
        if let ExprKind::Binary(BinOp::Mod, ..) = e.kind {
            return Err(CompileError::new(e.line, format!("`mod` beside `{beside}` has no mathematical notation: parenthesise")));
        }
        Ok(())
    }

    fn adjacent_lbrack(&self) -> bool {
        *self.peek() == Tok::LBrack && !self.toks[self.pos].space_before
    }

    fn postfix(&mut self) -> Result<Expr> {
        let mut e = self.primary()?;
        while self.adjacent_lbrack() {
            let line = self.line();
            let idx = self.index_slot()?;
            e = self.mk(line, ExprKind::Index(Box::new(e), Box::new(idx)));
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr> {
        let line = self.line();
        match self.peek().clone() {
            Tok::Int(s) => {
                self.bump();
                let v: i128 = s.parse().map_err(|_| CompileError::new(line, "integer literal too large"))?;
                Ok(self.mk(line, ExprKind::Int(v)))
            }
            Tok::Bin(s) => {
                self.bump();
                let v: f64 = s.parse().map_err(|_| CompileError::new(line, "bad bin literal"))?;
                Ok(self.mk(line, ExprKind::Bin(v, s)))
            }
            Tok::Text(s) => {
                self.bump();
                Ok(self.mk(line, ExprKind::Text(s)))
            }
            Tok::Name(s) => {
                self.bump();
                Ok(self.mk(line, ExprKind::Name(s)))
            }
            Tok::Newline => {
                self.bump();
                Ok(self.mk(line, ExprKind::Pieces(vec![Piece::Newline])))
            }
            Tok::LParen => {
                self.bump();
                let inner = self.expr()?;
                if self.piece_starts() {
                    return self.err("pieces join inside `[ ]`, not `( )`; `( )` groups arithmetic only");
                }
                self.expect(Tok::RParen, "`)`")?;
                Ok(self.mk(line, ExprKind::Paren(Box::new(inner))))
            }
            Tok::LBrack => {
                let vals = self.slot()?;
                match vals.len() {
                    0 => Err(CompileError::new(line, "`[]` has no type; a list needs at least one element")),
                    1 => {
                        // `[v]` inside a slot is the value itself (marked, so it may be a piece); `[v,]` already became a List
                        let v = vals.into_iter().next().unwrap();
                        Ok(self.mk(line, ExprKind::Bracket(Box::new(v))))
                    }
                    _ => Ok(self.mk(line, ExprKind::List(vals))),
                }
            }
            Tok::Ident(w) => match w.as_str() {
                "true" => { self.bump(); Ok(self.mk(line, ExprKind::Bool(true))) }
                "false" => { self.bump(); Ok(self.mk(line, ExprKind::Bool(false))) }
                "std" => self.std_call(),
                _ if is_keyword(&w) => self.err(format!("`{w}` cannot appear in a value")),
                _ => {
                    self.bump();
                    if !self.adjacent_lbrack() {
                        if is_operator_word(&w) {
                            return self.err(format!("`{w}` is an operator and needs an operand on each side"));
                        }
                        return self.err(format!("`{w}` is not a call (a call is `{w}[...]`); variables are quoted: `'{w}'`"));
                    }
                    let args = self.slot()?;
                    Ok(self.mk(line, ExprKind::Call(w, args)))
                }
            },
            other => self.err(format!("expected a value, found {}", describe(&other))),
        }
    }

    /// `std::print.stdout[...]`, `std::read.stdin[...]`, `std::range[a, b]`,
    /// `std::len[v]`, `std::fill[v, n]`, `std::to.T[v]`, `std::exit[n]`.
    fn std_call(&mut self) -> Result<Expr> {
        let line = self.line();
        self.expect_ident("std")?;
        self.expect(Tok::ColonColon, "`::` after `std`")?;
        let mut path = Vec::new();
        loop {
            match self.bump().tok {
                Tok::Ident(s) => path.push(s),
                other => return Err(CompileError::new(line, format!("expected a `std` member, found {}", describe(&other)))),
            }
            if *self.peek() == Tok::Dot && path[0] != "to" {
                self.bump();
                continue;
            }
            break;
        }
        let joined = path.join(".");
        match joined.as_str() {
            "print.stdout" | "printu.stdout" => {
                let args = self.slot()?;
                Ok(self.mk(line, ExprKind::Call(format!("std::{joined}"), args)))
            }
            "print" | "printu" => Err(CompileError::new(line, format!("`std::{joined}` needs its target: `std::{joined}.stdout[...]`"))),
            "print.stderr" | "printu.stderr" => Err(CompileError::new(line, "Kespar has no `stderr` target (provisional cut)")),
            "read.stdin" => {
                let args = self.slot()?;
                Ok(self.mk(line, ExprKind::Read(args)))
            }
            "read" => Err(CompileError::new(line, "`std::read` needs its source: `std::read.stdin[...]`")),
            "range" => {
                let mut args = self.slot()?;
                if args.len() != 2 {
                    return Err(CompileError::new(line, "`std::range` takes two values: `std::range[a, b]`"));
                }
                let b = args.pop().unwrap();
                let a = args.pop().unwrap();
                Ok(self.mk(line, ExprKind::Range(Box::new(a), Box::new(b))))
            }
            "len" => {
                let mut args = self.slot()?;
                if args.len() != 1 {
                    return Err(CompileError::new(line, "`std::len` takes one value"));
                }
                Ok(self.mk(line, ExprKind::Len(Box::new(args.pop().unwrap()))))
            }
            "fill" => {
                let mut args = self.slot()?;
                if args.len() != 2 {
                    return Err(CompileError::new(line, "`std::fill` takes two values: `std::fill[value, count]`"));
                }
                let n = args.pop().unwrap();
                let v = args.pop().unwrap();
                Ok(self.mk(line, ExprKind::Fill(Box::new(v), Box::new(n))))
            }
            "to" => {
                if *self.peek() != Tok::Dot {
                    return Err(CompileError::new(line, "`std::to` needs the target type: `std::to.int32[...]`"));
                }
                self.bump();
                let ty = self.type_chain()?;
                let mut args = self.slot()?;
                if args.len() != 1 {
                    return Err(CompileError::new(line, "`std::to.T` takes one value"));
                }
                Ok(self.mk(line, ExprKind::To(ty, Box::new(args.pop().unwrap()))))
            }
            "exit" => {
                let args = self.slot()?;
                Ok(self.mk(line, ExprKind::Call("std::exit".into(), args)))
            }
            _ => Err(CompileError::new(line, format!("`std::{joined}` is not a member of Kespar's std (members: print.stdout, printu.stdout, read.stdin, range, len, fill, to.<type>, exit)"))),
        }
    }
}

fn is_operator_word(s: &str) -> bool {
    matches!(s, "x" | "xx" | "mod" | "and" | "or" | "not")
}

pub fn describe(t: &Tok) -> String {
    match t {
        Tok::Int(s) => format!("the number `{s}`"),
        Tok::Bin(s) => format!("the number `{s}`"),
        Tok::Text(s) => format!("the text `\"{s}\"`"),
        Tok::Name(s) => format!("`'{s}'`"),
        Tok::Ident(s) => format!("`{s}`"),
        Tok::Newline => "`\\n`".into(),
        Tok::LBrack => "`[`".into(), Tok::RBrack => "`]`".into(),
        Tok::LBrace => "`{`".into(), Tok::RBrace => "`}`".into(),
        Tok::LParen => "`(`".into(), Tok::RParen => "`)`".into(),
        Tok::Comma => "`,`".into(), Tok::Semi => "`;`".into(), Tok::Dot => "`.`".into(),
        Tok::ColonColon => "`::`".into(), Tok::Walrus => "`:=`".into(),
        Tok::DollarWalrus => "`$:=`".into(), Tok::DollarEq => "`$=`".into(), Tok::Eq => "`=`".into(),
        Tok::EqEq => "`==`".into(), Tok::NotEq => "`!==`".into(), Tok::Lt => "`<`".into(), Tok::Gt => "`>`".into(),
        Tok::Le => "`<==`".into(), Tok::Ge => "`>==`".into(), Tok::Plus => "`+`".into(), Tok::Minus => "`-`".into(),
        Tok::Slash => "`/`".into(), Tok::Eof => "the end of the file".into(),
    }
}
