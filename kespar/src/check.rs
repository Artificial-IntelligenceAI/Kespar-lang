//! The static checker: scopes, types, forcing (§8.3 step 1), every compile
//! error language.md names. Produces `ir::Program`.

use std::collections::HashMap;

use crate::ast::{self, BinOp, BinWidth, ExprKind, IntWidth, Mutability, Piece, StmtKind, Tier, TypeSpec, UnOp};
use crate::diag::{CompileError, Result};
use crate::ir::*;
use crate::types::{IntTy, Kind, Ty, Types};

struct Scope {
    names: HashMap<String, VarId>,
}

struct FnCtx {
    id: FuncId,
    locals: Vec<Local>,
    scopes: Vec<Scope>,
    ret: Ty,
    /// The function is `func name` with no return type: `ret` is a variable that every `return` meets.
    ret_inferred: bool,
    has_value_return: bool,
    loop_depth: u32,
    loop_vars: Vec<VarId>,
    tiers: Vec<Tier>,
    is_main: bool,
}

struct Checker {
    types: Types,
    sites: Vec<Site>,
    reads: Vec<ReadInfo>,
    /// name -> (FuncId, param types, ret type)
    sigs: HashMap<String, (FuncId, Vec<Ty>, Ty, bool)>,
    /// Integer literals: (line, value, type) for the post-check fit test.
    literals: Vec<(u32, i128, Ty)>,
    /// Operations that need a post-check family test: (line, op description, type, what is refused).
    family_checks: Vec<(u32, Ty, FamilyRule)>,
}

#[derive(Clone, Copy)]
enum FamilyRule {
    /// `mod` refused on bins.
    NoBinMod,
    /// `==`/`!==` refused on lists.
    EqNotList,
    /// `< > <== >==` only on integers and bins.
    OrderNumeric,
    /// Unary `-` needs a number.
    NegNumeric,
    /// Arithmetic needs a number.
    ArithNumeric,
}

fn spec_to_ty(t: &TypeSpec) -> Ty {
    match t {
        TypeSpec::Int(w) => Ty::Int(IntTy::Fixed(*w)),
        TypeSpec::Bin(w) => Ty::Bin(*w),
        TypeSpec::Bool => Ty::Bool,
        TypeSpec::Str => Ty::Str,
        TypeSpec::List(e) => Ty::List(Box::new(spec_to_ty(e))),
    }
}

pub fn check(prog: &ast::Program) -> Result<Program> {
    let mut ck = Checker { types: Types::default(), sites: Vec::new(), reads: Vec::new(), sigs: HashMap::new(), literals: Vec::new(), family_checks: Vec::new() };

    // Signatures first, so calls may precede definitions and recurse.
    for (i, f) in prog.funcs.iter().enumerate() {
        if ck.sigs.contains_key(&f.name) {
            return Err(CompileError::new(f.line, format!("a second function named `{}`", f.name)));
        }
        let mut params = Vec::new();
        for p in &f.params {
            params.push(match &p.ty {
                Some(t) => spec_to_ty(t),
                None => ck.types.fresh(Kind::Any, format!("'{}'", p.name), p.line),
            });
        }
        let (ret, inferred) = match &f.ret {
            Some(t) => (spec_to_ty(t), false),
            None => (ck.types.fresh(Kind::Any, format!("the return value of `{}`", f.name), f.line), true),
        };
        ck.sigs.insert(f.name.clone(), (i as FuncId, params, ret, inferred));
    }

    let mut funcs = Vec::new();
    for (i, f) in prog.funcs.iter().enumerate() {
        funcs.push(ck.func(i as FuncId, f)?);
    }
    let main_id = funcs.len() as FuncId;
    let mut mctx = FnCtx { id: main_id, locals: Vec::new(), scopes: vec![Scope { names: HashMap::new() }], ret: Ty::Nothing, ret_inferred: false, has_value_return: false, loop_depth: 0, loop_vars: Vec::new(), tiers: vec![Tier::Default], is_main: true };
    let body = ck.block(&mut mctx, &prog.main, false)?;
    funcs.push(Func { name: "MAIN".into(), line: prog.main_line, params: Vec::new(), ret: Ty::Nothing, locals: mctx.locals, body });

    // Untyped functions with no `return [v]` return nothing.
    for (i, f) in prog.funcs.iter().enumerate() {
        let (_, _, ret, inferred) = &ck.sigs[&f.name];
        if *inferred {
            let r = ck.types.resolve(ret);
            if let Ty::Var(v) = r {
                if ck.types.kind_of(v) == Kind::Any {
                    ck.types.bind_nothing(v);
                    funcs[i].ret = Ty::Nothing;
                }
            }
        }
    }

    ck.types.finish()?;
    let mut types = ck.types;

    // Post-check tests that needed final types.
    for (line, v, t) in &ck.literals {
        if let Ty::Int(IntTy::Fixed(w)) = types.norm(t) {
            if !w.holds(*v) {
                return Err(CompileError::new(*line, format!("the literal {v} does not fit {}", w.name())));
            }
        }
    }
    for (line, t, rule) in &ck.family_checks {
        let t = types.norm(t);
        let bad = match rule {
            FamilyRule::NoBinMod => t.is_bin(),
            FamilyRule::EqNotList => matches!(t, Ty::List(_)) || matches!(t, Ty::Nothing),
            FamilyRule::OrderNumeric => !(t.is_int() || t.is_bin()),
            FamilyRule::NegNumeric | FamilyRule::ArithNumeric => !(t.is_int() || t.is_bin()),
        };
        if bad {
            let msg = match rule {
                FamilyRule::NoBinMod => "`mod` on bins is not defined".to_string(),
                FamilyRule::EqNotList => format!("`==` / `!==` is not defined on {}", t.name()),
                FamilyRule::OrderNumeric => format!("ordering is only defined on integers and bins, not {}", t.name()),
                FamilyRule::NegNumeric => format!("unary `-` needs a number, not {}", t.name()),
                FamilyRule::ArithNumeric => format!("arithmetic needs numbers, not {}", t.name()),
            };
            return Err(CompileError::new(*line, msg));
        }
    }

    let mut program = Program { funcs, main: main_id, sites: ck.sites, reads: ck.reads, free_names: types.frees.clone(), types };
    finalize(&mut program);
    Ok(program)
}

thread_local! {
    static NEXT_EXPR_ID: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
}

/// Normalise every type in the IR, number every expression, and switch off sites on bin operations.
fn finalize(p: &mut Program) {
    NEXT_EXPR_ID.with(|c| c.set(0));
    let mut types = std::mem::take(&mut p.types);
    let mut sites = std::mem::take(&mut p.sites);
    for f in &mut p.funcs {
        f.ret = types.norm(&f.ret);
        for l in &mut f.locals {
            l.ty = types.norm(&l.ty);
        }
        fin_block(&mut f.body, &mut types, &mut sites);
    }
    for r in &mut p.reads {
        r.ty = types.norm(&r.ty);
    }
    p.types = types;
    p.sites = sites;
}

fn fin_block(b: &mut [Stmt], types: &mut Types, sites: &mut [Site]) {
    for s in b {
        match &mut s.kind {
            SK::Let(_, e) | SK::Assign(_, e) | SK::Exit(e) | SK::CallStmt(e) => fin_expr(e, types, sites),
            SK::AssignIndex(t, i, v, _) => { fin_expr(t, types, sites); fin_expr(i, types, sites); fin_expr(v, types, sites); }
            SK::Print { pieces, .. } => pieces.iter_mut().for_each(|e| fin_expr(e, types, sites)),
            SK::If(brs, el) => {
                for (c, b) in brs { fin_expr(c, types, sites); fin_block(b, types, sites); }
                if let Some(b) = el { fin_block(b, types, sites); }
            }
            SK::Block(b) | SK::Loop(b) => fin_block(b, types, sites),
            SK::While(c, b) => { fin_expr(c, types, sites); fin_block(b, types, sites); }
            SK::ForRange { a, b, body, .. } => { fin_expr(a, types, sites); fin_expr(b, types, sites); fin_block(body, types, sites); }
            SK::ForList { list, body, .. } => { fin_expr(list, types, sites); fin_block(body, types, sites); }
            SK::Return(Some(e)) => fin_expr(e, types, sites),
            SK::Return(None) | SK::Break | SK::Continue => {}
        }
    }
}

fn fin_expr(e: &mut Expr, types: &mut Types, sites: &mut [Site]) {
    e.ty = types.norm(&e.ty);
    e.id = NEXT_EXPR_ID.with(|c| { let v = c.get(); c.set(v + 1); v });
    match &mut e.kind {
        EK::Int(_) | EK::Bin(_) | EK::Text(_) | EK::Bool(_) | EK::Var(_) | EK::Read(..) => {}
        EK::Pieces(ps) => {
            for p in ps {
                match p {
                    PieceIr::Var(_, t) => *t = types.norm(t),
                    PieceIr::Value(v) => fin_expr(v, types, sites),
                    _ => {}
                }
            }
        }
        EK::List(es) | EK::Call(_, es) => es.iter_mut().for_each(|x| fin_expr(x, types, sites)),
        EK::Neg(x, site) => {
            fin_expr(x, types, sites);
            if e.ty.is_bin() { if let Some(s) = site { sites[*s as usize].active = false; } }
            if let (Ty::Int(IntTy::Free(id)), Some(s)) = (&e.ty, site) { sites[*s as usize].free = Some(*id); }
        }
        EK::Not(x) | EK::Len(x) => fin_expr(x, types, sites),
        EK::Binary(_, a, b, s1, s2) => {
            fin_expr(a, types, sites);
            fin_expr(b, types, sites);
            if a.ty.is_bin() {
                for s in [&*s1, &*s2].into_iter().flatten() { sites[*s as usize].active = false; }
            }
            if let Ty::Int(IntTy::Free(id)) = &a.ty {
                if let Some(s) = s1 { sites[*s as usize].free = Some(*id); }
            }
        }
        EK::Index(a, b, _) | EK::Fill(a, b, _) => { fin_expr(a, types, sites); fin_expr(b, types, sites); }
        EK::To(x, site) => {
            fin_expr(x, types, sites);
            let into_int = e.ty.is_int();
            let from_bool_or_str = matches!(x.ty, Ty::Bool | Ty::Str);
            if let Some(s) = site {
                if !into_int || from_bool_or_str { sites[*s as usize].active = false; }
            }
        }
    }
}

impl Checker {
    fn site(&mut self, ctx: &FnCtx, line: u32, kind: SiteKind, what: String) -> SiteId {
        let id = self.sites.len() as SiteId;
        self.sites.push(Site { id, line, kind, tier: *ctx.tiers.last().unwrap(), func: ctx.id, what, active: true, free: None });
        id
    }

    fn func(&mut self, id: FuncId, f: &ast::Func) -> Result<Func> {
        let (_, param_tys, ret, inferred) = self.sigs[&f.name].clone();
        let mut ctx = FnCtx { id, locals: Vec::new(), scopes: vec![Scope { names: HashMap::new() }], ret: ret.clone(), ret_inferred: inferred, has_value_return: false, loop_depth: 0, loop_vars: Vec::new(), tiers: vec![Tier::Default], is_main: false };
        let mut params = Vec::new();
        for (p, t) in f.params.iter().zip(param_tys.iter()) {
            let v = self.declare(&mut ctx, &p.name, t.clone(), p.line, p.immut, p.ty.is_none(), false)?;
            params.push(v);
        }
        let body = self.block(&mut ctx, &f.body, false)?;
        let returns_value = if inferred { ctx.has_value_return } else { !matches!(ret, Ty::Nothing) };
        if returns_value && !always_returns(&body) {
            return Err(CompileError::new(f.line, format!("`{}` returns a value but can fall off its end without `return`", f.name)));
        }
        Ok(Func { name: f.name.clone(), line: f.line, params, ret, locals: ctx.locals, body })
    }

    fn lookup(&self, ctx: &FnCtx, name: &str) -> Option<VarId> {
        for s in ctx.scopes.iter().rev() {
            if let Some(v) = s.names.get(name) {
                return Some(*v);
            }
        }
        None
    }

    fn declare(&mut self, ctx: &mut FnCtx, name: &str, ty: Ty, line: u32, immut: bool, inferred: bool, shadow: bool) -> Result<VarId> {
        let visible = self.lookup(ctx, name);
        match (visible, shadow) {
            (Some(v), false) => {
                let prev = ctx.locals[v as usize].line;
                return Err(CompileError::new(line, format!("'{name}' is already visible (declared at line {prev}); shadowing is never silent — write `$:=` / `$=` to hide it on purpose")));
            }
            (None, true) => return Err(CompileError::new(line, format!("there is no '{name}' to shadow"))),
            (Some(v), true) => {
                if ctx.locals[v as usize].immut {
                    return Err(CompileError::new(line, format!("'{name}' is immut and cannot be shadowed")));
                }
            }
            (None, false) => {}
        }
        let id = ctx.locals.len() as VarId;
        ctx.locals.push(Local { name: name.to_string(), ty, line, immut, inferred });
        ctx.scopes.last_mut().unwrap().names.insert(name.to_string(), id);
        Ok(id)
    }

    fn block(&mut self, ctx: &mut FnCtx, stmts: &[ast::Stmt], new_scope: bool) -> Result<Vec<Stmt>> {
        if new_scope {
            ctx.scopes.push(Scope { names: HashMap::new() });
        }
        let mut out = Vec::new();
        for s in stmts {
            out.push(self.stmt(ctx, s)?);
        }
        if new_scope {
            ctx.scopes.pop();
        }
        Ok(out)
    }

    fn inner(&mut self, ctx: &mut FnCtx, stmts: &[ast::Stmt]) -> Result<Vec<Stmt>> {
        self.block(ctx, stmts, true)
    }

    fn stmt(&mut self, ctx: &mut FnCtx, s: &ast::Stmt) -> Result<Stmt> {
        let line = s.line;
        let kind = match &s.kind {
            StmtKind::Var { mutability, ty, name, shadow, value } => {
                let immut = *mutability == Mutability::Immut;
                if let ExprKind::Read(bounds) = &value.kind {
                    let Some(spec) = ty else {
                        return Err(CompileError::new(line, format!("'{name}' comes from outside the program; write its type (`var.int32 '{name}' = [std::read.stdin[...]]`)")));
                    };
                    let t = spec_to_ty(spec);
                    let e = self.read(ctx, value.line, name, &t, bounds)?;
                    let v = self.declare(ctx, name, t, line, immut, false, *shadow)?;
                    return Ok(Stmt { line, kind: SK::Let(v, e) });
                }
                let e = self.expr(ctx, value)?;
                if matches!(self.types.resolve(&e.ty), Ty::Nothing) {
                    return Err(CompileError::new(line, "a call that returns nothing cannot be used as a value"));
                }
                let t = match ty {
                    Some(spec) => {
                        let t = spec_to_ty(spec);
                        self.types.force(&e.ty, &t, line)?;
                        t
                    }
                    None => {
                        let v = self.types.fresh(Kind::Any, format!("'{name}'"), line);
                        self.types.unify(&v, &e.ty, line)?;
                        v
                    }
                };
                let v = self.declare(ctx, name, t, line, immut, ty.is_none(), *shadow)?;
                SK::Let(v, e)
            }
            StmtKind::Assign { name, value } => {
                let Some(v) = self.lookup(ctx, name) else {
                    return Err(CompileError::new(line, format!("'{name}' is not declared (a declaration starts with `var`)")));
                };
                let local = ctx.locals[v as usize].clone();
                if local.immut {
                    return Err(CompileError::new(line, format!("'{name}' is immut and cannot be assigned")));
                }
                if ctx.loop_vars.contains(&v) {
                    return Err(CompileError::new(line, format!("'{name}' is a loop variable and cannot be assigned")));
                }
                let e = if let ExprKind::Read(bounds) = &value.kind {
                    if local.inferred {
                        return Err(CompileError::new(line, format!("'{name}' was declared with `:=`; a value from outside needs a name with an explicit type")));
                    }
                    self.read(ctx, value.line, name, &local.ty, bounds)?
                } else {
                    let e = self.expr(ctx, value)?;
                    self.types.force(&local.ty, &e.ty, line)?;
                    e
                };
                SK::Assign(v, e)
            }
            StmtKind::AssignIndex { target, index, value } => {
                let te = self.expr(ctx, target)?;
                let name = short(target);
                let elem = self.types.fresh(Kind::Any, format!("an element of {name}"), line);
                if self.types.unify(&te.ty, &Ty::List(Box::new(elem.clone())), line).is_err() {
                    let r = self.types.resolve(&te.ty);
                    return Err(CompileError::new(line, format!("{name} is {} and cannot be indexed", r.name())));
                }
                let i = self.expr(ctx, index)?;
                self.index_ok(&i, line)?;
                let e = self.expr(ctx, value)?;
                self.types.force(&elem, &e.ty, line)?;
                let site = self.site(ctx, line, SiteKind::OutOfBounds, format!("{name}[…] ="));
                SK::AssignIndex(te, i, e, site)
            }
            StmtKind::Print { useful, pieces } => {
                let mut out = Vec::new();
                for p in pieces {
                    if matches!(p.kind, ExprKind::Int(_) | ExprKind::Bin(..)) {
                        return Err(CompileError::new(p.line, "a bare number is an operand, never a piece; give it a name first: `[\"count: \" 'n']`"));
                    }
                    let e = self.expr(ctx, p)?;
                    if matches!(self.types.resolve(&e.ty), Ty::Nothing) {
                        return Err(CompileError::new(p.line, "a call that returns nothing cannot be printed"));
                    }
                    out.push(e);
                }
                SK::Print { useful: *useful, pieces: out }
            }
            StmtKind::Exit(e) => {
                let e = self.expr(ctx, e)?;
                self.types.force(&e.ty, &Ty::Int(IntTy::Fixed(IntWidth::U8)), line)?;
                SK::Exit(e)
            }
            StmtKind::If { branches, else_body } => {
                let mut brs = Vec::new();
                for (c, b) in branches {
                    let ce = self.expr(ctx, c)?;
                    self.must_bool(&ce, "the condition of `if`")?;
                    let body = self.inner(ctx, b)?;
                    brs.push((ce, body));
                }
                let el = match else_body {
                    Some(b) => Some(self.inner(ctx, b)?),
                    None => None,
                };
                SK::If(brs, el)
            }
            StmtKind::Loop(b) => {
                ctx.loop_depth += 1;
                let body = self.inner(ctx, b)?;
                ctx.loop_depth -= 1;
                SK::Loop(body)
            }
            StmtKind::While(c, b) => {
                let ce = self.expr(ctx, c)?;
                self.must_bool(&ce, "the condition of `loop.while`")?;
                ctx.loop_depth += 1;
                let body = self.inner(ctx, b)?;
                ctx.loop_depth -= 1;
                SK::While(ce, body)
            }
            StmtKind::For { name, iter, body } => {
                ctx.loop_depth += 1;
                ctx.scopes.push(Scope { names: HashMap::new() });
                let kind = if let ExprKind::Range(a, b) = &iter.kind {
                    let ae = self.expr(ctx, a)?;
                    let be = self.expr(ctx, b)?;
                    self.types.force(&ae.ty, &be.ty, line)?;
                    let bound = self.types.fresh(Kind::IntLike, "a range bound", line);
                    self.types.force(&ae.ty, &bound, line)?;
                    let t = self.types.fresh(Kind::IntLike, format!("'{name}'"), line);
                    self.types.unify(&t, &ae.ty, line)?;
                    let v = self.declare(ctx, name, t, line, true, true, false)?;
                    ctx.loop_vars.push(v);
                    let body = self.block(ctx, body, false)?;
                    ctx.loop_vars.pop();
                    SK::ForRange { var: v, a: ae, b: be, body }
                } else {
                    let le = self.expr(ctx, iter)?;
                    let elem = self.types.fresh(Kind::Any, format!("'{name}'"), line);
                    if self.types.unify(&le.ty, &Ty::List(Box::new(elem.clone())), line).is_err() {
                        let r = self.types.resolve(&le.ty);
                        return Err(CompileError::new(line, format!("`loop.for` walks a list or a `std::range`, not {}", r.name())));
                    }
                    let v = self.declare(ctx, name, elem, line, true, true, false)?;
                    ctx.loop_vars.push(v);
                    let body = self.block(ctx, body, false)?;
                    ctx.loop_vars.pop();
                    SK::ForList { var: v, list: le, body }
                };
                ctx.scopes.pop();
                ctx.loop_depth -= 1;
                kind
            }
            StmtKind::Break => {
                if ctx.loop_depth == 0 { return Err(CompileError::new(line, "`break` outside a loop")); }
                SK::Break
            }
            StmtKind::Continue => {
                if ctx.loop_depth == 0 { return Err(CompileError::new(line, "`continue` outside a loop")); }
                SK::Continue
            }
            StmtKind::Return(v) => {
                if ctx.is_main {
                    return Err(CompileError::new(line, "`MAIN` is not a function; it has no `return` (it ends, or `std::exit[n]`)"));
                }
                match v {
                    None => {
                        if ctx.ret_inferred {
                            if ctx.has_value_return {
                                return Err(CompileError::new(line, "this function returns a value elsewhere; `return;` needs one too"));
                            }
                        } else if !matches!(ctx.ret, Ty::Nothing) {
                            return Err(CompileError::new(line, "this function returns a value; `return;` needs one"));
                        }
                        SK::Return(None)
                    }
                    Some(e) => {
                        let e = self.expr(ctx, e)?;
                        if matches!(ctx.ret, Ty::Nothing) && !ctx.ret_inferred {
                            return Err(CompileError::new(line, "this function returns nothing; `return [v]` has no place here"));
                        }
                        if matches!(self.types.resolve(&e.ty), Ty::Nothing) {
                            return Err(CompileError::new(line, "a call that returns nothing cannot be returned as a value"));
                        }
                        ctx.has_value_return = true;
                        self.types.force(&ctx.ret.clone(), &e.ty, line)?;
                        SK::Return(Some(e))
                    }
                }
            }
            StmtKind::Tier(t, b) => {
                ctx.tiers.push(*t);
                let body = self.inner(ctx, b)?;
                ctx.tiers.pop();
                SK::Block(body)
            }
            StmtKind::CallStmt(e) => {
                let e = self.expr(ctx, e)?;
                SK::CallStmt(e)
            }
        };
        Ok(Stmt { line, kind })
    }

    fn must_bool(&mut self, e: &Expr, what: &str) -> Result<()> {
        let r = self.types.resolve(&e.ty);
        match r {
            Ty::Bool => Ok(()),
            Ty::Var(_) => self.types.unify(&e.ty, &Ty::Bool, e.line),
            t => Err(CompileError::new(e.line, format!("{what} must be a bool, not {} (there is no truthiness)", t.name()))),
        }
    }

    fn index_ok(&mut self, i: &Expr, line: u32) -> Result<()> {
        let r = self.types.resolve(&i.ty);
        match r {
            Ty::Int(_) => Ok(()),
            Ty::Var(v) => match self.types.kind_of(v) {
                Kind::IntLike | Kind::NumLit => {
                    let t = self.types.fresh(Kind::IntLike, "an index", line);
                    self.types.unify(&i.ty, &t, line)
                }
                Kind::BinLike => Err(CompileError::new(line, "an index is an integer, not a bin")),
                Kind::Any => {
                    let t = self.types.fresh(Kind::IntLike, "an index", line);
                    self.types.unify(&i.ty, &t, line)
                }
            },
            t => Err(CompileError::new(line, format!("an index is an integer, not {}", t.name()))),
        }
    }

    fn read(&mut self, ctx: &FnCtx, line: u32, name: &str, t: &Ty, bounds: &[ast::Expr]) -> Result<Expr> {
        let lit = |e: &ast::Expr| -> Result<i128> {
            match &e.kind {
                ExprKind::Int(v) => Ok(*v),
                ExprKind::Unary(UnOp::Neg, inner) => match &inner.kind {
                    ExprKind::Int(v) => Ok(-*v),
                    _ => Err(CompileError::new(e.line, "a read bound is a literal")),
                },
                _ => Err(CompileError::new(e.line, "a read bound is a literal")),
            }
        };
        let rb = match t {
            Ty::Int(IntTy::Fixed(w)) => match bounds.len() {
                0 => ReadBounds { value: None, len: None },
                2 => {
                    let (lo, hi) = (lit(&bounds[0])?, lit(&bounds[1])?);
                    if !w.holds(lo) || !w.holds(hi) { return Err(CompileError::new(line, format!("a bound of '{name}' does not fit {}", w.name()))); }
                    if lo > hi { return Err(CompileError::new(line, "a bound's low end is above its high end")); }
                    ReadBounds { value: Some((lo, hi)), len: None }
                }
                _ => return Err(CompileError::new(line, "an integer read takes no bound or `[lo, hi]`")),
            },
            Ty::Bin(_) | Ty::Bool => {
                if !bounds.is_empty() { return Err(CompileError::new(line, format!("a {} read takes no bound", t.name()))); }
                ReadBounds { value: None, len: None }
            }
            Ty::Str => match bounds.len() {
                0 => ReadBounds { value: None, len: None },
                2 => {
                    let (lo, hi) = (lit(&bounds[0])?, lit(&bounds[1])?);
                    if lo < 0 || lo > hi { return Err(CompileError::new(line, "a length bound is non-negative with low <== high")); }
                    ReadBounds { value: None, len: Some((lo, hi)) }
                }
                _ => return Err(CompileError::new(line, "a text read takes no bound or `[lo, hi]` on its length")),
            },
            Ty::List(elem) => {
                let Ty::Int(IntTy::Fixed(w)) = **elem else {
                    return Err(CompileError::new(line, format!("only a list of an integer type can be read, not {}", t.name())));
                };
                match bounds.len() {
                    0 => ReadBounds { value: None, len: None },
                    2 => {
                        let (lo, hi) = (lit(&bounds[0])?, lit(&bounds[1])?);
                        if lo < 0 || lo > hi { return Err(CompileError::new(line, "a length bound is non-negative with low <== high")); }
                        ReadBounds { value: None, len: Some((lo, hi)) }
                    }
                    4 => {
                        let (lo, hi) = (lit(&bounds[0])?, lit(&bounds[1])?);
                        let (elo, ehi) = (lit(&bounds[2])?, lit(&bounds[3])?);
                        if lo < 0 || lo > hi { return Err(CompileError::new(line, "a length bound is non-negative with low <== high")); }
                        if !w.holds(elo) || !w.holds(ehi) { return Err(CompileError::new(line, format!("an element bound of '{name}' does not fit {}", w.name()))); }
                        if elo > ehi { return Err(CompileError::new(line, "an element bound's low end is above its high end")); }
                        ReadBounds { value: Some((elo, ehi)), len: Some((lo, hi)) }
                    }
                    _ => return Err(CompileError::new(line, "a list read takes no bound, `[lo, hi]` on its length, or `[lo, hi, elo, ehi]`")),
                }
            }
            Ty::Int(IntTy::Free(_)) | Ty::Var(_) | Ty::Nothing => unreachable!(),
        };
        let id = self.reads.len() as u32;
        self.reads.push(ReadInfo { line, name: name.to_string(), ty: t.clone(), func: ctx.id });
        Ok(Expr { id: 0, line, ty: t.clone(), kind: EK::Read(id, rb) })
    }

    fn expr(&mut self, ctx: &mut FnCtx, e: &ast::Expr) -> Result<Expr> {
        let line = e.line;
        Ok(match &e.kind {
            ExprKind::Int(v) => {
                let t = self.types.fresh(Kind::NumLit, format!("the literal {v}"), line);
                self.literals.push((line, *v, t.clone()));
                Expr { id: 0, line, ty: t, kind: EK::Int(*v) }
            }
            ExprKind::Bin(v, _) => {
                let t = self.types.fresh(Kind::BinLike, format!("the literal {v}"), line);
                Expr { id: 0, line, ty: t, kind: EK::Bin(*v) }
            }
            ExprKind::Text(s) => Expr { id: 0, line, ty: Ty::Str, kind: EK::Text(s.clone()) },
            ExprKind::Bool(b) => Expr { id: 0, line, ty: Ty::Bool, kind: EK::Bool(*b) },
            ExprKind::Name(n) => {
                let Some(v) = self.lookup(ctx, n) else {
                    return Err(CompileError::new(line, format!("'{n}' is not declared")));
                };
                Expr { id: 0, line, ty: ctx.locals[v as usize].ty.clone(), kind: EK::Var(v) }
            }
            ExprKind::Pieces(ps) => {
                let mut out = Vec::new();
                for p in ps {
                    out.push(match p {
                        Piece::Text(s) => PieceIr::Text(s.clone()),
                        Piece::Newline => PieceIr::Newline,
                        Piece::Value(v) => {
                            let ve = self.expr(ctx, v)?;
                            if matches!(self.types.resolve(&ve.ty), Ty::Nothing) {
                                return Err(CompileError::new(v.line, "a call that returns nothing cannot be a piece"));
                            }
                            PieceIr::Value(ve)
                        }
                        Piece::Name(n, l) => {
                            let Some(v) = self.lookup(ctx, n) else {
                                return Err(CompileError::new(*l, format!("'{n}' is not declared")));
                            };
                            PieceIr::Var(v, ctx.locals[v as usize].ty.clone())
                        }
                    });
                }
                Expr { id: 0, line, ty: Ty::Str, kind: EK::Pieces(out) }
            }
            ExprKind::List(vals) => {
                let elem = self.types.fresh(Kind::Any, "a list element", line);
                let mut out = Vec::new();
                for v in vals {
                    let ve = self.expr(ctx, v)?;
                    if matches!(self.types.resolve(&ve.ty), Ty::Nothing) {
                        return Err(CompileError::new(v.line, "a call that returns nothing cannot be a list element"));
                    }
                    self.types.force(&elem, &ve.ty, v.line)?;
                    out.push(ve);
                }
                Expr { id: 0, line, ty: Ty::List(Box::new(elem)), kind: EK::List(out) }
            }
            ExprKind::Paren(inner) | ExprKind::Bracket(inner) => self.expr(ctx, inner)?,
            ExprKind::Unary(UnOp::Not, inner) => {
                let x = self.expr(ctx, inner)?;
                self.must_bool(&x, "the operand of `not`")?;
                Expr { id: 0, line, ty: Ty::Bool, kind: EK::Not(Box::new(x)) }
            }
            ExprKind::Unary(UnOp::Neg, inner) => {
                let x = self.expr(ctx, inner)?;
                self.numeric(&x, line)?;
                self.family_checks.push((line, x.ty.clone(), FamilyRule::NegNumeric));
                let site = Some(self.site(ctx, line, SiteKind::Overflow, "-…".into()));
                Expr { id: 0, line, ty: x.ty.clone(), kind: EK::Neg(Box::new(x), site) }
            }
            ExprKind::Binary(op, a, b) => {
                let ae = self.expr(ctx, a)?;
                let be = self.expr(ctx, b)?;
                let what = format!("{} {} {}", short(a), op.spelling(), short(b));
                match op {
                    BinOp::And | BinOp::Or => {
                        self.must_bool(&ae, &format!("the operand of `{}`", op.spelling()))?;
                        self.must_bool(&be, &format!("the operand of `{}`", op.spelling()))?;
                        Expr { id: 0, line, ty: Ty::Bool, kind: EK::Binary(*op, Box::new(ae), Box::new(be), None, None) }
                    }
                    BinOp::Eq | BinOp::Ne => {
                        self.types.force(&ae.ty, &be.ty, line)?;
                        self.family_checks.push((line, ae.ty.clone(), FamilyRule::EqNotList));
                        Expr { id: 0, line, ty: Ty::Bool, kind: EK::Binary(*op, Box::new(ae), Box::new(be), None, None) }
                    }
                    BinOp::Lt | BinOp::Gt | BinOp::Le | BinOp::Ge => {
                        self.types.force(&ae.ty, &be.ty, line)?;
                        self.numeric(&ae, line)?;
                        self.family_checks.push((line, ae.ty.clone(), FamilyRule::OrderNumeric));
                        Expr { id: 0, line, ty: Ty::Bool, kind: EK::Binary(*op, Box::new(ae), Box::new(be), None, None) }
                    }
                    BinOp::Add | BinOp::Sub | BinOp::Mul | BinOp::Pow | BinOp::Div | BinOp::Mod => {
                        self.types.force(&ae.ty, &be.ty, line)?;
                        self.numeric(&ae, line)?;
                        self.family_checks.push((line, ae.ty.clone(), FamilyRule::ArithNumeric));
                        if *op == BinOp::Mod {
                            self.family_checks.push((line, ae.ty.clone(), FamilyRule::NoBinMod));
                        }
                        let s1 = Some(self.site(ctx, line, SiteKind::Overflow, what.clone()));
                        let s2 = match op {
                            BinOp::Div | BinOp::Mod => Some(self.site(ctx, line, SiteKind::DivZero, what.clone())),
                            BinOp::Pow => Some(self.site(ctx, line, SiteKind::NegExp, what.clone())),
                            _ => None,
                        };
                        let ty = ae.ty.clone();
                        Expr { id: 0, line, ty, kind: EK::Binary(*op, Box::new(ae), Box::new(be), s1, s2) }
                    }
                }
            }
            ExprKind::Index(target, idx) => {
                let te = self.expr(ctx, target)?;
                let elem = self.types.fresh(Kind::Any, "a list element", line);
                if self.types.unify(&te.ty, &Ty::List(Box::new(elem.clone())), line).is_err() {
                    let r = self.types.resolve(&te.ty);
                    return Err(CompileError::new(line, format!("{} cannot be indexed", r.name())));
                }
                let ie = self.expr(ctx, idx)?;
                self.index_ok(&ie, line)?;
                let site = self.site(ctx, line, SiteKind::OutOfBounds, format!("{}[{}]", short(target), short(idx)));
                Expr { id: 0, line, ty: elem, kind: EK::Index(Box::new(te), Box::new(ie), site) }
            }
            ExprKind::Call(name, args) => {
                if name == "std::exit" {
                    return Err(CompileError::new(line, "`std::exit` is a statement, not a value"));
                }
                if name.starts_with("std::print") {
                    return Err(CompileError::new(line, "`std::print` is a statement, not a value"));
                }
                let Some((fid, params, ret, _)) = self.sigs.get(name).cloned() else {
                    return Err(CompileError::new(line, format!("no function named `{name}`")));
                };
                if args.len() != params.len() {
                    return Err(CompileError::new(line, format!("`{name}` takes {} value(s), given {}", params.len(), args.len())));
                }
                let mut out = Vec::new();
                for (a, p) in args.iter().zip(params.iter()) {
                    let ae = self.expr(ctx, a)?;
                    if matches!(self.types.resolve(&ae.ty), Ty::Nothing) {
                        return Err(CompileError::new(a.line, "a call that returns nothing cannot be an argument"));
                    }
                    self.types.force(p, &ae.ty, a.line)?;
                    out.push(ae);
                }
                Expr { id: 0, line, ty: ret, kind: EK::Call(fid, out) }
            }
            ExprKind::Read(_) => {
                return Err(CompileError::new(line, "`std::read.stdin` must be the whole value of a typed `var` declaration (or assignment to a typed name)"));
            }
            ExprKind::Range(..) => return Err(CompileError::new(line, "`std::range` is only for `loop.for 'i' in [std::range[a, b]]`")),
            ExprKind::Len(inner) => {
                let x = self.expr(ctx, inner)?;
                let r = self.types.resolve(&x.ty);
                match r {
                    Ty::Str | Ty::List(_) => {}
                    Ty::Var(_) => return Err(CompileError::new(line, "`std::len` needs a list or text whose type is already known here")),
                    t => return Err(CompileError::new(line, format!("`std::len` takes a list or text, not {}", t.name()))),
                }
                let t = self.types.fresh(Kind::IntLike, "the result of `std::len`", line);
                Expr { id: 0, line, ty: t, kind: EK::Len(Box::new(x)) }
            }
            ExprKind::Fill(v, n) => {
                let ve = self.expr(ctx, v)?;
                if matches!(self.types.resolve(&ve.ty), Ty::Nothing) {
                    return Err(CompileError::new(line, "a call that returns nothing cannot fill a list"));
                }
                let ne = self.expr(ctx, n)?;
                self.index_ok(&ne, line)?;
                let site = self.site(ctx, line, SiteKind::OutOfBounds, "std::fill".into());
                Expr { id: 0, line, ty: Ty::List(Box::new(ve.ty.clone())), kind: EK::Fill(Box::new(ve), Box::new(ne), site) }
            }
            ExprKind::To(spec, inner) => {
                let x = self.expr(ctx, inner)?;
                let target = spec_to_ty(spec);
                let src = self.types.resolve(&x.ty);
                let src = match src {
                    Ty::Var(v) => {
                        // A literal or free value: it takes its own family; conversion is by value.
                        match self.types.kind_of(v) {
                            Kind::BinLike => Ty::Bin(BinWidth::B64),
                            _ => Ty::Int(IntTy::Free(u32::MAX)),
                        }
                    }
                    t => t,
                };
                let ok = match (&src, &target) {
                    (Ty::Int(_), Ty::Int(_)) | (Ty::Int(_), Ty::Bin(_)) | (Ty::Bin(_), Ty::Int(_)) | (Ty::Bin(_), Ty::Bin(_)) => true,
                    (Ty::Int(_), Ty::Str) | (Ty::Bin(_), Ty::Str) | (Ty::Bool, Ty::Str) => true,
                    (Ty::Bool, Ty::Bool) | (Ty::Str, Ty::Str) => true,
                    (Ty::List(a), Ty::List(b)) => a == b,
                    _ => false,
                };
                if !ok {
                    return Err(CompileError::new(line, format!("`std::to` cannot convert {} to {}", src.name(), target.name())));
                }
                let site = if target.is_int() && (src.is_int() || src.is_bin()) {
                    Some(self.site(ctx, line, SiteKind::Overflow, format!("std::to.{}", target.name())))
                } else {
                    None
                };
                Expr { id: 0, line, ty: target, kind: EK::To(Box::new(x), site) }
            }
        })
    }

    fn numeric(&mut self, e: &Expr, line: u32) -> Result<()> {
        let r = self.types.resolve(&e.ty);
        match r {
            Ty::Int(_) | Ty::Bin(_) => Ok(()),
            Ty::Var(v) => match self.types.kind_of(v) {
                Kind::Any => {
                    let t = self.types.fresh(Kind::NumLit, "a number", line);
                    self.types.unify(&e.ty, &t, line)
                }
                _ => Ok(()),
            },
            t => Err(CompileError::new(line, format!("arithmetic needs numbers, not {}", t.name()))),
        }
    }
}

fn short(e: &ast::Expr) -> String {
    match &e.kind {
        ExprKind::Int(v) => v.to_string(),
        ExprKind::Bin(_, s) => s.clone(),
        ExprKind::Name(n) => format!("'{n}'"),
        ExprKind::Paren(i) => format!("({})", short(i)),
        ExprKind::Bracket(i) => format!("[{}]", short(i)),
        ExprKind::Binary(op, a, b) => format!("{} {} {}", short(a), op.spelling(), short(b)),
        ExprKind::Unary(UnOp::Neg, i) => format!("-{}", short(i)),
        ExprKind::Index(t, i) => format!("{}[{}]", short(t), short(i)),
        ExprKind::Call(n, _) => format!("{n}[…]"),
        ExprKind::Len(i) => format!("std::len[{}]", short(i)),
        _ => "…".into(),
    }
}

/// Does every path through `stmts` end in a `return` (or leave the program)?
pub fn always_returns(stmts: &[Stmt]) -> bool {
    for s in stmts {
        match &s.kind {
            SK::Return(_) | SK::Exit(_) => return true,
            SK::If(brs, Some(el)) => {
                if brs.iter().all(|(_, b)| always_returns(b)) && always_returns(el) {
                    return true;
                }
            }
            SK::If(brs, None) => {
                // A tier block is represented as `if [true] { }` with no else.
                if brs.len() == 1 && matches!(brs[0].0.kind, EK::Bool(true)) && always_returns(&brs[0].1) {
                    return true;
                }
            }
            SK::Loop(b) => {
                if !has_break(b) {
                    return true;
                }
            }
            _ => {}
        }
    }
    false
}

fn has_break(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match &s.kind {
        SK::Break => true,
        SK::If(brs, el) => brs.iter().any(|(_, b)| has_break(b)) || el.as_ref().map_or(false, |b| has_break(b)),
        SK::Block(b) => has_break(b),
        _ => false, // a break inside a nested loop belongs to that loop
    })
}
