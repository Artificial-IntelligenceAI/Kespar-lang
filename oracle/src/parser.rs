//! Parser (design/language.md §1, §3, §5, §6.1, §6.2, §7).

use crate::ast::*;
use crate::lexer::{Tok, Token};
use crate::CompileError;

pub fn parse(toks: Vec<Token>) -> Result<Program, CompileError> {
    let mut p = Parser { toks, pos: 0, next_id: 0 };
    p.program()
}

struct Parser {
    toks: Vec<Token>,
    pos: usize,
    next_id: usize,
}

const KEYWORDS: &[&str] = &[
    "var", "func", "return", "if", "else", "loop", "in", "break", "continue", "check", "nocheck",
    "true", "false", "mut", "immut", "MAIN", "int8", "int16", "int32", "int64", "uint8", "uint16",
    "uint32", "uint64", "bin32", "bin64", "bool", "str", "utf8", "list", "while", "for",
];

fn is_keyword(w: &str) -> bool {
    KEYWORDS.contains(&w)
}

impl Parser {
    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn peek_at(&self, k: usize) -> &Tok {
        let i = (self.pos + k).min(self.toks.len() - 1);
        &self.toks[i].tok
    }
    fn line(&self) -> usize {
        self.toks[self.pos].line
    }
    fn col(&self) -> usize {
        self.toks[self.pos].col
    }
    fn advance(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos < self.toks.len() - 1 {
            self.pos += 1;
        }
        t
    }
    fn err<T>(&self, msg: &str) -> Result<T, CompileError> {
        Err(CompileError::new(msg, self.line()))
    }
    fn expect(&mut self, t: Tok, what: &str) -> Result<Token, CompileError> {
        if *self.peek() == t {
            Ok(self.advance())
        } else {
            self.err(&format!("expected {}, found {}", what, describe(self.peek())))
        }
    }
    fn is_ident(&self, w: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == w)
    }
    fn expect_ident(&mut self, w: &str) -> Result<(), CompileError> {
        if self.is_ident(w) {
            self.advance();
            Ok(())
        } else {
            self.err(&format!("expected '{}', found {}", w, describe(self.peek())))
        }
    }
    fn new_expr(&mut self, line: usize, col: usize, kind: ExprKind) -> Expr {
        let id = self.next_id;
        self.next_id += 1;
        Expr { id, line, col, kind }
    }

    // ---- top level -------------------------------------------------------

    fn program(&mut self) -> Result<Program, CompileError> {
        let mut funcs = Vec::new();
        let mut main: Option<Block> = None;
        loop {
            match self.peek().clone() {
                Tok::Eof => break,
                Tok::Ident(w) if w == "func" => funcs.push(self.func()?),
                Tok::Ident(w) if w == "MAIN" => {
                    if main.is_some() {
                        return self.err("MAIN appears more than once");
                    }
                    self.advance();
                    main = Some(self.block()?);
                }
                _ => {
                    return self.err(&format!(
                        "top level holds only func definitions and MAIN, found {}",
                        describe(self.peek())
                    ))
                }
            }
        }
        let main = match main {
            Some(m) => m,
            None => return Err(CompileError::new("no MAIN block", self.line())),
        };
        Ok(Program { funcs, main, expr_count: self.next_id })
    }

    fn func(&mut self) -> Result<Func, CompileError> {
        let ft = self.advance(); // func
        let ret = if *self.peek() == Tok::Dot {
            self.advance();
            Some(self.type_chain()?)
        } else {
            None
        };
        let name = match self.peek().clone() {
            Tok::Ident(w) if !is_keyword(&w) => {
                self.advance();
                w
            }
            Tok::Ident(w) => {
                return self.err(&format!("'{}' is a keyword and cannot name a function", w))
            }
            t => return self.err(&format!("expected a function name, found {}", describe(&t))),
        };
        self.expect(Tok::LBrack, "'[' before the parameters")?;
        let mut params = Vec::new();
        if *self.peek() != Tok::RBrack {
            loop {
                params.push(self.param()?);
                if *self.peek() == Tok::Comma {
                    self.advance();
                    continue;
                }
                break;
            }
        }
        self.expect(Tok::RBrack, "']' after the parameters")?;
        let body = self.block()?;
        Ok(Func { name, line: ft.line, col: ft.col, params, ret, body })
    }

    fn param(&mut self) -> Result<Param, CompileError> {
        self.expect_ident("var")?;
        let (immut, ty) = self.var_chain()?;
        let (name, line, col) = self.name_tok()?;
        Ok(Param { name, ty, immut, line, col })
    }

    /// After `var`: optional `.mut`/`.immut`, optional type chain.
    fn var_chain(&mut self) -> Result<(bool, Option<TypeSpec>), CompileError> {
        let mut immut = false;
        let mut ty = None;
        if *self.peek() == Tok::Dot {
            self.advance();
            if self.is_ident("mut") {
                self.advance();
            } else if self.is_ident("immut") {
                self.advance();
                immut = true;
            } else {
                ty = Some(self.type_chain()?);
                return Ok((immut, ty));
            }
            if *self.peek() == Tok::Dot {
                self.advance();
                ty = Some(self.type_chain()?);
            }
        }
        Ok((immut, ty))
    }

    /// A type chain: `int32`, `bin64`, `bool`, `str.utf8`, `list.T`.
    fn type_chain(&mut self) -> Result<TypeSpec, CompileError> {
        let w = match self.peek().clone() {
            Tok::Ident(w) => w,
            t => return self.err(&format!("expected a type, found {}", describe(&t))),
        };
        self.advance();
        if let Some(iw) = IntW::from_name(&w) {
            return Ok(TypeSpec::Int(iw));
        }
        match w.as_str() {
            "bin32" => Ok(TypeSpec::Bin(BinW::B32)),
            "bin64" => Ok(TypeSpec::Bin(BinW::B64)),
            "bool" => Ok(TypeSpec::Bool),
            "str" => {
                if *self.peek() == Tok::Dot && *self.peek_at(1) == Tok::Ident("utf8".into()) {
                    self.advance();
                    self.advance();
                    Ok(TypeSpec::Str)
                } else {
                    self.err("the typed form is always fully explicit: write str.utf8")
                }
            }
            "list" => {
                if *self.peek() == Tok::Dot {
                    self.advance();
                    Ok(TypeSpec::List(Box::new(self.type_chain()?)))
                } else {
                    self.err("list needs an element type: write list.T")
                }
            }
            "int" | "uint" | "bin" => self.err(&format!(
                "the typed form is always fully explicit: '{}' needs a width",
                w
            )),
            _ => self.err(&format!("'{}' is not a type", w)),
        }
    }

    fn name_tok(&mut self) -> Result<(String, usize, usize), CompileError> {
        match self.peek().clone() {
            Tok::Name(n) => {
                let t = self.advance();
                Ok((n, t.line, t.col))
            }
            t => self.err(&format!("expected a quoted name, found {}", describe(&t))),
        }
    }

    // ---- statements ------------------------------------------------------

    fn block(&mut self) -> Result<Block, CompileError> {
        let lb = self.expect(Tok::LBrace, "'{'")?;
        let mut stmts = Vec::new();
        while *self.peek() != Tok::RBrace {
            if *self.peek() == Tok::Eof {
                return self.err("unexpected end of file inside a block");
            }
            stmts.push(self.stmt()?);
        }
        self.advance();
        Ok(Block { line: lb.line, stmts })
    }

    fn stmt(&mut self) -> Result<Stmt, CompileError> {
        let line = self.line();
        let kind = match self.peek().clone() {
            Tok::Ident(w) => match w.as_str() {
                "var" => self.decl()?,
                "if" => self.if_stmt()?,
                "loop" => self.loop_stmt()?,
                "break" => {
                    self.advance();
                    self.expect(Tok::Semi, "';'")?;
                    StmtKind::Break
                }
                "continue" => {
                    self.advance();
                    self.expect(Tok::Semi, "';'")?;
                    StmtKind::Continue
                }
                "return" => {
                    self.advance();
                    if *self.peek() == Tok::Semi {
                        self.advance();
                        StmtKind::Return(None)
                    } else {
                        let v = self.value_bracket()?;
                        self.expect(Tok::Semi, "';'")?;
                        StmtKind::Return(Some(v))
                    }
                }
                "check" => {
                    self.advance();
                    StmtKind::Check(self.block()?)
                }
                "nocheck" => {
                    self.advance();
                    StmtKind::NoCheck(self.block()?)
                }
                "std" => self.std_stmt()?,
                "else" => return self.err("'else' without an 'if'"),
                _ if is_keyword(&w) => {
                    return self.err(&format!("'{}' cannot start a statement", w))
                }
                _ => {
                    // call statement
                    self.advance();
                    let args = self.arg_bracket()?;
                    self.expect(Tok::Semi, "';'")?;
                    StmtKind::Call { name: w, args }
                }
            },
            Tok::Name(n) => {
                let nt = self.advance();
                if *self.peek() == Tok::LBrack {
                    let mut indices = Vec::new();
                    while *self.peek() == Tok::LBrack {
                        let lb = self.advance();
                        let idx = self.expr(0)?;
                        self.expect(Tok::RBrack, "']' after the index")?;
                        indices.push((idx, lb.line, lb.col));
                    }
                    self.expect(Tok::Assign, "'=' after the index")?;
                    let value = self.value_bracket()?;
                    self.expect(Tok::Semi, "';'")?;
                    StmtKind::IndexAssign {
                        name: n,
                        name_line: nt.line,
                        name_col: nt.col,
                        indices,
                        value,
                    }
                } else {
                    match self.peek() {
                        Tok::Assign => {}
                        Tok::Walrus | Tok::DollarWalrus | Tok::DollarAssign => {
                            return self.err("':=' and '$' belong to a declaration: write var 'name' := […]")
                        }
                        t => {
                            let t = t.clone();
                            return self.err(&format!("expected '=' after the name, found {}", describe(&t)));
                        }
                    }
                    self.advance();
                    let value = self.value_bracket()?;
                    self.expect(Tok::Semi, "';'")?;
                    StmtKind::Assign { name: n, value }
                }
            }
            Tok::LBrace => return self.err("a bare block is not a statement (use check { } or nocheck { })"),
            t => return self.err(&format!("expected a statement, found {}", describe(&t))),
        };
        Ok(Stmt { line, kind })
    }

    fn decl(&mut self) -> Result<StmtKind, CompileError> {
        self.advance(); // var
        let (immut, ty) = self.var_chain()?;
        let (name, _, _) = self.name_tok()?;
        let (typed, shadow) = match self.peek() {
            Tok::Assign => (true, false),
            Tok::DollarAssign => (true, true),
            Tok::Walrus => (false, false),
            Tok::DollarWalrus => (false, true),
            t => {
                let t = t.clone();
                return self.err(&format!("expected '=' or ':=' after the name, found {}", describe(&t)));
            }
        };
        self.advance();
        if typed && ty.is_none() {
            return self.err("'=' is for the typed form; an untyped declaration is written var 'name' := […]");
        }
        if !typed && ty.is_some() {
            return self.err("':=' is for the untyped form; a typed declaration is written var.T 'name' = […]");
        }
        let value = self.value_bracket()?;
        self.expect(Tok::Semi, "';'")?;
        Ok(StmtKind::Decl { name, ty, immut, shadow, value })
    }

    fn if_stmt(&mut self) -> Result<StmtKind, CompileError> {
        self.advance(); // if
        let mut arms = Vec::new();
        let cond = self.value_bracket()?;
        let body = self.block()?;
        arms.push((cond, body));
        let mut otherwise = None;
        while self.is_ident("else") {
            self.advance();
            if *self.peek() == Tok::Dot {
                self.advance();
                self.expect_ident("if")?;
                let cond = self.value_bracket()?;
                let body = self.block()?;
                arms.push((cond, body));
            } else {
                otherwise = Some(self.block()?);
                break;
            }
        }
        Ok(StmtKind::If { arms, otherwise })
    }

    fn loop_stmt(&mut self) -> Result<StmtKind, CompileError> {
        self.advance(); // loop
        if *self.peek() == Tok::Dot {
            self.advance();
            if self.is_ident("while") {
                self.advance();
                let cond = self.value_bracket()?;
                let body = self.block()?;
                return Ok(StmtKind::While(cond, body));
            }
            if self.is_ident("for") {
                self.advance();
                let (var, _, _) = self.name_tok()?;
                self.expect_ident("in")?;
                let iter = self.value_bracket()?;
                let body = self.block()?;
                return Ok(StmtKind::For { var, iter, body });
            }
            return self.err("after 'loop.' write 'while' or 'for'");
        }
        Ok(StmtKind::Loop(self.block()?))
    }

    fn std_stmt(&mut self) -> Result<StmtKind, CompileError> {
        let st = self.advance(); // std
        self.expect(Tok::ColonColon, "'::' after std")?;
        let member = self.std_member_name()?;
        let kind = match member.as_str() {
            "print.stdout" | "printu.stdout" => {
                let useful = member.starts_with("printu");
                let pieces = self.pieces_bracket()?;
                StmtKind::Print { useful, pieces }
            }
            "exit" => {
                let args = self.arg_bracket()?;
                if args.len() != 1 {
                    return Err(CompileError::new("std::exit takes exactly one value", st.line));
                }
                StmtKind::Exit(args.into_iter().next().unwrap())
            }
            "read.stdin" | "range" | "len" | "fill" => {
                return Err(CompileError::new(
                    &format!("std::{} is a value, not a statement", member),
                    st.line,
                ))
            }
            m if m.starts_with("to.") => {
                return Err(CompileError::new("std::to is a value, not a statement", st.line))
            }
            m => {
                return Err(CompileError::new(
                    &format!("std has no member '{}'", m),
                    st.line,
                ))
            }
        };
        self.expect(Tok::Semi, "';'")?;
        Ok(kind)
    }

    /// The dotted member after `std::`, as text (`print.stdout`, `to.int32`, `to.str.utf8`).
    fn std_member_name(&mut self) -> Result<String, CompileError> {
        let mut parts = Vec::new();
        loop {
            match self.peek().clone() {
                Tok::Ident(w) => {
                    self.advance();
                    parts.push(w);
                }
                t => return self.err(&format!("expected a std member, found {}", describe(&t))),
            }
            if *self.peek() == Tok::Dot {
                self.advance();
                continue;
            }
            break;
        }
        Ok(parts.join("."))
    }

    // ---- brackets --------------------------------------------------------

    /// A `[ … ]` whose contents are one value (possibly a list literal).
    fn value_bracket(&mut self) -> Result<Expr, CompileError> {
        let lb = self.expect(Tok::LBrack, "'[' (every value position wears [ ])")?;
        let (items, trailing) = self.bracket_items()?;
        if items.is_empty() {
            return Err(CompileError::new("[] is empty: a list literal needs at least one element", lb.line));
        }
        if items.len() == 1 && !trailing {
            return Ok(items.into_iter().next().unwrap());
        }
        Ok(self.new_expr(lb.line, lb.col, ExprKind::ListLit(items)))
    }

    /// A `[ … ]` used as an argument list: zero or more values.
    fn arg_bracket(&mut self) -> Result<Vec<Expr>, CompileError> {
        let lb = self.expect(Tok::LBrack, "'[' before the arguments")?;
        let (items, trailing) = self.bracket_items()?;
        if trailing {
            return Err(CompileError::new("a trailing comma is a one-element list, not an argument list", lb.line));
        }
        Ok(items)
    }

    /// A `[ … ]` of print pieces.
    fn pieces_bracket(&mut self) -> Result<Vec<Piece>, CompileError> {
        let lb = self.expect(Tok::LBrack, "'[' before the pieces")?;
        let (items, trailing) = self.bracket_items()?;
        if items.len() != 1 || trailing {
            return Err(CompileError::new("print takes pieces joined by whitespace, not a comma-separated list", lb.line));
        }
        let item = items.into_iter().next().unwrap();
        Ok(match item.kind {
            ExprKind::Pieces(ps) => ps,
            _ => vec![Piece::Expr(item)],
        })
    }

    /// After `[`: values separated by commas, up to and including `]`.
    fn bracket_items(&mut self) -> Result<(Vec<Expr>, bool), CompileError> {
        let mut items = Vec::new();
        let mut trailing = false;
        if *self.peek() == Tok::RBrack {
            self.advance();
            return Ok((items, false));
        }
        loop {
            items.push(self.value()?);
            match self.peek() {
                Tok::Comma => {
                    self.advance();
                    if *self.peek() == Tok::RBrack {
                        trailing = true;
                        self.advance();
                        break;
                    }
                }
                Tok::RBrack => {
                    self.advance();
                    break;
                }
                t => {
                    let t = t.clone();
                    return self.err(&format!("expected ',' or ']', found {}", describe(&t)));
                }
            }
        }
        Ok((items, trailing))
    }

    /// One value: whitespace-joined pieces (§6.1). One piece is that piece.
    fn value(&mut self) -> Result<Expr, CompileError> {
        let line = self.line();
        let col = self.col();
        let mut pieces = Vec::new();
        loop {
            if *self.peek() == Tok::NewlinePiece {
                self.advance();
                pieces.push(Piece::Newline);
            } else if self.starts_expr() {
                let e = self.expr(0)?;
                pieces.push(Piece::Expr(e));
            } else {
                break;
            }
        }
        if pieces.is_empty() {
            return self.err(&format!("expected a value, found {}", describe(self.peek())));
        }
        if pieces.len() == 1 {
            if let Piece::Expr(_) = pieces[0] {
                if let Some(Piece::Expr(e)) = pieces.pop() {
                    return Ok(e);
                }
            }
        }
        Ok(self.new_expr(line, col, ExprKind::Pieces(pieces)))
    }

    fn starts_expr(&self) -> bool {
        match self.peek() {
            Tok::Int(_) | Tok::Bin(_) | Tok::Text(_) | Tok::Name(_) | Tok::LParen | Tok::LBrack | Tok::Minus => true,
            Tok::Ident(w) => {
                w == "true" || w == "false" || w == "std" || w == "not" || !is_keyword(w)
            }
            _ => false,
        }
    }

    // ---- expressions (§6.2) ----------------------------------------------

    /// Pratt parser. Binding powers, loosest to tightest:
    /// or 1, and 2, not 3 (prefix), comparisons 4, + - 5, x / mod 6,
    /// unary - 7 (prefix), xx 8 (right-associative).
    fn expr(&mut self, min_bp: u8) -> Result<Expr, CompileError> {
        let mut lhs = self.prefix()?;
        loop {
            let (op, bp) = match self.peek() {
                Tok::Plus => (BinOp::Add, 5),
                Tok::Minus => (BinOp::Sub, 5),
                Tok::Slash => (BinOp::Div, 6),
                Tok::Eq => (BinOp::Eq, 4),
                Tok::Neq => (BinOp::Neq, 4),
                Tok::Lt => (BinOp::Lt, 4),
                Tok::Gt => (BinOp::Gt, 4),
                Tok::Le => (BinOp::Le, 4),
                Tok::Ge => (BinOp::Ge, 4),
                Tok::Ident(w) => match w.as_str() {
                    "x" => (BinOp::Mul, 6),
                    "xx" => (BinOp::Pow, 8),
                    "mod" => (BinOp::Mod, 6),
                    "and" => (BinOp::And, 2),
                    "or" => (BinOp::Or, 1),
                    _ => break,
                },
                _ => break,
            };
            if bp < min_bp {
                break;
            }
            let opt = self.advance();
            // right-associative xx parses its right side at the same power; the rest one tighter
            let rhs = if op == BinOp::Pow { self.expr(bp)? } else { self.expr(bp + 1)? };
            check_adjacent(op, &lhs, opt.line)?;
            check_adjacent(op, &rhs, opt.line)?;
            lhs = self.new_expr(opt.line, opt.col, ExprKind::Binary(op, Box::new(lhs), Box::new(rhs)));
        }
        Ok(lhs)
    }

    fn prefix(&mut self) -> Result<Expr, CompileError> {
        match self.peek().clone() {
            Tok::Minus => {
                let t = self.advance();
                let operand = self.expr(8)?;
                if let ExprKind::Binary(BinOp::Mod, _, _) = operand.kind {
                    return Err(CompileError::new("parenthesise: 'mod' beside unary '-'", t.line));
                }
                Ok(self.new_expr(t.line, t.col, ExprKind::Unary(UnOp::Neg, Box::new(operand))))
            }
            Tok::Ident(w) if w == "not" => {
                let t = self.advance();
                let operand = self.expr(4)?;
                match operand.kind {
                    ExprKind::Binary(op, _, _) if op.is_cmp() => {
                        return Err(CompileError::new(
                            &format!("parenthesise: '{}' beside 'not'", op.spelling()),
                            t.line,
                        ))
                    }
                    ExprKind::Binary(BinOp::Mod, _, _) => {
                        return Err(CompileError::new("parenthesise: 'mod' beside 'not'", t.line))
                    }
                    _ => {}
                }
                Ok(self.new_expr(t.line, t.col, ExprKind::Unary(UnOp::Not, Box::new(operand))))
            }
            _ => self.postfix(),
        }
    }

    fn postfix(&mut self) -> Result<Expr, CompileError> {
        let mut e = self.primary()?;
        while *self.peek() == Tok::LBrack {
            let lb = self.advance();
            let idx = self.expr(0)?;
            self.expect(Tok::RBrack, "']' after the index")?;
            e = self.new_expr(lb.line, lb.col, ExprKind::Index(Box::new(e), Box::new(idx)));
        }
        Ok(e)
    }

    fn primary(&mut self) -> Result<Expr, CompileError> {
        let t = self.advance();
        let (line, col) = (t.line, t.col);
        let kind = match t.tok {
            Tok::Int(v) => ExprKind::IntLit(v),
            Tok::Bin(s) => ExprKind::BinLit(s),
            Tok::Text(s) => ExprKind::TextLit(s),
            Tok::Name(n) => ExprKind::Name(n),
            Tok::LParen => {
                let inner = self.expr(0)?;
                self.expect(Tok::RParen, "')'")?;
                ExprKind::Group(Box::new(inner))
            }
            Tok::LBrack => {
                // a [ ] inside a [ ] is a value
                let (items, trailing) = self.bracket_items()?;
                if items.is_empty() {
                    return Err(CompileError::new("[] is empty: a list literal needs at least one element", line));
                }
                if items.len() == 1 && !trailing {
                    ExprKind::Group(Box::new(items.into_iter().next().unwrap()))
                } else {
                    ExprKind::ListLit(items)
                }
            }
            Tok::Ident(w) => match w.as_str() {
                "true" => ExprKind::BoolLit(true),
                "false" => ExprKind::BoolLit(false),
                "std" => return self.std_value(line, col),
                _ if is_keyword(&w) => {
                    return Err(CompileError::new(&format!("'{}' cannot appear in an expression", w), line))
                }
                _ => {
                    if *self.peek() != Tok::LBrack {
                        return Err(CompileError::new(
                            &format!("'{}' is a bare word: a call is a word followed by '['; variables are quoted", w),
                            line,
                        ));
                    }
                    let args = self.arg_bracket()?;
                    ExprKind::Call(w, args)
                }
            },
            Tok::NewlinePiece => {
                return Err(CompileError::new("\\n is a piece, not an operand", line))
            }
            tok => return Err(CompileError::new(&format!("expected a value, found {}", describe(&tok)), line)),
        };
        Ok(self.new_expr(line, col, kind))
    }

    fn std_value(&mut self, line: usize, col: usize) -> Result<Expr, CompileError> {
        self.expect(Tok::ColonColon, "'::' after std")?;
        let member = self.std_member_name()?;
        let kind = match member.as_str() {
            "read.stdin" => ExprKind::Read(self.arg_bracket()?),
            "range" => {
                let args = self.arg_bracket()?;
                if args.len() != 2 {
                    return Err(CompileError::new("std::range takes exactly two values", line));
                }
                let mut it = args.into_iter();
                ExprKind::Range(Box::new(it.next().unwrap()), Box::new(it.next().unwrap()))
            }
            "len" => {
                let args = self.arg_bracket()?;
                if args.len() != 1 {
                    return Err(CompileError::new("std::len takes exactly one value", line));
                }
                ExprKind::Len(Box::new(args.into_iter().next().unwrap()))
            }
            "fill" => {
                let args = self.arg_bracket()?;
                if args.len() != 2 {
                    return Err(CompileError::new("std::fill takes exactly two values", line));
                }
                let mut it = args.into_iter();
                ExprKind::Fill(Box::new(it.next().unwrap()), Box::new(it.next().unwrap()))
            }
            "to" => return Err(CompileError::new("std::to needs a target type: write std::to.int32[…]", line)),
            m if m.starts_with("to.") => {
                let ty = parse_type_text(&m[3..], line)?;
                let args = self.arg_bracket()?;
                if args.len() != 1 {
                    return Err(CompileError::new("std::to takes exactly one value", line));
                }
                ExprKind::To(ty, Box::new(args.into_iter().next().unwrap()))
            }
            "print.stdout" | "printu.stdout" | "exit" => {
                return Err(CompileError::new(&format!("std::{} is a statement, not a value", member), line))
            }
            m => return Err(CompileError::new(&format!("std has no member '{}'", m), line)),
        };
        Ok(self.new_expr(line, col, kind))
    }
}

/// A dotted type text (`int32`, `str.utf8`, `list.list.int8`) for `std::to`.
fn parse_type_text(s: &str, line: usize) -> Result<TypeSpec, CompileError> {
    let parts: Vec<&str> = s.split('.').collect();
    fn go(parts: &[&str], line: usize) -> Result<TypeSpec, CompileError> {
        if parts.is_empty() {
            return Err(CompileError::new("std::to needs a target type", line));
        }
        if let Some(w) = IntW::from_name(parts[0]) {
            if parts.len() != 1 {
                return Err(CompileError::new(&format!("'{}' is not a type", parts.join(".")), line));
            }
            return Ok(TypeSpec::Int(w));
        }
        match parts[0] {
            "bin32" | "bin64" if parts.len() == 1 => Ok(TypeSpec::Bin(if parts[0] == "bin32" { BinW::B32 } else { BinW::B64 })),
            "bool" if parts.len() == 1 => Ok(TypeSpec::Bool),
            "str" if parts.len() == 2 && parts[1] == "utf8" => Ok(TypeSpec::Str),
            "str" => Err(CompileError::new("the typed form is always fully explicit: write str.utf8", line)),
            "list" if parts.len() >= 2 => Ok(TypeSpec::List(Box::new(go(&parts[1..], line)?))),
            _ => Err(CompileError::new(&format!("'{}' is not a type", parts.join(".")), line)),
        }
    }
    go(&parts, line)
}

/// §6.2 adjacency rules: a violation is a compile error that says "parenthesise".
fn check_adjacent(parent: BinOp, child: &Expr, line: usize) -> Result<(), CompileError> {
    let child_op: Option<&str> = match &child.kind {
        ExprKind::Binary(op, _, _) => Some(op.spelling()),
        ExprKind::Unary(UnOp::Neg, _) => Some("-"),
        ExprKind::Unary(UnOp::Not, _) => Some("not"),
        _ => None,
    };
    let child_bin = match &child.kind {
        ExprKind::Binary(op, _, _) => Some(*op),
        _ => None,
    };
    let cop = match child_op {
        Some(c) => c,
        None => return Ok(()),
    };
    let bad = |why: &str| -> Result<(), CompileError> {
        Err(CompileError::new(&format!("parenthesise: {}", why), line))
    };
    // mod beside any operator
    if parent == BinOp::Mod {
        return bad(&format!("'{}' beside 'mod'", cop));
    }
    if child_bin == Some(BinOp::Mod) {
        return bad(&format!("'mod' beside '{}'", parent.spelling()));
    }
    // / beside x or /; x beside /
    let muldiv = |o: BinOp| matches!(o, BinOp::Mul | BinOp::Div);
    if let Some(cb) = child_bin {
        if (parent == BinOp::Div && muldiv(cb)) || (parent == BinOp::Mul && cb == BinOp::Div) {
            return bad(&format!("'{}' beside '{}'", cop, parent.spelling()));
        }
        // comparisons never chained
        if parent.is_cmp() && cb.is_cmp() {
            return bad(&format!("'{}' chained with '{}'", cop, parent.spelling()));
        }
    }
    // comparisons never beside not
    if parent.is_cmp() && cop == "not" {
        return bad(&format!("'not' beside '{}'", parent.spelling()));
    }
    // unary minus is below xx
    if parent == BinOp::Pow && cop == "-" {
        return bad("unary '-' beside 'xx'");
    }
    Ok(())
}

pub fn describe(t: &Tok) -> String {
    match t {
        Tok::Int(v) => format!("the number {}", v),
        Tok::Bin(s) => format!("the number {}", s),
        Tok::Text(s) => format!("the text \"{}\"", s),
        Tok::Name(n) => format!("the name '{}'", n),
        Tok::Ident(w) => format!("'{}'", w),
        Tok::NewlinePiece => "'\\n'".to_string(),
        Tok::LBrack => "'['".to_string(),
        Tok::RBrack => "']'".to_string(),
        Tok::LBrace => "'{'".to_string(),
        Tok::RBrace => "'}'".to_string(),
        Tok::LParen => "'('".to_string(),
        Tok::RParen => "')'".to_string(),
        Tok::Comma => "','".to_string(),
        Tok::Semi => "';'".to_string(),
        Tok::Dot => "'.'".to_string(),
        Tok::ColonColon => "'::'".to_string(),
        Tok::Assign => "'='".to_string(),
        Tok::Walrus => "':='".to_string(),
        Tok::DollarAssign => "'$='".to_string(),
        Tok::DollarWalrus => "'$:='".to_string(),
        Tok::Eq => "'=='".to_string(),
        Tok::Neq => "'!=='".to_string(),
        Tok::Lt => "'<'".to_string(),
        Tok::Gt => "'>'".to_string(),
        Tok::Le => "'<=='".to_string(),
        Tok::Ge => "'>=='".to_string(),
        Tok::Plus => "'+'".to_string(),
        Tok::Minus => "'-'".to_string(),
        Tok::Slash => "'/'".to_string(),
        Tok::Eof => "end of file".to_string(),
    }
}
